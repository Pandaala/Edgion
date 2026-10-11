---
id: ARCH-062
status: candidate
kind: architecture-debt
priority: P3
partition: PART-203
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Borrow HTTPRoute matches for prefix-transform validation

## Current ownership and evidence

edgion-resources/src/resources/request_transform.rs:388–411 implements effective_matches_are_prefix by calling its private effective_matches helper. That helper returns an owned Vec<HTTPRouteMatch>, cloning an existing rule.matches or manufacturing a default match when matches is absent. The predicate only needs to determine whether the effective list is nonempty and every effective path match type is PathPrefix. Its owned list is immediately discarded. The owned helper has no other production caller.

HTTPRouteMatch derives Clone and contains a path, header matches, query-parameter matches and an optional method. Cloning the list copies their owned strings and vectors even though prefix-context validation never reads the headers, query parameters or method. It copies every entry before the predicate can reject the first non-prefix entry.

A supported concrete input is one PathPrefix /api match with sixteen exact header matches, each containing a 4096-character ASCII value, and a URLRewrite ReplacePrefixMatch /v2 filter with an ordinary Service backend. The vendored Gateway API v1.6.2 schema permits those header counts and value lengths. This causes at least 64 KiB of header-value contents to be cloned for a path-only question. This figure follows directly from the configured string lengths; it is not a measured heap or throughput result.

The actual consumers are cold validation and publication paths. Both authored and effective shared validation invoke validate_prefix_context. Controller HTTPRoute find_invalid_rule_details validates authored rules; delegation push_effective_rule validates effective rules; Gateway HTTPRoute publication validates each rule before compile_prevalidated_http_rule. Public compile_http_rule also performs validation. Gateway publication validates once per rule before compiling its matches, so a repeated per-match production multiplier is not established and is not claimed. Controller and Gateway validation remain independent trust boundaries.

## Concrete maintenance cost

The private helper turns a borrowed predicate into an owned projection of the entire match schema. Adding a new owned field to HTTPRouteMatch automatically adds another copy to prefix-transform validation, even when the field has no prefix-context meaning. This is an avoidable cold allocation and an ownership mismatch, not a request-path defect or a claim that the existing immutable runtime generation should borrow mutable configuration.

The code already has canonical accessors for effective path-match defaults. normalized_rule_value also constructs its bounded selected-input projection without carrying unrelated header data. No new public abstraction or cache is needed to ask this predicate through borrowed matches.

## Bounded improvement and alternative

Make effective_matches_are_prefix inspect rule.matches.as_deref() and use the existing effective_path_match_type accessor on each borrowed match. Handle absent matches through the existing canonical compiled-contract default directly, without constructing a full HTTPRouteMatch. Remove the now-unused private owned effective_matches helper. Keep the change within the prefix-context validation owner; do not add a public borrowing API, prepared-rule registry, generic projection framework or cache.

Preserve the current distinctions: Some([]) is false; any non-prefix effective path type is false; an explicit match with no path currently has PathPrefix semantics; omitted match types use the canonical default; absent matches use the canonical default; and unavailable-contract behavior must remain unchanged. In particular, do not hardcode PathPrefix for absent matches: the existing manufactured default resolves through the contract and can produce an empty effective type when the contract is unavailable. The borrowed branch must preserve that behavior through the canonical accessor or compiled-contract result.

Keeping the helper accepts the unnecessary full-schema copy. Copying only the path still introduces ownership that this boolean predicate does not require. Caching prepared matches or merging Controller and Gateway validation introduces lifecycle and trust-boundary changes disproportionate to this local cause. Owned copies that build immutable route/runtime generations elsewhere are legitimate and outside this proposal.

## Invariants and future verification

Keep prefix-transform detection, backend checks, transform conflicts, cardinality rules, delegation exemptions, violation order and text, and fail-closed admission unchanged. Preserve generated Gateway API grammar and defaults. No schema, request-body policy, observability, Controller workqueue or runtime matching change is required.

Existing request_transform unit tests cover prefix/default behavior, transport and multiple-backend context. The unrelated_large_match_data test has no prefix filter, so it does not enter this cloning branch and does not demonstrate that prefix validation avoids the copy. Future focused coverage should apply a valid prefix replacement to a large unrelated-header match and retain absent, empty, default and mixed-match cases. Static inspection should establish that the predicate performs no full HTTPRouteMatch or match-vector clone; tests should prove semantics rather than mirror the implementation or invent an allocation benchmark.

The original gateway-api-request-transform-validation-and-preparation guard requires independent authored/effective validation, generated defaults and preparation consistency; it does not require an owned list for this private predicate. Reuse-first and canonical cross-cutting rules favor the existing default accessors. Existing ARCH findings about body ownership, span caches, SAN matching, invocation-local plugin validation and session-lifetime projection address different causes. This proposal is a bounded P3 architecture-debt optimization. No builds, tests or network operations were executed, and no traffic incident or throughput improvement is claimed.
