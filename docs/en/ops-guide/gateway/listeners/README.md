# Listeners

How Gateway listeners bind ports, choose protocols, and decide which routes may attach.

## Overview

A `Gateway` declares one or more **listeners**. Each listener binds a `port` and a
`protocol`, optionally constrains traffic by `hostname`, configures TLS, and uses
`allowedRoutes` to control which routes (and from which namespaces) may attach. A single
Gateway commonly mixes several listeners — for example an HTTP listener that redirects to an
HTTPS listener.

## Listener fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Unique listener name within the Gateway. |
| `port` | int | Yes | Port to bind. |
| `protocol` | enum | Yes | One of `HTTP`, `HTTPS`, `TLS`, `TCP`, `UDP`. |
| `hostname` | string | No | SNI / Host header match. Supports wildcard `*.example.com`. |
| `tls` | object | Conditional | Required for `HTTPS` and `TLS`. Holds `mode` and `certificateRefs`. |
| `allowedRoutes` | object | No | Which route kinds and namespaces may attach. |

### TLS block

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `tls.mode` | enum | Yes (for `HTTPS`/`TLS`) | `Terminate` (decrypt at the gateway) or `Passthrough` (forward encrypted). |
| `tls.certificateRefs` | list | Required for `Terminate` | Secret references holding the certificate and key. |
| `tls.options` | map | No | Edgion TLS extensions via `edgion.io/*` keys. |

### AllowedRoutes block

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `allowedRoutes.namespaces.from` | enum | No | `Same` (default), `All`, or `Selector`. |
| `allowedRoutes.namespaces.selector` | object | Conditional | Label selector, used when `from: Selector`. |
| `allowedRoutes.kinds` | list | No | Route kinds allowed (e.g. `HTTPRoute`, `GRPCRoute`, `TLSRoute`, `TCPRoute`). |

## Protocol → route kind mapping

| Protocol | Default route kind | TLS mode |
|----------|--------------------|----------|
| `HTTP` | `HTTPRoute` (also `GRPCRoute` for h2c) | — |
| `HTTPS` | `HTTPRoute` (also `GRPCRoute`) | `Terminate` |
| `TLS` | `TLSRoute` | `Passthrough` (or `Terminate`) |
| `TCP` | `TCPRoute` | — |
| `UDP` | `UDPRoute` | — |

gRPC does not have its own listener protocol. Attach a `GRPCRoute` to an `HTTPS` listener
(h2 over TLS) or an `HTTP` listener (h2c, cleartext), with `GRPCRoute` in
`allowedRoutes.kinds`. See [grpc.md](grpc.md).

## Per-protocol pages

- [HTTP listener](http.md)
- [HTTPS listener](https.md)
- [gRPC listener](grpc.md)
- [TCP listener](tcp.md)

## Edge cases

- **Multiple listeners**: a Gateway can expose several listeners at once; each `name` must be
  unique. Two listeners may share a `port` only if they are distinguished by `hostname` and
  use compatible protocols (for example, multiple `HTTPS` listeners with different SNI).
- **Hostname selection**: a request is matched to the most specific listener `hostname`. A
  listener with no `hostname` matches any host; a wildcard `*.example.com` matches one label.
- **Cross-namespace AllowedRoutes**: by default (`from: Same`) only routes in the Gateway's
  own namespace may attach. Use `from: All` or `from: Selector` to allow routes from other
  namespaces.
- **TLS Passthrough**: for end-to-end encryption to the backend, use protocol `TLS` with
  `mode: Passthrough` and attach a `TLSRoute`. See
  [../../../user-guide/tls-route/overview.md](../../../user-guide/tls-route/overview.md).

## Related documents

- [Gateway overview](../overview.md)
- [GatewayClass](../gateway-class.md)
- [TLS routing (Passthrough)](../../../user-guide/tls-route/overview.md)
