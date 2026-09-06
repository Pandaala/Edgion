# TLSRoute Backends

Backends are the destinations a TLSRoute forwards matched TLS connections to. Because TLSRoute operates in passthrough mode, the gateway proxies the original encrypted stream to the backend without decrypting it.

## Overview

Each rule in a TLSRoute carries a `backendRefs` list. A backend reference points at a Kubernetes Service and a port. The gateway opens a connection to the selected backend and pipes the encrypted bytes through; the backend is responsible for terminating TLS. When a rule lists more than one backend, connections are distributed across them by `weight`.

## Basic configuration

A single backend receiving all matched connections:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
  rules:
    - backendRefs:
        - name: api-service
          port: 443
```

## Backend reference parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | — (required) | Name of the target Service |
| `port` | Integer | none | Target port on the Service; required when the Service exposes more than one port |
| `weight` | Integer | `1` | Relative share of connections sent to this backend when several are listed |
| `namespace` | String | current namespace | Namespace of the target Service; cross-namespace references require a ReferenceGrant |
| `kind` | String | `Service` | Kind of the referenced object |
| `group` | String | `""` (empty) | API group of the referenced object; `""` denotes the core Kubernetes API group |

## Common configuration scenarios

### 1. Single backend

The simplest case — every connection matching the route's SNI goes to one Service.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
  rules:
    - backendRefs:
        - name: api-service
          port: 443
```

### 2. Weighted backends (traffic splitting)

List multiple backends with `weight` to split traffic, for example during a canary rollout. The share each backend receives is its weight divided by the sum of all weights.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
  rules:
    - backendRefs:
        # 80% to the stable backend
        - name: api-service
          port: 443
          weight: 80
        # 20% to the canary backend
        - name: api-service-canary
          port: 443
          weight: 20
```

A backend with `weight: 0` receives no connections while remaining declared, which is useful for staging a backend before shifting traffic to it.

A denied cross-namespace backendRef and a non-Service `kind` keep their declared weight. Those selections fail closed (`RefDenied` / `UnsupportedBackend`) and do not retry onto another backend.

### 3. Cross-namespace backend (ReferenceGrant)

To forward to a Service in a different namespace, set `namespace` on the backend reference and create a ReferenceGrant in the target namespace that permits TLSRoute references from the route's namespace. Without a matching grant, the Controller marks the backendRef denied and the Gateway fails the connection closed without looking up or dialing the Service.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
  rules:
    - backendRefs:
        - name: api-service
          namespace: staging
          port: 443
---
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-production-tlsroutes
  namespace: staging
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: TLSRoute
      namespace: production
  to:
    - group: ""
      kind: Service
```

## Notes

- **Passthrough only.** The gateway does not decrypt the stream, so backend TLS verification (BackendTLSPolicy) does not apply to TLSRoute. If you need the gateway to validate or re-encrypt the connection to the backend, terminate TLS with an HTTPRoute or GRPCRoute instead.
- **Backend terminates TLS.** Because the original encrypted bytes are forwarded unchanged, the backend Service must present a certificate valid for the SNI the client sent.
- **Port is required for multi-port Services.** When the target Service exposes a single port you may omit `port`; otherwise specify it explicitly.

## Related documents

- [TLSRoute Overview](../overview.md)
- [TLS Termination](../../../ops-guide/gateway/tls/tls-termination.md)
- [TCPRoute Backends](../../tcp-route/backends/overview.md)
