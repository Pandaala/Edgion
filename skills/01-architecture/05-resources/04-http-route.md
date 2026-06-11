---
name: resource-http-route
description: HTTPRoute resource — routing rules, match conditions, BackendRef, Filter, plugin binding, route registration.
---

# HTTPRoute resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

HTTPRoute is the most complex resource in Edgion. It defines HTTP-layer routing rules, including path/header/query parameter matching, backend references, filters, and plugin execution.

## Source code locations

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`
- Gateway ConfHandler: `src/core/gateway/routes/http/conf_handler_impl.rs`
- Route manager: `src/core/gateway/routes/http/routes_mgr.rs`
- Match engine: `src/core/gateway/routes/http/match_engine/`
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

1. **Record cross-namespace references**: register cross-namespace references in backendRef into CrossNsRefManager so that ReferenceGrant changes automatically requeue the route
2. **Register Service references**: register all Service-typed backendRefs into ServiceRefManager so that Service changes automatically requeue the route
3. **Clear and recompute resolved_listeners**: from parentRef, resolve the target Listener's address and protocol, producing `Vec<ListenerKey>` (formatted `"{addr}/{protocol}"`) for per-listener route isolation on the Gateway side. Resolution logic is shared via `resolve_listener_keys()` (used by 6 handlers in `handlers/route_utils.rs`):
   - Always look up the Gateway to obtain address and protocol (the parentRef.port-only fast path no longer exists)
   - When sectionName + port are both present, validate consistency between them
   - If neither is specified → use all of the Gateway's Listeners (subject to the allowedRoutes namespace check)
4. **Resolve resolved_hostnames**: compute the intersection of the route's hostnames with the Gateway Listener's hostnames
5. **Mark rejected cross-namespace references**: for each cross-namespace backendRef, check ReferenceGrant; for unauthorized ones, set the `ref_denied` field so the Gateway side rejects requests accordingly

### on_change

1. Register into `GatewayRouteIndex` (Gateway changes will requeue this route)
2. Update `AttachedRouteTracker`; if the attachment relationship has changed, requeue the parent Gateway (to refresh the attachedRoutes count)

### on_delete

- Clear registrations in CrossNsRefManager, ServiceRefManager, GatewayRouteIndex, and AttachedRouteTracker
- If AttachedRouteTracker has changes, requeue the parent Gateway

### update_status

Generate an independent RouteParentStatus for each parentRef:
- `Accepted`: validates whether the Gateway/Listener corresponding to parentRef exists and admits this route (non-empty hostname intersection, namespace policy permits)
- `ResolvedRefs`: validates whether all backendRef-targeted Services exist and whether cross-namespace references are rejected
- Cleans up old status entries that correspond to parentRefs no longer present

## Gateway-side processing

HTTPRoute's ConfHandler implements route compilation and registration:

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
| HTTPRoute → Service | Service | backendRef | Route's backend target service |
| HTTPRoute → EdgionPlugins | EdgionPlugins | ExtensionRef filter | Plugin configuration referenced via ExtensionRef |
| HTTPRoute → Secret | Secret | indirect, via plugins | Plugins such as JWT/BasicAuth reference Secrets |
| HTTPRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |
