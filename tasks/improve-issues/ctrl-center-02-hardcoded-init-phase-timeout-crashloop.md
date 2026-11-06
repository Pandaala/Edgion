# ctrl-center-02: Hard-coded 15 s init-phase timeout overrides `cache_ready_deadline_secs` and can crash-loop the controller on large clusters

- **Category**: design-error
- **Severity**: P1
- **Scope**: local
- **Location**:
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs:947-998` (`const PHASE_TIMEOUT = 15s`, Phase 1 providers and Phase 2 Gateway)
  - `edgion-controller/src/conf_mgr/conf_center/non_kube_common.rs:544-582` (same constant for FS/etcd)
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs:44-90` (`FailureBudget`, `MAX_CONSECUTIVE_FAILURES = 10` → graceful shutdown)
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs:715-731` (controller `Err` → `AllControllersStopped`)
  - `edgion-controller/src/config/conf_center/kubernetes.rs:105,304` (`cache_ready_deadline_secs`, default 90)

## Summary

Phase 1 (GatewayClass, Secret, ConfigMap, ReferenceGrant, Service, EndpointSlice, EdgionConfigData,
Namespace) must finish its initial LIST **and** the synchronous init processing of every object within
a fixed 15 s, or the whole resource epoch fails. The operator knob `cache_ready_deadline_secs` (the
one the log tells operators to raise) cannot extend it. On a cluster whose cluster-wide Secret/
EndpointSlice LIST takes longer than 15 s, every attempt fails deterministically; after 10 attempts the
controller shuts itself down, the next replica inherits the same failure, and the control plane never
becomes ready.

## Current behavior (evidence)

- `run_controllers` spawns all provider watchers, then
  `wait_phase(..., wait_kinds_ready(&providers, PHASE_TIMEOUT), PHASE_TIMEOUT)`; on timeout it returns
  `Err("Resource phase timed out")`. Phase 2 repeats this for Gateway.
- The controller task maps any `Err` to `ControllerExitReason::AllControllersStopped`
  (`center.rs:715-731`); `run_main_flow`/all-serve call `FailureBudget::on_failure(false)` with
  backoff `1<<n` seconds, and after 10 consecutive failures call `shutdown_handle.shutdown()`.
- `cache_ready_deadline_secs` (default 90) only bounds the outer pre-ready wait; the log on its
  expiry says "raise cache_ready_deadline_secs if this cluster legitimately needs longer", but the
  inner 15 s phase timeout fires first and is not configurable.
- Phase 1 watches are cluster-wide by default (`watch_namespaces` empty) and include **all Secrets**
  (including Helm release Secrets, often the largest objects in a cluster) and all EndpointSlices; see
  ctrl-center-03. Init processing per object is serial in each kind's init path.
- Same constant in `non_kube_common.rs` for FileSystem/etcd (a large etcd prefix or a slow etcd has
  the same failure shape).

## Industry comparison

client-go / controller-runtime `WaitForCacheSync` has no hard deadline by default (it waits, with
the readiness probe reflecting progress). Istio waits for informer sync with a configurable timeout and
keeps serving readiness=false rather than restarting. Neither treats "the API server needs more than N
seconds to list" as a fatal error.

## Impact

Large or slow clusters (tens of thousands of Secrets/EndpointSlices, throttled API server, high
latency to the API server, or a cold etcd) cannot start the controller at all; HA does not help because
every replica fails identically. During a leader-only failover the new leader may never become
ready, freezing configuration for all gateways.

## Suggested direction

Remove the fixed phase constant: derive phase deadlines from `cache_ready_deadline_secs` (the
already-documented knob), or drop per-phase deadlines and rely on the single outer deadline plus
progress-based liveness (fail only when a source stops making LIST/page progress). Do not count a
deadline expiry during the initial sync of a source that is still making progress toward
`MAX_CONSECUTIVE_FAILURES`. Apply the same to `non_kube_common.rs`.

## Notes

Checked `skills/04-review/controller/regression-guards/liveness-probe-and-lifecycle-supervision.md`,
`single-worker-initialization.md`, `source-scoped-watch-snapshots.md`, and `concurrency/` pages; none
documents the 15 s phase constant or its interaction with `cache_ready_deadline_secs`. No `tasks/` entry.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P1
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs:947-968`: `const PHASE_TIMEOUT = 15s` bounds `wait_kinds_ready(&providers, ..)` and `wait_phase`; `anyhow::ensure!(ready, "Provider initialization timed out")`.
- `controller.rs:986-1000`: same constant for Phase 2 (Gateway).
- `edgion-controller/src/conf_mgr/conf_center/non_kube_common.rs:544-582`: same 15 s constant for FS/etcd.
- `grep -rn PHASE_TIMEOUT skills/` finds no review page accepting it. P1 kept: deterministic startup failure on large/slow clusters, with no operator knob.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1 (unchanged)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs:947,956,967,992,998`: `const PHASE_TIMEOUT = 15s` still bounds Phase 1 and Phase 2; file unchanged since `feature-08-30`.
- `edgion-controller/src/conf_mgr/conf_center/non_kube_common.rs:544-582`: same constant for FS/etcd, unchanged.
- `kubernetes/center.rs` diff is a single line (kube 4 API); `FailureBudget` / `MAX_CONSECUTIVE_FAILURES` shutdown path unchanged. No review page accepts the constant.
