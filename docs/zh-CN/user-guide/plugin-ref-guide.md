# EdgionPlugins Reference Guide

## Overview

EdgionPlugins supports referencing other EdgionPlugins via `ExtensionRef`, enabling plugin composition and reuse.

## Basic Usage

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: base-security
  namespace: default
spec:
  requestPlugins:
    - type: ipRestriction
      config:
        defaultAction: allow
        rules:
          - action: deny
            cidr: "10.0.0.0/8"
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: extended-security
  namespace: default
spec:
  requestPlugins:
    - type: extensionRef
      config:
        group: "edgion.io"
        kind: EdgionPlugins
        name: base-security
    - type: basicAuth
      config:
        users:
          - username: admin
            password: secret
```

## Reference Depth Limit

To prevent infinite loops, plugin references are limited to a maximum depth (default: 5).

### Configure in HTTPRoute

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: "edgion.io"
            kind: EdgionPlugins
            name: my-plugins
          extensionRefMaxDepth: 10  # Override default depth limit
```

### Configure in GRPCRoute

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: my-grpc-route
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: "edgion.io"
            kind: EdgionPlugins
            name: my-plugins
          extensionRefMaxDepth: 8  # Override default depth limit
```

## Behavior

- **Cycle Detection**: Circular references are automatically detected and blocked
- **Depth Limit**: Reference chains exceeding the configured depth return 500
- **Dynamic Resolution**: Plugin references are resolved at runtime, supporting hot updates
- **Per-Stage Isolation**: Depth limits apply independently to request/response stages
- **Error Handling**: Exceeded depth or missing plugins terminate the request with appropriate logging

## Best Practices

- Keep reference chains shallow (≤3 levels recommended)
- Use descriptive names for plugin compositions
- Monitor access logs for depth-exceeded errors
- Test complex reference chains in staging environments

