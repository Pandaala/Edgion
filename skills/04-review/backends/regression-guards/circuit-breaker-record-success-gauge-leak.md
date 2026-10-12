---
name: circuit-breaker-record-success-gauge-leak
description: Current identity-bound Service outlier ownership and preserved regression boundary.
status: fixed
finding_id: g7-006 Issue A
closed: 2026-05-29
knowledge_type: regression-guards
---

# Success settles Service outlier gauges once

Current Service completion uses `record_success_for_backend_with_token` and
coordinator-validated `record_success_for_backend_in`. It resets an existing
passive state's counters, retains its ejection/backoff history, and calls
`EndpointRuntimeState::clear_outlier_ejected`. The runtime atomically clears
its gate and gauge-accounted bit, recording recovery only once. Repeated success,
policy clear/change, cooldown recovery and retirement cannot decrement the same
accounted ejection twice. Late policy/membership completions are rejected.

The historical g7-006 Issue A fix recovered a gauge when removing an ejected
hash entry. That execution wrapper and drain path are removed under ARCH-018;
entry removal is no longer the normal success contract. Preserve the balance
invariant, not the obsolete `ejected_at`/entry-removal recipe.

`test_record_success_on_ejected_does_not_leak_gauge` now drives actual
identity/token APIs and checks a local recorder's gauge 1 -> 0 -> 0 while
asserting retained backoff. Lazy cooldown and policy/retirement tests assert
same once-only settlement. These synchronous metric assertions do not claim
coverage of thread-local recorders on concurrent worker threads.

Re-evaluate whenever runtime settlement or completion fencing changes.
Source: outlier_detection/store.rs and discovery/coordinator.rs.
Provenance: g7-006 Issue A (2026-05-29); ARCH-018.
