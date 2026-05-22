---
name: grpcroute-features
description: Full GRPCRoute schema: gRPC service/method matching, filters, gRPC-Web support.
---

# GRPCRoute Resource

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.4.0 Core resource

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
- Neither specified → match all gRPC requests.
- Exact match takes precedence over regex match; among regex entries, priority is by pattern length in descending order.

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

Currently, retries are only triggered on connection failure (`fail_to_connect`) and proxy errors (`error_while_proxy`), consistent with HTTPRoute's connection-level retry behavior.

## Differences from HTTPRoute

| Dimension | HTTPRoute | GRPCRoute |
|-----------|-----------|-----------|
| Match dimensions | path + headers + query + method | service + method + headers |
| Filters | 6 types (including Redirect, URLRewrite) | 4 types (no Redirect, URLRewrite) |
| Internal implementation | Independent matching engine | Shared route_utils, gRPC-specific MatchInfo |
| gRPC-Web | Not supported | Auto-detected |
