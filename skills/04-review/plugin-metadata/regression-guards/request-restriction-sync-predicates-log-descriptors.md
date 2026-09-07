---
name: request-restriction-sync-predicates-log-descriptors
description: Use when reviewing RequestRestriction condition observability. Request-phase synchronous and body-HMAC predicates emit their precompiled descriptors only after the final logical condition matches; dye evaluation remains silent.
knowledge_type: regression-guards
---

# RequestRestriction predicates log descriptors — FIXED (cl-08)

> **Current-shape note (2026-08-24):** RequestRestriction now consumes the
> shared bounded `ConditionSet`. The match-only guarantee below still applies,
> with the additional rule that a zero/one-leaf expression suppresses atomic
> match descriptors while retaining final decisions, named-group attribution,
> and errors. See `tasks/pending/condition-set-dnf-unification/01-design.md`.

## Conclusion

Fixed on 2026-08-24. `RequestRestriction::group_matches` previously evaluated
`CompiledPredicate::Sync` with `log=None`, suppressing the match-only descriptor
required by D-17 even though the request-phase function already owned a
`PluginLog`.

Synchronous predicates now receive `Some(&mut log)`. Body-validating HMAC
predicates retain their precompiled descriptor beside the verifier. Both paths
push the descriptor only when the final logical condition matches after
`negate`; a non-match pushes nothing. Group evaluation keeps the same ordered
allOf short-circuit and verdict. The existing `Deny group <name>` entry remains
the outer intent signal.

## Boundaries

- RequestRestriction HMAC body predicates retain their dedicated asynchronous
  verifier and body-read failure handling. Verification or digest capture
  failures are terminal and do not emit a match descriptor.
- The per-chunk upstream-response-body wrapper still uses log-less
  `CompiledPluginConditions::evaluate`.
- Dye rules remain log-less.
- No descriptor is formatted on the request path; it was built during condition
  compilation.

The tests `matching_sync_all_of_logs_compiled_descriptors` and
`unmatched_sync_predicate_does_not_log_descriptor` lock both sides of the
synchronous match-only contract. `hmac_body_all_of_matches_signed_request`
locks the body-HMAC descriptor on a real signed request, while
`negated_hmac_body_no_match_logs_descriptor` locks the `NoMatch + negate`
logical-match branch without reading a body.

## Resolution references

- `cl-08` — 2026-08-24.
