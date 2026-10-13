---
name: grpc-method-matching-and-precedence
description: Review GRPCRoute method-only predicates, invalid-rule handling, hostname precedence, and ranked candidate fallback.
knowledge_type: regression-guards
---

# GRPCRoute method matching and precedence

## Conclusion and scope

GRPCRoute supports Exact and regex method-only predicates across services.
Omitted type means Exact; only an omitted outer method matcher is unconstrained.
Explicit empty matchers and empty/invalid Exact names are invalid. Regex may omit
one field, or leave it empty, only when the other is non-empty. Either invalid
pattern invalidates the entire predicate and rule, never becoming Exact/catch-all.
Valid sibling rules remain usable. Delegation still rejects regex children and
inherits the parent service only for omitted matchers or valid method-only ones.
Exact service validation reproduces the vendored CEL Unicode simple case-fold
character set. Exact method validation remains ASCII, and runtime Exact matching
remains literal and case-sensitive.

Matching hostname specificity precedes service length, method length, header
count, creation time, namespace/name, and rule order. Only a declared Route
hostname actually matching the request contributes. Listener intersection may
narrow eligibility but must not promote wildcard/hostless Route priority.
Pattern character counts apply to regex;
Exact is not an additional priority tier. Match index resolves remaining ties.
Every candidate must pass the full predicate, with no-match continuing through
remaining candidates and broader matching hostname groups. This candidate
continuation is separate from protocol fallback. After earlier request-entry
gates succeed, unmatched native gRPC terminates with status 12 when
`has_routes()` reports candidates in the current Listener's published gRPC route
table. HTTPRoute matching remains available when that table has no candidates
and for gRPC-Web; an earlier rejection such as the native-gRPC HTTP/2 frontend
gate terminates before route matching.

## Rationale and evidence

Authority: the pinned [Gateway API v1.5.0 GRPCRoute definitions](https://github.com/kubernetes-sigs/gateway-api/blob/v1.5.0/apis/v1/grpcroute_types.go)
and vendored `config/crd/gateway-api/gateway-api-standard-v1.5.0.yaml`.
Method-only Exact and regex are implementation-specific support choices, not
claims that every Gateway API implementation must support them. The pinned
contract defines the precedence chain and per-rule invalidity behavior.

- `edgion-resources/src/resources/grpc_route.rs`: `compile_method` owns the
  bounded compilation policy; its static Exact service regex mirrors the vendored
  CEL instead of narrowing service identifiers to ASCII. `method_match_error`
  exposes the result to rule validation. No new serialized fields or CRD schema
  changes are needed.
- Controller `handlers/grpc_route.rs` classifies invalid rules and reports
  `PartiallyInvalid`, or `Accepted=False / UnsupportedValue` for all-invalid
  effective rules. Method errors do not use route-wide advisory validation.
  `grpc_route_delegation.rs` applies the same validation before flattening.
- Gateway `routes/grpc/conf_handler_impl.rs` compiles each match once and rejects
  the whole rule before constructing rule runtimes if any method is invalid,
  including status-less/stale configuration. Match units share rule runtime.
- `match_engine.rs` builds one ranked route array. Shared Algo `RadixHostnameMatcher`
  indexes ranks by declared Route hostname;
  its borrowed `matching_values` lookup visits
  exact, descending wildcard suffixes, then the engine's catch-all group.
  Each group merges five indexed streams using integer heads on the stack.
  Rank-dependent contents are rebuilt together; unchanged final hostname keys
  preserve the frozen hostname tree, including full-list and Gateway rebuilds.
- `match_unit.rs::deep_match` remains responsible for headers and attachment.
  Only no-match continues inside the engine; predicate errors propagate to its
  caller. `integration.rs::try_match_grpc_route` returns the match result, while
  `pg_request_filter` owns the protocol fallback/termination decision. ArcSwap
  generation replacement retains in-flight snapshots.

## Alternatives and limits

The old seven-tier engine omitted method-only predicates, let Exact buckets
override normative precedence, and checked hostname too late. Treating method-only
as catch-all would lose its predicate. Sorting all routes per request or scanning
unrelated Exact routes is unnecessary: build-time ranks preserve borrowed hostname
lookup while allowing cross-index precedence. No request allocation, blocking
lock, runtime pattern compilation, or request-level tracing is added by selection.
Regex traversal remains linear in the selected hostname group's regex candidates.
Correctness/instrumented candidate counts do not establish latency equivalence.

Duplicate equivalent gRPC header names are normalized separately at typed
decode; see the
[first-name-wins regression guard](grpc-header-match-first-name-wins.md).
Do not read this method-matching rule as proving complete GRPCRoute conformance.

## Verification and re-evaluation

The following results are historical method-matching repair evidence, not fresh
verification of the shared hostname-index migration.

Independent code review passed after correcting Listener-intersection priority
promotion. `cargo test -p edgion-resources --lib grpc_route` passed (8 tests),
`cargo test -p edgion-gateway --lib routes::grpc` passed (55 tests), and
`cargo test -p edgion-controller --lib grpc_route` passed (62 tests).
`cargo test -p edgion-resources --lib matcher::host_match::hash_match` passed
(20 tests, including the final invalid-input fast path). Workspace all-target
Clippy exited 0 with existing repository warnings. Final
`cargo check --workspace --all-targets`, format/diff whitespace checks, and the
SSA-force, metrics-inventory, and Pingora-source guards passed.
Added tests cover shared JSON/YAML defaults and
validation, Controller status/delegation/publication, all precedence dimensions,
actual Pingora Session header fallback, candidate continuation, and full/partial
snapshot replacement and deletion. The 1,000-Exact-route test counts
visited candidates; it is not a throughput benchmark. Allocation counters cover
Exact selection, hostname fallback, and regex selection after warming its existing
lazy search cache; they do not claim first-use regex allocation is absent.
Existing `edgion-tests/**`
are frozen. No live Kubernetes/etcd admission or HTTP/2 wire test is claimed.
The full agent-doc guard has seven pre-existing broken links outside this repair;
the 13 changed/new documents pass the same guard functions when scoped to the diff.
At the time of those historical results, source tasks remained open because that
repository-wide closure gate was blocked.

Fresh focused correction evidence: `cargo test -p edgion-resources --lib
grpc_route` passed 11 tests, including both non-ASCII simple-fold equivalents and
a valid ASCII sibling branch. `cargo test -p edgion-gateway --lib
routes::grpc::match_engine` passed 24 tests, including literal, case-sensitive
Exact selection for a CEL-accepted service. These focused tests do not constitute
live Kubernetes admission or HTTP/2 wire verification. Current closure verification
also ran `cargo test -p edgion-gateway --lib routes::grpc` (64 passed). Separately,
`cargo test -p edgion-gateway --lib request_entry_tests` passed the request-entry
tests that distinguish native gRPC status 12, an empty gRPC route table's HTTP
fallback, and gRPC-Web HTTP fallback. The in-memory sessions do not establish H2
negotiation or frame behavior. Workspace format, all-target check, all-target
Clippy (with one existing `edgion-tests` warning), the full agent-doc validator,
English-only check, and the SSA-force, metrics-inventory, and Pingora-source
guards also passed.

Re-evaluate on Gateway API version changes, delegation policy changes, new
matching dimensions, or changes to the shared hostname matcher, input normalization,
and table publication. Route wildcards match multiple labels; certificate matching
helpers are outside this contract and remain unchanged.

## Resolution references

- `grpc-route-matching-plan`: umbrella implementation plan for the canonical
  method matcher, ranked candidate engine, shared hostname index, and their
  cross-layer regression coverage.
- `grpc-route-method-only-match-policy`: confirmed correctness defect; repair
  supports the already documented method-only contract and rejects invalid matchers.
- `grpc-route-complete-precedence-and-hostname-ordering`: confirmed correctness
  defect; companion repair applies the full precedence chain across candidate indexes.
- `grpc-route-matching-review-corrections` issue 3: confirmed correctness defect;
  the shared Exact service validator now accepts the vendored CEL character set
  without changing runtime Exact comparison.
- `Astra-doc-01`: corrected the over-broad helper-level HTTP fallback claim;
  request-entry policy remains conditional on protocol and whether the current
  Listener's published gRPC route table contains candidates.

## Delegation rejection attribution (ARCH-343)

The static delegation probe for `/test.OtherService/Whatever` requires both
UNIMPLEMENTED and the same request's canonical `error/RouteNotFound` access-log
entry. Backend status 12 alone is not Gateway routing evidence. The probe sends
a unique W3C traceparent and uses the existing integration test-store projection
and authenticated client; no additional production observability is introduced.

Connection/readiness/RPC work is bounded to 15 seconds, complete log polling to
five seconds, and deletion of only this marker to five seconds, within the
existing 30-second case deadline. Cleanup failure fails the case; cleanup is a
bounded attempt and cannot guarantee removal after outer cancellation or a late
terminal entry. Pure verdict tests distinguish wrong trace, missing diagnostic,
wrong diagnostic kind and unrelated RPC status from the expected rejection.
These tests do not establish live delegation acceptance.
