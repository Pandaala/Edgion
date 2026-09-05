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

The Gateway publishes Service and EndpointSlice input as one immutable generation. A route port
selects a canonical Service section by exact number or name; a missing port is invalid rather than
falling back to another Service port. Equivalent EndpointSlice origins for one socket are merged,
and all supported load-balancer selectors are compiled before the generation becomes visible.
Invalid or unidentified input has no legacy raw Service/EndpointSlice projection, so other gateway
readers cannot route around the unavailable state.

Source and endpoint identities are transaction-scoped. Kubernetes UID recreation, authoritative
delete/recreate, any membership-affecting Service-port change, and EndpointSlice proof changes
publish a fresh membership. Health probes and passive-outlier attempt results are accepted only
when both that membership and the captured policy generation are still current.

## Health Checks, Circuit Breaking, and Load Balancing

These are no longer Service annotations or route-level `ExtensionRef` filters —
they are configured via the `EdgionBackendTrafficPolicy` CRD, attached to one
or more Services via `spec.targetRefs`. See
[20-edgion-backend-traffic-policy.md](20-edgion-backend-traffic-policy.md) for
the full schema, defaults, and conflict semantics.

## BackendTLSPolicy

For gateway-to-backend TLS configuration, see [12-backend-tls-policy.md](12-backend-tls-policy.md).
