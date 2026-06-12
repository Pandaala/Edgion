---
name: leader-pod-label-tick-resync
description: Use when reviewing leader-election pod label findings; covers the tick-based resync that replaced the fire-and-forget `edgion.io/leader` patch.
status: fixed
finding_id: leader-election-pod-label-set-failure
closed: 2026-06-10
---

# Leader Pod Label Fire-and-Forget Desync — FIXED (tick-based resync)

## Conclusion

The `edgion.io/leader` pod label is no longer fire-and-forget. `update_pod_leader_label`
returns success, and the `run()` loop tracks a local `label_synced` flag: when a patch
fails at a state transition, every subsequent election tick (default 2s) re-patches the
label with the *current* leadership state until it converges. Re-reports of the form
"label patch failure is swallowed / label permanently desyncs from the lease holder /
leader Service selector loses its endpoint" are **not accepted**.

Fix suggestions of the form "spawn a dedicated retry task with backoff" or "make the
label patch blocking before announcing leadership" are **not accepted**.

## Core Rationale

**1. The label is a routing selector, so desync was a real outage, not cosmetics**

`deploy/kubernetes/controller/service.yaml` selects `edgion.io/leader: "true"` for the
`edgion-controller-leader` Service. In the default `leader-only` HA mode all Gateways
sync config through that Service. A transient apiserver error exactly at the
acquired-leadership transition left the Service with zero endpoints, and nothing ever
rewrote the label while leadership stayed stable — a non-self-healing config-sync
blackhole observable only as one `error` log line.

**2. The election loop is already the retry scheduler**

`run()` ticks every `retry_period_secs` regardless. Piggybacking the resync on the tick
(re-patch only while `label_synced == false`, always with the current `is_leader()`
state) is bounded, needs no extra task/lock/shared state, and is naturally correct when
leadership flips again between a failure and its retry.

**3. The remaining unguarded paths are covered elsewhere**

- Startup: `preflight_check()` (`center.rs`) calls `ensure_pod_label_patch_ready`,
  which both fail-fasts on RBAC and resets the label to `false` on every (re)start.
- Shutdown: the patch stays best-effort (one shot) — the loop is exiting, a
  terminating pod drops out of Service endpoints, and a restart resets the label in
  preflight.
- Identity mismatch is a non-issue: `POD_NAME` is injected via the Downward API
  (`metadata.name`), and `with_identity` has no production caller.

## Fix Suggestions Not Accepted

- Dedicated retry task / exponential backoff — duplicates the existing tick cadence and
  adds task-lifecycle and ordering complexity for no convergence benefit.
- Step down / force re-election after N failed label patches — the failure cause
  (apiserver outage, RBAC, admission webhook) is shared by all replicas, so a new leader
  fails the same patch and the cluster ping-pongs leadership (cache rebuild + gRPC
  interruption per switch). Global apiserver failure already forces step-down via the
  lease `renew_deadline`; the label is derived routing state and must not drive
  leadership. If desync visibility is ever needed, add a controller metric instead.
- Block leadership announcement until the label patch succeeds — couples reconcile
  startup to an unrelated apiserver write; lease ownership, not the label, is the
  source of truth for leadership.
- Integration-test scenario — the trigger is a transient apiserver failure exactly at a
  leadership transition; the integration harness cannot inject apiserver faults and has
  no leader-election scenario today.

## Re-evaluation Triggers

Re-open this decision only if:

- The label gains consumers beyond Service selectors that need stronger guarantees
  (e.g. ordering with reconcile start).
- The election loop's single-tick cadence is removed/refactored so there is no longer a
  natural retry point.

## Reference Cases

- Original task: `tasks/todo/leader-election-pod-label-set-failure.md` (closed 2026-06-10)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/leader_election.rs` —
  `label_synced` in `run()`, `update_pod_leader_label` returning `bool`
- `skills/01-architecture/01-controller/03-config-center/02-kubernetes/01-ha-mode.md` —
  leader-only mode, leader Service routing
