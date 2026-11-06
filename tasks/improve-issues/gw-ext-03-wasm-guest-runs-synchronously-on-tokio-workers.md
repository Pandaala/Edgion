# gw-ext-03: Wasm guest code and cold instantiation block Tokio worker threads for up to 10 s per hook

- **Category**: best-practice
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/wasm/vm_manager.rs:84` (`EPOCH_TICK_MS`), `vm_manager.rs:343-360` (engine config, no `async_support`), `vm_manager.rs:1190-1234` (`acquire_inner` → `new_vm`), `vm_manager.rs:1429-1499` (`new_vm`), `vm_manager.rs:1552-1880` (synchronous `call_*`/`open_and_call_with`), `edgion-gateway/src/plugins/http/wasm/plugin.rs:243-317,613-660,789-811`, `edgion-resources/src/resources/edgion_plugins/plugin_configs/wasm.rs:546,636`

## Summary
Every guest callback is a synchronous `TypedFunc::call` made directly on the Pingora/Tokio
worker thread that owns the request. That includes `proxy_on_request_headers`, body hooks,
callout callbacks, done/delete, and cold-path instantiation with `_start`,
`proxy_on_vm_start` and `proxy_on_configure`. The only bound is an epoch deadline of
`timeoutMs`, which defaults to 100 ms and can be configured up to 10 000 ms. A slow or
looping guest therefore blocks a whole executor thread, and every other connection and task
queued on it, for the full budget. Wasmtime's supported way to embed in Tokio is to make
guest execution cooperative (`async_support` plus `epoch_deadline_async_yield_and_update`).
Edgion does not use it.

## Current behavior (evidence)
- The engine is built with `epoch_interruption(true)` and the pooling allocator only. No
  `cfg.async_support(true)` is set (`vm_manager.rs:343-360`).
- Each hook arms `store.set_epoch_deadline(timeout / EPOCH_TICK_MS + 2)` and then calls the
  export synchronously (`vm_manager.rs:1572, 1605, 1729, 1763, 1793, 1843`). On deadline the
  guest traps. Before that, the worker thread never yields.
- `run_leased_request` is an `async fn`, but `lease.open_request(...)` and the following
  calls are blocking calls inside it (`plugin.rs:268-271`). The standalone
  `run_upstream_response_filter` (`plugin.rs:789-811`) and the per-chunk body filter
  (`plugin.rs:613-660`) are synchronous traits.
- On a pool miss, `acquire_inner` calls `new_vm` inline (`vm_manager.rs:1205-1214`). That
  instantiates the module and runs the guest's whole initialization sequence on the request
  thread. Each step is bounded only by the same epoch deadline (`vm_manager.rs:1443`).
- The config accepts `timeoutMs` up to `MAX_TIMEOUT_MS = 10_000` (`wasm.rs:546, 636`).
- In contrast, module *compilation* is correctly moved off the workers with
  `prepare_blocking` (`vm_manager.rs:633-664`).

## Industry comparison
- **Wasmtime embedding guidance (used by Spin, wasmCloud, Fastly-style hosts)**: in an async
  host, enable `Config::async_support(true)`, call with `call_async`, and use
  `Store::epoch_deadline_async_yield_and_update(n)`. The guest then yields back to the
  executor every epoch tick, and a separate fuel or epoch limit still terminates runaway
  code. Long guest work becomes fair, preemptible async work instead of a blocked thread.
- **Envoy**: runs Wasm on its single-threaded worker event loop, which has the same blocking
  property. Envoy, however, never instantiates on the request path. VMs are created on
  configuration load and cloned per worker ahead of time.

## Impact
- One misbehaving guest at the default 100 ms budget adds up to 100 ms of latency for
  unrelated requests on the same worker each time it runs. A trap-then-retry pattern across
  requests can occupy every worker. At the allowed 10 s budget, a handful of concurrent bad
  requests can stall the gateway data plane.
- Cold-start bursts run guest init (often parsing JSON configuration or compiling regexes in
  `on_configure`) on request threads. The global idle cap is 32 instances (see gw-ext-01),
  so in deployments with several Wasm entries this happens often, not only once at startup.

## Suggested direction
1. Enable `async_support`. Use `instantiate_async`/`call_async` for request-stage hooks
   (already in an async context) with `epoch_deadline_async_yield_and_update(1)`. Keep
   `timeoutMs` as a *total* CPU/wall budget, tracked by counting yields.
2. Never instantiate on the request path. Pre-instantiate up to `instancePoolSize` on the
   preparation path (already off-worker), and refill the pool in the background. A pool miss
   should wait briefly on an async semaphore or fail fast with 503, not run guest init inline.
3. Lower `MAX_TIMEOUT_MS` substantially (for example 1 s) for request hooks. Ten seconds of
   synchronous guest CPU is not a meaningful per-hook budget for a proxy.
4. The synchronous response-filter and body stages cannot yield. Removing the standalone
   stages (gw-ext-05) leaves only the async attached companions, which can use the same
   async path.

## Notes
- Checked `skills/04-review/wasm/*`. `request-execution-lifecycle.md` documents the reduced
  per-chunk budget (`max(timeoutMs/10, 10)`) as a response to a blocking-window finding,
  which confirms that blocking time is a recognized concern. No page records synchronous
  execution or inline instantiation as an accepted design.
- Related: gw-ext-01 (exclusive VM per request) and gw-ext-05 (standalone sync stages).

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/plugins/http/wasm/vm_manager.rs:343-360`: engine `Config` sets `epoch_interruption(true)` and pooling only; no `async_support`/`call_async` anywhere in the file.
- `vm_manager.rs:1443,1572,1605,1729,1763,1793,1843`: each hook arms `set_epoch_deadline` and runs synchronously; `EPOCH_TICK_MS = 10` (`:84`).
- `vm_manager.rs:1204-1214`: a pool miss runs `new_vm` (full guest init) inline on the request path.
- `edgion-resources/.../plugin_configs/wasm.rs:546,636`: `timeoutMs` accepted up to 10 000 ms.
- Note: Envoy also runs Wasm callbacks synchronously on its worker, so the industry delta is mainly inline instantiation and the 10 s per-hook ceiling rather than synchronous callbacks per se.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `wasm/vm_manager.rs`, `wasm/plugin.rs` and `plugin_configs/wasm.rs` are unchanged; the engine is still configured without `async_support` and guest calls stay synchronous on Tokio workers.
- No new or changed `skills/04-review` page in this area accepts the behavior; the only area review change is a test-process-isolation note in `wasm/regression-guards/request-execution-lifecycle.md`, which does not affect production behavior.
