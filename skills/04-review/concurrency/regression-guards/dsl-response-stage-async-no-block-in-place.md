---
name: dsl-response-stage-async-no-block-in-place
description: Use when reviewing where the DSL response-header script runs, any sync-to-async bridge (`block_in_place`, `Handle::block_on`) on a Gateway plugin path, or DSL behavior with `server.work_stealing: false`.
knowledge_type: regression-guards
finding_id: dsl-08
---

# DSL response stage is async; no `block_in_place` bridge

## Conclusion and scope

The DSL response-header script runs only in the asynchronous
`upstreamResponsePlugins` stage (`UpstreamResponse`, Pingora `response_filter`
hook). `DslPlugin::run_upstream_response` awaits `execute_safe` directly. DSL is
not valid in the synchronous `upstreamResponseFilterPlugins` stage: the shared
stage table rejects it during Controller validation, and the Gateway constructor
answers `WrongStage` (fail closed) if such an entry reaches it.

No Gateway plugin may bridge a synchronous stage to async work with
`tokio::task::block_in_place` + `Handle::block_on`. A plugin that needs to await
I/O belongs in an async stage. Pure-CPU response-header plugins
(`ResponseHeaderModifier`, `ResponseRewrite`, CORS, ...) remain synchronous.

## Rationale and evidence

Root cause (finding dsl-08, 2026-11-05): the former sync entry point used
`block_in_place(|| Handle::current().block_on(execute_safe(..)))`. Its doc
comment claimed the Gateway always runs a multi-thread Tokio runtime. That is
false when the supported gateway option `server.work_stealing` is `false`: the
Pingora fork's `NoStealRuntime` builds one `Builder::new_current_thread()`
runtime per worker (`pingora-runtime/src/lib.rs`, `new_no_steal`), and
`block_in_place` panics there on the first request reaching a DSL response
filter. The panic happens before `execute_safe`'s `catch_unwind`, so DSL panic
isolation did not apply. On multi-thread runtimes the bridge also pinned a worker
OS thread for the whole `http.call` / `redis.*` latency.

Fixed shape:

- Stage table: `HTTP_PLUGIN_STAGE_EXCEPTIONS` in
  `edgion-resources/src/resources/edgion_plugins/validate.rs` lists `Dsl` as
  request + upstream-response. Reference binding (`plugin_fields.rs`
  `supports_stage`) and dye capabilities derive from the same table.
- Gateway: `edgion-gateway/src/plugins/http/dsl/plugin.rs` implements
  `UpstreamResponse`; `PluginRuntime::create_upstream_response_from_edgion`
  builds it, and `add_from_upstream_responses_with_cache` builds
  LinkSys-referencing DSL only from the owning `response_entries()` binding.
- Late request body: the request-body view is released at the end of the async
  chain in `pg_response_filter.rs` instead of the end of the sync chain, so a
  response-stage DSL can still read a body captured by `RequestBodyBuffer`. A
  sync-stage denial that aborts before `response_filter` releases it in
  `pg_upstream_response_filter.rs`, as the AI early return does.

Both stages share `PingoraSessionAdapter::with_response_header` and
`check_response_stage_terminate`, so local replies, denials, gRPC denials,
transcoder sealed-header checks, and dye recording behave the same as before.
The AI observation-encoding early return stashes a local reply first, which
settles the response stage, so the async stage is skipped there as the sync
chain was.

## Alternatives and limits

Rejected alternatives:

- Keep the sync stage and check `runtime_flavor()` per call, failing through
  `errorPolicy` on a current-thread runtime. This avoids the panic but leaves DSL
  unusable with `work_stealing: false` and keeps the worker-pinning bridge.
- Reject `work_stealing: false` at startup. DSL configuration is dynamic, so this
  would remove a supported knob for every deployment.
- Make `UpstreamResponseFilter` async. Wider than needed: an async header stage
  already exists, and the pure-CPU sync plugins gain nothing from it.
- `spawn_blocking` (historical F-008 suggestion): does not compile, because the
  session and log borrows are neither `'static` nor `Send`.

Accepted consequences (pre-release, no compatibility shim):

- Configurations must use `upstreamResponsePlugins` for DSL.
- Async response plugins run after every synchronous response-header entry at
  all levels (route, backend, Gateway, global), so DSL now observes headers set
  by those filters instead of interleaving by list position.
- DSL now shares the async chain with request-owned response companions
  (ExtProc, Guardrail) and the `spec.waf` response phase. Within one runtime
  those are added before the `upstreamResponsePlugins` entries, so they run
  before that runtime's DSL entries, and an inner level's companions run before
  outer-level DSL. Previously DSL
  (sync) always ran before every async companion. A companion therefore sees
  headers before DSL edits them, and DSL may overwrite a companion's header
  mutation.
- Except on `101`, DSL runs after `response_filter` strips hop-by-hop headers,
  so it no longer sees `Connection` / `Transfer-Encoding` / `Keep-Alive`, and a
  hop-by-hop header it sets is removed by the second sanitizer pass.
- Because the late request-body view now lives through the async chain, the
  async response companions (ExtProc, Guardrail, WAF) can read a buffered
  request body instead of getting `NotBuffered`.
- The DSL entry's access-log stage key is `upstream_responses` instead of
  `upstream_response_filters`.

## Verification and re-evaluation

Verified on 2026-11-05 by:

- `edgion-gateway/tests/unit/plugins/http/dsl/mod_test/response_builtins.rs::test_upstream_response_phase_runs_on_current_thread_runtime`
  (`#[tokio::test(flavor = "current_thread")]`; panicked with the former bridge).
- `edgion-resources/tests/unit/resources/edgion_plugins/validate_test.rs::dsl_in_sync_response_filter_stage_is_wrong_stage`
  and `edgion-gateway/tests/unit/plugins/runtime/plugin_runtime_test.rs::dsl_in_sync_response_filter_stage_fails_closed`.
- `edgion-gateway/tests/unit/routes/http/proxy_http/pg_response_filter_test.rs::late_request_body_view_survives_sync_hook_until_async_dsl_reads_it`.
- Integration: `EdgionPlugins/PluginValidationStatus` case
  `dsl_sync_response_stage_invalid` (fixture 10).
- Existing DSL terminate/denial, LinkSys binding, response cache and transcoder
  unit tests migrated to the async stage; `cargo test -p edgion-resources`,
  `-p edgion-gateway` and `-p edgion-controller` pass.
- Integration fixtures under `edgion-tests/{integration,k8s}/conf` were moved to
  `upstreamResponsePlugins`; they were not executed as part of this change.

Re-evaluate if a synchronous stage gains a plugin that awaits I/O, if Pingora's
`response_filter` hook stops being async, or if a new `block_in_place` /
`Handle::block_on` appears on a Gateway request path.

## Resolution references

- dsl-08 (2026-11-05): `block_in_place` panic with `work_stealing: false`;
  fixed by moving DSL to the async upstream-response stage.
- F-008 (`tasks/plugins/`, 2026-05-26), historical: accepted the
  multi-thread-only bridge as a documented tradeoff on the premise that
  production never runs a current-thread runtime. dsl-08 disproved that premise;
  the tradeoff is retired.
