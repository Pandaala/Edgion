# Route Delegation (Multi-Tenancy)

Route delegation lets a platform team hand ownership of a path prefix to an
application team's namespace, without giving that team any access to the
Gateway or the root route. A parent HTTPRoute rule points its `backendRef` at
child HTTPRoutes instead of a Service, and each child names that parent in its
own `parentRefs`. The Gateway then matches a request level by level: first the
root route, then the child route that the matched root rule delegates to, and
so on, down to the rule that finally selects a backend.

## Why

In a multi-tenant cluster, only the platform namespace can attach routes to
the shared Gateway (`allowedRoutes` defaults to `Same`). Without delegation,
every application routing change has to go through the platform team's root
route. With delegation, the platform team writes one delegating rule per
tenant prefix, and each tenant manages child routes freely inside their own
namespace. Delegation is an ownership boundary: a child can only narrow the
traffic its parent hands over, and it cannot affect matching outside that
scope or at the levels above it.

## Quick start

Parent (root) route, owned by the platform team in the Gateway's namespace:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: delegation-parent
  namespace: edgion-test
spec:
  parentRefs:
    - name: delegation-gateway
  hostnames: ["delegation.example.com"]
  rules:
    - matches:
        - path: { type: PathPrefix, value: /api/v1/billing }
      filters:                       # mandatory for all delegated traffic
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set: [{ name: x-parent-filter, value: "on" }]
      backendRefs:
        - group: gateway.networking.k8s.io
          kind: HTTPRoute
          name: "*"                  # every HTTPRoute in edgion-deleg that names this parent
          namespace: edgion-deleg
    - matches:
        - path: { type: PathPrefix, value: /api }
      backendRefs:                   # the platform's own fallback for /api
        - name: test-http
          port: 30001
```

Child route, owned by the billing team in their namespace:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: billing-child
  namespace: edgion-deleg
spec:
  parentRefs:                        # required: the child must name its parent
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      name: delegation-parent
      namespace: edgion-test
  rules:
    - matches:
        - path: { type: PathPrefix, value: /api/v1/billing/pay }   # absolute path
      backendRefs:
        - name: child-svc
          port: 30001
```

`GET /api/v1/billing/pay` matches the parent's delegating rule, then the
child's rule, and reaches `child-svc` with `x-parent-filter: on`. The fixtures
under `edgion-tests/integration/conf/HTTPRoute/Delegation/` are complete
working examples of every scenario on this page.

## Building a delegation tree

### parentRefs shape

A route's `parentRefs` decide its role, and the two roles are exclusive:

| Shape | Role |
| --- | --- |
| One or more `kind: Gateway` refs | **Root** route. It attaches to listeners as usual. |
| 1–3 refs with `group: gateway.networking.k8s.io, kind: HTTPRoute` | **Child** route. It never attaches to a listener. |
| No `parentRefs` | Inert. The route is never reachable. |

Mixing Gateway refs and HTTPRoute refs, naming more than three parent routes,
or naming a different route kind (for example `kind: GRPCRoute`) makes the
whole route invalid: every `parentRefs` entry reports
`Accepted=False / InvalidParentRefs` and the route serves nothing. A child must
also leave `spec.hostnames` empty, because the hostname belongs to the root;
a child that sets hostnames is inert and reports
`Accepted=False / UnsupportedValue`.

### Delegating rule

A rule delegates when its single `backendRef` has
`group: gateway.networking.k8s.io, kind: HTTPRoute`. Constraints:

- Exactly one backendRef; no mixing with Service refs.
- `port` and per-backendRef `filters` must be absent. `weight` is ignored.
- Every match must use `PathPrefix`. A rule without matches, or a match without
  a path, delegates `PathPrefix /`. Header, query and method conditions on the
  delegating rule are allowed; they are checked at the parent's level.
  A match with only headers, query or method therefore delegates every path
  under `/` that meets those conditions: add a `PathPrefix` to keep the
  delegated scope narrow.
- `name: "*"` delegates to every HTTPRoute in the target namespace that names
  this parent in its `parentRefs`. `namespace` defaults to the parent's
  namespace.
- Plugins that select the upstream themselves are valid only on terminal
  rules. An ExtensionRef on a delegating rule that enables `Canary`,
  `DirectEndpoint`, `DynamicInternalUpstream`, `DynamicExternalUpstream`,
  `RegionRoute`, `GraphqlOperationRouter`, `AllEndpointStatus` or `AiProxy`
  makes that rule invalid.

A malformed delegating rule is dropped and reported through the parent's
`PartiallyInvalid` condition. Its sibling rules keep serving.

### Bidirectional confirmation

An edge from a parent to a child exists only when **both** sides agree:

1. a rule of the parent delegates to the child, by name or through `name: "*"`
   in the child's namespace, **and**
2. the child lists that parent in its `parentRefs`.

The child's `parentRefs` entry is the child team's consent; no ReferenceGrant
is involved. A named child that exists but does not list the parent is
reported on the parent as `ResolvedRefs=False / RefNotPermitted`. A wildcard
simply picks up the children that declare the parent; zero matching children is
not an error.

### Depth: at most five levels

A delegation chain has at most **5 levels, counting the root**: the root plus
four delegation edges. A level-5 route may still have terminal rules, but its
delegating rules never match. The child that would sit at level 6 reports
`Accepted=False / DepthExceeded`. The `chain-l1` … `chain-l6` fixtures show
this limit.

There is no limit on the number of children per parent. Cycles are not
detected or reported; they cannot loop (see
[Backtracking](#backtracking-and-the-first-entry-wins-limitation)). A cycle
that is not reachable from a Gateway-attached root reports `NotAttached`.

## Matching

### Child paths are absolute; the scope is the intersection

Child routes write **absolute** paths. Delegation never prepends or strips the
parent prefix. A request has to match the parent rule's `PathPrefix` and then
the child rule's own path, so the effective scope is the intersection of both:

- A child path inside the parent prefix (`/api/v1/billing/pay` under
  `/api/v1/billing`) serves that sub-path.
- A child path disjoint from the parent prefix (`/evil` under
  `/api/v1/billing`) never matches through that parent. It is not rejected
  and not reported.
- A child match with no path, or with `PathPrefix /`, covers the whole
  delegated scope.
- A child `RegularExpression` effectively means "parent prefix AND child
  regex".
- One child can serve several parents. Under each parent it serves the
  intersection with that parent's prefix. In the `multi-child` fixture,
  `/v1/billing/invoice` is served through `billing-v1-parent` and
  `/v2/billing/invoice` through `billing-v2-parent`.

The whole chain is matched against the original request before any filter or
plugin runs. A parent `URLRewrite` or a header added by a parent plugin never
changes how the child matches.

### Precedence is per level

Each level ranks only its own rules, with the ordinary
[matching precedence](matches/README.md): path specificity, then method,
header count, query-parameter count, `creationTimestamp`, `namespace/name`,
and rule/match index. Parent conditions are not added to the child's counts,
and child conditions do not bubble up.

As a result, a more specific child path does not outrank a different root
route at the root level. In the `precedence-*` fixtures, root A delegates
`/api` to a child with `Exact /api/users`, and root B has
`PathPrefix /api/users` on the same hostname. Root B wins `/api/users` at the
root level. A tenant therefore cannot take traffic away from a sibling root
route.

### Backtracking and the "first entry wins" limitation

A delegating rule does not end the match. After its own conditions match, the
Gateway descends into the delegated children:

- If a child rule matches (directly or through further delegation), the chain
  is complete.
- If nothing matches below, the delegating rule counts as not matched and the
  Gateway **backtracks**: it tries the next candidate at the same level, then
  the next hostname group, then regex routes, then 404. In the
  `delegation-parent` fixture, `/api/v1/billing/unknown` enters the billing
  children, finds no match, and is served by the parent's own `/api` rule.

Each delegated subtree is entered **at most once per request**. If a later
candidate leads into a subtree this request has already tried, that candidate
is skipped. This caps per-request work and makes cycles harmless.

The first path that enters a subtree decides its outcome for this request. In
two shapes this can return 404 even though another path could have matched:

- **Uneven diamonds**: a child is reachable through two paths of different
  length, and the first path reaches it at a depth where its own delegating
  rules can no longer descend.
- **Mixed mandatory filters**: two rules of one parent delegate to the same
  target with different standard filters. They share one subtree, so the
  first rule's filter conflicts decide which child rules are tried.

This is a documented limitation, not a bug. Keep such shapes out of production
trees: delegate a target from one rule per parent, and keep chains of equal
length when a child has several paths to the root.

## Inheritance

Filters and plugins run in chain order, root → … → leaf, in every stage
(request, upstream request, upstream response, response, logging). Within one
level, the authored order is kept. Response stages also run parent first.

- **Standard filters always run.** A delegating rule's standard filters
  (`RequestHeaderModifier`, `ResponseHeaderModifier`, `RequestRedirect`,
  `URLRewrite`, `RequestMirror`, `CORS`, `ExternalAuth`) run for all traffic
  delegated through it. A child rule that declares a standard filter of a type
  already on the chain, either on the rule itself or on one of its
  `backendRefs[].filters`, is skipped under that parent; the child reports
  `PartiallyInvalid=True` on that parent's entry. Skipping works like any
  other miss: the Gateway backtracks, so the request may be served by another
  rule, including the parent's own fallback rule, and reach a different
  backend. The same child rule still serves under a parent without the
  conflict.
- **Levels run in order; later levels see earlier changes.** Each level works
  on the request as the levels before it left it, and response stages likewise
  run in chain order. The Gateway does not stop a lower level from changing what
  a higher level set: a child `ProxyRewrite` can set a header the parent
  removed, a child `ResponseRewrite` can change a header the parent set, and a
  child `RequestRedirect` builds its `Location` from the path and `Host` as the
  parent's `URLRewrite` left them. Gateway API forbids `URLRewrite` and
  `RequestRedirect` in one rule, but delegation does not check the pair across
  levels. The owner of the child route is responsible for these interactions;
  matching itself always uses the original request.
- **EdgionPlugins run parent first, then child.** Each level's `ExtensionRef`
  resolves EdgionPlugins in the namespace of the route that declares it: parent
  plugins in the parent's namespace, child plugins in the child's namespace.
  Each level's plugins and templates (`${matchedHttpRoute:...}`, labels,
  annotations) see their own level's route and rule, so a child cannot change
  the settings of a parent-owned plugin.
- **One WAF per request.** WAF elects a single owner for the whole request and
  never runs twice. On a delegated chain the deepest level that declares a WAF
  wins by default, so a child can replace its parent's WAF. A parent that must
  keep its own WAF sets `delegationPrecedence: parent` in the WAF config; the
  outermost level with that setting then wins over every level below it. Within
  one level the usual `priority` rule still applies, and a route-level WAF still
  wins over Gateway and global WAFs. `delegationPrecedence` applies only to WAFs
  referenced from routes; on a Gateway or global WAF it has no effect, so a
  platform that must keep its WAF on delegated traffic attaches a `parent` WAF to
  the root route.
- **`timeouts` and `retry` come from the deepest level that sets them**, per
  sub-field: the leaf's own value first, otherwise the nearest ancestor's.
  `timeouts.request` and `timeouts.backendRequest` fall back separately, as do
  `retry.attempts`, `retry.backoff` and `retry.codes`.
- **`sessionPersistence` is inherited as a whole block**: the leaf's block if it
  sets one, otherwise the nearest ancestor's block. Sub-fields are never mixed
  across levels.
- **Backends are leaf only.** A child `backendRef` without `namespace` means
  the child's namespace, and cross-namespace Service references are checked
  against ReferenceGrants with the child as the referrer.
- **Exclusive owners stay exclusive across the chain.** At most one AI owner,
  gRPC-JSON transcoder or GraphQL operation router may appear on the whole
  matched chain, with the same rules as for an ordinary route.
- The route annotations `edgion.io/forward-raw-path` and
  `edgion.io/retry-non-idempotent` are read from the leaf route only.

## Status

### Parent route (has delegating rules)

| Condition | Meaning |
| --- | --- |
| `PartiallyInvalid=True` | One of its own delegating rules is malformed or carries a backend-selecting plugin. If no rule is left to serve, the attachment becomes `Accepted=False / UnsupportedValue`. |
| `ResolvedRefs=False / BackendNotFound` | A named child does not exist. |
| `ResolvedRefs=False / RefNotPermitted` | A named child exists but does not list this parent in its `parentRefs`, or the rule names this route itself (a route cannot delegate to itself). |

Problems inside a child are never reported on the parent.

### Child route

The child gets one `status.parents` entry per `kind: HTTPRoute` parentRef:

| Condition | Meaning |
| --- | --- |
| `Accepted=True / DelegationAccepted` | The parent exists, delegates to this child, is reachable from a root, and is at level 4 or less. A root is a route with Gateway `parentRefs` that passes its own validation; whether a listener accepts it is not checked here. |
| `Accepted=False / NoMatchingParent` | The parent does not exist or does not delegate to this child. |
| `Accepted=False / NotAttached` | The parent is not reachable from a root (this includes cycles with no root), or the parent or one of its ancestors fails its own validation and serves no delegated traffic. Any own-validation error blocks a parent, including a single malformed ordinary rule: fix the parent and its children recover. |
| `Accepted=False / DepthExceeded` | The parent is at level 5, so this child would be at level 6. |
| `Accepted=False / InvalidParentRefs` | The `parentRefs` shape is invalid (on every entry). |
| `Accepted=False / UnsupportedValue` | The child sets `spec.hostnames` (on every entry), or no rule of the child can serve under this parent because every rule is outside its scope or conflicts with its filters (on that entry). |
| `PartiallyInvalid=True` (with `Accepted=True`) | Some child rules can never serve under this parent. `Rule(s) outside parent <Kind> '<namespace/name>' scope: …` lists rules whose `Exact`/`PathPrefix` paths never fall inside any prefix this parent delegates to the child (regular-expression and pathless matches are not checked). `Conflicting Rule(s) with parent <Kind> '<namespace/name>': …` lists rules that conflict with the standard filters of every delegating rule that can reach them. |

The Gateway installs a child under a parent only while that entry is
`Accepted=True`. Depth is computed from the shortest path to a root. A longer
path through the same parent may still exceed five levels at request time; it
then simply does not descend.

## Observability

- **Access log**: `match_info` is an array ordered root → leaf, one element per
  matched level. Each element carries that level's own route
  (`rsc.kind` / `rsc.ns` / `rsc.name`), its authored `rule_id` and
  `match_id`, and its `sv` sync version. An ordinary route logs a
  one-element array. See the
  [access-log field reference](../../ops-guide/observability/access-log.md).
- **Metrics**: `route_namespace` / `route_name` label the leaf route.
- `AccessLogExtern` `from: routeLabel` / `routeAnnotation` read the leaf route.

## Limitations

- "First entry wins" for shared subtrees, described in
  [Backtracking](#backtracking-and-the-first-entry-wins-limitation).
- Delegated requests are never stored in or served from the response cache.
  The cache key and credential checks cover only the leaf route's plugins, so
  caching is turned off for delegated traffic rather than risking a response
  shared across a parent's auth boundary.
- Conflicts are reported per rule and only when certain. A child rule that
  conflicts with only some of the parent's delegating rules that reach it (for
  example, it conflicts with the parent's `/a` rule but not with its `/a/b` rule)
  is not listed in status; the Gateway still skips it for requests that come
  through the conflicting rule. A conflict with a grandparent or higher level
  is enforced at request time but not reported in status.
- One request enters at most 64 delegated subtrees, counted across all
  backtracking. Further delegating candidates are treated as misses. Empty
  subtrees (a target with no accepted children) are skipped and do not count.
- Path parameters (for example `:tid` in `/t/:tid`) are captured from the leaf
  rule's pattern only; plugins at a parent level do not see the parent pattern's
  parameters.
- A CORS preflight (`OPTIONS`) is answered from one level's EdgionPlugins
  `Cors` config; when several levels configure it, the deepest level's config
  answers.
- In-memory per-rule plugin state of a child route (for example local
  counters) restarts whenever a Gateway is added to or removed from the
  Gateway process.
- A parent change and a child change are applied independently. For a short
  window one request may see the new parent with the old child, or the
  reverse.

## Compatibility

Delegation activates only on `backendRef {kind: HTTPRoute}` and route-kind
`parentRefs`. Routes attached directly to a Gateway behave exactly as before.
`allowedRoutes` semantics are untouched (default remains `Same`); child routes
never attach to listeners, so listener policy does not apply to them.
