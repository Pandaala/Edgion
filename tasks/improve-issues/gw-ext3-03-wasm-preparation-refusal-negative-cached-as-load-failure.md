# gw-ext3-03: Wasm preparation-capacity refusal is cached as a 30 s module-load failure, so a 5th concurrently loading plugin fails even when every source is healthy

- **Category**: design-error
- **Severity**: P2 (downgraded from P1 by independent verification)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/wasm/admission.rs:178-185` (`try_prepare`: non-waiting `try_acquire_owned` on `max_preparations`, default 4), `edgion-gateway/src/plugins/http/wasm/vm_manager.rs:614` (`load_inner_with_linker` first statement `try_prepare()?`), `vm_manager.rs:540-593` (`get_or_load_with_linker`: every `Err` sets `LoadState::Failed` and `load_retry_due_ms = now + LOAD_RETRY_SECS*1000` = 30 s), `vm_manager.rs:519,554` (preload and request path honor the negative-cache window), `vm_manager.rs:619,629` (OCI / URL network fetch runs while the preparation permit is held), `edgion-gateway/src/plugins/http/wasm/oci/mod.rs:49,52,138` (OCI pipeline retains the preparation permit; 60 s x 3 attempts per request), `edgion-gateway/src/plugins/http/wasm/plugin.rs:122,773` (`spawn_preload` at build, `get_or_load` on every request)

## Summary
Process admission for module preparation (fetch + hash + analysis + compile + warm instance) is a
non-waiting semaphore with 4 permits by default. A refusal returns the ordinary load error string,
and `get_or_load_with_linker` cannot distinguish it from a broken source: it records
`LoadState::Failed` and suppresses all retries for 30 s. During that window every request on the
plugin's routes takes `fail_mode("load")` (HTTP 500 with the default `failOpen: false`). Because
network fetches hold the permit, four slow OCI/URL pulls also starve inline and file modules. The
result is that a gateway with five or more Wasm plugins, all healthy, routinely black-holes some of
them for 30 s at startup and after every reconstruction.

## Current behavior (evidence)
- `try_prepare()` (`admission.rs:178`) uses `try_acquire_owned()` and maps exhaustion to
  `"wasm preparation capacity exhausted"`.
- `load_inner_with_linker` calls it before anything else (`vm_manager.rs:614`) and keeps the permit
  (`preparation`, cloned into the OCI pipeline at `oci/mod.rs:138`) through `fetch_oci_bytes` /
  `fetch_bytes`, digest check, `edgion_wasm::analyze`, `Module::new`, and the eager warm instance.
- `get_or_load_with_linker` handles every `Err` identically (`vm_manager.rs:583-591`): metric
  `module_load{result="error"}`, `load_retry_due_ms = now + 30 s`, `LoadState::Failed`. Subsequent
  callers inside the window return the cached error immediately (`vm_manager.rs:554`), and
  `prepare_preload` refuses to schedule a retry (`vm_manager.rs:519`).
- Every `WasmPlugin::build` spawns a preload (`plugin.rs:122`), so all Wasm plugins of a config
  generation start loading at once. With N > `max_preparations` plugins, plugins N-4.. fail on the
  first attempt regardless of source health; their routes fail for the next 30 s, then the first
  request after the window retries — and may be refused again if other loads are in flight.
- OCI pulls default to 60 s per request and 3 attempts with backoff (`oci/mod.rs:49,52`), for both
  manifest and blob, so a degraded registry can hold all four permits for minutes while healthy
  `inline`/`file` modules keep failing in 30 s cycles.
- The user documentation (`docs/en/user-guide/http-route/filters/edgion-plugins/wasm.md`, "Process
  resource admission") says exhausted preparation capacity "follows existing load/admission failure
  and failOpen policy" and "Admission never creates an unbounded waiter queue". It does not mention
  that a transient admission refusal is negative-cached like a source failure.

## Industry comparison
- Envoy creates Wasm VMs during config warming; a remote-fetch failure NACKs the update and
  `RemoteAsyncDataSource` retries with its own backoff policy; capacity is never reported as a
  module failure.
- Istio's agent downloads modules on a bounded worker pool and *queues* requests for the same or
  different modules; only a real fetch/verification error produces a failed WasmPlugin status.
- Generic admission practice (Envoy overload manager, Kubernetes API priority-and-fairness) treats
  "busy" as retry-later with a short or event-driven backoff, distinct from "broken".

## Impact
- Startup and every EdgionPlugins reconstruction (see `gw-ext-09`) with more than four Wasm entries
  produce 30 s outages for healthy plugins; with fail-close defaults these are 500s on production
  routes.
- The failure is attributed to "load", so metrics and logs point operators at the module source
  rather than at admission contention.
- Raising `max_preparations` (max 32) only moves the cliff and multiplies peak compile memory.

## Suggested direction
1. Separate admission from source failure: make preparation a bounded *waiting* queue (FIFO
   semaphore `acquire_owned` with a deadline, e.g. the load timeout) or, if non-waiting refusal is
   kept, return a distinct `Busy` outcome that is never negative-cached and is retried by the
   preload task with a short jittered backoff.
2. Do not hold the CPU-bound preparation permit across network I/O: acquire a separate fetch permit
   (the existing `url_fetch_permits` / OCI pipeline semaphore already exist) for download, and the
   preparation permit only for hash/analysis/compile/warm.
3. Report capacity waits/refusals with their own metric reason and PluginLog cause.

## Notes
- Related but distinct from `gw-ext-09` (unwarmed runtime published on reload; source failures lose
  the last-good module). That finding is about failing *sources*; this one fails *healthy* sources
  because of admission, and would persist even if loads were warmed before publication.
- Checked `skills/04-review/wasm/` (`preload-claim-before-owned-inputs.md`,
  `request-execution-lifecycle.md`, `intentional-design/bounded-oci-artifact-memory-cache.md`): they
  cover preload single-flight and OCI pipeline permits, not refusal caching. The documented
  "no unbounded waiter queue" goal is compatible with a bounded wait or a non-cached retry.

## Verification (independent)

Verdict: CONFIRMED, DOWNGRADED P1 -> P2.
- `try_prepare` is non-waiting `try_acquire_owned` (`wasm/admission.rs:178-185`) and is the first statement of `load_inner_with_linker` (`vm_manager.rs:614`); the permit is passed into `fetch_oci_bytes` (`:619`), so it is held across network I/O.
- `get_or_load_with_linker` (`vm_manager.rs:540-593`) negative-caches every `Err` for `LOAD_RETRY_SECS` and `prepare_preload` (`:513-522`) refuses retries inside that window, so a capacity refusal is indistinguishable from a broken source.
- Downgrade reason: impact requires more simultaneously loading Wasm plugins than `max_preparations` (default 4, configurable to 32) and the outage window is bounded (30 s per cycle). Real and deterministic, but limited to larger Wasm deployments.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `wasm/admission.rs` changed only at `:236` (`try_update`, same semantics); `try_prepare` is still the non-waiting gate at `admission.rs:178-185`.
- `vm_manager.rs` and `oci/mod.rs` are unchanged, so a preparation refusal still enters the 30 s `LoadState::Failed` window.
- No new or changed `skills/04-review` page in this area accepts the behavior; the only area review change is a test-process-isolation note in `wasm/regression-guards/request-execution-lifecycle.md`, which does not affect production behavior.
