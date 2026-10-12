---
name: set-service-config-toctou-race
description: Current identity-bound Service outlier ownership and preserved regression boundary.
knowledge_type: regression-guards
---

# Serialized outlier policy replacement

`OutlierDetectionStore::set_service_config_in` serializes writers using
`config_lock`. Equality checking, checked policy-identity allocation,
coordinator policy publication and service-map insert/remove occur inside that
critical section. Replacement does not remove then reinsert the service key.
Identical config preserves passive state and policy identity. Changed policy
updates endpoint completion-policy identities and clears runtime ejection under
coordinator/endpoint locks before publishing the replacement store state.
Captured old tokens then fail identity validation.

The historical backends-F004 correction used a DashMap `entry()` swap. That
implementation detail is superseded; it is not the current correctness gate.
The old hash-only drain path is removed by ARCH-018. Runtime eligibility owns
exactly-once gauge settlement. No new keyed mutex is required.

Coverage: `test_config_change_resets_state_unchanged_preserves`,
`test_concurrent_config_update_no_panic_and_key_present`, and
`policy_replacement_and_retirement_settle_gauge_once_and_reject_late_completion`
in the store unit tests; coordinator's in-flight policy-clear regression.
Re-evaluate if writer serialization, captured-policy fencing, no-op preservation,
or coordinator publication ordering changes. Historical passing results do not
establish current correctness; inspect current code and run these tests.

Provenance: backends-F004 (2026-05-21); ARCH-018.
