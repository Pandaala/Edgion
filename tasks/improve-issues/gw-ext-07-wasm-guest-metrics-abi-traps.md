# gw-ext-07: Proxy-Wasm guest-metric and log-level host calls trap, breaking common SDK filters at load or per request

- **Category**: best-practice
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-wasm/src/lib.rs:105-115` (`trap` helper), `edgion-wasm/src/lib.rs:172-176` (`PROXY_GET_LOG_LEVEL`, `PROXY_DEFINE_METRIC`, `PROXY_GET_METRIC`, `PROXY_RECORD_METRIC`, `PROXY_INCREMENT_METRIC` → `HostBehavior::Trap`), `edgion-gateway/src/plugins/http/wasm/host_abi.rs:2414` (trap installation), `edgion-gateway/src/plugins/http/wasm/vm_manager.rs:1462-1489` (`proxy_on_vm_start`/`proxy_on_configure` during `new_vm`)

## Summary
The host profile accepts the standard metric imports (`proxy_define_metric`,
`proxy_increment_metric`, `proxy_record_metric`, `proxy_get_metric`) and
`proxy_get_log_level`, but *invokes a trap* when a guest calls them. Defining metrics is one
of the most common things a Proxy-Wasm filter does, usually in `on_vm_start` or
`on_configure`; the Go SDK's `DefineCounterMetric` and the Rust SDK's
`hostcalls::define_metric` are examples. A module that defines a metric therefore fails
initialization (`proxy_on_configure: trap`), which fails the module load. If it defines
lazily, every request traps instead. Either way, the default fail-close returns 500 for all
traffic. The static compatibility report flags the imports only as warnings, and the load
still succeeds.

## Current behavior (evidence)
- `FUNCTIONS` lists `trap(PROXY_DEFINE_METRIC, ..)`, `trap(PROXY_GET_METRIC, ..)`,
  `trap(PROXY_RECORD_METRIC, ..)`, `trap(PROXY_INCREMENT_METRIC, ..)` and
  `trap(PROXY_GET_LOG_LEVEL, ..)` (`edgion-wasm/src/lib.rs:172-176`). The gateway installs
  every `HostBehavior::Trap` import as a trapping host function (`host_abi.rs:2414`).
- `new_vm` runs `proxy_on_vm_start` and `proxy_on_configure` inside `run_guest_boundary`. A
  trap there returns `Err` (`vm_manager.rs:1462-1489`). At load, the eager warm instance
  turns this into `"wasm instance init failed"` (`vm_manager.rs:719-723`), the runtime stays
  in `LoadState::Failed`, and every request takes `fail_mode("load")` → 500 by default.
- The user doc lists "No ... guest-defined metrics: calling them traps" under limitations
  (`docs/en/.../wasm.md:522`). There is a documented `AcceptedNoop` class
  (`edgion-wasm/src/lib.rs:117-122`) that returns OK without semantics, but these calls do
  not use it.

## Industry comparison
- **Envoy / Istio**: implement guest metrics fully. They are exported as Envoy stats with a
  `wasmcustom.` prefix, and Istio's own telemetry filters rely on them.
- **Kong ngx_wasm_module**: implements `proxy_define_metric`/`increment`/`record`/`get`
  backed by shared memory, with a configurable slab size.
- **Proxy-Wasm spec guidance**: hosts that do not support a feature should return
  `Unimplemented` (status 12), so the SDK can surface an error the guest may ignore. They
  should not trap.

## Impact
- Many otherwise compatible third-party filters cannot be loaded, including observability,
  security and rate-limit filters that count their own decisions. The compatibility report
  says `compatible: true` with warnings, so operators find out only at runtime, when the
  route is fully failed.
- The fix is cheap compared with the 8k-line host. Without it, the "Proxy-Wasm compatible"
  positioning loses much of its practical value.

## Suggested direction
1. Minimum: change the four metric calls and `proxy_get_log_level` to non-trapping
   behavior. `define_metric` returns OK with a host-assigned id, `increment`/`record` return
   OK, `get_metric` returns OK/0 or `NOT_FOUND`, and `get_log_level` returns the fixed level
   that corresponds to `proxy_log` filtering. That is the existing `AcceptedNoop` class plus
   result writes.
2. Better: implement a bounded per-runtime registry with a cap of N definitions and validated
   ASCII names. Export it through one fixed metric family, for example
   `edgion_wasm_guest_metric{module, name}`, on the existing module label, where the
   `name` cardinality is bounded by the definition cap and allow-listed in the metrics
   inventory guard. Counters and gauges map directly. Histograms can be dropped (Envoy maps
   them to stats histograms).
3. Keep trapping for gRPC calls, shared queues and foreign functions. Those are real
   semantics that cannot be faked safely. Returning `Unimplemented` instead of trapping
   would still be preferable.

## Notes
- Checked `skills/04-review/wasm/*`. No page records the metric trap as an intentional
  decision. The limitation is documented only in user docs.
- Metrics-inventory rule (`cicd/checks/check_metrics_inventory.py`): option 2 must use a
  fixed family name and a bounded, allow-listed label. Option 1 adds no metrics.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-wasm/src/lib.rs:105-115,172-176`: `PROXY_GET_LOG_LEVEL` and all four metric imports use `HostBehavior::Trap`, while an `AcceptedNoop` class exists (`:117-122`).
- `edgion-gateway/src/plugins/http/wasm/host_abi.rs:2411-2418` installs every `Trap` import as a trapping host function; `vm_manager.rs:1471-1488` runs `proxy_on_vm_start`/`proxy_on_configure` during `new_vm`, and `:719-721` turns an init failure into `wasm instance init failed` at load.
- The limitation is documented only in the user doc (`docs/en/user-guide/http-route/filters/edgion-plugins/wasm.md:122,522`); no `skills/04-review/wasm` page records the trap (vs. returning a status) as an accepted design. Widely used filters (e.g. coraza-proxy-wasm) define metrics at plugin start, so the impact is practical.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `edgion-wasm/src/lib.rs`, `wasm/host_abi.rs` and `wasm/vm_manager.rs` are unchanged; the metrics and log-level ABI entries are still `HostBehavior::Trap`.
- No new or changed `skills/04-review` page in this area accepts the behavior; the only area review change is a test-process-isolation note in `wasm/regression-guards/request-execution-lifecycle.md`, which does not affect production behavior.
