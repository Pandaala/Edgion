---
name: leader-pod-label-tick-resync
description: Use when reviewing leader-election pod label findings; covers the tick-based resync that replaced the fire-and-forget `edgion.io/leader` patch.
status: fixed
finding_id: leader-election-pod-label-set-failure
closed: 2026-06-10
knowledge_type: regression-guards
---

# Leader Pod Label Fire-and-Forget Desync — FIXED (tick-based resync)

## Conclusion

The `edgion.io/leader` Pod label is derived routing state. The election loop tracks
`label_synced` and retries the current state on subsequent ticks after an error or
timeout. It announces acquired authority before label IO. Dedicated retry tasks,
label-failure-driven elections and blocking leadership announcement on label
success remain rejected within this scope.

ARCH-112 corrects an uncovered boundary in the original fix: a pending Lease or
label request previously prevented the loop from checking `renew_deadline`.
`run` now owns an absolute Tokio deadline across sleep, Lease GET/CAS and label
IO. Expiry clears both the watch flag and stored fence before further IO.
`try_acquire_or_renew` returns a candidate fence without publishing it; the owner
rejects results at or after expiry. Reacquisition requires a fresh GET/CAS and new
local term, even when a cancelled server write left this identity as holder.

Label attempts run after Lease processing and are bounded by the next scheduled
attempt, followed by only the remaining retry sleep. Thus permanently stalled
labels do not double the cadence or cause avoidable loss even with retry=2s and
renew deadline=3s. Transient Lease errors preserve the existing absolute deadline;
404/409/competing-holder loss remains immediate. This supersedes the original
blanket statement that all label-desync reports were disproved: the old tests and
implementation did not establish behavior under pending IO.

Cancellation is local: a submitted server write may still complete. Each label
attempt now reads the Pod and merge-patches with its nonempty UID/resourceVersion,
the desired label and a fresh `edgion.io/leader-label-write-id` UUID. The marker
forces a real metadata change even when the label already matches; a label-only
no-op can retain resourceVersion and would leave an earlier opposite patch valid.
Only a response preserving UID, label and marker with a changed nonempty version
acknowledges synchronization. Missing metadata, conflicts, mutated responses and
errors remain dirty for a fresh read on the next tick, within the existing bounds.

An earlier prepared patch cannot overwrite a newer acknowledged synchronization:
if the newer write commits first, the old resourceVersion conflicts; if the old
write commits first, the newer attempt conflicts and retries. UID protects against
Pod recreation between read and write. Merge preserves unrelated labels and
annotations. Both Pod `get` and `patch` permissions are required; the canonical
controller Role already grants both and preflight identifies the failed operation.

This is conditional-write protection for submitted patches, not leadership
fencing or exactly-once execution. It does not stop another process from performing
a fresh GET against the same Pod name, prevent external label edits after success,
or guarantee bounded recovery under persistent API/admission failures. Shutdown
remains a one-shot best effort. Labels remain derived routing state.

## Core Rationale

**1. The label is a routing selector, so desync was a real outage, not cosmetics**

`cicd/deploy/kubernetes/controller/service.yaml` selects `edgion.io/leader: "true"` for the
`edgion-controller-leader` Service. In the default `leader-only` HA mode all Gateways
sync config through that Service. A transient apiserver error exactly at the
acquired-leadership transition left the Service with zero endpoints, and nothing ever
rewrote the label while leadership stayed stable — a non-self-healing config-sync
blackhole observable only as one `error` log line.

**2. The election loop is already the retry scheduler**

`run()` schedules the next attempt after `retry_period_secs` from Lease completion. Piggybacking the resync on the tick
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
- The election loop's cadence, active deadline ownership or cancellation boundaries change.

## Reference Cases

- Original task: `tasks/todo/leader-election-pod-label-set-failure.md` (closed 2026-06-10)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/leader_election.rs` —
  `label_synced` in `run()`, `update_pod_leader_label` returning `bool`
- `skills/01-architecture/01-controller/03-config-center/02-kubernetes/01-ha-mode.md` —
  leader-only mode, leader Service routing

## Verification boundary

Production-owner regressions live in
`edgion-controller/tests/unit/conf_mgr/conf_center/kubernetes/leader_election_test/renewal_tests.rs`.
They drive the real loop through an in-process kube service with paused Tokio time,
covering held GET/replace, acquisition/retry label stalls, tight cadence, sleep
expiry, grace/recovery, deadline/result competition, late response and fresh term,
authoritative loss, shutdown and preflight. The Pod model preserves state and
resourceVersion, deliberately retains versions for no-op writes, and commits held
patches independently of canceled client response futures. Regressions cover late
opposite-value writes in both directions, fresh-read conflict recovery, Pod GET
stalls, metadata/permission failures, recreation, response validation and preservation
of unrelated metadata. No sockets or Kubernetes cluster are
used. These checks do not prove remote write cancellation or scheduler progress
when the runtime cannot poll the election task.
