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

### Native backend reference ownership

`refs::route_fields::HttpRouteBackendRefs` borrows the actual typed Route. Only an authored,
validated terminal loan can construct a Grant query; effective/status/AI loans,
named/wildcard delegation selectors, and owned dependency receipts cannot. The
canonical `refs::targets` classifier requires omitted/empty group and omitted/exact
`Service` kind plus port `1..=65535`; literal `core` and pseudo Service kinds are
unsupported. Exact same-family delegation uses the Gateway API group and no port;
`*` is a namespace selector, not an object identity. HTTP alone also supports exact
`edgion.io/EdgionBackend` without a backend port.

Source namespace omission defaults to `default`; an explicitly invalid namespace
or missing/invalid source name closes the existing Accepted gate, including direct
status calls, and empties derived executable rules/listeners. No native target is
looked up or registered for that invalid source. A valid parent selection of an
invalid child retains its named/wildcard recovery edge but drops the child before
native backend reads, Grant checks, or rule cloning.

Delegation authorizes each original child terminal before cloning, using the
child's source namespace even when its target is the root namespace. The root
Route owns wakeups. Valid terminal receipts survive later rule filtering and
missing/denied targets. Service manager owners retain their explicit canonical
namespace while root Grant wakeups retain raw metadata namespace representation.
Apply replaces all prior dependencies and requeues dropped policy keys.

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
- `ResolvedRefs`: canonical effective target views validate Service/EdgionBackend identity and existence. Invalid references map through the existing `InvalidKind` reason. Valid denied terminals retain `RefNotPermitted` without an existence read. Valid delegation selectors are excluded from terminal existence reads; missing named children surface as `BackendNotFound` via `DelegationIssue::MissingChild`.
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
There is one fact per original backend slot in both vectors; invalid or denied
targets produce `Missing` without reading Service/EdgionBackend state or changing
weights. Only same-namespace or authorized valid targets reach those stores.
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

### ExtensionRef ownership and recovery

The Controller consumes `HttpRouteExtensionRefs` loans from actual authored or
effective filter positions. Exact `edgion.io/EdgionPlugins`, a valid required name,
and a valid owning Route identity precede dependency registration or content
lookup. Authored loans ignore namespace stamps. Effective loans accept the existing
controller-owned `resolved_namespace` only from `resolved_rules`; they describe the
published root and field position, without claiming the original child's UID or
generation. No ExtensionRef loan can mint a ReferenceGrant query.

Delegation registers every valid original child's plugin dependency under the
root wakeup identity before rule drops. Only structurally eligible scope checks
read plugin content, using `get_raw_any` after registration so a pending desired
generation is visible. Parse entry clears the prior plugin dependency set once;
later authored/effective/status registration is additive. An omitted root namespace
stays omitted in the manager wakeup key while local targets use `default`.
Invalid owners and targets contribute no new plugin edge or content lookup.
For a valid owner, parse is the sole dependency-clear boundary and status only
extends the set. A standalone status call for an invalid source may withdraw
stale dependencies and derived rules/listeners/hostnames; an invalid target,
dropped rule or missing plugin under a valid owner does not permit that cleanup.
Gateway rule and selected-backend builders now borrow the actual Route and
its effective rule/backend/filter slots through the same canonical facade. A
validated local target becomes one cold complete store key; invalid descriptors
retain a positional `None` and cannot query or consume a same-name plugin.
Publication demand, AI admission and adaptive facts use that same projection.
Request resolution, CORS/WAF selection and access-log declarations consume these
validated keys or their positional results under the captured class scope.
Authored stamps are ignored even on direct Gateway input; only actual
`resolved_rules` can supply validated child-local namespace stamps. These loans
still carry no Grant authority or original-child identity beyond the transport.
R3a2 verification is scoped to these consumers; mirror/descriptor work and live
conformance remain separate.

## Gateway-side processing

HTTPRoute's ConfHandler implements route compilation and registration:

0. **Effective rules**: compiles `route.effective_rules()` — `spec.resolved_rules` (controller-flattened delegation result) when present, else `spec.rules`. `invalid_rule_indices` applies only to `spec.rules` (flattening already dropped invalid rules). Delegation children have no Gateway parentRefs / `resolved_listeners`, so they register nothing and are inert on the data plane. Flattening stamps every rule- and backend-level ExtensionRef with the Namespace of the HTTPRoute that declared it, preserving parent-local and child-local EdgionPlugins resolution in the shared Gateway runtime. Authored `resolvedNamespace` values are cleared before flattening in filesystem/etcd modes
1. **parentRef filtering**: require a nonempty processed Controller stamp and a current owned parent status with `Accepted=True`; missing or stale status admits no parent
2. **Per-listener route management**: based on resolved_listeners, register route rules into the `HttpRouteManager` of the corresponding `ListenerKey`
3. **Domain route table**: each physical Listener maintains authored hostname groups; only omitted/empty hostname lists create a separate internal catch-all. Both planes reject a bare `*` or any other invalid hostname entry for the complete Route.
4. **Multi-level match engine**:
   - Algo `RadixHostnameMatcher<RouteRules>`: shared immutable exact/wildcard index consumed by Gateway; matching suffixes continue from longest to shortest without changing Listener eligibility
   - `RadixRouteMatchEngine`: path-prefix matching based on a Radix Tree
   - `RegexRoutesEngine`: regex path matching
5. **Request handling flow**: `port → domain match → path match → header/query deep match → selected rule → execute plugin chain → proxy_http`
6. **Post-rebuild publication**: after the route cache and listener snapshots are updated, publish HTTPRoute inputs to the canonical plugin publisher for route-generation admission and update adaptive AI route contributions for the process-global manager. Backend traffic and load-balancing policy are owned independently by typed `EdgionBackendTrafficPolicy` through `BackendTrafficPolicyStore`; HTTPRoute does not synchronize or clean up those policies.

HTTP Route deltas replace touched hostname contents while retaining untouched
path engines. The shared hostname tree is reused whenever final keys agree,
including full-list/Gateway rebuilds; the table is published through one outer
snapshot. See the [hostname algorithm contract](../../12-algorithms/route-hostname-matching.md).

### Compiled native backend ownership

`HttpRouteRuleUnit::compile_http_rule` takes the actual HTTPRoute and effective rule
index. Its source-validated `HttpRouteBackendRefs::effective_at` loan supplies the
canonical target. Private compiled schema, slot, identity and target cannot be
reconstructed from a detached backend schema or mutable request context. Invalid
owners create no plugin runtime; invalid or denied backend slots retain their
weight and position with an inert backend runtime.

A permitted Service carries one cold key, policy target and checked numeric section.
Selection, sticky candidates, named jumps, DirectEndpoint authorization, TLS policy
and final peer selection borrow that same target. `BackendContext` is a checked
diagnostic projection and cannot restore authority for an Invalid slot. Invalid
descriptors fail with `BackendUnsupportedKind`/500 before backend plugins; denied
slots retain `RefDenied`/500. Ordinary AI selection retains its separate 503 failure.
Live failures of a valid Service section/endpoints retain their existing 503 mapping.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| HTTPRoute → Gateway | Gateway | parentRef | Route attaches to the Gateway's Listener |
| HTTPRoute → HTTPRoute | HTTPRoute | delegation backendRef (`kind: HTTPRoute`) + `RouteDelegationIndex` | Parent flattens child rules into `resolved_rules`; child changes requeue parents (named + wildcard-namespace edges, transitively registered at the root) |
| HTTPRoute → Service | Service | backendRef | Route's backend target service |
| HTTPRoute → EdgionPlugins | EdgionPlugins | ExtensionRef filter | Plugin configuration referenced via ExtensionRef |
| HTTPRoute → EdgionBackend | EdgionBackend | shared `backend_ref_mgr` | AI capability drives rule validation; apply/delete requeues the route |
| HTTPRoute → Secret/ConfigMap | Secret/ConfigMap | internal Edgion-auth and indirect plugins | Auth material uses the actual declaring route for permission and the published root for recovery |
| HTTPRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |

### RequestMirror owner-bound references (R3b)

Native rule/backend mirror fields use `HttpRouteMirrorRefs` actual-owner loans.
Authored loans alone authorize core Service targets; effective loans compile cold
Gateway Service/section keys and carry no Grant authority. Delegation resolves the
original child slot before cloning or rule drops. Valid Service and Grant-namespace
recovery edges register before authorization under the published root's actual
ResourceRef. An invalid mirror owner, union, target, name, namespace or port contributes no
new mirror dependency or mirror-target lookup. HTTP parse clears Service roles once after snapshotting old forwarding
targets; final forwarding registration adds to surviving child mirror edges.

An omitted metadata namespace remains omitted in the dependency owner and worker
wake key. `default` applies to target semantics and GatewayRouteIndex lookup only.
Current/dropped forwarding policy notifications and mirror-role exclusion remain
separate from mirror recovery. Status retains the narrow post-authorization cached
presence check that gives a missing Service BackendNotFound precedence over denial.

### Internal Edgion ExternalAuth material

`refs::route_auth_fields` separates authored authorization loans from effective
Gateway locality. The supported internal FS/etcd ForwardAuth shape remains
separate from unsupported native Gateway API ExternalAuth; native and mixed
objects are not converted into Edgion authentication configurations.

Parse clears root Secret dependencies once and scrubs incoming auth locality,
request-secret maps and TLS material before early returns. `flatten_route`
resolves the root exactly once even without delegation, returns authored root
decisions for parse to apply, and prepares every raw child declaration before
rule drops or inheritance. Parent, intermediate, terminal and backend filter
positions retain their own namespace. New material is acquired only through the
canonical bound credential pipeline; its owner-equality check remains intact.
A private receipt registers valid target recovery under the actual root before
Grant/store reads. Invalid source/target/root yields no new edge or material read;
valid missing/denied references keep recovery edges even when a rule is dropped.

Readonly status uses the same flatten/auth decisions without registering edges;
failures use existing ResolvedRefs diagnostics and remain attached to auth filters
for Gateway fail-closed sealing, rather than becoming an unguarded rule fallback.
Apply replaces all prior material/dependencies and delete clears the root's
Secret edges. Secret and Grant events use existing queues to reprocess the root.
Disabled local TLS stays inert; unrelated request templates are still resolved.
Service targets retain their separate no-new-grant/no-new-dependency contract.

The optional `resolvedExternalAuth` filter sidecar is serialized for conf_sync and
hidden from schema. It proves only Controller-derived locality under the existing
trusted transport boundary. It does not represent a borrowed child object,
ReferenceGrant authority or ongoing request-time revocation.
