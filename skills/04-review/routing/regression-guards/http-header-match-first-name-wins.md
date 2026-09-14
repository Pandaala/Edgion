---
name: http-header-match-first-name-wins
description: Use when reviewing HTTPRoute header predicate decoding, duplicate names, regex indexes, delegation, or header specificity.
knowledge_type: regression-guards
---

# HTTP header predicates retain the first equivalent configured name

## Conclusion and scope

Within each `HTTPRouteMatch.headers`, only the first ASCII-case-equivalent
name contributes a predicate. Its original spelling, match type, value, and
relative order survive. Distinct names remain AND conditions and separate
matches remain independent OR alternatives. Missing/null and empty vectors
keep their original representation.

`deserialize_header_matches` in `edgion-resources/src/resources/http_route.rs`
owns normalization at typed decode. Controller validation and delegation,
Gateway header regex compilation, request matching, and header specificity
therefore consume the same effective conditions. No request-time deduplication,
allocation, logging, or additional cache is introduced.

## Rationale and evidence

[Gateway API v1.5.0 HTTPHeaderMatch](https://github.com/kubernetes-sigs/gateway-api/blob/v1.5.0/apis/v1/httproute_types.go#L577-L590)
requires first-equivalent-name semantics. The vendored HTTPRoute CRD has a raw
`name` list-map key, so `X-Mode` and `x-mode` can both pass Kubernetes admission.

For `CAND-P2-002`, a match declaring `X-Mode: one` then `x-mode: two`
incorrectly required both values. Equal duplicate values instead inflated
`build_specificity`'s header count and could override later tie-breakers.
`compile_match_regexes` and `deep_match_common` use positional indexes into the
same vector, so deduplicating only one consumer would be insufficient.

Decode precedes `validate_http_route_if_enabled`, which must not reject an
otherwise valid route because an ignored later predicate has an invalid regex.
Decode also precedes `route_delegation::merge_scope_constraints`: that function
merges equal effective constraints and rejects conflicting mandatory parent
constraints. Gateway conf-sync full and watch decoding cover both declared
`rules` and controller-generated `resolved_rules`.

## Alternatives and limits

Gateway-only preprocessing misses Controller validation and delegation.
Request-time deduplication repeats configuration work and leaves ranking wrong.
The schema fields and public types remain unchanged. Rust callers constructing
matches directly must preserve uniqueness themselves; current production
constructors either have no headers or merge canonical delegation inputs.
Revisit this boundary if a new programmatic producer adds header predicates.

This rule concerns configured HTTPRoute predicates only, not repeated headers
in request messages, header modifiers, GRPCRoute, or duplicate query predicates.
Structurally malformed entries can still fail typed deserialization; ignoring
a duplicate does not bypass required-field/type decoding or Kubernetes schema
validation.

## Verification and re-evaluation

Fresh verification on 2026-09-12:

- `cargo test -p edgion-resources --lib resources::http_route`: 59 passed.
- Controller header validation regression: 1 passed; Controller
  `route_delegation` filter: 84 passed (HTTP and gRPC delegation tests).
- `cargo test -p edgion-gateway --lib routes::http::`: 419 passed.
- `cargo fmt --all`, workspace/all-targets check and Clippy passed. Existing
  Clippy warnings remain; none of the diagnostics point to changed lines.
- Agent-doc, SSA force, metrics inventory, Pingora source policy, and Gateway
  no-tracing guards passed.
- Three fresh independent read-only code reviewers returned LGTM for this
  correction and its direct matching call chains.
- Shared FileSystem Controller/Gateway `HTTPRoute/Match`: 17 cases passed,
  including 5 new groups with 19 wire probes checking exact backend identity,
  first-value/reversal, later-rule priority, regex alignment, AND and OR.
- `HttpRouteDuplicateHeaders`: passed on a fresh shared topology, checking
  absent -> one/30001 -> two/30002 -> one/30001 -> absent.
- All three new static/dynamic route generations passed server-side dry-run
  against the local Kubernetes API server's v1.5.0 CRD. Only metadata namespace
  changed to `default` for this non-persisting check because the test namespace
  was absent. This is schema-admission evidence, not Kubernetes watcher,
  status, or automatic EndpointSlice wire verification.

The first dynamic attempt followed the integration runner's final wipe-all
and reloaded its Gateway into the same process; both static and dynamic
requests then returned 404. This is consistent with the listener recreation
failure recorded during this review under
`http-listener-recreation-stale-route-manager`.
On 2026-09-13 the user accepted this as an
[unsupported Gateway lifecycle limitation](../../listeners/intentional-design/gateway-recreation-requires-restart.md)
requiring a data-plane restart, with no production fix. The failed run remains
historical evidence; this disposition does not turn it into a passing test.
The successful run used the standard runner's fresh topology, deferred the
residue wipe until after the dynamic test, and did not restart during scenario
phases. This correction does not claim to fix listener deletion/recreation.

Fresh live Kubernetes verification on 2026-09-14 used current-source arm64
Controller, Gateway, test-client, and test-server images on the `orbstack`
context. After the standard post-fixture Gateway restart, `HTTPRoute/Match`
passed 17/17, including all five duplicate-header groups. The route reported
`Accepted=True` and `ResolvedRefs=True`; its selector-owned Service resolved to
three Pod endpoints. `HttpRouteDuplicateHeaders` then passed delete -> initial
(`one`/30001) -> update (`two`/30002) -> initial -> cleanup/restore with observed
Controller/Gateway resource-version convergence.

The first Kubernetes prepare attempt exposed a test-fixture layout defect: this
scenario used `updated/`, but the canonical initial loader and ownership guard
exclude only `updates*` and `delete*` generations. Renaming both local and
Kubernetes fixture directories to `updates/` and changing the scenario path
restored the standard dynamic-fixture contract. The ownership validator's own
regression test and a full-conf ownership scan then passed.

The mandatory Kubernetes inventory-parity guard still fails for six unrelated
dynamic scenario gaps. Its own regression test passes, this scenario is
registered in both inventories, and the remaining work is tracked by
`tasks/todo/k8s-dynamic-scenario-inventory-parity.md`. The selected task remains
blocked only by that global gate; no empty registrations or unsupported
exceptions were added. Full workspace tests were not run as part of the live
traffic reproduction.

Re-evaluate if the protocol contract, typed ingress, delegation merge,
programmatic match producers, or compilation/indexing ownership changes.

## Resolution references

- `CAND-P2-002` / `http-route-duplicate-header-match-names`: confirmed correctness
  defect, corrected with targeted and FileSystem wire verification on
  2026-09-12 and live Kubernetes wire verification on 2026-09-14; active task
  retained only for the unrelated inventory-parity closure gate.
