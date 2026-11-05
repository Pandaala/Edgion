---
name: delegation-cascading-match-issue-010
description: Use when reviewing HTTPRoute/GRPCRoute delegation cost, matching, inheritance, status or transport, or any proposal to re-materialize delegation trees in the Controller (ISSUE-010).
knowledge_type: regression-guards
finding_id: ISSUE-010
---

# Route delegation is a Gateway cascading match (ISSUE-010)

## Conclusion and scope

HTTPRoute and GRPCRoute delegation is matched **in the data plane, level by
level**. The Controller validates and publishes every route once, unchanged, and
writes on each child only the confirmed `resolved_delegation_keys` (≤6). The
Gateway compiles one subtree per distinct delegation target and descends into it
only after a delegating rule's own conditions match. Controller work, conf_sync
payload and Gateway index size are linear in authored configuration; ordinary
routes pay nothing extra.

This page is the guard for that model. A change that reintroduces any of the
following is a regression of ISSUE-010, even if it "works":

- an OR × AND cross product, a per-path copy, or a per-delegating-rule subtree;
- Controller-side materialization of child rules into a parent;
- a child edit that forces a parent republish or a full listener rebuild;
- added work, allocation or branches on the ordinary-route hot path beyond the
  one `delegate: Option<Arc<DelegationSlot>>` branch tested after `deep_match`;
- Gateway behavior for the delegation graph (depth, cycles, parent count/shape,
  bidirectional confirmation) that depends on trusting Controller validation.
- a `standard_filter_mask_{http,grpc}` that reads only `rule.filters`. The mask
  must OR every `backendRefs[*].filters` entry too: backend-level filters run
  after the route chain, so omitting them lets a child override a parent's
  mandatory filter with no conflict and no `PartiallyInvalid`. Guarded by
  `{http,grpc}_filter_mask_includes_backend_ref_filters` (resources),
  `child_backend_ref_filter_cannot_smuggle_mandatory_override{,_grpc}`
  (controller) and `{,grpc_}backend_ref_filter_mask_conflict_skips_child_candidate`
  (gateway).

Authority: design `tasks/todo/route-delegation-cascading-match-design.md`; task
`tasks/todo/route-delegation-cascading-match.md`; user semantics
`docs/en/user-guide/http-route/delegation.md` and `grpc-route/delegation.md`.

## Rationale and evidence

**Original finding (ISSUE-010, confirmed, high).** Controller flattening expanded
every root-to-leaf path into a copied rule set on the parent: diamonds re-expanded
per path, N delegating rules × M child rules multiplied, nested scopes multiplied
per level, and all of it ran synchronously in the sole per-kind worker during
parse and again in status. A small authored graph could stall the worker and
exhaust Controller memory, independent of Gateway attachment. The root cause was
that the Gateway table had no delegation concept.

**Current mechanism (source):**

- Shared vocabulary: `edgion-resources/src/resources/route_delegation.rs`
  (`classify_parent_refs`, `delegation_target_of_{http,grpc}_rule`,
  `DelegationKey`, `standard_filter_mask_*`, `MAX_ROUTE_DELEGATION_LEVELS = 5`,
  `MAX_ROUTE_KIND_PARENT_REFS = 3`).
- Controller: participants-only `DelegationGraph`
  (`resource_processor/delegation_graph.rs`) with parent → targets, named target →
  parents, parent → declared children, child → declaration, roots and minimum
  level; facts, keys and status in `handlers/route_delegation.rs` and
  `grpc_route_delegation.rs`. All requeues go through the workqueue; handlers read
  only in-memory indexes.
- Gateway: `routes/delegation/{slot,index,descent,inherit}.rs`,
  `routes/{http,grpc}/delegation_build.rs`. Delegating units hold their slot from
  compile time; `DescentState` is created lazily on the first delegating hit; the
  chain lives in `ctx.delegated: Option<Box<DelegatedMatch>>` (8 bytes, `None` for
  ordinary routes) while `ctx.route_unit` keeps the leaf.

**Gateway self-enforcement.** Depth: chain capacity 5, a full chain never descends.
Cycles: per-request visited set plus the depth cap; no graph analysis. Parent
count/shape and bidirectional confirmation: `accept_child_keys` keeps a key only
if its parent is in the child's own parentRefs and the shape is a valid child.
Missing endpoints mean the edge does not exist; the other side's arrival rebuilds
the slot.

**Trust boundary ruling (Task 6).** As for ordinary routes, the Gateway also
honors Controller status: `child_accepted_keys` installs a child key only while the
child's `status.parents` entry for that parent is `Accepted=True`. The Controller
still publishes keys for every confirmed edge regardless of reachability, so a
reachability or depth change reaches the Gateway as a child status republish
driven by level requeue. Design §4.2 records this.

## Cost bounds (acceptance targets)

| Quantity | Bound |
| --- | --- |
| Controller work per event | O(own rules) + O(3) level + O(edges touched × (parent rule + child summary)) |
| Mandatory-filter conflict per edge | per child rule, against every delegating rule of that parent whose scope can reach it (`classify_child_rules`); O(child rules × delegating rules to this child) |
| conf_sync payload | authored routes + ≤6 keys per child; ordinary payload byte-identical |
| Gateway index | one slot per distinct target; each child unit indexed in ≤6 slots, `Arc`-shared |
| Child change | republish only the child; rebuild only its ≤6 subtrees; a declared parent is requeued only when it names the child as a target (a `*` parent or a non-delegating parent is never requeued by a child) |
| Ordinary route, per request | unchanged path; one `Option` branch replacing the old parent-match check; no slot, no visited set, no allocation |
| Ordinary route, rest | context +8 bytes; rebuild touches 0 slots; Controller event ≤1 index probe |
| Delegated request | each slot entered at most once; chain ≤5; at most 64 slot entries per request across all backtracking (`MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST`), empty slots skipped without counting; the visited scan is linear in entries, so bounded by 64. Traefik reaches a bound by not backtracking at all (per-parent child muxer, 404 on a child miss); Edgion keeps backtracking and bounds it with the entry budget |

## Alternatives and limits

### Rejected alternatives

- **Keep flattening, add expansion budgets.** Budgets cap the damage but keep the
  multiplicative shape, leave the worker doing O(product) work up to the cap, and
  turn legitimate trees into budget errors. The cost belongs in matching, which
  is linear.
- **Relation on the parent** (parent lists resolved children). Every child change
  would republish the parent and rebuild its listener table, and the relation
  grows with wildcard fan-out. On the child it is ≤6 keys and a child change
  touches only the child.
- **Relative child paths** (Contour-style concatenation). Rejected for absolute
  paths with intersection semantics (kgateway/agentgateway, Istio delegate and
  Traefik also use absolute paths). Intersection needs no prefix inheritance,
  containment check or ancestor guard, and a subtree can be shared by parents with
  different prefixes.
- **Cycle detection in the Controller.** A cycle cannot be distinguished from a
  legitimate diamond with adjacent-level information, and full traversal is the
  work this design removes. Cycles cannot loop (visited set + depth cap); levels
  above 5 become "none", so a rootless cycle settles to `NotAttached` and the
  requeue chain ends.
- **Match after running parent filters** (Traefik runs parent middleware before
  child matching). Matching is synchronous and pure while plugins are async and may
  terminate, and absolute child paths are defined on the original request. The
  whole chain is matched first, then executed.

### Accepted limitations

- **First entry wins.** The visited set is keyed by slot only. The first path
  that enters a subtree decides its outcome for that request. In an uneven
  diamond (one path reaches the shared child at a depth where it can no longer
  descend) or with mixed masks (two rules of one parent delegate to one target
  with different mandatory filters, so they share one slot), a request may 404
  although another path could match. User decision: documented, not
  special-cased.
- **Status depth is the minimum level.** A longer path through the same parent may
  exceed five levels at runtime and simply not descend.
- **Non-adjacent mandatory-filter conflicts** are enforced at runtime by the chain
  mask but reported in status only for adjacent levels.
- **Conflict and scope reports are per rule and never over-report.** The
  Controller stores each delegating rule with its mask and scope for the edge
  (`edge_rules`). A child rule is listed as conflicting only when it conflicts
  with *every* delegating rule whose scope can reach it, and as out of scope only
  when no delegating prefix (HTTP) or service (gRPC) can reach it; regex, pathless
  and other uncheckable matches count as reachable. A conflict that only part of
  the traffic hits (child `/a` conflicts with rule `/a` but not with rule `/a/b`)
  is not reported; the Gateway still enforces it per request. When the listed
  rules cover every child rule, the child's entry for that parent is
  `Accepted=False / UnsupportedValue`.
- **Any own-validation error blocks a parent (user ruling, round 3).** A route
  that fails its own validation, including a single malformed ordinary rule, gets
  no level; its children become `NotAttached` and the Gateway drops their keys.
  This is intended: fix the parent. Do not report it as over-blocking.
- **No response caching for delegated requests** (Task 8 ruling). The cache key
  and credential checks cover only leaf plugins, so `response_cache/storage.rs`
  skips caching whenever `ctx.delegated` is set (fail-safe). Follow-up: fold
  ancestors into the cache bind/key.
- **`sessionPersistence` inherits as a whole block** (Task 8 ruling), matching the
  previous block-level precedence; `timeouts` and `retry` fall back per sub-field.
- **Leaf-only route annotations.** `edgion.io/forward-raw-path` and
  `edgion.io/retry-non-idempotent` are request-wide and read from the leaf
  (`ctx.route_unit`) only.
- **Child per-rule plugin state resets on any Gateway identity-set change.** Child
  units are compiled with leaf metric handles and plugin scopes for every Gateway
  in the process, so adding or removing any Gateway recompiles them (slightly
  broader than for ordinary routes).
- **`store_subtrees` is not atomic across keys.** A child change swaps its ≤6 slots
  one by one, and parent and subtree slots swap independently. One request may see
  mixed generations (root N, subtree N+1). Accepted: each level loads its slot
  once and keeps the `Arc`, and cross-level checks (mask, depth) run at descent,
  so a mixed generation can at worst skip a candidate. Per-level `sv` in the access
  log exposes it.
- **Backend-selecting plugins are rejected on delegating rules** (Task 3/8
  rulings): upstream-jump plugins (`DirectEndpoint`, `DynamicInternalUpstream`,
  `DynamicExternalUpstream`, `RegionRoute`, `Canary`, `GraphqlOperationRouter`)
  plus `AllEndpointStatus` and `AiProxy` make the delegating rule invalid
  (`is_backend_selecting_plugin`). A delegating rule has no backends to select.
- **Owner exclusivity spans the chain.** AI owner, gRPC-JSON transcoder and
  GraphQL operation router are evaluated over ancestors + leaf
  (`route_owner_conflict`) with the ordinary exclusivity rules; ordinary routes
  pass no ancestors.
- **WAF: one owner per request, child-first by default (user ruling).** On a
  delegated chain each level elects its own candidate by `priority`; the
  deepest level with a candidate wins, unless an ancestor's WAF sets
  `delegationPrecedence: parent`, in which case the outermost such level wins.
  The Route scope still beats Gateway and Global. A child replacing its
  parent's default WAF is intended, not a finding.
- **Later levels may change what earlier levels set (user ruling).** Levels run
  root → leaf; a child plugin (`ProxyRewrite`, `ResponseRewrite`) or a child
  `RequestRedirect` after a parent `URLRewrite` sees and may alter the parent's
  result. Only a same-type standard filter is blocked (the mask). This is
  documented behaviour owned by the child route's owner, not a bypass finding.
- **Root = Gateway parentRefs + Root shape** (Task 4 ruling), not listener
  acceptance: a child can show `Accepted` under a root that a listener rejects.
- **Ordinary-route shape changes** are limited to the access-log `match_info`
  becoming a one-element array and one nullable 8-byte box in `RouteMatchResult`
  and the request context (see the
  [context layout budget](../../cpu-memory/intentional-design/http-request-context-layout-budget.md)).

## Verification and re-evaluation

Evidence is recorded by the implementing tasks of the redesign (plan Tasks 1–13),
not re-run for this page. Relevant coverage: Controller
`delegation_graph_test.rs`, `handlers/http_route_test/delegation_status_test.rs`,
`grpc_route_delegation_test.rs` and the cost tests that count index entries, keys
and rebuilt slots; Gateway `tests/unit/routes/delegation/`,
`routes/{http,grpc}/delegation_build_test.rs`,
`pg_request_filter_test/delegation_tests.rs`, access-log `entry_test.rs`;
integration `HTTPRoute/Delegation`, `GRPCRoute/Delegation`,
`RouteDelegationDynamic`, `GrpcRouteDelegationDynamic` and the
ParentRefGatewayCollision scenarios. Kubernetes `Delegation` fixtures were not
cluster-run during the redesign.

Re-evaluate when: the depth cap or parent-count cap changes; a Controller-level
delegation switch or configurable depth is added (task follow-up); response
caching folds ancestors into its key; delegation gains method-level or delegate-all
gRPC scopes; or any proposal moves delegation work back into the Controller.

## Resolution references

- `ISSUE-010` (`tasks/important_issue/run-e6cbd068`, baseline `e6cbd068`) —
  redesigned as cascading match (design agreed 2026-11-03; implemented on
  branch `feature-08-30`, pending user review and commit).
- Supersedes: [ARCH-156 / prefix inheritance](../superseded/delegation-root-prefix-inherits-scope.md).
- Related: [ExtensionRef locality](delegated-extensionref-remains-child-local.md),
  [parentRef / Gateway name collision](delegation-parentref-gateway-name-collision.md),
  [radix candidate precedence](radix-candidate-precedence-rank-precomputed.md)
  (ranks are per engine, so per subtree).
