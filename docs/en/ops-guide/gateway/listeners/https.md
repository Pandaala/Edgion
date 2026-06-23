# HTTPS Listener

A TLS-terminating listener that decrypts traffic at the gateway and attaches `HTTPRoute` (or
`GRPCRoute`).

## Overview

An HTTPS listener binds a port with `protocol: HTTPS` and a `tls` block in `Terminate` mode.
The gateway presents a certificate, decrypts the connection, and routes the plaintext request
to a backend. This is the standard way to serve public HTTPS and h2 gRPC over TLS.

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
        kinds:
          - kind: HTTPRoute
```

## Field reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Unique listener name within the Gateway. |
| `port` | int | Yes | Port to bind (commonly `443`). |
| `protocol` | enum | Yes | Must be `HTTPS`. |
| `hostname` | string | No | SNI / Host match. Supports wildcard `*.example.com`. |
| `tls.mode` | enum | Yes | Must be `Terminate` for HTTPS. |
| `tls.certificateRefs` | list | Yes | Secret references holding the certificate and key. |
| `allowedRoutes.namespaces.from` | enum | No | `Same` (default), `All`, or `Selector`. |
| `allowedRoutes.kinds` | list | No | `HTTPRoute` (default) or `GRPCRoute`. |

## Scenarios

### Terminate TLS for one host

A single HTTPS listener with one certificate.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: app-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
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

### Serve multiple hosts on port 443 (SNI)

Two HTTPS listeners share port `443`, distinguished by `hostname` and each with its own
certificate.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: multi-host-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: https-app
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
    - name: https-api
      port: 443
      protocol: HTTPS
      hostname: api.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: api-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
```

### Wildcard certificate for a subdomain

Serve any single-label subdomain with one wildcard certificate.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: wildcard-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: https-wildcard
      port: 443
      protocol: HTTPS
      hostname: "*.example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: wildcard-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
```

## Edge cases and caveats

- **Terminate only**: HTTPS listeners use `tls.mode: Terminate`. For end-to-end encryption
  without decrypting at the gateway, use a `TLS` listener with `Passthrough` and a `TLSRoute`
  — see [../../../user-guide/tls-route/overview.md](../../../user-guide/tls-route/overview.md).
- **certificateRefs required**: `Terminate` mode needs at least one certificate Secret;
  without it the listener cannot complete the handshake.
- **Hostname / SNI selection**: when multiple HTTPS listeners share a port, the client's SNI
  picks the listener; the most specific `hostname` wins, and a wildcard matches one label.
- **Cross-namespace certificate refs**: a `certificateRef` in another namespace requires a
  `ReferenceGrant` in the Secret's namespace.
- **Cross-namespace AllowedRoutes**: default `from: Same`; widen with `from: All` or
  `from: Selector`.
- **gRPC over TLS**: include `GRPCRoute` in `allowedRoutes.kinds` to serve h2 gRPC. See
  [grpc.md](grpc.md).

## Related documents

- [Listeners overview](README.md)
- [HTTP listener](http.md)
- [gRPC listener](grpc.md)
- [HTTP to HTTPS redirect](../http-to-https-redirect.md)
- [TLS routing (Passthrough)](../../../user-guide/tls-route/overview.md)
