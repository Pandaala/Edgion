# gw-ext-04: DSL response stage blocks the worker per response (`block_in_place`) and panics with `work_stealing: false`

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/dsl/plugin.rs:142-177` (`run_upstream_response_filter`), `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:2731,2761,2950` (DSL stage factory; `upstreamResponsePlugins` → `WrongStage`), `edgion-gateway/src/plugins/runtime/traits/upstream_response.rs:11-24` (async stage), `edgion-gateway/src/cli/config.rs:695-699` and `edgion-gateway/src/cli/pingora.rs:40` (`work_stealing` option)

## Summary
A DSL entry in `upstreamResponseFilterPlugins` runs on every response through
`tokio::task::block_in_place(|| Handle::current().block_on(execute_safe(..)))`. This happens
even when the script never awaits anything. The accepted-design page justifies this with two
claims: (a) the sync trait is the only response-header stage available, and (b) production
always runs a multi-thread runtime. Both claims are now false. An async
`upstreamResponsePlugins` stage (`UpstreamResponse`, awaited in Pingora `response_filter`)
exists, but DSL is rejected there as `WrongStage`. The gateway also exposes
`server.work_stealing` / `--work-stealing`, and setting it to `false` makes Pingora build
per-thread `current_thread` runtimes. With that setting, `block_in_place` panics on every
request that reaches a DSL response entry.

## Current behavior (evidence)
- The only `block_in_place` in the gateway is at `dsl/plugin.rs:169`, inside the sync
  `UpstreamResponseFilter` impl.
- `plugin_runtime.rs:2761` builds DSL for the sync response-filter stage. The
  `UpstreamResponse` (async) factory lists `EdgionPlugin::Dsl(_) => CreateResult::WrongStage`
  (`plugin_runtime.rs:2950`). The async trait doc says that implementations "may await,
  mutate the current response headers ... before the downstream header is committed"
  (`traits/upstream_response.rs:11-24`), which is exactly what DSL needs.
- `ServerConfig.work_stealing: Option<bool>` is documented as "Enable work stealing
  (default: true)" (`cli/config.rs:695-699`) and passed to Pingora unchanged
  (`cli/pingora.rs:40,53`). In the locked Pingora fork, `work_stealing = false` selects
  `NoStealRuntime`, which builds `Builder::new_current_thread()` per worker
  (`pingora-runtime/src/lib.rs:517, 626`). Tokio's `block_in_place` panics on a
  current-thread runtime. `execute_safe`'s `catch_unwind` sits *inside* the `block_on`, so it
  does not cover this panic.
- Even on the multi-thread runtime, every call hands the worker core to another thread via
  the blocking pool. A response-stage `http.call` or `redis.*` then holds one OS thread
  blocked for up to `httpMaxTimeoutMs`. Many concurrent slow calls grow the blocking pool to
  its limit, after which the core handoff itself queues and that worker's tasks stall.

## Industry comparison
- **Envoy Lua** `envoy_on_response` coroutines yield on `httpCall` back to the event loop;
  nothing blocks a worker thread. **Kong / OpenResty** `header_filter` cannot yield at all, so
  cosockets are disallowed there by design. Neither moves a whole worker to a blocking thread
  for each response.

## Impact
- An operator who sets `work_stealing: false`, a documented and supported Pingora tuning,
  gets a panic on every request whose route has a DSL response entry. That kills the request
  task.
- With the default runtime, every DSL response-stage request pays a worker handoff. Slow
  response-stage I/O pins OS threads and can exhaust the blocking pool.

## Suggested direction
Run DSL in the async `upstreamResponsePlugins` stage, and stop supporting DSL in the sync
`upstreamResponseFilterPlugins` stage. The project is pre-release, so the stage can move
without a compatibility shim; Controller validation should reject DSL under the sync list
with a clear message. Delete the `block_in_place` bridge. If a sync placement must remain,
at least reject `work_stealing: false` while any DSL response entry is configured, or reject
response-stage `http.call`/`redis.*` statically and run the VM synchronously without
`block_on`.

## Notes
- Checked `skills/04-review/concurrency/intentional-design/block-in-place-multi-thread-runtime.md`.
  Its two premises are contradicted by current code: (1) "`block_in_place` is the only viable
  bridge" ignores the async `UpstreamResponse` stage; (2) "The gateway binary builds the
  Tokio default multi-thread runtime ... the panic can only be hit by a future test or
  embedder" ignores `server.work_stealing: false`. The page's own re-evaluation trigger
  ("if the gateway ever runs the DSL response stage on a current_thread runtime") is met by
  configuration. That page should be corrected along with the fix.
- `skills/01-architecture/02-gateway/dsl-engine.md` §4 repeats the premise.
- `skills/04-review/plugin-runtime/compatibility-constraints/filter-iteration-control-sync-phase-tradeoff.md`
  confirms that the Pingora response hooks are async and that the sync Edgion traits are a
  product choice, not a Pingora constraint.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/plugins/http/dsl/plugin.rs:142-177`: sync `UpstreamResponseFilter` uses `block_in_place(|| Handle::current().block_on(execute_safe(..)))`; its doc comment asserts the shipped gateway is always multi-thread.
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:2950` (inside `create_upstream_response_from_edgion`): DSL is `WrongStage` for the async response stage.
- `edgion-gateway/src/cli/pingora.rs:40,53` and `cli/config.rs:699,1294`: `work_stealing` is operator-configurable; pinned fork `pingora-runtime/src/lib.rs:481,626` builds per-thread `Builder::new_current_thread()` when it is false, where `block_in_place` panics.
- `skills/04-review/concurrency/intentional-design/block-in-place-multi-thread-runtime.md:30,47-48,78`: its premises (only viable bridge; panic only reachable by test/embedder) are contradicted by the async stage and the config knob, and its own re-evaluation trigger is met.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `dsl/plugin.rs` is unchanged; `tokio::task::block_in_place` is still at `dsl/plugin.rs:169` inside `run_upstream_response_filter` (`:142-177`).
- `plugin_runtime.rs` (one `fetch_update` -> `try_update` line at `:316`) and `cli/config.rs` (serde_yaml -> yaml_serde at `:1417-1429`) changed without shifting the cited lines.
- The cited concurrency and plugin-runtime review pages are unchanged.
