---
name: grpcroute-features
description: Full GRPCRoute schema: gRPC service/method matching, filters, gRPC-Web support.
---

# GRPCRoute Resource

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.5.0 Core resource

GRPCRoute defines routing rules for the gRPC protocol, supporting matching by service/method and gRPC-Web.

## Full Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: my-grpc-route
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
      sectionName: https

  hostnames:
    - "grpc.example.com"

  rules:
    - matches:
        - method:
            type: Exact                            # Exact | RegularExpression
            service: "mypackage.MyService"         # gRPC service name
            method: "GetItem"                      # gRPC method name
          headers:
            - type: Exact
              name: x-custom-header
              value: "value"

      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: x-backend-version
                value: "v2"

        - type: ResponseHeaderModifier
          responseHeaderModifier:
            add:
              - name: x-trace-id
                value: "{{generated}}"

        - type: RequestMirror
          requestMirror:
            backendRef:
              name: mirror-service
              port: 50051
            fraction:
              numerator: 5
              denominator: 100

        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: grpc-auth

      backendRefs:
        - name: grpc-service
          port: 50051
          weight: 100

      timeouts:
        request: "30s"
        backendRequest: "10s"

      retry:
        attempts: 3                                # Maximum retry attempts
        backoff: "500ms"                           # Retry backoff interval (exponential backoff base)
        codes:                                     # gRPC status codes (⚠️ not yet supported, see notes below)
          - 14                                     # UNAVAILABLE

      sessionPersistence:
        type: Cookie
        sessionName: "GRPC_SESSION"
```

## Match Condition Schema

### GRPCMethodMatch

| Field | Type | Description |
|-------|------|-------------|
| `type` | `String` | `Exact` / `RegularExpression` |
| `service` | `String?` | gRPC service fully qualified name (e.g., `mypackage.MyService`) |
| `method` | `String?` | gRPC method name |

Matching logic:
- `type: Exact` (default): match by literal value.
- `type: RegularExpression`: both service and method are matched by regex (`type` applies to both fields simultaneously).
- Both `service` + `method` specified → match a single method.
- Only `service` specified → match all methods of that service.
- Only `method` specified → match that method across all services (Edgion's implementation-specific support).
- Omit the outer method matcher → no service/method constraint. An explicit `method: {}` is invalid, not catch-all.
- Exact fields must be non-empty valid names. Service validation follows the vendored CEL Unicode simple case-fold set, while method validation remains ASCII and runtime Exact matching remains literal and case-sensitive. Regex fields may be absent/empty if the other field is non-empty; invalid patterns invalidate the entire rule, retaining valid sibling rules.
- Priority is matching hostname specificity, then descending service and method character counts (pattern lengths for regex), then header count, oldest creation time, namespace/name, and first rule. Match index breaks remaining ties. Exact is not a separate priority tier.

### GRPCHeaderMatch

Same as the headers matching of HTTPRoute.

## Filter Types

| Type | Description |
|------|-------------|
| `RequestHeaderModifier` | Modify request headers |
| `ResponseHeaderModifier` | Modify response headers |
| `RequestMirror` | Request mirroring |
| `ExtensionRef` | Reference an EdgionPlugins resource |

ExtensionRef requires exact `group: edgion.io`, `kind: EdgionPlugins`, and a
valid resource name. It is local to the declaring Route, with `default` for an
omitted Route namespace and no operator namespace selector. Controller owning
filter loans reject invalid identities before plugin dependency registration or
content reads. Valid missing bundles retain recovery edges. Under delegation
every level is compiled from its own route, so an ExtensionRef always resolves
in the declaring route's namespace; there is no namespace stamp.


**Note**: GRPCRoute does not support `RequestRedirect` or `URLRewrite`.

## gRPC-Web Support

Edgion automatically detects gRPC-Web requests (via the `content-type` header), no extra configuration required. gRPC-Web requests are routed by GRPCRoute rules.

## Retry Support

GRPCRoute supports `retry.attempts` (max retry attempts) and `retry.backoff` (exponential backoff base, capped at 30s).

**`retry.codes` limitation**: GRPCRoute's `codes` field contains gRPC status codes (0-16), not HTTP status codes. gRPC status codes live in HTTP/2 trailers (`grpc-status`), and the current Pingora proxy architecture cannot intercept the trailers stage to trigger retries. As a result, the `codes` field is parsed but **ignored at runtime**, and a warning log is emitted. Once trailers interception is implemented, this feature will be supported.

Currently, retries are triggered on connection failure (`fail_to_connect`), proxy errors (`error_while_proxy`), and per-try timeouts — with a retry policy present, an attempt exceeding `backendRequest` is canceled and retried while nothing has been sent downstream (GEP-1731, shared with HTTPRoute; see [04-httproute.md](04-httproute.md)). Caution for long-lived gRPC streams: a tight `backendRequest` combined with a retry policy cuts idle streams — size it for the longest expected quiet period.

## Route Delegation (Multi-Tenancy)

A rule delegates a **gRPC service** to child GRPCRoutes (possibly in other
namespaces) by pointing its **single** `backendRef` at
`group: gateway.networking.k8s.io, kind: GRPCRoute` (`name: "*"` = every route
in the target namespace that declares this parent). The child names the parent
in its own `parentRefs` (`kind: GRPCRoute`). It is the same cascading match as
[HTTPRoute delegation](04-httproute.md#route-delegation-multi-tenancy): the
Controller publishes each route once plus the child's
`resolved_delegation_keys`; the Gateway matches level by level through
per-target `GrpcMatchEngine` subtrees. The delegated unit is a gRPC service
name compared by string equality.

```yaml
rules:
  - matches:
      - method: { type: Exact, service: billing.v1.PaymentService }  # method must be absent
    filters:              # mandatory for all delegated traffic
      - type: RequestHeaderModifier
        requestHeaderModifier: { set: [{ name: x-tenant, value: billing }] }
    backendRefs:
      - group: gateway.networking.k8s.io
        kind: GRPCRoute
        name: "*"
        namespace: billing
```

Key semantics (canonical user doc: [`docs/en/user-guide/grpc-route/delegation.md`](../../../docs/en/user-guide/grpc-route/delegation.md); architecture: [`../../01-architecture/05-resources/05-grpc-route.md`](../../01-architecture/05-resources/05-grpc-route.md)):

- parentRefs shape, bidirectional confirmation, the 5-level cap, per-level
  precedence, backtracking with "first entry wins", inheritance and status
  reasons are identical to HTTPRoute.
- A delegating rule's `matches` must be present and non-empty, and every match
  must carry a `method` matcher with `type` Exact-or-omitted, non-empty
  `service`, and no `method` — **a matchless delegating rule is invalid**
  (delegate-all is out of scope).
- Child matches are absolute; the scope is the intersection with the
  delegated service. A child `service` that differs never matches (no error);
  a child without `service` matches the delegated one; a matchless child rule
  is a catch-all inside the subtree (safe: the root level fixed the service).
  Ordinary GRPCRoute method validation applies to child rules unchanged.
- Parent standard filters (`RequestHeaderModifier`/`ResponseHeaderModifier`)
  are mandatory; `ExtensionRef` chains run parent first.

### Failure semantics

| Condition | Surface |
|-----------|---------|
| Named delegation child does not exist | Parent `ResolvedRefs=False` / `BackendNotFound` |
| Named child exists but does not declare the parent | Parent `ResolvedRefs=False` / `RefNotPermitted` |
| Delegating rule malformed (matchless, `method.method` set, regex match, header-only match, empty service, `port`/`filters`/multiple backendRefs present) or carrying a backend-selecting plugin | Rule dropped; parent `PartiallyInvalid=True` |
| Child rule conflicts with a parent mandatory filter | Child entry `Accepted=True` + `PartiallyInvalid=True`; rule skipped under that parent only |
| Parent unreachable from a root / parent at level 5 | Child entry `Accepted=False` / `NotAttached` / `DepthExceeded` |
| Child sets `spec.hostnames` / invalid parentRefs shape | Every child entry `Accepted=False` / `UnsupportedValue` / `InvalidParentRefs`; route inert |
| Wildcard (`name: "*"`) resolves to zero children | Valid — no-op, not an error |
| Unmatched gRPC request at runtime | `UNIMPLEMENTED` (gRPC status code 12), not HTTP 404 |

Internal processing: [`01-architecture/05-resources/05-grpc-route.md`](../../01-architecture/05-resources/05-grpc-route.md).

## Differences from HTTPRoute

| Dimension | HTTPRoute | GRPCRoute |
|-----------|-----------|-----------|
| Match dimensions | path + headers + query + method | service + method + headers |
| Filters | 6 types (including Redirect, URLRewrite) | 4 types (no Redirect, URLRewrite) |
| Internal implementation | Independent matching engine | Shared route_utils, gRPC-specific MatchInfo |
| gRPC-Web | Not supported | Auto-detected |
| Delegation scope unit | Path prefix (intersection) | gRPC service name (string equality) |
| Matchless delegating rule | Delegates `"/"` (default) | Invalid (delegate-all out of scope for v1) |
