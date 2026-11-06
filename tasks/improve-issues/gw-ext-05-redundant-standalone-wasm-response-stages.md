# gw-ext-05: Standalone Wasm response stages duplicate attached `response:` hooks and break Proxy-Wasm stream context

- **Category**: over-design
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/wasm/plugin.rs:135-166` (`create_response_filter`, `create_body_filter`), `plugin.rs:523-660` (standalone `UpstreamResponseBodyFilter`), `plugin.rs:784-811` (standalone `UpstreamResponseFilter`), `plugin.rs:218-238` (`run_leased_response`), `edgion-gateway/src/plugins/http/wasm/vm_manager.rs:1652-1680` (`call_on_response_headers` / `call_on_response_body` open and close a fresh context per call), `edgion-gateway/src/plugins/http/wasm/attached.rs:33-233` (attached companions)

## Summary
Wasm response processing can be attached in two parallel ways:
1. The **attached** model: a request-stage entry with `response.headers/body/trailers: true`.
   It reuses the request's HTTP context and VM through `WasmRequestBinding` and is
   async-capable.
2. The **standalone** model: the same Wasm entry listed under
   `upstreamResponseFilterPlugins` or `upstreamResponseBodyFilterPlugins`. Each callback
   leases a pooled VM, *creates a brand-new HTTP context*, calls one hook, and deletes the
   context. For the body filter this happens **per chunk**.

The standalone form violates the basic Proxy-Wasm contract that one HTTP stream maps to one
context, created once and seeing headers before body. It also duplicates the attached
feature. Both paths have to be maintained, tested and documented, and the standalone one is
the less correct of the two.

## Current behavior (evidence)
- `create_response_filter` / `create_body_filter` build a standalone plugin whenever
  `config.response` is not enabled (`plugin.rs:135-166`).
- Standalone response headers: `entry.acquire(timeout)` → `run_leased_response` →
  `call_on_response_headers` = `open_and_call` + `close_context` (`vm_manager.rs:1652-1657`).
  A guest that set per-stream state in `on_http_request_headers` cannot see it. The context
  id is new, and the VM may differ from the one that ran the request hook.
- Standalone body filter: each chunk calls `entry.acquire(chunk_budget)`, which takes an
  active-request permit and possibly instantiates a VM. It then runs `call_on_response_body`,
  which opens a fresh context, calls the hook once and closes the context
  (`plugin.rs:629-659`, `vm_manager.rs:1665-1680`). Per stream, a guest therefore sees N
  unrelated contexts, each with a single body callback and no headers. The code has to
  detect SDK `Pause` and count it in `edgion_wasm_body_pause_unsupported_total`, because
  standard guests silently observe the wrong data (`plugin.rs:576-597`).
- An acquire failure in the standalone response-header filter goes through `fail_mode` and
  returns a 500 *after* the upstream answered (`plugin.rs:801-809`). A miss on the sync stage
  cannot load the module, so it returns `fail_mode("not loaded")` (`plugin.rs:795-798`).
- The attached companions (`attached.rs`) already cover headers, a bounded read-only body
  and trailers on the correct context. The lifecycle page notes that
  "Standalone synchronous entries retain independent per-hook contexts and do not share
  these bindings."

## Industry comparison
- **Envoy / Istio / Kong (ngx_wasm_module)**: a Wasm HTTP filter is installed once in the
  filter chain and receives every callback for that stream on one context:
  request headers → body → trailers → response headers → body → trailers → done. No
  mainstream host exposes "response-only" placement that fabricates a fresh context per
  callback.

## Impact
- Guests written for any SDK behave incorrectly when placed in the standalone lists. For
  example, per-stream state is lost and body hooks lack headers. The only signal is a
  metric.
- Per-chunk VM leasing on the body path costs a pool mutex, a semaphore, possibly an
  instantiation, and context create/delete calls into the guest on every chunk of every
  stream.
- The sync placement cannot load modules, cannot await, and cannot yield (see gw-ext-03).
- The extra placements add a stage matrix for one plugin type to construction, validation,
  docs and tests.

## Suggested direction
Pre-release, no compatibility burden: remove Wasm from `upstreamResponseFilterPlugins` and
`upstreamResponseBodyFilterPlugins` by returning `WrongStage` with a Controller validation
message that points to `response.headers/body/trailers`. Keep the attached model as the only
response surface, and delete `run_leased_response`, the standalone body-filter impl and the
`create_response_filter`/`create_body_filter` paths. If response-only placement is truly
needed, implement it as an attached binding created lazily at response headers, with one
context for the rest of the stream, rather than one context per callback.

## Notes
- Checked `skills/04-review/wasm/regression-guards/request-execution-lifecycle.md` and
  `wasm-body-counter-delta-rerun-safe.md`. They describe the standalone behavior but do not
  record it as a deliberate product requirement.
- The body-filter doc comment references `tasks/wasm-followups/body-buffering-spec-semantics.md`
  as future work. That path does not exist in the current tree.
- Related: gw-ext-01 (lease model) and gw-ext-03 (sync execution).

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/plugins/http/wasm/plugin.rs:135-157`: standalone response/body filters are built whenever `config.response` is not enabled.
- `plugin.rs:628-660`: every body chunk calls `entry.acquire(chunk_budget_ms)`; on acquire failure the chunk silently skips the guest (fail-open per chunk), and Pause is only counted in a metric.
- `vm_manager.rs:1652-1680`: `call_on_response_headers`/`call_on_response_body` open a fresh context and close it per call, so per-stream guest state and header-before-body ordering are lost.
- `skills/04-review/wasm/regression-guards/request-execution-lifecycle.md` describes standalone entries as independent per-hook contexts but does not record them as a product requirement.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `wasm/plugin.rs`, `wasm/attached.rs` and `wasm/vm_manager.rs` are unchanged, so the standalone response stages and attached companions still coexist at the cited lines.
- No new or changed `skills/04-review` page in this area accepts the behavior; the only area review change is a test-process-isolation note in `wasm/regression-guards/request-execution-lifecycle.md`, which does not affect production behavior.
