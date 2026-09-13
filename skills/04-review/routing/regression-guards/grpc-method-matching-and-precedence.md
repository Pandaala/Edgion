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

Matching hostname specificity precedes service length, method length, header
count, creation time, namespace/name, and rule order. Only a declared Route
hostname actually matching the request contributes. Listener intersection may
narrow eligibility but must not promote wildcard/hostless Route priority.
Pattern character counts apply to regex;
Exact is not an additional priority tier. Match index resolves remaining ties.
Every candidate must pass the full predicate, with no-match continuing through
remaining candidates and broader matching hostname groups. gRPC-first/HTTP
fallback for overlapping hostnames is unchanged.

## Rationale and evidence

Authority: the pinned [Gateway API v1.5.0 GRPCRoute definitions](https://github.com/kubernetes-sigs/gateway-api/blob/v1.5.0/apis/v1/grpcroute_types.go)
and vendored `config/crd/gateway-api/gateway-api-standard-v1.5.0.yaml`.
Method-only Exact and regex are implementation-specific support choices, not
claims that every Gateway API implementation must support them. The pinned
contract defines the precedence chain and per-rule invalidity behavior.

- `edgion-resources/src/resources/grpc_route.rs`: `compile_method` owns the
  bounded compilation policy; `method_match_error` exposes it to rule validation.
  No new serialized fields or CRD schema changes are needed.
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
  caller. `integration.rs::try_match_grpc_route` retains HTTP
  fallback. ArcSwap generation replacement retains in-flight snapshots.

## Alternatives and limits

The old seven-tier engine omitted method-only predicates, let Exact buckets
override normative precedence, and checked hostname too late. Treating method-only
as catch-all would lose its predicate. Sorting all routes per request or scanning
unrelated Exact routes is unnecessary: build-time ranks preserve borrowed hostname
lookup while allowing cross-index precedence. No request allocation, blocking
lock, runtime pattern compilation, or request-level tracing is added by selection.
Regex traversal remains linear in the selected hostname group's regex candidates.
Correctness/instrumented candidate counts do not establish latency equivalence.

Duplicate equivalent gRPC header names still need first-name-wins handling;
this is a separate existing defect, not accepted behavior. It is tracked in
[grpc-header-match-first-name-wins](../../../../tasks/todo/grpc-header-match-first-name-wins.md).
Do not read this rule as proving complete GRPCRoute conformance.

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
actual Pingora Session header fallback, gRPC-to-HTTP fallback, and full/partial
snapshot replacement and deletion. The 1,000-Exact-route test counts
visited candidates; it is not a throughput benchmark. Allocation counters cover
Exact selection, hostname fallback, and regex selection after warming its existing
lazy search cache; they do not claim first-use regex allocation is absent.
Existing `edgion-tests/**`
are frozen. No live Kubernetes/etcd admission or HTTP/2 wire test is claimed.
The full agent-doc guard has seven pre-existing broken links outside this repair;
the 13 changed/new documents pass the same guard functions when scoped to the diff.
Source tasks remain open while the repository-wide closure gate is blocked.

Re-evaluate on Gateway API version changes, delegation policy changes, new
matching dimensions, or changes to the shared hostname matcher, input normalization,
and table publication. Route wildcards match multiple labels; certificate matching
helpers are outside this contract and remain unchanged.

## Resolution references

- `grpc-route-method-only-match-policy`: confirmed correctness defect; repair
  supports the already documented method-only contract and rejects invalid matchers.
- `grpc-route-complete-precedence-and-hostname-ordering`: confirmed correctness
  defect; companion repair applies the full precedence chain across candidate indexes.
