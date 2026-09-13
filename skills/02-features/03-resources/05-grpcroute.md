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
- Exact fields must be non-empty valid names. Regex fields may be absent/empty if the other field is non-empty; invalid patterns invalidate the entire rule, retaining valid sibling rules.
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
in the target namespace). The controller flattens the tree at parse time into
the controller-resolved `resolved_rules` field; the data plane never sees the
nesting. Unlike HTTPRoute's path-prefix scope, the delegated unit here is a
gRPC service name compared by spec-string equality (no canonicalization gap).

```yaml
rules:
  - matches:
      - method: { type: Exact, service: billing.v1.PaymentService }  # method must be absent
    filters:              # mandatory, inherited by all child rules
      - type: RequestHeaderModifier
        requestHeaderModifier: { set: [{ name: x-tenant, value: billing }] }
    backendRefs:
      - group: gateway.networking.k8s.io
        kind: GRPCRoute
        name: "*"
        namespace: billing
```

Key semantics (canonical user doc: [`docs/en/user-guide/grpc-route/delegation.md`](../../../docs/en/user-guide/grpc-route/delegation.md); architecture: [`../../01-architecture/05-resources/05-grpc-route.md`](../../01-architecture/05-resources/05-grpc-route.md)):

- A delegating rule's `matches` must be present and non-empty, and every match
  must carry a `method` matcher with `type` Exact-or-omitted, non-empty
  `service`, and no `method` — **a matchless delegating rule is invalid**
  (deliberately does NOT mirror HTTPRoute's `"/"` default; delegate-all is out
  of scope for v1).
- A child match with a non-empty, non-matching `method.service` is out of
  scope (dropped); one with a missing outer matcher or valid method-only
  matcher inherits the delegated service. Explicit empty matchers and empty
  Exact fields invalidate the child rule. A matchless
  child rule flattens to **one inherited match per delegated scope**, never
  cloned as-is (a raw clone would compile to a gateway catch-all).
- `type: RegularExpression` in a child match invalidates that child rule
  (v1 rejects the regex match outright rather than proving containment).
- Children must not set `hostnames`; children may delegate further (cycle
  detection, depth cap 8); children may reverse-lock parents via
  `parentRefs {kind: GRPCRoute}`.
- Parent standard filters (`RequestHeaderModifier`/`ResponseHeaderModifier`)
  are mandatory (same-type child filter ⇒ rule dropped); `ExtensionRef` chains
  concatenate (parent first). `timeouts` / `retry` / `sessionPersistence`:
  child-first-else-inherit.
- Child backendRef namespace defaults to the child's namespace; cross-ns
  Service refs are ReferenceGrant-checked with the child as referrer.
- Additive: `backendRef {kind: GRPCRoute}` was previously invalid config;
  `allowedRoutes` semantics are untouched.

### Failure semantics

| Condition | Surface |
|-----------|---------|
| Named delegation child does not exist | `ResolvedRefs=False` / `BackendNotFound` |
| Delegating rule malformed (matchless, `method.method` set, regex match, header-only match, empty service, `port`/`filters`/multiple backendRefs present) | Rule dropped; `PartiallyInvalid=True` |
| Child rule has a header conflict, standard-filter conflict, all matches out of scope, or a `RegularExpression` match | Rule dropped; `PartiallyInvalid=True` |
| Delegation cycle / depth > 8 | Rule dropped; `PartiallyInvalid=True` (`DelegationLoop` / `DelegationDepthExceeded`) |
| Child sets `spec.hostnames` | Child rules from that route skipped |
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
| Delegation scope unit | Path prefix (containment) | gRPC service name (spec-string equality) |
| Matchless delegating rule | Delegates `"/"` (default) | Invalid (delegate-all out of scope for v1) |
