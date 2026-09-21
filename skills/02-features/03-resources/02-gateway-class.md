---
name: gateway-class-resource
description: GatewayClass resource schema: controller binding and runtime configuration reference.
---

# GatewayClass Resource

> API: `gateway.networking.k8s.io/v1` | Scope: Cluster
> Gateway API v1.5.0 Core resource

GatewayClass defines the "type" of a Gateway, associating it with a specific controller implementation and optionally referencing a runtime configuration.

> **Design decision**: Edgion adopts the 1 Controller : 1 GatewayClass pattern (consistent with Envoy Gateway and NGINX Gateway Fabric); a single instance explicitly does not support managing multiple GatewayClasses. `parametersRef` is optional — when unset, the Gateway uses built-in defaults; when set, it must reference `edgion.io/EdgionGatewayConfig`.

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
| `controllerName` | `String` | Yes | Must match the configured `controller_name` (default `edgion.io/gateway-controller`) |
| `parametersRef` | `ParametersRef?` | No | Reference to an EdgionGatewayConfig CRD |
| `description` | `String?` | No | Human-readable description |

### parametersRef Schema

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
- In K8s mode, the Controller matches both the configured GatewayClass name and controller name.
  Gateway processing also requires a matching raw and published class incarnation (nonempty UID).
- FileSystem/etcd retain GatewayClass filtering by the default controller name without a
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
