---
name: resource-http-route
description: HTTPRoute resource — routing rules, match conditions, BackendRef, Filter, plugin binding, route registration.
---

# HTTPRoute resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

HTTPRoute is the most complex resource in Edgion. It defines HTTP-layer routing rules, including path/header/query parameter matching, backend references, filters, and plugin execution.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`
- Gateway ConfHandler: `edgion-gateway/src/routes/http/conf_handler_impl.rs`
- Route manager: `edgion-gateway/src/routes/http/routes_mgr.rs`
- Match engine: `edgion-gateway/src/routes/http/match_engine/`
- Type definitions: `edgion-resources/src/resources/http_route.rs`
- Preparse: `edgion-resources/src/resources/http_route_preparse.rs`

## Controller-side processing

### validate

1. Calls `validate_http_route_if_enabled()` (currently a no-op; cross-namespace validation is handled via `ref_denied` in the parse stage)

### preparse (type level)

HTTPRoute's `preparse()` method builds runtime structures at the type level:
- Compiles `filters` into `PluginRuntime` (plugin execution chain)
- Pre-compiles regex matchers
- Builds path-matching information

### parse

0. **ReferenceGrant marking, then delegation flatten**: ref_denied marking on spec.rules runs FIRST (flatten clones non-delegating sibling rules verbatim into resolved_rules, which the gateway compiles instead of spec.rules — marking later would leave the compiled copies un-denied). Then, if any rule delegates (`backendRef {group: gateway.networking.k8s.io, kind: HTTPRoute}`), `route_delegation::flatten_route()` DFS-expands the delegation tree into `spec.resolved_rules` (schemars-skip, serialized for conf_sync) and stores diagnostics in the in-process `spec.delegation_issues`. The flattened Service/cross-ns refs are folded into the registrations below **under the parent's ResourceRef**, and `RouteDelegationIndex` (child → parents) is updated; edge deltas requeue affected children so their child-side status stays fresh. Canonical user semantics: [`docs/en/user-guide/http-route/delegation.md`](../../../docs/en/user-guide/http-route/delegation.md); module: `handlers/route_delegation.rs`
1. **Record cross-namespace references**: register cross-namespace references in backendRef into CrossNsRefManager so that ReferenceGrant changes automatically requeue the route
2. **Register Service references**: register all Service-typed backendRefs into ServiceRefManager so that Service changes automatically requeue the route
3. **Clear and recompute resolved_listeners**: from parentRef, resolve the target Listener's address and protocol, producing `Vec<ListenerKey>` (formatted `"{addr}/{protocol}"`) for per-listener route isolation on the Gateway side. Resolution logic is shared via `resolve_listener_keys()` (used by 6 handlers in `handlers/route_utils.rs`):
   - Always look up the Gateway to obtain address and protocol (the parentRef.port-only fast path no longer exists)
   - When sectionName + port are both present, validate consistency between them
   - If neither is specified → use all of the Gateway's Listeners (subject to the allowedRoutes namespace check)
4. **Resolve resolved_hostnames**: compute the intersection of the route's hostnames with the Gateway Listener's hostnames
5. (ReferenceGrant `ref_denied` marking happens in step 0, before the flatten — see above)

### on_change

1. Register into `GatewayRouteIndex` (Gateway changes will requeue this route)
2. Update `AttachedRouteTracker`; if the attachment relationship has changed, requeue the parent Gateway (to refresh the attachedRoutes count)
3. Requeue delegating parents via `RouteDelegationIndex.parents_of()` (this route may be someone's delegation child; parents must re-flatten)

> **Post-init one-shot (not an on_change step):** `trigger_delegation_parent_revalidation` (`ref_grant/revalidation_listener.rs`, invoked from `run_post_cache_ready_revalidations`) requeues all registered delegating parents once caches are ready, closing the init window where a parent flattened before its children were loaded.

### on_delete

- Clear registrations in CrossNsRefManager, ServiceRefManager, GatewayRouteIndex, and AttachedRouteTracker
- Delegation cleanup: clear this route's own parent-side edges via `update_parent(key, ∅, ∅)` and requeue its former children from the returned delta (their child-side Accepted must refresh), then requeue any parents still delegating to it
- If AttachedRouteTracker has changes, requeue the parent Gateway

### update_status

Generate an independent RouteParentStatus for each parentRef:
- `Accepted`: validates whether the Gateway/Listener corresponding to parentRef exists and admits this route (non-empty hostname intersection, namespace policy permits). Only `kind: Gateway` parentRefs enter this path
- `ResolvedRefs`: validates whether all backendRef-targeted Services exist and whether cross-namespace references are rejected (delegation refs are excluded from Service validation; a missing named delegation child surfaces as `BackendNotFound` via `DelegationIssue::MissingChild`)
- `PartiallyInvalid`: own invalid rules plus every delegation-dropped rule message (`DelegationIssue::RuleDropped` — out-of-prefix child rules, filter conflicts, cycles, depth, malformed delegating rules)
- **Per-rule findings never flip `Accepted` while a sibling rule still serves** — `Accepted=False` removes the whole route from the gateway route table (`filter_accepted_parent_refs`), so a rule-level problem rides `PartiallyInvalid` instead. The one escalation is an AI attachment/retry-cap violation (`ai_rule_invalid_reason`) on a route where **no** rule serves any more: then `Accepted=False/UnsupportedValue` lists every `rules[i]: <reason>` and `PartiallyInvalid` is never set True (an earlier True is cleared to False with reason `UnsupportedValue`), so the two conditions cannot contradict each other
- Child-side delegation status: each `kind: HTTPRoute` parentRef (reverse lock) gets an `Accepted` condition — True iff that parent currently delegates to this route (`DelegationAccepted` / `NoMatchingParent`, computed from `RouteDelegationIndex`)
- Cleans up old status entries that correspond to parentRefs no longer present

## Gateway-side processing

HTTPRoute's ConfHandler implements route compilation and registration:

0. **Effective rules**: compiles `route.effective_rules()` — `spec.resolved_rules` (controller-flattened delegation result) when present, else `spec.rules`. `invalid_rule_indices` applies only to `spec.rules` (flattening already dropped invalid rules). Delegation children have no Gateway parentRefs / `resolved_listeners`, so they register nothing and are inert on the data plane. Flattening stamps each ExtensionRef with the Namespace of the HTTPRoute that declared it, preserving parent-local and child-local EdgionPlugins resolution in the shared Gateway runtime
1. **parentRef filtering**: only compile parentRefs with `Accepted=True` (when status is not yet present, optimistically compile all)
2. **Per-listener route management**: based on resolved_listeners, register route rules into the `HttpRouteManager` of the corresponding `ListenerKey`
3. **Domain route table**: each port maintains a `domain → route_rules` mapping; routes without a hostname are registered under `*` (wildcard)
4. **Multi-level match engine**:
   - `RadixHostMatchEngine`: domain matching based on a Radix Tree
   - `RadixRouteMatchEngine`: path-prefix matching based on a Radix Tree
   - `RegexRoutesEngine`: regex path matching
5. **Request handling flow**: `port → domain match → path match → header/query deep match → selected rule → execute plugin chain → proxy_http`
6. **LB policy sync**: when routes change, the corresponding load-balancing policy is synced

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| HTTPRoute → Gateway | Gateway | parentRef | Route attaches to the Gateway's Listener |
| HTTPRoute → HTTPRoute | HTTPRoute | delegation backendRef (`kind: HTTPRoute`) + `RouteDelegationIndex` | Parent flattens child rules into `resolved_rules`; child changes requeue parents (named + wildcard-namespace edges, transitively registered at the root) |
| HTTPRoute → Service | Service | backendRef | Route's backend target service |
| HTTPRoute → EdgionPlugins | EdgionPlugins | ExtensionRef filter | Plugin configuration referenced via ExtensionRef |
| HTTPRoute → EdgionBackend | EdgionBackend | shared `backend_ref_mgr` | AI capability drives rule validation; apply/delete requeues the route |
| HTTPRoute → Secret | Secret | indirect, via plugins | Plugins such as JWT/BasicAuth reference Secrets |
| HTTPRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |
