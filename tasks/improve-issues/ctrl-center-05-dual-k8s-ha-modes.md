# ctrl-center-05: Two Kubernetes HA modes; the default `leader-only` is a cold standby that needs pod-label routing machinery

- **Category**: over-design
- **Severity**: P3 (downgraded from P2 by independent verification)
- **Scope**: large-direction
- **Location**:
  - `edgion-controller/src/config/conf_center/kubernetes.rs:11-20` (`HaMode::{LeaderOnly (default), AllServe}`)
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs:427-650` (`run_main_flow`), `:772-830` (`run_leader_only_lifecycle`), `:943-1300` (`run_all_serve_lifecycle`, `run_serving_flow`, `start_event_watchers_all_serve`) — two lifecycles with comments warning "Do not unify them"
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/leader_election.rs:425-540` (pod `edgion.io/leader` label patching with `edgion.io/leader-label-write-id` fencing)
  - `edgion-controller/src/services/conf_sync_ca/provision.rs:125-140` and the `edgion-controller-leader` Service

## Summary

Kubernetes HA ships two lifecycles. `leader-only` (default) keeps followers completely cold (no
watches, no cache) and routes gateways to the leader through a Service that selects a pod label the
election loop patches; on failover the new leader must wait for lease expiry, LIST everything, run all
init phases and only then serve, while every gateway relists every kind. `all-serve` already does what
mature gateways do (every replica serves, the leader owns status/ACME/CA rotation/federation). Keeping
both doubles the lifecycle and test surface for no capability the second mode lacks.

## Current behavior (evidence)

- `center.rs` contains separate `run_main_flow` and `run_serving_flow` loops, separate
  `start_event_watchers` / `start_event_watchers_all_serve`, and separate leadership-loss wiring
  (comments at `center.rs:752-756` explicitly require them to stay different).
- `leader-only` failover cost, per the HA architecture page: "gRPC interruption on leader
  switchover: Yes (cache rebuild ~5–15s)", on top of lease expiry (15 s) — and subject to the hard
  init-phase timeout in ctrl-center-02.
- The leader pod label exists to steer the `edgion-controller-leader` Service; it needs its own
  conditional patch protocol (write-id annotation, retry ticks, ARCH-112 regression guard) and Pod
  patch RBAC. Admin writes in Kubernetes do not need it (`leadership_status()` is `NotApplicable`).
- The only argument for `leader-only` in the docs is memory on followers ("resource-sensitive").

## Industry comparison

Istio (istiod), Envoy Gateway, Contour and ingress-nginx all run every replica as an active
xDS/config server; leader election gates only status writes and singleton jobs. None routes data-plane
clients to "the leader" via label-patched Services.

## Impact

Slower and riskier failover by default (cold rebuild + global relist), extra RBAC (pod patch),
an extra Service/SAN set, and ~700 lines of lifecycle code plus label-fencing logic that must be kept
correct in two variants.

## Suggested direction

Make `all-serve` the only Kubernetes mode (pre-release: no compat burden): delete `run_main_flow`/
`run_leader_only_lifecycle`, the pod-label patching and the `edgion-controller-leader` Service; point
gateways at the all-pods Service. Prerequisite: fix ctrl-center-01 (instance count is per-replica in
all-serve). If memory on followers is a real constraint, the answer is narrower ingest
(ctrl-center-03), not a cold standby.

## Notes

Checked `concurrency/intentional-design/leader-status-write-per-write-guard-vs-lifecycle-gate.md`
(accepts per-write status gating in all-serve — consistent with this proposal),
`concurrency/regression-guards/leader-pod-label-tick-resync.md`, `controller/regression-guards/leader-jobs-call-site-consolidation.md`
and the HA-mode architecture page. No page argues that two modes are required.

## Verification (independent)

- **Verdict**: DOWNGRADED
- **Severity**: P3 (was P2)
- `edgion-controller/src/config/conf_center/kubernetes.rs:11-26`: `HaMode::{LeaderOnly (default), AllServe}`.
- `skills/01-architecture/01-controller/03-config-center/02-kubernetes/01-ha-mode.md:39-40`: leader-only documented for "Small deployments, resource-sensitive"; all-serve for HA/gRPC LB.
- No defect is shown beyond cost/complexity; all-serve currently carries ctrl-center-01 (wrong rate-limit divisor), so it is not yet a drop-in sole mode.
- Downgraded to P3: valid simplification direction to decide after ctrl-center-01/03, not an urgent issue.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3 (unchanged)
- `edgion-controller/src/config/conf_center/kubernetes.rs:14-20`: `HaMode::{LeaderOnly (default), AllServe}` unchanged.
- `kubernetes/center.rs:427` (`run_main_flow`), `:772` (`run_leader_only_lifecycle`), `:943` (`run_all_serve_lifecycle`), `:999` (`run_serving_flow`): both lifecycles still present.
- ctrl-center-01 (all-serve divisor) is still open, so the prerequisite noted in the original verification still holds.
