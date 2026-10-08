---
name: gateway-class-resource
description: GatewayClass resource schema: controller binding and runtime configuration reference.
---

# GatewayClass Resource

> API: `gateway.networking.k8s.io/v1` | Scope: Cluster
> Gateway API v1.5.0 Core resource

GatewayClass defines the "type" of a Gateway, associating it with a specific controller implementation and optionally referencing a runtime configuration.

> **Design decision**: One deployment serves every GatewayClass matching its resolved controllerName. Each Gateway uses its class's own optional parametersRef; when unset, it uses built-in defaults, and when set, it must reference `edgion.io/EdgionGatewayConfig`. Independent deployments use distinct identities and election domains. Cross-class physical listener sharing is not supported.

## Full Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller   # Required: controller identifier
  parametersRef:                                   # Optional: runtime configuration reference
    group: edgion.io
    kind: EdgionGatewayConfig
    name: default-config
  description: "Edgion Gateway powered by Pingora" # Optional: description
```

## spec Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `controllerName` | `String` | Yes | Must match the deployment's explicitly configured or generated `controller_name` |
| `parametersRef` | `ParametersRef?` | No | Reference to an EdgionGatewayConfig CRD |
| `description` | `String?` | No | Human-readable description |

### parametersRef Schema

EdgionGatewayConfig is cluster-scoped: `namespace` MUST be unset, including no
explicit empty namespace. Group/kind must match exactly and name must be a valid
resource name. An absent parametersRef retains built-in defaults; an invalid
reference never selects a same-name configuration.

```yaml
parametersRef:
  group: String      # "edgion.io"
  kind: String       # "EdgionGatewayConfig"
  name: String       # EdgionGatewayConfig resource name
```

## Relationship with Gateway

```
GatewayClass (cluster-scoped)
  ├── controllerName: identifies which controller handles it
  ├── parametersRef → EdgionGatewayConfig (runtime configuration)
  │
  └── Gateway.spec.gatewayClassName: "edgion"
       ├── Listener 1 (port, protocol, tls)
       ├── Listener 2
       └── ...
```

- One GatewayClass can correspond to multiple Gateways.
- In K8s mode, the Controller matches the resolved controller name, without a class-name selector.
  Gateway processing also requires a matching raw and published class incarnation (nonempty UID).
- FileSystem/etcd retain GatewayClass filtering by the explicitly configured controller name without a
  selected-class restriction; their Gateway handlers do not require class publication.

## Status Schema

```yaml
status:
  conditions:
    - type: Accepted                    # Controller has accepted this GatewayClass
      status: "True"
      reason: Accepted
    - type: SupportedVersion            # API version supported
      status: "True"
```
