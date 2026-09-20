# GatewayClass

Selects the Edgion controller and, optionally, a tuning parameter set.

## Overview

A `GatewayClass` is a cluster-scoped resource that tells Kubernetes which controller owns a
class of Gateways. To run a Gateway on Edgion, create a GatewayClass whose `controllerName`
matches the Edgion controller, then set `gatewayClassName` on your Gateways to reference it. A
GatewayClass may also point at an `EdgionGatewayConfig` via `parametersRef` to tune data-plane
behavior.

Edgion follows a 1 controller : 1 GatewayClass model: a single Edgion instance manages one
GatewayClass. `parametersRef` is optional — when unset, Gateways use built-in defaults; when
set, it must reference an `edgion.io/EdgionGatewayConfig`.

## Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
```

## Field reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `spec.controllerName` | String | Yes | Must match the Controller's configured `controller_name`, which defaults to `edgion.io/gateway-controller`. |
| `spec.parametersRef` | Object | No | Reference to an `EdgionGatewayConfig` that tunes gateway behavior. When set, `group` must be `edgion.io` and `kind` must be `EdgionGatewayConfig`, or the GatewayClass is rejected (`Accepted: False`). |
| `spec.parametersRef.group` | String | No | API group of the parameters resource: `edgion.io`. |
| `spec.parametersRef.kind` | String | No | `EdgionGatewayConfig`. |
| `spec.parametersRef.name` | String | No | Name of the `EdgionGatewayConfig` resource. |
| `spec.description` | String | No | Human-readable description. |

`EdgionGatewayConfig` is cluster-scoped, so `parametersRef` does not take a `namespace`.

## controllerName

The Edgion controller name defaults to:

```
edgion.io/gateway-controller
```

In Kubernetes mode, set `controller_name` in the Controller configuration to customize this name;
the GatewayClass must match the configured value.

When troubleshooting Route attachment, check both `status.parents[].parentRef` and `controllerName`.
The Gateway uses the current Edgion controller's matching `Accepted` result: another controller's
`Accepted=True` cannot override Edgion's `Accepted=False`. Different resource kinds, Listener sections
and ports are evaluated separately. The Controller preserves observed foreign statuses and removes
its own stale or duplicate entries.

## Scenarios

### Basic class with no parameters

The simplest class. Gateways referencing it use Edgion defaults.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  description: "Edgion Gateway powered by Pingora"
```

### Class with tuning parameters

> **🔌 Edgion extension**
>
> `parametersRef` referencing the `EdgionGatewayConfig` CRD is an Edgion extension; it is not part of standard Gateway API.

Attach an `EdgionGatewayConfig` to tune data-plane behavior for all Gateways in this class.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: prod-gateway-config
---
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: prod-gateway-config
spec:
  server:
    enableCompression: true
    downstreamKeepaliveRequestLimit: 1000
  httpTimeout:
    backend:
      defaultConnectTimeout: "5s"
      defaultRequestTimeout: "60s"
```

See [EdgionGatewayConfig](../configuration/edgion-gateway-config.md) for the full set of
tunable fields.

### Switch parameters without recreating Gateways

Because Gateways reference the class by name, you can repoint `parametersRef` at a different
`EdgionGatewayConfig`. Dynamically resolved policies apply to subsequent requests or connections;
listener-construction fields under `spec.server` require a Gateway process restart.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: tuned-gateway-config
```

## Edge cases and caveats

- `controllerName` is immutable in the Gateway API; to change controllers you must delete and
  recreate the GatewayClass.
- If `controllerName` does not exactly match the configured Controller name, Edgion never sets
  an `Accepted` condition and Gateways referencing the class stay unprogrammed.
- A `parametersRef` with the wrong `group` or `kind` is rejected with `Accepted: False`; an
  unset `parametersRef` is valid and falls back to defaults.
- `EdgionGatewayConfig` is cluster-scoped, so `parametersRef` carries no `namespace`.
- `EdgionGatewayConfig` changes apply dynamically (no Gateway restart); see its page for the
  fields that hot-reload.

## Related documents

- [Gateway overview](overview.md)
- [Listeners](listeners/README.md)
- [EdgionGatewayConfig](../configuration/edgion-gateway-config.md)
- [Preflight policy](preflight-policy.md)
