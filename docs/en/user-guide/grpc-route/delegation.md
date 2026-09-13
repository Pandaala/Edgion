# GRPCRoute Delegation (Multi-Tenancy)

GRPCRoute delegation lets a platform team hand ownership of a **gRPC service**
to an application team's namespace, without giving that team any access to
the Gateway or the root route. A parent GRPCRoute rule points its
`backendRef` at child GRPCRoutes instead of a Service; the controller expands
the tree into a flat route table, so the data plane never sees the nesting.

It mirrors [HTTPRoute delegation](../http-route/delegation.md) one-to-one,
with the delegated "scope" redefined for gRPC: instead of a path prefix, a
delegating rule hands over one **gRPC service name** (e.g.
`billing.v1.PaymentService`). Method-level delegation and delegate-all (no
service) are out of scope for v1.

## Why

In a multi-tenant cluster, only the platform namespace can attach routes to
the shared Gateway (`allowedRoutes` defaults to `Same`). Without delegation,
every application routing change has to go through the platform team's root
route. With delegation, the platform team writes one delegating rule per
tenant service, and each tenant manages child routes freely inside their own
namespace — RBAC is the security boundary.

## Quick start

Parent (root) route, owned by the platform team in the Gateway's namespace:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: root
  namespace: infra
spec:
  parentRefs:
    - name: shared-gateway
  hostnames: ["grpc.example.com"]
  rules:
    - matches:
        - method:
            type: Exact                # Exact or omitted; method must be absent
            service: billing.v1.PaymentService
      filters:                         # mandatory for all delegated traffic
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set: [{ name: x-tenant, value: billing }]
      backendRefs:
        - group: gateway.networking.k8s.io
          kind: GRPCRoute
          name: "*"                    # every GRPCRoute in that namespace
          namespace: billing
```

Child route, owned by the billing team in their namespace:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: billing-routes
  namespace: billing
spec:
  # No parentRefs needed: reachable only through the delegation.
  rules:
    - matches:
        - method:
            type: Exact
            method: Charge              # service is inherited from the delegation scope
      backendRefs:
        - name: payment-svc
          port: 50051
```

## Semantics

### Delegating rule

A rule delegates when its single `backendRef` has
`group: gateway.networking.k8s.io, kind: GRPCRoute`. Constraints:

- Exactly one backendRef (no mixing with Service refs).
- `port` and per-backendRef `filters` must be absent; `weight` is ignored.
- `matches` must be present and non-empty. **Unlike HTTPRoute delegation, a
  matchless delegating rule is invalid** — HTTPRoute treats a matchless
  delegating rule as delegating `"/"`, but delegate-all is out of scope for
  gRPC in v1, so that default is deliberately not mirrored.
- Every match must carry a `method` matcher with `type: Exact` (or omitted,
  which means Exact per Gateway API), a non-empty `service`, and no
  `method`. A match with no `method` matcher at all (header-only), a
  `type: RegularExpression` matcher, or a matcher that sets `method.method`
  makes the whole delegating rule invalid.
- `name: "*"` delegates to every GRPCRoute in the target namespace (excluding
  the parent itself); `namespace` defaults to the parent's namespace.
- A rule with multiple matches delegates each service independently (each
  with its own `headers` constraints) to the same children.

A malformed delegating rule is dropped and reported via the route's
`PartiallyInvalid` condition; a missing named child is reported via
`ResolvedRefs=False / BackendNotFound`.

### Child routes

- A child match whose `method.service` is **non-empty** must be string-equal
  to one of the rule's delegated services; a non-equal service is out of
  scope and the entire child rule is dropped.
- A valid child match whose `method.service` is **absent** — no `method`
  matcher at all, or a method-only matcher — inherits
  the delegated service: it flattens to `method: {type: Exact, service: S}`
  (plus the child's `method.method` if set). With multiple delegated
  services, such a match expands once per scope.
- An explicit empty `method: {}`, an empty Exact service or method, or an
  invalid Exact name invalidates the whole child rule before inheritance.
  Other valid child rules remain active. These checks use the same method
  policy as ordinary GRPCRoutes on Kubernetes, filesystem, and etcd inputs.
- A **matchless child rule** (`matches` absent or empty) flattens to **one
  inherited match per delegated scope**:
  `matches: [{method: {type: Exact, service: S}}]` (plus any merged header
  constraints) — never cloned as-is. This is security-critical: the gRPC
  gateway compiles a matchless rule into a catch-all unit, so an unmodified
  clone would match ALL traffic on the parent's hostnames.
- `type: RegularExpression` in a child match is rejected — that whole child
  rule is invalid (dropped + reported). v1 rejects the regex match outright
  rather than proving a regex service cannot escape the delegated scope.
- Rules with zero valid matches are dropped and reported.
- Children must **not** set `spec.hostnames` (hostname is owned by the root
  route / listener); violating children are skipped.
- Children may delegate further (multi-level). Cycles are rejected and the
  chain depth is capped at 8.
- A child may *reverse-lock* which parents can delegate to it by listing them
  in `parentRefs` with `kind: GRPCRoute`. Without such entries, any parent's
  delegation is accepted (delegation itself is authorized by the parent —
  the trust root is write access to the parent's namespace).

### Policy inheritance (mandatory parent filters)

- The delegating rule's standard filters (`RequestHeaderModifier`,
  `ResponseHeaderModifier`) are inherited by every flattened child rule and
  **cannot be overridden**: a child rule declaring a standard filter of an
  inherited type is dropped (reported via `PartiallyInvalid`). GRPCRoute has
  no `RequestRedirect` / `URLRewrite` filters to conflict on.
- `ExtensionRef` (EdgionPlugins) chains concatenate instead: the parent's
  plugins run first, then the child's.
- The delegating match's `headers` constraints are merged into every
  flattened child match (case-insensitive header names); a conflicting
  header (same name, different value or match type) drops the child rule.
  Constraints accumulate down a multi-level delegation chain.
- `timeouts` / `retry` / `sessionPersistence`: the child rule's own value
  wins; otherwise the nearest ancestor's value is inherited.

### Backends and ReferenceGrant

Child `backendRefs` keep child-local semantics: a ref without `namespace`
means the **child's** namespace, and cross-namespace Service references are
checked against ReferenceGrants with the child as the referrer.

### Precedence and collisions

Flattening moves the child's rules onto the parent, so on the data plane a
delegated rule is matched exactly like an ordinary rule of the **parent**
route: it uses the parent's declared hostnames for priority and the parent's
resolved hostname/Listener intersection for eligibility. All candidates follow
the unified [matching precedence](matches/overview.md#precedence); Exact/regex
buckets do not impose a separate order.

The two consequences matter only when routes collide on an identical
service/method with identical specificity — a misconfiguration in practice,
but the resolution is deterministic:

- **A delegated rule inherits the parent's identity for tie-breaking.** A
  flattened child rule's `creation_ts_ms` and `route_key` are the
  **parent's**, not the child's own. So a newly created child delegated by an
  old parent inherits that parent's (older, higher) precedence against an
  unrelated route on the same hostname.
- **Parent-own rule vs. delegated child rule on the same service/method:**
  since both entries then tie on `creation_ts`/`route_key` (both are the
  parent's), the winner is decided by `rule_index` — the entry's position in
  the parent's flattened `resolved_rules`, which preserves the order of
  `spec.rules` (a delegating rule's expansion lands where the delegating
  rule itself was). Put the parent's own rule **before** the delegating rule
  to guarantee the platform rule wins; put it after to let the delegated
  rule win. Between two children delegated by the same wildcard,
  `rule_index` is likewise the deciding factor, and wildcard expansion
  enumerates children in ascending `"namespace/name"` order — so the
  alphabetically first child wins a tie.

None of this lets a child escape its delegated service; it only decides
which of two colliding same-service/method rules is selected.

## Status

- **Parent route**: `ResolvedRefs=False/BackendNotFound` when a named child
  is missing; `PartiallyInvalid=True` listing every dropped rule
  (out-of-scope service, regex match, filter conflict, cycle, depth,
  malformed delegating rule). If no effective rule survives those invalid
  configurations, attachment is instead `Accepted=False / UnsupportedValue`.
- **Child route**: for each `parentRefs` entry with `kind: GRPCRoute`, an
  `Accepted` condition reports whether that parent currently delegates to it
  (`DelegationAccepted` / `NoMatchingParent`).

## Compatibility

Delegation activates only on `backendRef {kind: GRPCRoute}`, which was
previously an invalid configuration — existing routes behave exactly as
before. `allowedRoutes` semantics are untouched (default remains `Same`);
child routes never attach to listeners, so listener policy does not apply to
them. At runtime an unmatched gRPC request still gets the existing gateway
behavior (`UNIMPLEMENTED`, gRPC status code 12), not an HTTP 404.

## Observability

Flattened rules carry a generated rule `name`
(`<child-ns>.<child-name>.r<rule-index>`, unless the child rule already names
itself), so logs and status referencing rule names identify the owning
child.
