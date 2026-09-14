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

1. Calls `validate_http_route_if_enabled()` for matcher enum/regex validation; cross-namespace validation is handled via `ref_denied` in the parse stage.
2. Per-rule header modifier validation uses the shared `HTTPRouteRule::header_modifier_error()` in the invalid-rule path, including backend filters and delegation source rules. Invalid rules are excluded while valid siblings survive; all-invalid header rules reject attachment. See [header modifier validation](../../04-review/plugin-runtime/regression-guards/header-modifier-validation.md).

### Decode and type-level preparse

`HTTPRouteMatch.headers` normalizes at typed deserialization: only the first
ASCII-case-equivalent name survives, with its original type, value, spelling,
and relative order. This happens before Controller validation and delegation,
and also covers Gateway decoding of both declared and resolved rules.
Delegation still rejects conflicting effective parent/child constraints.

`HTTPRoute::preparse()` normalizes paths and resolves rule annotations;
`ResourceMeta::pre_parse()` additionally parses timeouts and retries.
Plugin runtimes and match regexes are compiled later by the Gateway route
builder. Regex indexes, predicates, and header specificity all consume the
same decoded header vector. See the [header normalization guard](../../04-review/routing/regression-guards/http-header-match-first-name-wins.md).

### parse

0. **Replace ReferenceGrant dependencies, mark denials, then flatten delegation**: clear the prior CrossNsRefManager set once at parse entry. Before every ReferenceGrant store read, register the target namespace under the root Route owner; this applies to ordinary backendRefs, RequestMirror, and child-declared flattened backends. Then mark `ref_denied` on spec.rules before flattening (flatten clones non-delegating sibling rules verbatim into resolved_rules, which the gateway compiles instead of spec.rules — marking afterwards would leave the compiled copies un-denied). If any rule delegates (`backendRef {group: gateway.networking.k8s.io, kind: HTTPRoute}`), `route_delegation::flatten_route()` DFS-expands the delegation tree into `spec.resolved_rules` (schemars-skip, serialized for conf_sync) and stores diagnostics in the in-process `spec.delegation_issues`. `RouteDelegationIndex` (child → parents) is updated; edge deltas requeue affected children so their child-side status stays fresh. Canonical user semantics: [`docs/en/user-guide/http-route/delegation.md`](../../../docs/en/user-guide/http-route/delegation.md); module: `handlers/route_delegation.rs`
1. **Complete cross-namespace reference collection**: idempotently add all authored and flattened target namespaces to CrossNsRefManager without another clear, preserving dependencies installed before authorization reads
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
- Native CORS on any backendRef invalidates the entire owning rule through
  `HTTPRouteRule::unsupported_backend_cors_reason`. The same check is used during
  delegation flattening and independently on the Gateway's effective rules before
  runtime compilation. Rule-level native CORS remains supported; no backend policy
  is promoted or merged into the rule. See the
  [scope decision](../../04-review/cors/intentional-design/native-cors-rule-scope.md).
  If an authored rule has this scope error and no effective rule survives, the
  Gateway attachment uses `Accepted=False/UnsupportedValue` instead. Delegating
  parents include the rejected child rule's backend CORS reason in their diagnostics.
- **Per-rule findings never flip `Accepted` while a sibling rule still serves** — `Accepted=False` removes the whole route from the gateway route table (`filter_accepted_parent_refs`), so a rule-level problem rides `PartiallyInvalid` instead. Independent delegation drops and authored native backend CORS scope violations escalate on a route where **no** effective rule serves any more: then `Accepted=False/UnsupportedValue` lists the offending `rules[i]: <reason>` and `PartiallyInvalid` is never set True (an earlier True is cleared to False with reason `UnsupportedValue`), so the two conditions cannot contradict each other
- Child-side delegation status: each `kind: HTTPRoute` parentRef (reverse lock) gets an `Accepted` condition — True iff that parent currently delegates to this route (`DelegationAccepted` / `NoMatchingParent`, computed from `RouteDelegationIndex`)
- Cleans up old status entries that correspond to parentRefs no longer present

Native CORS also rejects `allowCredentials: true` with any exact `*` entry in
`exposeHeaders`, including mixed lists. `HTTPRouteRule::unsupported_cors_reason`
combines this support restriction with backend scope enforcement for Controller
status/delegation and Gateway publication. Valid siblings survive; an authored
violation with no effective rule uses the existing attachment rejection path.
See the [credentialed exposure policy](../../04-review/cors/intentional-design/native-credentialed-exposure-policy.md).

### AI source diagnostics and execution admission

After resolving references and delegated rules, the Controller attaches bounded
`resolved_ai_admission` backend facts to both authored and flattened rules.
These transport-only fields are hidden from CRD schema and carry no backend
credentials. Shared pure AI validation produces intrinsic source diagnostics;
those findings do not drop the matched rule or flip parent acceptance merely
because no policy-free AI shape is valid.

Gateway publication combines these facts with effective AiProxy entries from the
class scope and records a route-generation-specific rule gate. A current Block
wins before that gate. Bypassed AiProxy with incompatible AI targets returns the
matched rule's generic 500 instead of ordinary upstream or another rule fallback.
Missing required facts and mismatched route generations fail closed. Backend
ExtensionRef execution remains first-use admission of the selected backend under
the captured scope; restrictions on unselected candidates do not reject early. Independent
backend WAF/native CORS/delegation rejection behavior remains unchanged.

## Gateway-side processing

HTTPRoute's ConfHandler implements route compilation and registration:

0. **Effective rules**: compiles `route.effective_rules()` — `spec.resolved_rules` (controller-flattened delegation result) when present, else `spec.rules`. `invalid_rule_indices` applies only to `spec.rules` (flattening already dropped invalid rules). Delegation children have no Gateway parentRefs / `resolved_listeners`, so they register nothing and are inert on the data plane. Flattening stamps every rule- and backend-level ExtensionRef with the Namespace of the HTTPRoute that declared it, preserving parent-local and child-local EdgionPlugins resolution in the shared Gateway runtime. Authored `resolvedNamespace` values are cleared before flattening in filesystem/etcd modes
1. **parentRef filtering**: only compile parentRefs with `Accepted=True` (when status is not yet present, optimistically compile all)
2. **Per-listener route management**: based on resolved_listeners, register route rules into the `HttpRouteManager` of the corresponding `ListenerKey`
3. **Domain route table**: each physical Listener maintains authored hostname groups; only omitted/empty hostname lists create a separate internal catch-all. Both planes reject a bare `*` or any other invalid hostname entry for the complete Route.
4. **Multi-level match engine**:
   - Algo `RadixHostnameMatcher<RouteRules>`: shared immutable exact/wildcard index consumed by Gateway; matching suffixes continue from longest to shortest without changing Listener eligibility
   - `RadixRouteMatchEngine`: path-prefix matching based on a Radix Tree
   - `RegexRoutesEngine`: regex path matching
5. **Request handling flow**: `port → domain match → path match → header/query deep match → selected rule → execute plugin chain → proxy_http`
6. **LB policy sync**: when routes change, the corresponding load-balancing policy is synced

HTTP Route deltas replace touched hostname contents while retaining untouched
path engines. The shared hostname tree is reused whenever final keys agree,
including full-list/Gateway rebuilds; the table is published through one outer
snapshot. See the [hostname algorithm contract](../../12-algorithms/route-hostname-matching.md).

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
