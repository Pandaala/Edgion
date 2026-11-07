# UDPRoute Overview

UDPRoute is used for UDP traffic forwarding, suitable for scenarios such as DNS, log collection, and game communication.

Edgion accepts UDPRoute v1 and forwards traffic on listeners bound when the
Gateway process starts. Adding a new listener port later requires a Gateway
process restart. The Gateway API v1.6.2 `GATEWAY-UDP` profile creates ports
dynamically, so Edgion does not advertise `UDPRoute` in
`GatewayClass.status.supportedFeatures` yet.

## Basic Structure

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: UDPRoute
metadata:
  name: udp-route
  namespace: production
spec:
  parentRefs:             # Which Gateway to bind to
    - name: main-gateway
      sectionName: udp
  rules:                  # List of routing rules
    - backendRefs:        # Backend services
        - name: my-udp-service
          port: 53
```

Key fields:

| Field | Description |
|-------|-------------|
| `parentRefs` | Binds the route to one or more Gateway UDP listeners |
| `rules` | List of routing rules, each containing backendRefs |

> UDPRoute operates at Layer 4 and does not support Matches or Filters. Routing decisions rely solely on listener bindings.

## Core Concepts

### ParentRefs

Specifies which UDP listener of which Gateway this route binds to:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Gateway name |
| `namespace` | String | No | Gateway namespace (defaults to the route's namespace) |
| `sectionName` | String | No | Listener name |

Each UDP listener typically binds to only one UDPRoute, since there are no Layer 7 match conditions to differentiate traffic.

### Rules

UDPRoute Rules contain only backendRefs, with no matches or filters fields. Each Rule forwards traffic directly to the specified backends.

### BackendRefs

Defines the list of backend services for UDP traffic forwarding:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Service name |
| `port` | Integer | Yes | Port number |
| `weight` | Integer | No | Weight (default `1`), used for weighted traffic splitting |

## Full Example

The following example demonstrates a complete Gateway + UDPRoute configuration, forwarding DNS and syslog traffic through the gateway:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edge-gw
  namespace: gateway-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: udp-dns
      protocol: UDP
      port: 53
    - name: udp-syslog
      protocol: UDP
      port: 514
---
# DNS route
apiVersion: gateway.networking.k8s.io/v1
kind: UDPRoute
metadata:
  name: dns-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: udp-dns
  rules:
    - backendRefs:
        - name: dns-service
          port: 53
---
# Syslog route
apiVersion: gateway.networking.k8s.io/v1
kind: UDPRoute
metadata:
  name: syslog-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: udp-syslog
  rules:
    - backendRefs:
        - name: syslog-service
          port: 514
```

## Notes

1. UDP is connectionless; retry and session semantics differ from TCP/HTTP.
2. Reliability compensation should be handled at the backend application layer.
3. Once a client session is established, its upstream is pinned for the session's
   lifetime: route or backend changes apply only to new sessions. Existing sessions
   keep forwarding to their original upstream until they expire after 60 s of
   inactivity, at which point the next packet re-runs route matching.
   Exception: when service discovery no longer publishes the pinned endpoint (the
   endpoint address, the Service port, or the Service itself was removed), the
   session is retired within about 10 s even if the client is still sending, and
   its access-log entry carries `status: UpstreamEndpointRemoved`. The next packet
   creates a new session against a current endpoint. Route, ReferenceGrant and
   stream-plugin changes still apply only to new sessions.
4. During Gateway shutdown, UDP continues serving through Pingora's shutdown phase.
   Once Pingora stops, UDP stops receiving and forwarding, then cleans up tasks and
   sessions within the shared five-second auxiliary-service grace window. A timeout
   forces task cancellation and a non-zero exit. Cancellation acknowledgement still
   depends on runtime scheduling, so five seconds is not a hard process-exit limit.
   Shutdown can discard in-flight datagrams; applications must handle retries.

## Sub-document Navigation

| Topic | Document | Description |
|-------|----------|-------------|
| Backend Configuration | [backends/](./backends/overview.md) | Backend service references and weight configuration |
| Stream Plugins | [stream-plugins.md](../tcp-route/stream-plugins.md) | Edgion extension plugins for TCP/UDP traffic |
