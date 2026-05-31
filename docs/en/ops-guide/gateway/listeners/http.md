# HTTP Listener

A plaintext listener that accepts HTTP/1.1 and h2c traffic and attaches `HTTPRoute` (or
`GRPCRoute` for h2c).

## Overview

An HTTP listener binds a port with `protocol: HTTP` and serves cleartext traffic. It is the
right choice for internal traffic, for a port that redirects to HTTPS, or for h2c gRPC where
TLS is terminated upstream. No `tls` block is used.

## Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: web-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      hostname: app.example.com
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: HTTPRoute
```

## Field reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Unique listener name within the Gateway. |
| `port` | int | Yes | Port to bind (commonly `80`). |
| `protocol` | enum | Yes | Must be `HTTP`. |
| `hostname` | string | No | Host header match. Supports wildcard `*.example.com`. Omit to match any host. |
| `allowedRoutes.namespaces.from` | enum | No | `Same` (default), `All`, or `Selector`. |
| `allowedRoutes.kinds` | list | No | Route kinds allowed; `HTTPRoute` (default) or `GRPCRoute` for h2c. |

An HTTP listener has no `tls` block. TLS mode does not apply.

## Scenarios

### Basic HTTP service

A single plaintext listener for an internal service.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: internal-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
```

### Redirect HTTP to HTTPS

Pair an HTTP listener with the redirect annotation so port-80 clients are upgraded.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: site-gateway
  namespace: edgion-system
  annotations:
    edgion.io/http-to-https-redirect: "true"
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      hostname: app.example.com
      allowedRoutes:
        namespaces:
          from: Same
    - name: https
      port: 443
      protocol: HTTPS
      hostname: app.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: app-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
```

See [../http-to-https-redirect.md](../http-to-https-redirect.md).

### Accept routes from other namespaces

Let `HTTPRoute`s in any namespace attach to a shared ingress listener.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: shared-ingress
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: All
        kinds:
          - kind: HTTPRoute
```

## Edge cases and caveats

- **Multiple listeners**: an HTTP listener can coexist with HTTPS/TLS/TCP/UDP listeners on
  the same Gateway; each listener `name` must be unique.
- **Hostname selection**: a request is matched to the most specific `hostname`. A listener
  with no `hostname` accepts any Host; `*.example.com` matches a single label.
- **Cross-namespace AllowedRoutes**: the default `from: Same` restricts attachment to the
  Gateway's namespace; use `from: All` or `from: Selector` to widen it.
- **h2c gRPC**: to serve cleartext gRPC, set `allowedRoutes.kinds` to include `GRPCRoute`.
  See [grpc.md](grpc.md).
- Plaintext HTTP exposes traffic on the wire; for public endpoints prefer HTTPS, optionally
  with the redirect annotation on the HTTP listener.

## Related documents

- [Listeners overview](README.md)
- [HTTPS listener](https.md)
- [gRPC listener](grpc.md)
- [HTTP to HTTPS redirect](../http-to-https-redirect.md)
