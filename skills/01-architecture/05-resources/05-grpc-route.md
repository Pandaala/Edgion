---
name: resource-grpc-route
description: GRPCRoute resource — gRPC Service/Method matching, gRPC-Web, and the relationship to HTTPRoute.
---

# GRPCRoute resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

GRPCRoute defines gRPC-layer routing rules. Its Controller-side processing is highly aligned with HTTPRoute (sharing route_utils helper functions); the main difference is the matching method on the Gateway side: it matches by gRPC service name and method name rather than by HTTP path.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs`
- Gateway ConfHandler: `edgion-gateway/src/routes/grpc/conf_handler_impl.rs`
- Route manager: `edgion-gateway/src/routes/grpc/routes_mgr.rs`
- Match unit: `edgion-gateway/src/routes/grpc/match_unit.rs`
- Type definitions: `edgion-resources/src/resources/grpc_route.rs`

## Controller-side processing

### validate

1. Calls `validate_grpc_route_if_enabled()` (currently a no-op; cross-namespace validation is handled via `ref_denied` in the parse stage)
2. Calls `validate_backend_refs()` to validate all backendRefs

### parse

Mirrors HTTPRoute's step order, including the delegation flatten:
0. **ReferenceGrant marking, then delegation flatten**: ref_denied marking on `spec.rules` runs FIRST (flatten clones non-delegating sibling rules verbatim into `resolved_rules`, which the gateway compiles instead of `spec.rules` — marking later would leave the compiled copies un-denied). Then, if any rule delegates (`backendRef {group: gateway.networking.k8s.io, kind: GRPCRoute}`), `grpc_route_delegation::flatten_route()` DFS-expands the delegation tree into `spec.resolved_rules` (schemars-skip, serialized for conf_sync) and stores diagnostics in the in-process `spec.delegation_issues` (the kind-agnostic `DelegationIssue` enum, reused unchanged from `http_route.rs`). Flattened Service/cross-ns refs fold into the registrations below **under the parent's ResourceRef**, and a **second `RouteDelegationIndex` instance** — `grpc_route_delegation_index` on `ControllerRuntime`, same type as HTTP's `route_delegation_index`, disjoint `"ns/name"` keys — is updated so GRPCRoute changes requeue only gRPC parents. Canonical user semantics: [`docs/en/user-guide/grpc-route/delegation.md`](../../../docs/en/user-guide/grpc-route/delegation.md); module: `handlers/grpc_route_delegation.rs`
1. Record cross-namespace references in CrossNsRefManager
2. Register Service references in ServiceRefManager
3. Clear and recompute `resolved_listeners` (resolve `Vec<ListenerKey>` from parentRef via `resolve_listener_keys()`)
4. Resolve `resolved_hostnames` (intersection of route hostnames with Gateway Listener hostnames)
5. (ReferenceGrant `ref_denied` marking happens in step 0, before the flatten — see above)

### on_change

Same as HTTPRoute: register in GatewayRouteIndex, update AttachedRouteTracker, requeue the parent Gateway when needed, and requeue delegating parents via `grpc_route_delegation_index.parents_of()`. The same post-cache-ready one-shot revalidation as HTTPRoute (`ref_grant/revalidation_listener.rs`) has a gRPC parallel requeueing `"GRPCRoute"`, closing the cold-start window where a parent flattens before its children are loaded.

### on_delete

Same as HTTPRoute: clear all reference and index registrations, including this route's own parent-side delegation edges (`update_parent(key, ∅, ∅)`, requeueing former children and any parents still delegating to it).

### update_status

Same as HTTPRoute: produce a RouteParentStatus for each parentRef and set the Accepted and ResolvedRefs Conditions, plus delegation-specific status surfaces:
- `ResolvedRefs`: delegation refs (`kind: GRPCRoute`) are **excluded** from Service backendRef validation (they are not Service refs; including them would spuriously flag `InvalidKind`); a missing named delegation child surfaces as `BackendNotFound` via `DelegationIssue::MissingChild`.
- `PartiallyInvalid`: own invalid rules plus every delegation-dropped rule message (`DelegationIssue::RuleDropped`).
- Child-side delegation status: each `kind: GRPCRoute` parentRef (reverse lock) gets its own `Accepted` condition — True iff that parent currently delegates to this route (`DelegationAccepted` / `NoMatchingParent`, from `grpc_route_delegation_index`). The Gateway-acceptance loop must gate on `is_gateway_parent_ref` so this doesn't run `validate_parent_ref_accepted` against reverse-lock parentRefs.
- `collect_ref_denied_list`-equivalent status folding scans `effective_rules()` so flattened children's ReferenceGrant denials surface on the parent.

## Gateway-side processing

GRPCRoute's ConfHandler compiles routes into `GlobalGrpcRouteManagers`:

0. **Effective rules**: compiles `route.effective_rules()` — `spec.resolved_rules` (controller-flattened delegation result) when present, else `spec.rules`. `invalid_rule_indices` applies only to `spec.rules` (flattening already dropped invalid rules). Delegation children have no Gateway parentRefs / `resolved_listeners`, so they register nothing and are inert on the data plane
1. **preparse**: in the route cache, call `route.preparse()` to build runtime structures
2. **Route units**: each rule is compiled into a `GrpcRouteRuleUnit` containing `GrpcMatchInfo` (service, method, header matching)
3. **Domain matching**: use resolved_hostnames (preferred) or spec.hostnames as the match domain; with no hostname, use `*` (wildcard)
4. **Per-listener management**: based on resolved_listeners, register into the route manager of the corresponding `ListenerKey`; on change, only rebuild the route table for the affected listeners
5. **gRPC-Web support**: automatically supports the gRPC-Web protocol via content-type detection

## Delegation (GRPCRoute → GRPCRoute)

GRPCRoute delegation is a parallel module (`handlers/grpc_route_delegation.rs`), not a trait generalization of `route_delegation.rs` — the Rule/Match/Filter/BackendRef type families are disjoint and the gRPC scope unit (a service name, spec-string-compared) differs from HTTP's path-prefix containment. Full user-facing semantics: [`docs/en/user-guide/grpc-route/delegation.md`](../../../docs/en/user-guide/grpc-route/delegation.md); feature schema: [`02-features/03-resources/05-grpcroute.md`](../../02-features/03-resources/05-grpcroute.md).

**Four security-critical divergences from a literal HTTP mirror** (regressions here are routing-boundary escapes — see the doc comment at the top of `grpc_route_delegation.rs` for the authoritative statement):

1. A delegating rule with `matches` absent/empty is **invalid** (HTTP treats a matchless delegating rule as delegating `"/"`; that default is not mirrored — delegate-all is out of scope for v1).
2. A terminal child rule with `matches` absent/empty flattens to **one inherited match per delegated scope**, never cloned matchless (the gateway synthesizes a catch-all unit for a matchless rule; an unmodified clone would match ALL traffic on the parent's hostnames).
3. Any child match whose `method.service` is absent or empty (missing matcher, empty `method: {}`, or method-only) **inherits** the delegated service as `{type: Exact, service: S}`.
4. Any child match with `method.type: RegularExpression` **invalidates that whole child rule** (v1 rejects the match outright rather than proving a regex service cannot escape the scope).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| GRPCRoute → Gateway | Gateway | parentRef | Route attaches to the Gateway's Listener |
| GRPCRoute → GRPCRoute | GRPCRoute | delegation backendRef (`kind: GRPCRoute`) + `grpc_route_delegation_index` | Parent flattens child rules into `resolved_rules`; child changes requeue parents (named + wildcard-namespace edges, transitively registered at the root) |
| GRPCRoute → Service | Service | backendRef | Route's backend target service |
| GRPCRoute → EdgionPlugins | EdgionPlugins | ExtensionRef filter | Plugin configuration referenced via ExtensionRef |
| GRPCRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |
