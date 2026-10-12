---
name: active-health-task-status-ownership
description: Use when reviewing active health task replacement, stale probe completion, or HealthStatusStore residue after Service policy removal.
knowledge_type: regression-guards
---

# Active health reconciliation owns status markers

## Conclusion and scope

`HealthCheckManager` owns `HealthStatusStore` Service-marker registration and
removal inside its serialized reconciliation path. `health_check_loop` does not
hold the store and cannot recreate a marker after an aborted task's policy has
been removed.

Actual routing eligibility and success/failure thresholds are separate. Probe
completions go to `ServiceDiscoveryCoordinator::record_health_completion`,
which accepts a result only when the health-policy identity, Service membership,
and endpoint runtime identity still match. A backend that remains in the same
active generation may therefore contribute a valid result; removed endpoints or
superseded policy generations cannot affect current routing state.

## Rationale and evidence

`JoinHandle::abort` requests cancellation but does not synchronously interrupt
code already executing between await points. Previously, every probe cycle
called `HealthStatusStore::register_service` after reading the current backends.
Removal aborted the task and then unregistered the Service. An old task already
in that synchronous interval could register afterward and leave an empty entry
visible through the admin cache statistics.

The correction moves the empty marker registration to `upsert_task`, after the
old handle is cancelled and before the replacement is published. The no-runtime
failure path cancels the old task and unregisters its marker. Policy removal
continues to cancel the task before unregistering the marker. Because the loop
has no reference to `HealthStatusStore`, abort timing can no longer reverse the
marker removal.

The generation fence for real health state is implemented independently in
`HealthCheckConfigStore` and `ServiceDiscoveryCoordinator`. The Coordinator
resets threshold counters on policy replacement/removal and rejects a completion
whose policy `Arc`/generation, membership identity, or endpoint identity is no
longer current.

## Alternatives and limits

Awaiting every aborted task or adding a cancellation token is unnecessary for
this marker-only race. Probes are already cancellation-safe at their await
points, and the Coordinator independently validates every result. Keeping one
generation field in the legacy status store would duplicate ownership without
improving routing correctness.

ARCH-022 removed the former `HealthStatusStore::record_check`/`is_healthy`
APIs and hash-keyed threshold state. Their earlier retention for isolated tests
was historical duplicate ownership, not a routing invariant. The store now holds
only a Service-key set in one `ArcSwap` with serialized membership writers.
Threshold transitions, opposite-counter resets and same-socket isolation tests
exercise real Coordinator completions and identity-bound runtime state.
The store's role remains the bounded Service marker exposed by admin statistics. This rule does not change probe concurrency, startup jitter, ticker
cadence, transport policy, or the global semaphore.

## Verification and re-evaluation

The focused manager lifecycle test verifies that reconciliation creates the
marker for an active policy, removes it with the policy, and leaves no task entry
after cancellation. Replacement checks observe old-task completion; synchronous
no-runtime reconciliation cancels a seeded dormant task and clears count/keys.
Concurrent registry writers retain all expected keys and remove them to zero.
The Coordinator policy-generation test verifies that policy
removal and replacement reject an old probe while accepting the current one.
Repository-wide closing checks are reported by the issue runner.

Re-evaluate if background probe tasks regain write access to a Service-level
status registry, if status markers acquire routing semantics, or if completion
validation stops checking policy, membership, or endpoint identity.

## Resolution references

- `health-task-status-generation-ownership` — fixed by assigning marker writes
  to serialized reconciliation; the original high-severity threshold-pollution
  claim had already been eliminated by Coordinator generation ownership, leaving
  a low-frequency admin-statistics residue race.

- `ARCH-022` — removed the unused hash-keyed threshold engine while preserving
  marker ownership and moving meaningful algorithm coverage to the Coordinator.
