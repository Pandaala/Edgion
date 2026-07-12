# Route Delegation (Multi-Tenancy)

Route delegation lets a platform team hand ownership of a path prefix to an
application team's namespace, without giving that team any access to the
Gateway or the root route. A parent HTTPRoute rule points its `backendRef` at
child HTTPRoutes instead of a Service; the controller expands the tree into a
flat route table, so the data plane never sees the nesting.

## Why

In a multi-tenant cluster, only the platform namespace can attach routes to
the shared Gateway (`allowedRoutes` defaults to `Same`). Without delegation,
every application routing change has to go through the platform team's root
route. With delegation, the platform team writes one delegating rule per
tenant prefix, and each tenant manages child routes freely inside their own
namespace — RBAC is the security boundary.

## Quick start

Parent (root) route, owned by the platform team in the Gateway's namespace:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: root
  namespace: infra
spec:
  parentRefs:
    - name: shared-gateway
  hostnames: ["api.example.com"]
  rules:
    - matches:
        - path: { type: PathPrefix, value: /api/v1/billing }
      filters:                       # mandatory for all delegated traffic
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set: [{ name: x-tenant, value: billing }]
      backendRefs:
        - group: gateway.networking.k8s.io
          kind: HTTPRoute
          name: "*"                  # every HTTPRoute in that namespace
          namespace: billing
```

Child route, owned by the billing team in their namespace:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: billing-routes
  namespace: billing
spec:
  # No parentRefs needed: reachable only through the delegation.
  rules:
    - matches:
        - path: { type: PathPrefix, value: /api/v1/billing/pay }   # absolute path
      backendRefs:
        - name: payment-svc
          port: 8080
```

## Semantics

### Delegating rule

A rule delegates when its single `backendRef` has
`group: gateway.networking.k8s.io, kind: HTTPRoute`. Constraints:

- Exactly one backendRef (no mixing with Service refs).
- `port` and per-backendRef `filters` must be absent; `weight` is ignored.
- Every match in the rule must be `PathPrefix`.
- `name: "*"` delegates to every HTTPRoute in the target namespace;
  `namespace` defaults to the parent's namespace.

A malformed delegating rule is dropped and reported via the route's
`PartiallyInvalid` condition; a missing named child is reported via
`ResolvedRefs=False / BackendNotFound`.

### Child routes

- Child paths are **absolute** and must fall inside the delegated prefix at a
  segment boundary (`/api/v1/billing` admits `/api/v1/billing/pay`, not
  `/api/v1/billingX`). Out-of-prefix rules are dropped and reported on the
  parent's status.
- Paths are normalized (dot-segments, percent-encoding) before the containment
  check, so `/api/../admin` cannot escape a `/api` delegation.
- `Exact` and `RegularExpression` matches are allowed if they stay inside the
  prefix. A regex must start with `^` followed by the literal prefix and must
  not use alternation (`|`) — the data plane matches regexes unanchored, so
  unanchored patterns and alternate branches are rejected.
- A child match without a path inherits the delegated prefix.
- Children must **not** set `spec.hostnames` (hostname is owned by the root
  route / listener); violating children are skipped.
- Children may delegate further (multi-level). Cycles are rejected and the
  chain depth is capped at 8.
- A child may *reverse-lock* which parents can delegate to it by listing them
  in `parentRefs` with `kind: HTTPRoute`. Without such entries, any parent's
  delegation is accepted (delegation itself is authorized by the parent —
  the trust root is write access to the parent's namespace).

### Policy inheritance (mandatory parent filters)

- The delegating rule's standard filters (RequestHeaderModifier, CORS,
  URLRewrite, ...) are inherited by every flattened child rule and **cannot be
  overridden**: a child rule declaring a standard filter of an inherited type
  is dropped (reported via `PartiallyInvalid`).
- `ExtensionRef` (EdgionPlugins) chains concatenate instead: the parent's
  plugins run first, then the child's.
- The delegating rule's `headers` / `queryParams` / `method` match constraints
  are merged into child matches; a contradiction drops the child rule.
- `timeouts` / `retry` / `sessionPersistence`: the child rule's own value
  wins; otherwise the nearest ancestor's value is inherited.

### Backends and ReferenceGrant

Child `backendRefs` keep child-local semantics: a ref without `namespace`
means the **child's** namespace, and cross-namespace Service references are
checked against ReferenceGrants with the child as the referrer.

### Precedence and collisions

Flattening moves the child's rules onto the parent, so on the data plane a
delegated rule is matched exactly like an ordinary rule of the **parent**
route. The two consequences matter only when routes collide on an identical
path with identical match specificity (method / header / query counts) — a
misconfiguration in practice, but the resolution is deterministic:

- **A delegated rule inherits the parent's identity for tie-breaking.** Gateway
  API resolves same-specificity ties by the route's `creationTimestamp` (oldest
  wins) and then `namespace/name`. A flattened child rule uses the **parent's**
  timestamp and key, not the child's own. So a newly created child delegated by
  an old parent inherits that parent's (older, higher) precedence against an
  unrelated route on the same hostname.
- **Parent-own rule vs. delegated child rule on the same path:** the winner is
  decided by their order in the parent's `spec.rules`. Put the parent's own
  rule **before** the delegating rule to guarantee the platform rule wins;
  put it after to let the delegated rule win. Between two children delegated by
  the same wildcard, the tie-break is the child key in ascending order.

None of this lets a child escape its delegated prefix; it only decides which
of two colliding same-path rules is selected.

## Status

- **Parent route**: `ResolvedRefs=False/BackendNotFound` when a named child is
  missing; `PartiallyInvalid=True` listing every dropped rule (out-of-prefix,
  filter conflict, cycle, depth, malformed delegating rule).
- **Child route**: for each `parentRefs` entry with `kind: HTTPRoute`, an
  `Accepted` condition reports whether that parent currently delegates to it
  (`DelegationAccepted` / `NoMatchingParent`).

## Compatibility

Delegation activates only on `backendRef {kind: HTTPRoute}`, which was
previously an invalid configuration — existing routes behave exactly as
before. `allowedRoutes` semantics are untouched (default remains `Same`);
child routes never attach to listeners, so listener policy does not apply to
them.

## Observability

Flattened rules carry a generated rule `name`
(`<child-ns>.<child-name>.r<rule-index>`, unless the child rule already names
itself), so logs and status referencing rule names identify the owning child.
