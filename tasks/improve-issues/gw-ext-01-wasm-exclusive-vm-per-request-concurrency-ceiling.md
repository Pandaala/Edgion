# gw-ext-01: Wasm pins one exclusive VM per in-flight request, capping route concurrency at 64

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: large-direction
- **Location**: `edgion-gateway/src/plugins/http/wasm/vm_manager.rs:1171-1234` (`ModuleEntry::acquire`), `vm_manager.rs:1429-1499` (`new_vm`), `vm_manager.rs:1882-1914` (`VmLease::drop`), `edgion-gateway/src/plugins/http/wasm/plugin.rs:243-317` (`run_leased_request`), `edgion-gateway/src/plugins/http/wasm/request.rs:32-109` (`WasmRequestBinding`), `edgion-gateway/src/plugins/http/wasm/admission.rs:33-48` (`WasmLimits::default`), `edgion-resources/src/resources/edgion_plugins/plugin_configs/wasm.rs:266-271,516-522,559`

## Summary
Every request that runs a request-stage Wasm entry leases one whole Wasmtime instance and
keeps it until the request's logging terminal, including the entire upstream wait and
response stream. Admission fails immediately when the lease cannot be obtained. As a result,
one Wasm entry can never have more than `maxActiveRequests` concurrent requests (default 32,
maximum 64) per gateway process. The process-wide limit is `max_instances` (default 128,
maximum 1024). The ceiling is therefore set by upstream latency rather than by Wasm CPU cost.
Envoy's Proxy-Wasm model runs many HTTP contexts in one VM and does not have this limit.

## Current behavior (evidence)
- `WasmPlugin::run_request` → `entry.acquire(timeout_ms)` (`plugin.rs:776`). `acquire` takes
  a permit from the per-entry `active_requests` semaphore with `try_acquire_owned()` and
  returns `"wasm active request limit reached"` immediately if none is left
  (`vm_manager.rs:1171-1184`). The caller maps the error to `fail_mode(..., "acquire")`, so
  the default `failOpen: false` returns **HTTP 500 "wasm plugin error"** (`plugin.rs:168-193`).
- `new_vm` also takes a process-wide `instance_permits()` permit with `try_acquire_owned`
  (`vm_manager.rs:1430-1433`). Its default is 128 from `WasmLimits::default`, and the
  configured value can be at most 1024.
- `run_leased_request` wraps the lease in a `WasmRequestBinding` that is installed in the
  session (`plugin.rs:250-254`). `loan.complete()` returns the lease to the binding; it does
  not return it to the pool (`request.rs:89-104`). The VM is released only by
  `WasmRequestBinding::finish` from the request terminal hook (`request.rs:68-83`). The VM,
  the active-request permit (`VmLease.request_permit`) and the process instance permit are
  therefore all held while the upstream is connected, while the upstream is processing,
  and for the whole downloaded or streamed response.
  This holds even when the entry has no response hooks at all.
- The config limits `max_active_requests` to `1..=64` (`wasm.rs:559`).
- `skills/04-review/wasm/regression-guards/request-execution-lifecycle.md` records
  "Long streams hold admission" and "no waiting queue". The user doc
  (`docs/en/.../wasm.md:481`) says the same. Neither page analyzes the throughput
  consequence.
- Cold path: when the idle pool is empty, `acquire_inner` calls `new_vm` synchronously on the
  request task. `new_vm` instantiates the instance and runs `_initialize`/`_start`,
  `proxy_on_context_create(root)`, `proxy_on_vm_start` and `proxy_on_configure`
  (`vm_manager.rs:1453-1489`). Idle retention is capped globally at 32 instances and 128 MiB
  (`admission.rs:42-43`). With a few Wasm entries in the process, a burst therefore keeps
  paying full guest initialization on the request path.

Throughput bound: with 64 leases and a 200 ms upstream p50, one entry serves at most about
320 req/s per process. A route with 65 concurrent SSE/LLM streams answers the 65th request
with 500 even when the CPU is idle.

## Industry comparison
- **Envoy / Istio Proxy-Wasm**: each worker thread has one VM per `vm_id` (or a shared VM).
  Each HTTP stream is only a *context id* inside that VM, created with
  `proxy_on_context_create`. The VM is busy only while a callback runs. Thousands of
  concurrent streams share one VM per worker, and guest memory scales with per-context
  state, not with whole instances.
- **Kong / APISIX Wasm (ngx_wasm_module / wasm-nginx-module)**: follow the same model: one
  VM per worker and many filter contexts per VM.
- Neither returns 5xx because concurrency is high. Overload is handled globally (Envoy
  overload manager), not by refusing work per Wasm entry.

## Impact
- Wasm effectively cannot be used on high-concurrency routes, long-lived routes (SSE, LLM
  streaming, downloads, WebSocket upgrades) or slow-upstream routes. Under such traffic the
  default fail-close turns a capacity limit into a 500 storm. `failOpen: true` instead
  silently skips the guest for some of the requests, which bypasses a policy Wasm (authz,
  WAF).
- Each held request keeps a full guest heap (up to 128 MiB of linear memory) for
  its whole lifetime instead of a few KB of per-context state.
- The returned status is 500, so the condition cannot be told apart from a guest bug. The
  correct overload status would be 503.

## Suggested direction
Option A (recommended): follow the Proxy-Wasm model and multiplex HTTP contexts.
- Keep a small fixed set of VMs per entry, for example one per Pingora worker thread or N per
  entry. A request owns a *context id plus a VM index*, not the VM.
- Take a short per-VM `Mutex` only for the duration of each callback, so a VM is never held
  across an await or the upstream wait. Callouts already resume through
  `proxy_on_http_call_response` on a context id, so they fit this model.
- Admission then bounds *callback concurrency* (cheap) instead of *request lifetime*.
  `maxActiveRequests` can become a bound on open contexts with a large default, or be
  removed.
- The "warm instance may retain root state" property stays the same. Per-request isolation
  then rests on context ids, which matches Envoy's model and the SDKs' assumptions.

Option B (minimal): release the VM after the request-headers and request-body hooks when no
`response.*` hook is configured. Run `proxy_on_done`/`proxy_on_delete` at that point, or
defer them only for entries with response hooks. Raise the 64 cap substantially, and map
admission failure to 503 rather than going through `fail_mode` as an ordinary 500.

Option A removes the ceiling. Option B removes it only for request-only guests.

## Notes
- Checked `skills/04-review/wasm/regression-guards/request-execution-lifecycle.md`. It
  records the lease lifetime and says "Re-evaluate when introducing multiple concurrent HTTP
  contexts per instance". That is the redesign proposed here. The page is a regression
  guard, not an accepted-tradeoff decision about throughput. The new evidence is the
  latency-bound throughput ceiling and the 500-on-concurrency behavior.
- Also checked `wasm-module-cache-key-runtime-config.md` (per-plugin runtime ownership). This
  proposal keeps per-plugin ownership and changes only how requests share a runtime's VMs.
- No related task found under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P1 unchanged)
- `edgion-gateway/src/plugins/http/wasm/vm_manager.rs:1171-1181`: `acquire` uses `try_acquire_owned()` on the per-entry `active_requests` semaphore and fails immediately; `new_vm` (`:1429-1433`) likewise fails fast on the process `instance_permits()`.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/wasm.rs:559-560`: `maxActiveRequests` is hard-limited to `1..=64`.
- `edgion-gateway/src/plugins/http/wasm/request.rs:68-104`: `ExecutionLoan::complete` returns the lease to the binding, and only `WasmRequestBinding::finish` (request terminal) closes it, so the VM and permits are held across upstream wait and response streaming.
- `plugin.rs:778,802` route acquire failure through `fail_mode`, whose fail-close result is status 500 (`plugin.rs:189`).
- `skills/04-review/wasm/regression-guards/request-execution-lifecycle.md:31-35,58` records "streams hold admission" as a regression guard and lists multi-context-per-instance as a re-evaluation trigger; it does not accept the throughput ceiling as a tradeoff.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `git diff feature-08-30..HEAD` over the cited Wasm sources: only `wasm/admission.rs:236` changed (`fetch_update` -> `try_update`, same semantics); `vm_manager.rs`, `plugin.rs`, `request.rs` and `plugin_configs/wasm.rs` are byte-identical, so the cited lines remain current.
- Exclusive per-request `VmLease` acquisition (`vm_manager.rs:1171-1234`) and `WasmLimits::default` (`admission.rs:33-48`) are unchanged.
- No new or changed `skills/04-review` page in this area accepts the behavior; the only area review change is a test-process-isolation note in `wasm/regression-guards/request-execution-lifecycle.md`, which does not affect production behavior.
