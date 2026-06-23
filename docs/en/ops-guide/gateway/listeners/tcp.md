# TCP Listener

A Layer 4 listener that forwards raw TCP connections and attaches `TCPRoute`.

## Overview

A TCP listener binds a port with `protocol: TCP` and forwards the connection to a backend
without inspecting application-layer data. Use it for non-HTTP protocols — databases, message
brokers, or any custom TCP service. Because TCP routing is L4, there is no `hostname` matching
and no TLS termination at the listener.

## Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: tcp-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: postgres
      port: 5432
      protocol: TCP
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: TCPRoute
```

## Field reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Unique listener name within the Gateway. |
| `port` | int | Yes | Port to bind. |
| `protocol` | enum | Yes | Must be `TCP`. |
| `hostname` | string | No | Not used for TCP; L4 has no host concept and the value is ignored. |
| `allowedRoutes.namespaces.from` | enum | No | `Same` (default), `All`, or `Selector`. |
| `allowedRoutes.kinds` | list | No | `TCPRoute` (default for `TCP`). |

A TCP listener has no `tls` block; TLS is not terminated at the listener. For encrypted
forwarding, use a `TLS` listener in `Passthrough` mode (see below).

## Scenarios

### Forward a database port

Expose a PostgreSQL backend through the gateway.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: db-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: postgres
      port: 5432
      protocol: TCP
      allowedRoutes:
        namespaces:
          from: Same
```

### Multiple TCP services on one Gateway

Bind several ports, one listener per service.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: l4-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: redis
      port: 6379
      protocol: TCP
      allowedRoutes:
        namespaces:
          from: Same
    - name: mysql
      port: 3306
      protocol: TCP
      allowedRoutes:
        namespaces:
          from: Same
```

### Accept TCP routes from another namespace

Let a `TCPRoute` owned by an app team attach to a shared L4 listener.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: shared-tcp-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: broker
      port: 9092
      protocol: TCP
      allowedRoutes:
        namespaces:
          from: Selector
          selector:
            matchLabels:
              edgion.io/tcp-tenant: "true"
        kinds:
          - kind: TCPRoute
```

## Edge cases and caveats

- **No hostname routing**: TCP is L4, so `hostname` does not apply. A `TCPRoute` selects its
  backend, not a host. To route by SNI, use a `TLS` listener instead.
- **No TLS termination**: a TCP listener does not terminate TLS. For encrypted backends, use
  protocol `TLS` with `tls.mode: Passthrough` and a `TLSRoute` — see
  [../../../user-guide/tls-route/overview.md](../../../user-guide/tls-route/overview.md).
- **One backend per port**: because there is no L7 matching, typically one `TCPRoute` (one
  backend) attaches per TCP listener; run multiple services on distinct ports.
- **Cross-namespace AllowedRoutes**: default `from: Same`; widen with `from: All` or
  `from: Selector`.

## Related documents

- [Listeners overview](README.md)
- [HTTPS listener](https.md)
- [TLS routing (Passthrough)](../../../user-guide/tls-route/overview.md)
