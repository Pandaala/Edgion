---
name: ebtp-typed-service-key-btp-requeue
description: Use when reviewing BackendTLSPolicy-to-EdgionBackendTrafficPolicy requeue edges or changing either policy's conflict-index key format; covers the required translation from BackendTLSPolicy section-aware keys to EBTP typed Service keys.
status: fixed
finding_id: ctrl-03
closed: 2026-08-09
---

# BackendTLSPolicy Requeues Use the EBTP Typed Service Key

## Conclusion

BackendTLSPolicy changes, deletes, and retargets must query the
EdgionBackendTrafficPolicy conflict index with the same typed
`PolicyTargetKey` identity used by the EBTP writer. For a core Service that key
is `"/Service/<namespace>/<name>"`; the BackendTLSPolicy-local
`"<namespace>/<name>[::section]"` key is not interchangeable.

The fixed reader constructs the typed Service key for current targets and
translates dropped BackendTLSPolicy base keys before querying the EBTP index.

## Core Rationale

The two policy types deliberately need different local identities:

- BackendTLSPolicy conflict precedence is section-aware and remains keyed by
  `make_target_conflict_key`.
- EdgionBackendTrafficPolicy supports multiple target kinds, so its conflict
  index uses `PolicyTargetKey::to_string()` to keep Service and
  EdgionAIService identities disjoint.

Using a BackendTLSPolicy key directly against the EBTP manager produces an
exact-lookup miss. The missing requeue leaves EBTP's cross-object
`upstreamAuthority` status stale until an unrelated event happens to reconcile
the policy.

## Fix Suggestions Not Accepted

- Revert only Service-target EBTP registrations to the legacy `ns/name` key.
  A single EBTP conflict manager must retain one typed identity scheme for all
  supported target kinds.
- Remove section suffixes from BackendTLSPolicy's own conflict index. Section
  identity is part of that policy's sibling arbitration contract.
- Add a fuzzy or prefix lookup to `BidirectionalRefManager`. Exact keys are a
  useful invariant; callers crossing index contracts should translate
  explicitly.

## Re-evaluation Triggers

Revisit the translation if BackendTLSPolicy gains a non-Service target kind,
if `PolicyTargetKey` serialization changes, or if both resources move to a
shared typed key structure at the storage boundary.

## Reference Cases

- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs`
  (`requeue_dependent_ebtps`, dropped-target sweep).
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend_traffic_policy.rs`
  (`parse`, conflict-index registration).
- Regression tests:
  `backend_tls_policy_change_requeues_dependent_ebtp`,
  `backend_tls_policy_delete_requeues_dependent_ebtp`, and
  `parse_policy_requeues_dropped_target_ebtps_on_retarget`.
- Sibling rule: `btp-section-key-requeue-gap.md`.
