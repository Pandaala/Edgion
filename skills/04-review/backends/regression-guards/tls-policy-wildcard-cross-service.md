---
name: tls-policy-wildcard-cross-service
description: Use when reviewing BackendTLSPolicy target or section isolation; current publication rejects multi-target policies and records invalid occupancy per canonical target.
knowledge_type: regression-guards
---

# BackendTLSPolicy target and section isolation

## Conclusion and scope

One valid Gateway `BackendTLSPolicy` names exactly one target. A policy with
multiple `targetRefs` cannot contribute a valid exact-section or whole-target
winner. Instead, the Gateway records invalid occupancy for every canonical
target and section named by that policy. A section belonging to one Service can
therefore never become a valid match for another Service.

Single-target policies are additionally isolated by typed `PolicyTargetKey`, so
a core `Service` and an `edgion.io/EdgionBackend` with the same namespace and
name cannot share candidates or winners.

## Current rationale and evidence

`BackendTLSPolicyStore::is_valid_candidate` rejects any policy whose
`target_refs.len() != 1`. It also requires the Controller-resolved target class
to match the canonical target kind and rejects a section-scoped
`EdgionBackend` target.

Publication still retains deserializable resources that are runtime-invalid or
semantically invalid for diagnostics. For each target bucket,
`build_target_buckets` calls `valid_target_key`; when a candidate is not valid,
it visits that policy's targetRefs and marks invalid occupancy only where the
targetRef's canonical key equals the bucket's typed target. Lookup then uses the
fixed occupancy order:

1. valid exact-section winner;
2. invalid exact-section occupancy;
3. valid whole-target winner;
4. invalid whole-target occupancy.

This makes the boundary fail closed without allowing one targetRef's section to
select TLS policy for another target.

Focused source evidence is in
`edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs`.
Regression coverage includes
`multiple_target_refs_are_invalid_on_every_named_service`,
`typed_targets_and_sections_do_not_leak`, and
`invalid_exact_section_does_not_fall_back_to_whole_target`.

## Historical mechanism

`backends-F006` originally fixed an older implementation in which a policy could
be indexed under several Services and request query-time closures scanned all of
its targetRefs. Those closures were changed to filter by target name before
checking `sectionName`, preventing another Service's ref from satisfying the
query.

That query-time mechanism, `select_policy_for_service`, and the former
`build_reverse_index` are gone. The historical fix remains evidence for the
cross-target bug class, but it is not the reason current code is safe. Reviews
must verify the current single-target validity and typed target-bucket rules
above.

## Limits and re-evaluation

- An invalid exact candidate blocks whole-target fallback only when there is no
  valid exact winner. A valid whole winner similarly precedes invalid whole
  occupancy. This is occupancy-first behavior, not precedence arbitration
  between valid and invalid policies.
- Unsupported group/kind refs cannot form a canonical target bucket. The raw
  policy remains visible, but it cannot affect a supported target lookup.

Re-evaluate if BackendTLSPolicy begins accepting multiple valid targets, target
keys stop including `PolicyTargetKind`, cross-namespace refs are introduced, or
the fixed lookup occupancy order changes.

## Resolution references

- `backends-F006` — historical cross-Service targetRef filtering fix,
  2026-05-24; owning mechanism later replaced.
- `incremental-publication-02-backend-tls-policy` — current target-local
  candidate and arbitration publication, 2026-09-08.
