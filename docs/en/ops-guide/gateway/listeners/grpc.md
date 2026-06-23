# gRPC Listener

How to expose gRPC (HTTP/2) services through a Gateway listener.

## Overview

gRPC runs over HTTP/2, so it does **not** have its own listener protocol in the Gateway API.
You serve gRPC by attaching a `GRPCRoute` to an existing listener:

- **`HTTPS`** — h2 over TLS. The standard, secure choice.
- **`HTTP`** — h2c (cleartext HTTP/2). Use only on trusted networks or behind upstream TLS
  termination.

In both cases the listener `protocol` stays `HTTP` / `HTTPS`; you opt the listener into gRPC
by listing `GRPCRoute` in `allowedRoutes.kinds`.

## Configuration

A TLS-terminating listener that accepts gRPC routes:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: grpc-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: grpc-tls
      port: 443
      protocol: HTTPS
      hostname: grpc.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: grpc-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: GRPCRoute
```

## Field reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `protocol` | enum | Yes | `HTTPS` (h2 over TLS) or `HTTP` (h2c). There is no `GRPC` protocol value. |
| `port` | int | Yes | Port to bind (commonly `443` for h2, or any port for h2c). |
| `hostname` | string | No | SNI / authority match. Supports wildcard `*.example.com`. |
| `tls.mode` | enum | Yes for `HTTPS` | `Terminate`. |
| `tls.certificateRefs` | list | Yes for `Terminate` | Secret references with the certificate and key. |
| `allowedRoutes.kinds` | list | Yes | Must include `GRPCRoute` for the listener to accept gRPC routes. |

## Scenarios

### gRPC over TLS (h2)

The recommended setup: terminate TLS and route gRPC.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: grpc-secure-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: grpc-tls
      port: 443
      protocol: HTTPS
      hostname: grpc.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: grpc-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: GRPCRoute
```

### gRPC over cleartext (h2c)

For internal traffic where TLS is handled elsewhere, use an `HTTP` listener with `GRPCRoute`.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: grpc-internal-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: grpc-h2c
      port: 8080
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: GRPCRoute
```

### Share a listener between HTTP and gRPC routes

A single HTTPS listener can accept both `HTTPRoute` and `GRPCRoute` by listing both kinds.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: mixed-h2-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
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
        kinds:
          - kind: HTTPRoute
          - kind: GRPCRoute
```

## Edge cases and caveats

- There is no `GRPC` listener protocol; using one is invalid. Always use `HTTP` or `HTTPS`
  and select gRPC via `allowedRoutes.kinds`.
- **h2c is cleartext**: do not expose an h2c listener on an untrusted network.
- **GRPCRoute must be allowed**: if `allowedRoutes.kinds` omits `GRPCRoute`, gRPC routes will
  not attach even though the port is open.
- **Hostname / SNI selection**: as with HTTPS, multiple listeners can share a port,
  distinguished by `hostname`.
- **Cross-namespace AllowedRoutes**: default `from: Same`; widen with `from: All` or
  `from: Selector`.

## Related documents

- [Listeners overview](README.md)
- [HTTPS listener](https.md)
- [HTTP listener](http.md)
- [GRPCRoute overview](../../../user-guide/grpc-route/overview.md)
