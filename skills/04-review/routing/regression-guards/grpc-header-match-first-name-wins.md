---
name: grpc-header-match-first-name-wins
description: Use when reviewing GRPCRoute header predicate decoding, validation, delegation, regex indexes, specificity, or matching.
knowledge_type: regression-guards
---

# GRPC header predicates retain the first equivalent configured name

## Conclusion and scope

Within each `GRPCRouteMatch.headers`, only the first ASCII-case-equivalent
name contributes a predicate. Its original spelling, match type, value, and
relative order survive. Distinct names remain AND conditions and separate
matches remain independent OR alternatives. Missing and empty vectors retain
their original representation.

`deserialize_header_matches` in
`edgion-resources/src/resources/grpc_route.rs` owns normalization at typed
decode. Controller validation and delegation, Gateway header regex compilation,
request matching, and header specificity therefore consume the same effective
conditions. No request-time deduplication, allocation, logging, or cache is
introduced.

## Rationale and evidence

Authority: the pinned Gateway API v1.5.0 GRPCRoute definition and vendored
`config/crd/gateway-api/gateway-api-standard-v1.5.0.yaml`. Both require only the
first equivalent header name to be considered; header names are compared
case-insensitively.

Before this correction, `GRPCRouteMatch.headers` decoded directly into a
vector. Controller validation could reject a later duplicate with an invalid
regex or unsupported type, and delegation could treat it as a mandatory
constraint. Gateway `GrpcMatchInfo::new`, `GrpcMatchSpecificity.header_count`,
and `GrpcRouteRuleUnit::deep_match` then compiled, ranked, and required every
retained entry. A pair such as `X-Mode: one` followed by `x-mode: two` therefore
became an impossible AND predicate, while equal duplicates inflated route
precedence.

Typed decoding precedes every production consumer, including JSON/YAML config
backends and Controller-to-Gateway conf-sync. Under delegation each level matches
its own decoded vector; nothing is merged across levels. This keeps validation,
delegation, positional regex indexes,
specificity, and request matching aligned on one vector.

## Alternatives and limits

Gateway-only filtering would leave Controller validation and delegation with
different semantics. Filtering only compiled regexes would also misalign their
positional indexes with the retained predicates and would not correct
specificity. Request-time filtering would repeat configuration work on the hot
path.

The schema fields and public Rust types are unchanged. Direct Rust callers that
construct `GRPCRouteMatch` without serde must preserve unique effective header
names themselves. This rule does not govern repeated fields in an incoming
request or GRPCRoute header modifier filters.

## Verification and re-evaluation

Fresh targeted verification on 2026-09-14:

- `cargo test -p edgion-resources --lib resources::grpc_route`: 10 passed.
- Controller duplicate-header validation test: passed.
- Controller delegation canonical-vector test: passed.
- Gateway compile/rank/real-session matching test: passed.

Re-evaluate if the Gateway API contract, typed ingress, delegation merge,
programmatic match producers, or header compilation/indexing ownership changes.

## Resolution references

- `tasks/todo/grpc-header-match-first-name-wins.md`: confirmed correctness
  defect, corrected on 2026-09-14 at the shared typed-decoding boundary.
