---
name: route-common-concepts
description: Route common concepts: parentRef attachment, backendRef backend references, cross-namespace, resolved_listeners.
---

# Route Common Concepts

## parentRef — Route Attachment

All routes attach to a Gateway Listener via `parentRefs`:

```yaml
spec:
  parentRefs:
    - name: my-gateway          # Gateway name (required)
      namespace: default        # Namespace (cross-namespace requires ReferenceGrant)
      sectionName: https        # Specify Listener name (optional)
      port: 443                 # Specify port (optional)
```

### parentRef Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `String` | Yes | Gateway resource name |
| `namespace` | `String?` | No | Gateway namespace |
| `sectionName` | `String?` | No | Listener name |
| `port` | `u16?` | No | Listening port |
| `group` | `String?` | No | Default `gateway.networking.k8s.io` |
| `kind` | `String?` | No | Default `Gateway` |

### Port Resolution Rules (resolved_listeners)

The Controller automatically computes the target port of a route based on parentRef:

1. `parentRef.port` is specified → use it directly
2. `parentRef.sectionName` is specified → look up the matching Listener to obtain the port
3. Neither specified → use the ports of all matching Listeners on the Gateway

## backendRef — Backend Reference

```yaml
rules:
  - backendRefs:
      - name: my-service        # Service name
        namespace: backend-ns   # Optional: cross-namespace requires ReferenceGrant
        port: 8080              # Target port
        weight: 80              # Weight (used for traffic splitting)
        kind: Service           # Default Service
        group: ""               # Default core group
```

### backendRef Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `String` | Yes | Backend resource name |
| `port` | `i32?` | No | Target port |
| `weight` | `i32?` | No | Traffic weight (default 1) |
| `namespace` | `String?` | No | Namespace |
| `kind` | `String?` | No | Default `Service` |
| `group` | `String?` | No | Default `""` |

## Cross-namespace References

Routes referencing resources (Gateway, Service) in another namespace require a corresponding ReferenceGrant in the target namespace:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-route-from-app
  namespace: backend-ns        # Namespace where the referenced resource lives
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: app-ns        # Source namespace of the reference
  to:
    - group: ""
      kind: Service
```

Unauthorized cross-namespace references set `ref_denied` on the backendRef; the Gateway side rejects requests accordingly (returning 500).

## hostname Intersection

For routes that support hostnames (HTTPRoute, GRPCRoute, TLSRoute), the Controller computes the intersection of the route hostnames and the Gateway Listener hostnames and stores it in `resolved_hostnames`. Only domains in the intersection are actually routed.

## Status Common Structure

Status of all route resources is grouped by parentRef:

```yaml
status:
  parents:
    - parentRef:
        name: my-gateway
        namespace: default
        sectionName: https
      controllerName: edgion.io/gateway-controller
      conditions:
        - type: Accepted         # Route accepted
          status: "True"
        - type: ResolvedRefs     # All references resolved
          status: "True"
```
