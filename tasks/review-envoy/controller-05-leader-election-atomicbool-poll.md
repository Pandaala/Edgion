---
module: controller-translation
title: Leader election state propagates via AtomicBool poll, creating a window where two replicas simultaneously write status
severity: medium
envoy_reference: client-go / controller-runtime leader election: the controller does not start processing (and therefore does not write status) until it has won the election; leadership loss immediately stops the reconcile loop, not just gates individual writes. The leader write guard is a start/stop lifecycle gate, not a per-write boolean check.
---

# Leader Status Write Guard Is a Per-Write Poll, Not a Lifecycle Gate

## Envoy/Istio design (the proven baseline)

In client-go's `leaderelection` package (used by Istio Pilot and all controller-runtime controllers), the controller callback is only invoked while the leader holds the lock. The `OnStartedLeading` callback runs the entire reconcile loop; `OnStoppedLeading` cancels the context, stopping all reconciliation. There is no per-write boolean check: the reconciler either runs (leader) or does not run (non-leader). This prevents split-brain status writes: when leadership transitions, the old leader's reconcile goroutine is cancelled before the new leader starts.

## Edgion's current design

Edgion uses `LeaderHandle::is_leader()` — an `Arc<AtomicBool>` — as a per-write guard in the worker loop:

```rust
// resource_controller.rs:530
let can_write_status = leader_handle.as_ref().is_none_or(|h| h.is_leader());
if status_changed && can_write_status {
    persist_k8s_status(...).await
}
```

The `AtomicBool` is written by the `LeaderElection::run()` background task (`leader_election.rs:250,265`) with `Ordering::Release`, and read with `Ordering::Acquire`. Memory ordering is correct.

However, the race condition is not about memory ordering — it is about the window between the old leader setting `is_leader=false` and the worker's current iteration reading the flag:

1. Worker dequeues item, calls `process_work_item()` (may take O(tens of ms) for complex routes with many backend refs).
2. Mid-processing, the lease expires and `LeaderElection::run()` sets `is_leader=false`.
3. Worker finishes processing, reads `is_leader()` at line 530 — sees `false` — skips status write. Correct.

But consider the reverse scenario (AllServe mode):

1. Two replicas both call `process_work_item()` simultaneously (because in AllServe mode, all replicas run watchers and workers).
2. Replica A is leader, Replica B is not. Both process the same item (triggered by the watcher on both replicas).
3. Replica B's leader status transitions from `false` to `true` (it just won the lease) between Replica B's `process_work_item()` start and the `is_leader()` check at line 530.
4. Both Replica A and Replica B write status for the same resource within the same reconcile cycle.

This is a status write race during leadership transition, not a functional split-brain (K8s handles concurrent patch_status with optimistic concurrency on the resource version), but it results in redundant API calls and potentially out-of-order status writes during the transition window.

More critically: in `leader-only` mode, the non-leader replica is entirely idle (`wait_until_leader()` blocks). But in `AllServe` mode (which is an explicitly supported and documented HA strategy), both replicas run workers and the leadership check is purely the per-write AtomicBool. The `run_serving_flow()` in `center.rs` makes this explicit:

```
run_all_serve_lifecycle():
  └── loop:
      └── run_serving_flow()
          ├── start_event_watchers_all_serve()
          │   ├── Controller (with leader_handle)   # status writes are guarded
```

The comment "status writes are guarded" refers to the per-write `is_leader()` check, not a lifecycle gate.

## The problem / gap

1. **Per-write guard does not stop in-progress reconciliation**: After `is_leader` flips to false, any worker already inside `process_work_item()` will complete the full translation pipeline (validate, preparse, parse, update_status, on_change, save-to-cache) before reaching the `is_leader()` check. The new state is committed to `ServerCache` (updating the gRPC sync source) and `on_change` triggers cross-resource requeues — all from a non-leader replica. Only the final status write is gated. In AllServe mode, this means non-leader replicas are pushing config to the data plane without being responsible for status writes, which is the intended behavior, but the transition window produces spurious `on_change` side effects (cross-resource requeues, tracker updates) from a replica that just lost leadership.

2. **No graceful quiesce on leadership loss**: client-go calls `OnStoppedLeading` and cancels the context, ensuring in-flight reconciles drain before the old leader declares itself done. Edgion has no equivalent: `LeadershipLost` in AllServe mode calls `stop_leader_services()` (stops ACME) but the existing worker tasks continue running indefinitely. A newly-elected leader may start writing status while the old leader's workers are still mid-reconcile, creating a brief window with concurrent status writes.

3. **The `leader_handle` is optional (`Option<LeaderHandle>`)**: When `None`, the check `is_none_or(|h| h.is_leader())` returns `true`, meaning all writes are unconditionally allowed. This is correct for single-replica and FileSystem modes, but if the `Option` is accidentally `None` in a multi-replica K8s deployment (e.g., due to a missed injection), no status write guard applies. There is no warning or assertion for this case.

## Impact

- In AllServe mode: brief windows during leadership transition where both the old and new leader write status for the same resources, resulting in potentially out-of-order `lastTransitionTime` updates visible to external tools.
- Worker tasks from a just-demoted leader continue running cross-resource requeues (`on_change`) after losing leadership, potentially causing unnecessary Gateway syncs in the data plane.
- No auditability: there is no log or metric for "leadership transition: workers still in flight = N".

## Verification note

Severity downgraded from high to medium. Concurrent status writes during leadership transition are real (confirmed: `stop_leader_services()` at line 909 stops only ACME, does not cancel worker tasks or drain in-flight reconciles; workers continue running from the demoted replica). However, the concurrent write race does not cause data corruption: Kubernetes uses optimistic concurrency on the resourceVersion, so a conflicting write returns 409 Conflict and the losing write is silently dropped. The more significant gap is the missing worker quiesce: non-leader workers continue committing to `ServerCache` and firing `on_change` cross-resource requeues after losing leadership. The `leader_handle = None` injection risk is real but low-probability (would require a code bug in the wiring layer).

## Suggested direction

Introduce a cancellation token (e.g., `tokio_util::sync::CancellationToken`) that is created fresh when leadership is acquired and cancelled when leadership is lost. Pass this token to all worker tasks spawned during a leadership term. Workers check the token at natural suspension points (before the status write, or between workqueue items). On leadership loss, `stop_leader_services()` cancels the token and waits for in-flight workers to drain (with a timeout) before returning. This matches the client-go `OnStoppedLeading` / context-cancellation pattern and eliminates the per-write AtomicBool race.
