# GRPCRoute Delegation (Multi-Tenancy)

GRPCRoute delegation lets a platform team hand ownership of a **gRPC service**
to an application team's namespace, without giving that team any access to
the Gateway or the root route. A parent GRPCRoute rule points its `backendRef`
at child GRPCRoutes instead of a Service, and each child names that parent in
its own `parentRefs`. The Gateway matches a call level by level: first the root
route, then the child that the matched root rule delegates to, down to the rule
that selects a backend.

It works exactly like [HTTPRoute delegation](../http-route/delegation.md):
the same parentRefs shape, bidirectional confirmation, five-level limit,
per-level precedence, backtracking, inheritance, status reasons and access-log
shape. This page covers what differs for gRPC: instead of a path prefix, a
delegating rule hands over one **gRPC service name** (for example
`test.TestService`). Method-level delegation and delegate-all (no service) are
not supported.

## Quick start

Parent (root) route, owned by the platform team in the Gateway's namespace:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: delegation-parent
  namespace: edgion-test
spec:
  parentRefs:
    - name: grpc-delegation-gateway
      sectionName: grpc
  hostnames: ["grpc-delegation.example.com"]
  rules:
    - matches:
        - method:
            type: Exact                # Exact or omitted; method must be absent
            service: test.TestService
      filters:                         # mandatory for all delegated traffic
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set: [{ name: x-parent-filter, value: "on" }]
      backendRefs:
        - group: gateway.networking.k8s.io
          kind: GRPCRoute
          name: "*"                    # every GRPCRoute in that namespace that names this parent
          namespace: edgion-grpc-deleg
```

Child route, owned by the application team in their namespace:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: child
  namespace: edgion-grpc-deleg
spec:
  parentRefs:                          # required: the child must name its parent
    - group: gateway.networking.k8s.io
      kind: GRPCRoute
      name: delegation-parent
      namespace: edgion-test
  rules:
    - matches:
        - method:
            type: Exact
            service: test.TestService
            method: SayHello
      backendRefs:
        - name: child-grpc-svc
          port: 30021
```

The fixtures under `edgion-tests/integration/conf/GRPCRoute/Delegation/` are
complete working examples.

## Building a delegation tree

### parentRefs shape and confirmation

The rules are the same as for HTTPRoute, with `kind: GRPCRoute`:

- One or more `kind: Gateway` refs make a **root** route; 1–3
  `group: gateway.networking.k8s.io, kind: GRPCRoute` refs make a **child**
  route; a route with no `parentRefs` is inert.
- Mixing Gateway and GRPCRoute refs, more than three parent routes, or another
  route kind (for example `kind: HTTPRoute`) invalidates the whole route
  (`Accepted=False / InvalidParentRefs` on every entry).
- A child must not set `spec.hostnames` (`Accepted=False / UnsupportedValue`).
- An edge exists only when a parent rule delegates to the child (by name or
  through `name: "*"`) **and** the child lists that parent in its
  `parentRefs`. No ReferenceGrant is involved.
- At most **5 levels, counting the root**. Cycles are not reported and cannot
  loop.

### Delegating rule

A rule delegates when its single `backendRef` has
`group: gateway.networking.k8s.io, kind: GRPCRoute`. Constraints:

- Exactly one backendRef; no mixing with Service refs.
- `port` and per-backendRef `filters` must be absent. `weight` is ignored.
- `matches` must be present and non-empty. **Unlike HTTPRoute, a matchless
  delegating rule is invalid**, because delegate-all is not supported for gRPC.
- Every match must carry a `method` matcher with `type: Exact` (or omitted),
  a non-empty `service`, and no `method`. A header-only match, a
  `RegularExpression` matcher, or a matcher that sets `method.method` makes the
  whole delegating rule invalid. Header conditions beside the service are
  allowed and are checked at the parent's level.
- A rule with several matches delegates each service to the same target.
- `name: "*"` delegates to every GRPCRoute in the target namespace that names
  this parent. `namespace` defaults to the parent's namespace.
- Backend-selecting plugins (`Canary`, `DirectEndpoint`,
  `DynamicInternalUpstream`, `DynamicExternalUpstream`, `RegionRoute`,
  `GraphqlOperationRouter`, `AllEndpointStatus`, `AiProxy`) are valid only on
  terminal rules; on a delegating rule they make the rule invalid.

A malformed delegating rule is dropped and reported through the parent's
`PartiallyInvalid` condition.

## Matching

### Child matches are absolute; the scope is the intersection

The parent level has already fixed the service, so each child match only has
to match within it:

- A child match with a `service` matches only when it equals the request's
  service. A different service (the `test.OtherService` rule of the `child`
  fixture) never matches through that parent. It is not an error.
- A child match with no `service` (no method matcher, or a method-only
  matcher) matches any service, which after the intersection means the
  delegated one.
- A **matchless child rule** is a catch-all inside the delegated service
  (`matchless-child` fixture). This is safe: other services were already
  rejected at the root level.
- Ordinary GRPCRoute method validation still applies to child rules: an empty
  `method: {}`, an empty Exact service or method, or an invalid name
  invalidates that child rule.
- One child can serve several parents (`multi-child` fixture). Under each parent
  it carries only that parent's mandatory filters.

### Precedence and backtracking

Each level ranks only its own rules with the ordinary
[GRPCRoute precedence](matches/overview.md#precedence). A more specific child
match never outranks a different root route at the root level.

If nothing below a delegating rule matches, the Gateway backtracks to the next
candidate at the same level. In the `grpc-backtrack-parent` fixture,
`test.TestService/StreamNumbers` enters `backtrack-child` (which serves only
`SayHello`), misses, and is served by the parent's own catch-all rule. Each
delegated subtree is entered at most once per call, and the first path that
enters it decides its outcome; see
[the HTTPRoute limitation](../http-route/delegation.md#backtracking-and-the-first-entry-wins-limitation).
A call that no route matches gets the usual `UNIMPLEMENTED` (gRPC status 12).

## Inheritance

Identical to [HTTPRoute](../http-route/delegation.md#inheritance):

- Standard filters on the delegating rule (`RequestHeaderModifier`,
  `ResponseHeaderModifier`) are mandatory. A child rule with a filter of the
  same type, on the rule or on one of its `backendRefs[].filters`, is skipped
  under that parent and reported as `PartiallyInvalid`. A skipped rule is a
  miss, so the call may backtrack to another rule and backend.
- Levels run in order, root → leaf, and each level sees what earlier levels
  changed. The Gateway does not stop a child plugin from changing a header a
  parent set; the child route's owner is responsible for that.
- EdgionPlugins run parent first, then child; each level's `ExtensionRef`
  resolves in its own route's namespace.
- One WAF owner per call: the deepest level's WAF wins unless an ancestor's
  WAF sets `delegationPrecedence: parent`.
- `timeouts` and `retry` come from the deepest level that sets each
  sub-field. `sessionPersistence` is inherited as a whole block.
- Backends are leaf only, with child-local namespace defaults and
  ReferenceGrant checks.
- At most one exclusive owner (AI owner, transcoder) on the whole chain.
- `edgion.io/retry-non-idempotent` is read from the leaf route only.

## Status

Status reasons are the same as for
[HTTPRoute](../http-route/delegation.md#status), with `kind: GRPCRoute`
parentRefs:

- **Parent**: `PartiallyInvalid` for its own malformed delegating rules;
  `ResolvedRefs=False / BackendNotFound` for a missing named child;
  `ResolvedRefs=False / RefNotPermitted` for a named child that does not list
  this parent.
- **Child**, one entry per `kind: GRPCRoute` parentRef:
  `Accepted=True / DelegationAccepted`, or `Accepted=False` with
  `NoMatchingParent`, `NotAttached`, `DepthExceeded`, `InvalidParentRefs` or
  `UnsupportedValue`; `PartiallyInvalid=True` lists child rules that can never
  serve under that parent: `Rule(s) outside parent <Kind> '<namespace/name>'
  scope: …` for method matches naming a service other than the delegated one,
  and `Conflicting Rule(s) with parent <Kind> '<namespace/name>': …` for rules
  that conflict with the standard filters of every delegating rule that can
  reach them. When those rules cover every child rule, the entry is
  `Accepted=False / UnsupportedValue` instead. A parent that fails its own
  validation (any error, including one malformed rule) makes its children
  `NotAttached`; a rule naming its own route gets `RefNotPermitted`.

## Observability

The access-log `match_info` is an array ordered root → leaf, with one element
per matched level, the same array shape as HTTPRoute; each GRPCRoute element also carries `rule_name` and `matched`. Metrics label the leaf route.

## Limitations

The [HTTPRoute limitations](../http-route/delegation.md#limitations) apply
unchanged: "first entry wins" for shared subtrees, at most 64 delegated
subtrees entered per call, no response caching for delegated calls, child
per-rule plugin state restarting when the set of Gateways in the process
changes, and parent and child changes applied independently.
