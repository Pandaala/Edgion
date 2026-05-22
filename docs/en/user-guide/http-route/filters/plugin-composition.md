# Plugin Composition and References

> **🔌 Edgion Extension**
>
> EdgionPlugins is referenced from routes via the `ExtensionRef` filter on HTTPRoute / GRPCRoute (Gateway API standard), allowing multiple routes to share the same plugin-chain configuration.

## Overview

HTTPRoute and GRPCRoute reference an `EdgionPlugins` resource through `ExtensionRef` in `rules[].filters`. When a request matches the route, the plugin chain declared in the referenced `EdgionPlugins` is executed. `HTTPBackendRef.filters` also supports `ExtensionRef`.

> **Note:** The nested form `type: ExtensionRef` inside `EdgionPlugins.spec.*Plugins` (i.e. "plugin references plugin") was removed in release `<version>`. Use the two recommended reuse patterns below instead. See `docs/en/migrations/<version>-extension-ref.md` for migration details.

## Basic Usage

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: shared-bundle
  namespace: default
spec:
  requestPlugins:
    - type: ipRestriction
      config:
        defaultAction: allow
        rules:
          - action: deny
            cidr: "10.0.0.0/8"
    - type: basicAuth
      config:
        users:
          - username: admin
            password: secret
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: shared-bundle
```

## Recommended Reuse Patterns

### 1. Aggregate Pattern (preferred)

Combine multiple plugins into a single `EdgionPlugins` resource and reference it once from the route.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: extended-security
  namespace: default
spec:
  requestPlugins:
    - type: ipRestriction
      config:
        defaultAction: allow
        rules:
          - action: deny
            cidr: "10.0.0.0/8"
    - type: basicAuth
      config:
        users:
          - username: admin
            password: secret
```

### 2. Side-by-side Pattern

List multiple `ExtensionRef` entries side-by-side in `HTTPRoute.rules[].filters`, each pointing to a different `EdgionPlugins`.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: shared-bundle-1
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: shared-bundle-2
```

### GRPCRoute

`ExtensionRef` on a GRPCRoute filter list works identically to HTTPRoute:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: my-grpc-route
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: shared-bundle
```

## Behavior

- **Per-stage fanout.** A single `ExtensionRef` declaration fans out across all three stages (request / upstream_response_filter / upstream_response_body_filter); each stage automatically pulls the corresponding subset from the target `EdgionPlugins`.
- **Dynamic resolution.** Plugin references are resolved at runtime by `namespace/name` from `PluginStore`, supporting hot updates.
- **Error handling.** When a target `EdgionPlugins` is missing, behavior follows each stage's default policy; errors are recorded in the access log.

## Design Rationale

Align with the Envoy filter chain model: a filter chain is a flat list, not a graph. Reuse is expressed as "multiple Routes referencing the same resource" (parallel to Envoy ECDS), not as "plugin references plugin".

## Best Practices

- Prefer the aggregate pattern: group semantically related plugins into one `EdgionPlugins`.
- Use the side-by-side pattern for orthogonal capabilities (e.g. auth vs. rate-limit) maintained as separate `EdgionPlugins` resources, composed at the route filter list.
- Use descriptive resource names (e.g. `shared-auth-stack`, `canary-rate-limit`) to encourage cross-team reuse.
