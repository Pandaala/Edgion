# Discovery section traversal storage

Status: deferred for user decision. No runtime defect or replacement is selected.
Source baseline: 277050cd0.

## Evidence

The private unique_resolved_sections and unique_normalized_sections helpers in
edgion-gateway/src/backends/discovery/coordinator.rs deduplicate section Arc
pointers while traversing the ordered alias map. They return Vecs of borrowed
references. Each helper's seen HashSet drops before the caller consumes its Vec.

The consumers have different storage behavior:

- canonical_probe_backends grows its own backend-identity set and owned output.
- identified_backends_for_reuse grows proof/socket maps containing owned backend
  Arcs; later insertions retain the existing overwrite precedence.
- projection_membership_identity collects and sorts the complete proof multiset.
- unique_normalized_slot_count only accumulates a scalar count.

A lazy iterator or visitor can remove the intermediate reference vector, but
keeps section-deduplication storage alive while the first three consumers build
their own collections. Fewer allocations do not establish lower peak retained
storage or faster execution. Count-only traversal has no growing output and
should be assessed separately rather than assigned the same tradeoff blindly.

## Decision and acceptance

Retain the current implementation, or authorize a bounded measured traversal
change. Evaluate a specialized count path separately from general iterator/visitor
conversion. No alternative is selected. Keeping the current helpers is a valid
outcome when savings do not justify added complexity.

Before implementation, use IMPL with an exact allow-list and review:

- Allocation count/bytes, reference copying, peak retained storage and timing for
  empty, alias-heavy, multi-section and endpoint-heavy inputs. Include retained
  output capacity, not just allocations inside the helper.
- Pointer-based alias deduplication and invalid-section skipping. Equal socket
  addresses or equal field contents do not merge distinct section identities.
- Ordered traversal and last-insert reuse-map precedence, unchanged slot/proof
  multiplicity and fingerprint bytes, and exact epoch reservation demand.
- Owned backend outputs and existing captured-generation, retirement and stale
  completion boundaries. Borrowed traversal must not escape the owning maps.

Do not add a persistent canonical-section inventory, index or cache as an
incidental optimization. Such a representation needs a separate ownership and
retained-memory design. Do not remove required normalization/preflight passes or
weaken fail-closed epoch handling to avoid traversal.

## Evidence limits

Relevant sources are coordinator.rs and its mirrored coordinator_test.rs,
skills/01-architecture/02-gateway/07-backends.md, and
skills/04-review/backends/regression-guards/discovery-normalized-geometry-ownership.md.

Fresh existing coordinator tests passed 43 cases serially. They cover aliases,
distinct identities, reuse, generations, retirement and epochs, but do not
measure traversal storage or latency. No runtime/test changed, no alternative
was prototyped, and no full suite, live transport or benchmark ran.
