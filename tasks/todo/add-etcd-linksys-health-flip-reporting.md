# Add Etcd LinkSys Health Flip Reporting

## Status

Ready for a separate design and implementation task.

## Problem

The published Etcd LinkSys generation contributes to
`edgion_link_sys_health`, and the shared Etcd client logs health changes, but
Etcd does not increment `edgion_link_sys_health_flips_total`. Flapping that
occurs entirely between two metric scrapes is therefore absent from the flip
counter.

## Constraints

- Keep metrics and operational observability binary-owned; `edgion-common`
  must not depend on Gateway metrics.
- Preserve published-generation ownership and do not count unpublished probe
  candidates or retired clients.
- Define initialization, generation replacement, deletion, and duplicate
  transition semantics before implementation.
- Do not add endpoint, credential, error-detail, or other high-cardinality
  metric labels.
- Reuse the existing `edgion_link_sys_health_flips_total` inventory entry and
  validated `namespace/name` label.

## Acceptance direction

Introduce a binary-owned transition-reporting seam or equivalent
Gateway-side state, prove exact-once transition counting across generation
replacement, and add deterministic tests for degradation, recovery, deletion,
and unpublished candidates.
