# Decide claim projection path preparation

Status: deferred for user decision. Scope: immutable claim paths in the shared
JWT, OIDC and JWE header projection plan.

## Evidence

`edgion-resources/src/resources/edgion_plugins/plugin_configs/auth_header_projection.rs`
retains each claim path as a String in ClaimHeaderProjectionEntry. The compiler
validates and canonicalizes destination headers, rejects collisions and sorts
entries. It does not prepare claim path segments.

`edgion-gateway/src/plugins/http/common/auth_common.rs::set_claims_headers`
calls resolve_claim_path for each entry on every projection. That resolver
iterates path.split('.') and calls Value::get with each string key. Splitting
borrows input and allocates nothing; the remaining candidate is repeated
separator scanning, not a path String clone or per-segment allocation.

The same resolver also serves JWT condition extraction. The separate
[condition ownership task](condition-01-jwt-claim-evaluation.md) covers repeated
JSON parsing and eager candidate allocation. Path preparation would not remove
those allocations or resolve that task's response-body allocation constraint.

## Decision

Compare retaining the allocation-free split with storing segment ranges into the
existing path String or separately owned segments in the immutable plan. Ranges
add persistent metadata and usually a collection allocation; separate Strings
also add per-segment ownership. Account for construction, plan clones, overlapping
configuration generations and short one-segment paths as well as repeated deep
lookups. Object lookup may dominate delimiter scanning; no measured performance
benefit is established.

Any selected design must keep one canonical traversal implementation. Do not
fork path semantics among JWT, OIDC, JWE and conditions. A cross-crate change to
the Resources plan requires an explicitly scoped implementation design; this
task authorizes no schema, transport, cache or runtime change.

## Required acceptance

- Preserve case-sensitive object-key traversal, including empty path segments,
  leading/trailing dots and consecutive dots. Do not silently add path validation.
- Numeric segments remain string object keys, never array indexes. A literal
  dotted key has no priority over the existing nested traversal. Missing keys
  and non-object intermediate values still return None.
- Preserve canonical destination order, reserved/duplicate destination checks,
  skipped-write behavior, value formatting, byte budgets and sensitive setters.
- Verify ordinary, missing, empty-key, numeric-key and permitted UTF-8 cases
  against the current resolver. Existing dot-notation tests cover ordinary
  nested and missing paths only, not this complete edge-case matrix.
- Measure complete projection work and retained plan memory across realistic
  path depths and request reuse before choosing a representation. No new cache,
  per-request logging or claim/token exposure is permitted.

This is a source-backed deferred optimization, not a correctness finding or an
allocation-removal claim. No runtime tests or benchmark were run for this
new documentation-only task.
