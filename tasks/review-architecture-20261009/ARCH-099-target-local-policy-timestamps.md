---
id: ARCH-099
status: candidate
kind: architecture-debt
priority: P3
partition: PART-403
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-099: Project only a target's policy timestamps under the existing snapshot lock

## Current ownership and concrete cost

BackendTLSPolicyState owns a bidirectional target-reference manager and a separately locked timestamp map. In backend_tls_policy_ref.rs:94-122, is_winner first obtains the target's references and preserves a fast return for zero or one reference. For a conflicting target, it clones the entire timestamp HashMap, including owned policy-key strings, before projecting timestamps into the target-local entries vector.

Both BackendTLSPolicyHandler::update_status and EdgionBackendTrafficPolicyHandler::update_status use this implementation for real policy arbitration. Their runtime instances remain separate. Eligible policy registration, removal and sibling requeues maintain each instance's inputs.

Consider many independent conflicting policy pairs, each targeting a different Service. A decision for one pair needs two timestamps but copies the keys and timestamps of every registered policy in that instance. A reconciliation pass over those pairs repeats the inventory-wide copy. This couples target-local arbitration work to unrelated policy population; it is more than a small target-local Vec or sort. The source proves the copies, not a measured latency, throughput or heap regression.

## Bounded improvement

Keep the existing reference lookup and zero/one-reference shortcut. Acquire one timestamp read guard and build the same target-local entries vector from only the referenced policy keys and copied DateTime values. Release the guard before sorting. Keep the comparator and winner selection unchanged.

This retains one coherent view of timestamp values while eliminating the owned snapshot of unrelated entries. It requires no additional index, cached winner, public abstraction, new lock, or alternate arbitration implementation.

The target-reference lookup and timestamp lock are still separate operations. This proposal does not claim or create an atomic snapshot spanning both owners.

## Required semantics and risks

Preserve oldest creationTimestamp first, namespace/name key tie-breaking, Some timestamps before missing timestamps, the existing zero/one-reference behavior, eligible-target filtering, typed target and section isolation, and existing sibling requeues. set_timestamp(None) must continue to remove a previous timestamp rather than retain stale state.

The read guard must cover every relevant timestamp lookup. Replacing the full clone with separately locked get_timestamp calls would restore the inconsistency that the historical correction addressed. Do not hold the guard through sorting or handler work.

## Original knowledge and deduplication

backend-tls-winner-single-snapshot.md records the previous timestamps.read().clone() correction. Its consistency requirement remains applicable; retaining unrelated owned entries is an implementation detail that can be replaced by one guarded projection.

The BackendTLSPolicy architecture specifies oldest-wins and tie-breaking. ARCH-055 concerns timestamp precision across source/status/transport boundaries and remains independent. This proposal changes neither precision nor ordering.

## Future verification

Retain the existing target-key, single-policy, timestamp ordering, equal-timestamp tie, section isolation and sibling tests. Add focused cases for timestamp removal and unrelated timestamp entries. Source inspection must show one guarded projection and no full timestamp-map clone. No benchmark or production performance result is claimed by this review.
