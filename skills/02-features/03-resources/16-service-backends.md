---
name: backends-features-detail
description: Full reference for backend Service/EndpointSlice discovery.
---

# Backend Features

## Service Discovery

Edgion discovers backend endpoints via the Kubernetes EndpointSlice API. The Controller continuously syncs EndpointSlice resources via K8s watch, and pushes endpoint information to the Gateway via gRPC ConfigSync.

### Service Resource

Routes reference Services via `backendRefs`:

```yaml
backendRefs:
  - name: my-service          # Service name
    port: 8080                # Service port
    weight: 100               # Traffic weight
```

Service changes (e.g., port updates) automatically trigger reprocessing of dependent routes via ServiceRefManager.

## Health Checks, Circuit Breaking, and Load Balancing

These are no longer Service annotations or route-level `ExtensionRef` filters —
they are configured via the `EdgionBackendTrafficPolicy` CRD, attached to one
or more Services via `spec.targetRefs`. See
[20-edgion-backend-traffic-policy.md](20-edgion-backend-traffic-policy.md) for
the full schema, defaults, and conflict semantics.

## BackendTLSPolicy

For gateway-to-backend TLS configuration, see [12-backend-tls-policy.md](12-backend-tls-policy.md).
