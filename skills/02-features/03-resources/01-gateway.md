---
name: gateway-resource
description: Full Gateway resource schema: Listener, protocol, TLS, AllowedRoutes, Status.
---

# Gateway Resource

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.4.0 Core resource

Gateway is Edgion's core entry resource, defining a gateway instance's Listeners (listening ports, protocols, hostnames, TLS configuration). It is the attachment point for all route resources.

## Full Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: my-gateway
  namespace: default
  annotations:
    # Edgion extension annotations
    edgion.io/enable-http2: "true"              # Listener-side HTTP/2 (default true)
    edgion.io/http-to-https-redirect: "true"    # Automatic HTTP→HTTPS redirect
    edgion.io/https-redirect-port: "443"        # HTTPS redirect port
    edgion.io/edgion-stream-plugins: "ns/name"  # Gateway-level StreamPlugins binding
spec:
  gatewayClassName: edgion                       # Required: name of the associated GatewayClass

  listeners:
    - name: http                                 # Required: unique Listener name
      hostname: "*.example.com"                  # Optional: matched hostname (wildcard supported)
      port: 80                                   # Required: listening port
      protocol: HTTP                             # Required: protocol type
      allowedRoutes:                             # Optional: route attachment rules
        namespaces:
          from: Same                             # Same | All | Selector
          # selector:                            # Required when from=Selector
          #   matchLabels: {}
        kinds:                                   # Optional: allowed route kinds
          - group: gateway.networking.k8s.io
            kind: HTTPRoute

    - name: https
      hostname: "*.example.com"
      port: 443
      protocol: HTTPS
      tls:                                       # Required for HTTPS/TLS protocols
        mode: Terminate                          # Terminate | Passthrough
        certificateRefs:                         # Certificate references when mode=Terminate
          - name: my-cert-secret                 # Secret name
            namespace: default                   # Optional: cross-namespace requires ReferenceGrant
            kind: Secret                         # Default Secret
            group: ""                            # Default core group
        frontendValidation:                      # Optional: Gateway API standard mTLS client certificate validation
          caCertificateRefs:                     # CA certificate Secret reference (must contain a ca.crt key)
            - name: client-ca
              kind: Secret
              group: ""
        options:                                 # Edgion extension options
          edgion.io/cert-provider: "edgion-tls"  # Certificate source: "secret" (default) | "edgion-tls"

    - name: tcp
      port: 9000
      protocol: TCP

    - name: tls-passthrough
      hostname: "secure.example.com"
      port: 8443
      protocol: TLS
      tls:
        mode: Passthrough                        # TLS passthrough (no termination)

    - name: udp
      port: 5353
      protocol: UDP

  addresses:                                     # Optional: addresses assigned for requests
    - type: IPAddress                            # IPAddress | Hostname
      value: "10.0.0.1"
```

## Listener Configuration Details

### Protocol Support

| Protocol | Applicable Routes | TLS Requirement | Description |
|----------|--------------------|-----------------|-------------|
| `HTTP` | HTTPRoute, GRPCRoute | — | Plain HTTP traffic |
| `HTTPS` | HTTPRoute, GRPCRoute | `tls` required | HTTP traffic after TLS termination |
| `TLS` | TLSRoute | `tls` required | TLS termination or passthrough |
| `TCP` | TCPRoute | — | Raw TCP traffic |
| `UDP` | UDPRoute | — | UDP traffic |

### TLS Configuration Schema

```yaml
tls:
  mode: String           # "Terminate" | "Passthrough"
  certificateRefs:       # Certificate list when mode=Terminate
    - name: String       # Secret name (required)
      namespace: String? # Namespace (cross-namespace requires ReferenceGrant)
      kind: String?      # Default "Secret"
      group: String?     # Default "" (core group)
  options:               # Implementation-specific options (JSON)
    edgion.io/cert-provider: String   # "secret" | "edgion-tls"
  # secrets: [...]       # Runtime-populated (injected by Controller after resolution; users do not fill this in)
```

### AllowedRoutes Schema

```yaml
allowedRoutes:
  namespaces:
    from: String         # "Same" | "All" | "Selector"
    selector:            # Required when from=Selector
      matchLabels: {}    # K8s label selector
  kinds:                 # List of route kinds allowed to attach
    - group: String?     # Default "gateway.networking.k8s.io"
      kind: String       # HTTPRoute | GRPCRoute | TCPRoute | TLSRoute | UDPRoute
```

**from values**:
| Value | Description |
|-------|-------------|
| `Same` | Only allow routes in the same namespace as the Gateway (default) |
| `All` | Allow routes from all namespaces |
| `Selector` | Filter namespaces by label selector |

## Status Schema

```yaml
status:
  addresses:                              # Assigned addresses
    - type: IPAddress
      value: "10.0.0.1"
  conditions:                             # Gateway-level conditions
    - type: Accepted                      # Resource accepted
      status: "True"
      reason: Accepted
    - type: ListenersNotValid             # True when there is a port conflict
      status: "False"
      reason: Valid
  listeners:                              # Per-Listener status
    - name: http
      supportedKinds:                     # Route kinds supported by this Listener
        - group: gateway.networking.k8s.io
          kind: HTTPRoute
      attachedRoutes: 3                   # Number of attached routes
      conditions:
        - type: Accepted
          status: "True"
        - type: Conflicted                # Port conflict
          status: "False"
        - type: ResolvedRefs              # Certificate references resolved
          status: "True"
```

## Listener Conflict Detection

When the same `(socket_addr, hostname)` combination conflicts across multiple Gateways, the Listener of the later-created Gateway is marked `Conflicted=True`. Conflict detection keys are grouped by socket address (rather than port+protocol) via `make_conflict_key(SocketAddr)`. After the conflicting Gateway is deleted, the state recovers automatically.

## Edgion Extension Annotations

| Annotation | Value | Default | Description |
|------------|-------|---------|-------------|
| `edgion.io/enable-http2` | `"true"\|"false"` | `"true"` | Listener-side HTTP/2: HTTP listener enables h2c, HTTPS listener enables ALPN h2. Does not affect the backend protocol (the backend is determined by Service `appProtocol`). |
| `edgion.io/http-to-https-redirect` | `"true"\|"false"` | `"false"` | Automatic HTTP→HTTPS redirect |
| `edgion.io/https-redirect-port` | `port` | `"443"` | HTTPS redirect target port |
| `edgion.io/edgion-stream-plugins` | `ns/name` | — | Gateway-level StreamPlugins |
