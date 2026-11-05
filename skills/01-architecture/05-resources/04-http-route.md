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
2. Per-rule header modifier validation uses the shared `HTTPRouteRule::header_modifier_error()` in the invalid-rule path, including backend filters and delegating rules. Invalid rules are excluded while valid siblings survive; all-invalid header rules reject attachment. See [header modifier validation](../../04-review/plugin-runtime/regression-guards/header-modifier-validation.md).

### Decode and type-level preparse

`HTTPRouteMatch.headers` normalizes at typed deserialization: only the first
ASCII-case-equivalent name survives, with its original type, value, spelling,
and relative order. This happens before Controller validation and also covers
Gateway decoding. Delegation does not merge parent and child predicates: each
level matches its own decoded headers.

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
looked up or registered for that invalid source.

Under delegation every route, parent or child, authorizes and registers only its
own authored backendRefs under its own owner; no rule is cloned across routes.
A child's terminal backends are therefore checked with the child as the referrer
and wake the child. Valid terminal receipts survive later rule filtering and
missing/denied targets. Service manager owners retain their explicit canonical
namespace while Grant wakeups retain raw metadata namespace representation.
Apply replaces all prior dependencies and requeues dropped policy keys.

### parse

0. **Replace ReferenceGrant dependencies and mark denials**: clear the prior CrossNsRefManager set once at parse entry. Before every ReferenceGrant store read, register the target namespace under the Route owner; this applies to ordinary backendRefs and RequestMirror. Mark `ref_denied` on `spec.rules`. Clear any authored `resolved_delegation_keys` (controller-owned). Then sync the participants-only `DelegationGraph` (`http_route_delegation_graph`, `delegation_graph.rs`) from this route's own facts (`route_delegation.rs::http_delegation_facts`): parentRef shape, delegating targets with their standard-filter masks, declared parents, root flag. An ordinary route costs one participant probe. A non-inert child gets `resolved_delegation_keys` = its confirmed keys (≤6). The returned requeue set (target children, declared parents, level dependents) goes through the workqueue. See [Delegation (cascading match)](#delegation-cascading-match).
1. **Complete cross-namespace reference collection**: idempotently add all authored target namespaces to CrossNsRefManager without another clear, preserving dependencies installed before authorization reads
2. **Register Service references**: register all Service-typed backendRefs into ServiceRefManager so that Service changes automatically requeue the route
3. **Clear and recompute resolved_listeners**: from parentRef, resolve the target Listener's address and protocol, producing `Vec<ListenerKey>` (formatted `"{addr}/{protocol}"`) for per-listener route isolation on the Gateway side. Resolution logic is shared via `resolve_listener_keys()` (used by 6 handlers in `handlers/route_utils.rs`):
   - Always look up the Gateway to obtain address and protocol (the parentRef.port-only fast path no longer exists)
   - When sectionName + port are both present, validate consistency between them
   - If neither is specified → use all of the Gateway's Listeners (subject to the allowedRoutes namespace check)
4. **Resolve resolved_hostnames**: compute the intersection of the route's hostnames with the Gateway Listener's hostnames
5. (ReferenceGrant `ref_denied` marking happens in step 0 — see above)

### Delegation (cascading match)

The Controller validates and publishes each route once; it no longer flattens
delegation trees. The Gateway matches level by level (see the
[HTTP route engine](../02-gateway/03-routes/01-http-route.md#delegation-cascading-match)).
Design: [`tasks/todo/route-delegation-cascading-match-design.md`](../../../tasks/todo/route-delegation-cascading-match-design.md);
user semantics: [`docs/en/user-guide/http-route/delegation.md`](../../../docs/en/user-guide/http-route/delegation.md);
review knowledge: [cascading match (ISSUE-010)](../../04-review/routing/regression-guards/delegation-cascading-match-issue-010.md).

- **Shared vocabulary** (`edgion-resources/src/resources/route_delegation.rs`):
  `classify_parent_refs` (`None` / `Root` / `Child(≤3)` / `Invalid`),
  `delegation_target_of_http_rule` (exactly one HTTPRoute backendRef, no port, no
  per-backend filters, PathPrefix matches only), `DelegationKey
  { kind, parent_ns, parent_name, target_ns, target_name | "*" }`,
  `standard_filter_mask_http`, `MAX_ROUTE_DELEGATION_LEVELS = 5`,
  `MAX_ROUTE_KIND_PARENT_REFS = 3`. Controller and Gateway use the same helpers.
- **Facts** (`handlers/route_delegation.rs`): malformed delegating rules and
  delegating rules with a backend-selecting plugin (`is_backend_selecting_plugin`:
  upstream-jump plugins plus `AllEndpointStatus` and `AiProxy`) become invalid
  rules (`PartiallyInvalid`). A route with an invalid shape, or a child that sets
  `hostnames`, is inert: no targets, no keys. A route that fails its own
  validation (any error, including one malformed ordinary rule) is `blocked`: it
  keeps its edges but gets no level, so its children become `NotAttached`.
- **Graph** (`delegation_graph.rs`): parent → (target, edge rules: each delegating
  rule's standard-filter mask and scope, canonical path prefixes for HTTP or
  service names for gRPC), named target →
  parents (keeps `BackendNotFound` / `RefNotPermitted` fresh on any route
  create/delete with one probe), parent → declared children, child → declaration,
  roots, and the minimum level (root = 1, child = 1 + min over confirmed parents,
  above 5 = none). A wildcard target filters the declared-children index by
  namespace, never scans a namespace. Cycles are not detected; levels converge to
  none once the cycle loses its root.
- **Requeue** (workqueue only): parent target change → old and new target
  children; child parentRefs change → only those old and new declared parents that
  name this child as a target (a `*` parent's status never depends on a child);
  level or `blocked` change → children (at most 4 hops). Handlers read only in-memory
  indexes.
- **Transport**: the conf_sync payload is the authored route plus, on a child,
  `resolved_delegation_keys` (`#[schemars(skip)]`, skipped when empty, so an
  ordinary route's payload is unchanged). There is no copied rule set, parent-match
  sidecar or namespace stamp.

### on_change

1. Register into `GatewayRouteIndex` (Gateway changes will requeue this route)
2. Update `AttachedRouteTracker`; if the attachment relationship has changed, requeue the parent Gateway (to refresh the attachedRoutes count)
3. Requeue the parents that name this route as a delegation target (`DelegationGraph::parents_naming`, one probe; refreshes their `BackendNotFound` / `RefNotPermitted`). All other delegation requeues are issued by `parse`

> **Post-init one-shot (not an on_change step):** `trigger_delegation_parent_revalidation` (`ref_grant/revalidation_listener.rs`, invoked from `run_post_cache_ready_revalidations`) requeues all registered delegating parents once caches are ready, closing the init window where a parent was processed before its children were loaded.

### on_delete

- Clear registrations in CrossNsRefManager, ServiceRefManager, GatewayRouteIndex, and AttachedRouteTracker
- Delegation cleanup: `DelegationGraph::on_route_deleted` removes a participant and returns its declared children, declared parents and the parents naming it; each is requeued (one read-locked probe for an ordinary route)
- If AttachedRouteTracker has changes, requeue the parent Gateway

### update_status

Generate an independent RouteParentStatus for each parentRef:
- `Accepted`: validates whether the Gateway/Listener corresponding to parentRef exists and admits this route (non-empty hostname intersection, namespace policy permits). Only `kind: Gateway` parentRefs enter this path
- `ResolvedRefs`: canonical effective target views validate Service/EdgionBackend identity and existence. Invalid references map through the existing `InvalidKind` reason. Valid denied terminals retain `RefNotPermitted` without an existence read. Valid delegation selectors are excluded from terminal existence reads. For named delegation targets, `parent_named_target_errors` reports a missing child as `BackendNotFound` and an existing child that does not declare this parent as `RefNotPermitted`; wildcard targets never report.
- `PartiallyInvalid`: own invalid rules only, including malformed delegating rules and delegating rules carrying a backend-selecting plugin. Child-caused problems never appear on the parent
- Native CORS on any backendRef invalidates the entire owning rule through
  `HTTPRouteRule::unsupported_backend_cors_reason`. The same check runs on the
  Controller and independently on the Gateway's rules before
  runtime compilation. Rule-level native CORS remains supported; no backend policy
  is promoted or merged into the rule. See the
  [scope decision](../../04-review/cors/intentional-design/native-cors-rule-scope.md).
  If an authored rule has this scope error and no effective rule survives, the
  Gateway attachment uses `Accepted=False/UnsupportedValue` instead. A delegation
  child reports the reason on its own entries, never on its parent.
- **Per-rule findings never flip `Accepted` while a sibling rule still serves** — `Accepted=False` removes the whole route from the gateway route table (`filter_accepted_parent_refs`), so a rule-level problem rides `PartiallyInvalid` instead. Authored native backend CORS scope violations escalate on a route where **no** effective rule serves any more: then `Accepted=False/UnsupportedValue` lists the offending `rules[i]: <reason>` and `PartiallyInvalid` is never set True (an earlier True is cleared to False with reason `UnsupportedValue`), so the two conditions cannot contradict each other
- Child-side delegation status (`child_entry_verdict`): each `kind: HTTPRoute` parentRef gets its own entry — `Accepted=True/DelegationAccepted` when the edge is confirmed and the parent has a level ≤4; otherwise `Accepted=False` with `NoMatchingParent` (no confirmed edge), `NotAttached` (parent has no level) or `DepthExceeded` (parent level 5). An invalid shape (`InvalidParentRefs`) or child `hostnames` (`UnsupportedValue`) rejects every entry. A mandatory-filter conflict with that parent (edge mask AND child union mask, then per-rule masks to name rules) keeps `Accepted=True` and adds `PartiallyInvalid=True`. The child's own invalid rules ride its entries as for any route. The Gateway installs child keys only for entries with `Accepted=True`
- Cleans up old status entries that correspond to parentRefs no longer present

Native CORS also rejects `allowCredentials: true` with any exact `*` entry in
`exposeHeaders`, including mixed lists. `HTTPRouteRule::unsupported_cors_reason`
combines this support restriction with backend scope enforcement for Controller
status and Gateway publication. Valid siblings survive; an authored
violation with no effective rule uses the existing attachment rejection path.
See the [credentialed exposure policy](../../04-review/cors/intentional-design/native-credentialed-exposure-policy.md).

### AI source diagnostics and execution admission

After resolving references, the Controller attaches bounded
`resolved_ai_admission` backend facts to the route's own rules.
There is one fact per original backend slot; invalid or denied
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
backend WAF/native CORS rejection behavior remains unchanged.

### ExtensionRef ownership and recovery

The Controller consumes `HttpRouteExtensionRefs` loans from the route's own
filter positions. Exact `edgion.io/EdgionPlugins`, a valid required name,
and a valid owning Route identity precede dependency registration or content
lookup. Every ExtensionRef resolves in the namespace of the route that declares
it; there is no namespace stamp, and delegation never moves a filter into another
route. No ExtensionRef loan can mint a ReferenceGrant query. (The `authored*` and
`effective*` facade methods now return the same data; collapsing them is an open
follow-up.)

Each route registers its own plugin dependencies under its own owner. Only
structurally eligible scope checks read plugin content, using `get_raw_any` after
registration so a pending desired generation is visible. Parse entry clears the
prior plugin dependency set once; later status registration is additive. An
omitted route namespace stays omitted in the manager wakeup key while local
targets use `default`. Invalid owners and targets contribute no new plugin edge
or content lookup. For a valid owner, parse is the sole dependency-clear boundary
and status only extends the set. A standalone status call for an invalid source
may withdraw stale dependencies and derived listeners/hostnames; an invalid
target, dropped rule or missing plugin under a valid owner does not permit that
cleanup. Gateway rule and selected-backend builders borrow the actual Route and
its rule/backend/filter slots through the same canonical facade. A validated
local target becomes one cold complete store key; invalid descriptors retain a
positional `None` and cannot query or consume a same-name plugin. Publication
demand, AI admission and adaptive facts use that same projection. Request
resolution, CORS/WAF selection and access-log declarations consume these
validated keys or their positional results under the captured class scope. On a
delegated request each ancestor level resolves its own ExtensionRef array against
the root-matched Gateway's plugin scope (`DelegatedMatch::ext_refs`); see the
[ExtensionRef locality guard](../../04-review/routing/regression-guards/delegated-extensionref-remains-child-local.md).

## Gateway-side processing

HTTPRoute's ConfHandler implements route compilation and registration:

0. **Rules and delegation children**: compiles `spec.rules` minus `invalid_rule_indices`. A delegation child (`delegation_build.rs::is_delegation_child`: all parentRefs are HTTPRoute refs) has no `resolved_listeners`; it is not bucketed by listener and is routed to the delegation slot path instead. Its accepted keys are `child_accepted_keys`: Controller-resolved keys, locally re-checked against the child's own parentRefs and shape (`accept_child_keys`), and kept only for parentRef entries with a current `Accepted=True` status. Child units are compiled once (own namespace, no hostname, no listener check; leaf metric handles and plugin scopes prebuilt for every Gateway in the process) and `Arc`-shared into the subtrees of their ≤6 keys. A delegating rule unit obtains its `Arc<DelegationSlot>` at compile time from the global per-kind `DelegationSlotTable`
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
| HTTPRoute → HTTPRoute | HTTPRoute | delegation backendRef (`kind: HTTPRoute`) + child `parentRefs {kind: HTTPRoute}` + `DelegationGraph` | Edge requires bidirectional confirmation; child carries `resolved_delegation_keys`; Gateway matches through per-key subtrees. A child change republishes only the child |
| HTTPRoute → Service | Service | backendRef | Route's backend target service |
| HTTPRoute → EdgionPlugins | EdgionPlugins | ExtensionRef filter | Plugin configuration referenced via ExtensionRef |
| HTTPRoute → EdgionBackend | EdgionBackend | shared `backend_ref_mgr` | AI capability drives rule validation; apply/delete requeues the route |
| HTTPRoute → Secret/ConfigMap | Secret/ConfigMap | internal Edgion-auth and indirect plugins | Auth material uses the actual declaring route for both permission and recovery |
| HTTPRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |

### RequestMirror owner-bound references (R3b)

Native rule/backend mirror fields use `HttpRouteMirrorRefs` actual-owner loans.
Authored loans alone authorize core Service targets; effective loans compile cold
Gateway Service/section keys and carry no Grant authority. Each route resolves its
own mirror slots. Valid Service and Grant-namespace recovery edges register before
authorization under that route's actual ResourceRef. An invalid mirror owner, union, target, name, namespace or port contributes no
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

Parse clears the route's Secret dependencies once and scrubs incoming auth locality,
request-secret maps and TLS material before early returns.
`route_terminal::root_auth` resolves the route's own auth fields exactly once for
every route, delegating or not, and returns decisions for parse to apply. Every
level of a delegation chain resolves its own auth filters in its own namespace. New material is acquired only through the
canonical bound credential pipeline; its owner-equality check remains intact.
A private receipt registers valid target recovery under the actual route before
Grant/store reads. Invalid source/target/route yields no new edge or material read;
valid missing/denied references keep recovery edges even when a rule is dropped.

Readonly status uses the same auth decisions without registering edges;
failures use existing ResolvedRefs diagnostics and remain attached to auth filters
for Gateway fail-closed sealing, rather than becoming an unguarded rule fallback.
Apply replaces all prior material/dependencies and delete clears the route's
Secret edges. Secret and Grant events use existing queues to reprocess the route.
Disabled local TLS stays inert; unrelated request templates are still resolved.
Service targets retain their separate no-new-grant/no-new-dependency contract.

The optional `resolvedExternalAuth` filter sidecar is serialized for conf_sync and
hidden from schema. It proves only Controller-derived locality under the existing
trusted transport boundary. It does not represent a borrowed child object,
ReferenceGrant authority or ongoing request-time revocation.

### Session persistence lifetime ownership

Gateway rule compilation validates each rule's own `sessionPersistence` and
resolves its optional lifetime before publishing any match units. The shared
resource policy uses the canonical duration parser with strict surrounding
whitespace rejection and whole-second ceiling/minimum-one semantics (including
zero). Both direct compilation and normal full/incremental publication reject an
invalid policy, including Permanent cookies without `absoluteTimeout`.

The scalar belongs to the immutable Gateway rule unit alongside its effective
rule, not to the serialized resource. A request retains that same generation
through response issuance after reload. Response code borrows config and lifetime
together and uses one scalar for wire expiry and Permanent-cookie Max-Age; it
never parses configured duration text. See the
[session persistence regression guard](../../04-review/routing/regression-guards/session-persistence-token-extraction.md)
for ARCH-060 verification boundaries.
