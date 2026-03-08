# DSL Plugin

> **🔌 Edgion Extension**
> 
> DSL is a custom scripting plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

The DSL plugin allows custom request processing logic through inline EdgionDSL scripts. Scripts execute in a secure sandbox VM with configurable resource limits. Suitable for scenarios requiring flexible customization without developing standalone plugins.

**Features**:
- Sandboxed execution with configurable resource limits
- Supports both source code and pre-compiled bytecode
- Configurable error policies

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dsl-plugin
spec:
  requestPlugins:
    - enable: true
      type: Dsl
      config:
        name: "header-check"
        source: |
          let token = req.header("X-Api-Token")
          if token == nil {
            return deny(403, "missing X-Api-Token header")
          }
```

---

## Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `name` | String | Yes | none | Script name (used for logging and debugging) |
| `source` | String | Yes* | none | DSL source code |
| `bytecode` | String | Yes* | none | Pre-compiled bytecode (Base64) |
| `maxSteps` | Integer | No | `10000` | Maximum execution steps |
| `maxLoopIterations` | Integer | No | `100` | Maximum loop iterations |
| `maxCallCount` | Integer | No | `500` | Maximum function call count |
| `maxStackDepth` | Integer | No | `128` | Maximum stack depth |
| `maxStringLen` | Integer | No | `8192` | Maximum string length |
| `errorPolicy` | String | No | `Ignore` | Error policy: `Ignore` / `Deny` / `DenyWith` |

\* `source` and `bytecode` are mutually exclusive; at least one must be provided.

### Error Policies

| Policy | Behavior |
|--------|----------|
| `Ignore` | Ignore script execution errors, continue processing |
| `Deny` | Reject request on script error (returns 500) |
| `DenyWith` | Return custom status code and message on error |

---

## Usage Scenarios

### Scenario 1: Header Validation

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "require-api-token"
      source: |
        let token = req.header("X-Api-Token")
        if token == nil {
          return deny(403, "missing X-Api-Token header")
        }
      errorPolicy: deny
```

### Scenario 2: Conditional Routing Tags

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "set-routing-tag"
      source: |
        let ua = req.header("User-Agent")
        if ua != nil && contains(ua, "Mobile") {
          req.set_header("X-Client-Type", "mobile")
        } else {
          req.set_header("X-Client-Type", "desktop")
        }
```

### Scenario 3: Strict Resource Limits

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "strict-check"
      source: |
        let method = req.method()
        if method == "DELETE" {
          return deny(405, "DELETE not allowed")
        }
      maxSteps: 1000
      maxLoopIterations: 10
      errorPolicy: ignore
```

---

## Important Notes

1. DSL scripts run in a sandbox with no access to the file system or network
2. Scripts exceeding resource limits (`maxSteps`, `maxLoopIterations`, etc.) are terminated
3. Using `bytecode` skips runtime compilation for better performance
4. In production, set `errorPolicy` to `Ignore` or `DenyWith` to prevent script errors from causing service disruptions

---

## Complete Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: dsl-route
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: dsl-validation
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dsl-validation
spec:
  requestPlugins:
    - enable: true
      type: Dsl
      config:
        name: "request-validation"
        source: |
          let token = req.header("Authorization")
          if token == nil {
            return deny(401, "Authorization header required")
          }
          let method = req.method()
          if method == "DELETE" {
            let admin = req.header("X-Admin-Token")
            if admin == nil {
              return deny(403, "Admin token required for DELETE")
            }
          }
        errorPolicy: deny
```

## Related Docs

- [Filters Overview](../overview.md)
- [Plugin Composition](../plugin-composition.md)
