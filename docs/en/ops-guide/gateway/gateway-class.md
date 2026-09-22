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

The examples use an explicitly configured `conf_center.controller_name` of
`edgion.io/gateway-controller`. Replace it with your deployment's resolved identity.

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
| `spec.controllerName` | String | Yes | Must exactly match the Controller's configured or generated `controller_name`. |
| `spec.parametersRef` | Object | No | Reference to an `EdgionGatewayConfig` that tunes gateway behavior. When set, `group` must be `edgion.io` and `kind` must be `EdgionGatewayConfig`, or the GatewayClass is rejected (`Accepted: False`). |
| `spec.parametersRef.group` | String | No | API group of the parameters resource: `edgion.io`. |
| `spec.parametersRef.kind` | String | No | `EdgionGatewayConfig`. |
| `spec.parametersRef.name` | String | No | Name of the `EdgionGatewayConfig` resource. |
| `spec.description` | String | No | Human-readable description. |

`EdgionGatewayConfig` is cluster-scoped, so `parametersRef` does not take a `namespace`.

## controllerName

When Kubernetes configuration omits `conf_center.controller_name`, the identity is:

```
edgion.io/gateway-controller/{POD_NAMESPACE}/{EDGION_CONTROLLER_DEPLOYMENT_NAME}
```

Inject the namespace through the Downward API and explicitly inject the stable
Deployment name. Never use a Pod name: all replicas must resolve the same identity.
Missing or invalid components stop startup. An explicit `controller_name` takes
precedence; blank or invalid values fail startup. FileSystem and etcd require an
explicit value. See [Controller configuration](../configuration/controller-config.md).

The Controller reports status for every GatewayClass with this controller name.
Classes other than the configured `gateway_class` receive
`Accepted=False/Unsupported`, with no supported features, and are not published to
Gateways. The GatewayClass watch ignores `label_selector` so unlabeled classes are
also reported. `no_watch_kinds: [GatewayClass]` disables both reporting and Gateway
processing.

Kubernetes ownership checks both `controller_name` and the configured `gateway_class`.
The Controller processes a Gateway only after it has published the matching GatewayClass
incarnation (the same nonempty Kubernetes UID). A missing, foreign or unwatched class
does not authorize Gateway processing; `no_watch_kinds: [GatewayClass]` therefore disables
Gateway processing. Deleting or replacing the class withdraws affected cached Gateways
until ownership is established again. Class `Accepted=False` does not itself transfer
ownership, and an ordinary generation update does not revoke it.

Use distinct controller names and GatewayClasses for independent deployments.
Explicit identity collisions are not detected at runtime. HA replicas sharing one class
and controller name must share their election domain and configuration/resource view.
The version check below does not prevent two independently elected, overlapping deployments
from alternating different desired statuses.

EdgionAcme handler admission and service eligibility use the same Gateway/class
ownership proof. This does not establish fencing or recovery for an already
running certificate attempt during deployment handoff; that scheduler boundary
is separate from GatewayClass status reporting.

When troubleshooting Route attachment, check both `status.parents[].parentRef` and `controllerName`.
The Gateway uses the current Edgion controller's matching `Accepted` result: another controller's
`Accepted=True` cannot override Edgion's `Accepted=False`. Different resource kinds, Listener sections
and ports are evaluated separately. The Controller preserves observed foreign statuses and removes
its own stale or duplicate entries.

In Kubernetes mode, generic Controller status updates check the resource version used to
compute them. If another writer updates the resource first, the stale write fails with
`409 Conflict`, preserving the successful newer state. The Controller records the failure
without an immediate retry; a later resource event, related dependency change or successful
LIST can trigger another reconciliation. Recovery has no fixed deadline. If status stays
stale, check Controller status-write warnings and API permissions.

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
    downstreamKeepaliveRequestLimit: 10000
  httpTimeout:
    backend:
      defaultConnectTimeout: "5s"
      defaultRequestTimeout: "60s"
```

See [EdgionGatewayConfig](../configuration/edgion-gateway-config.md) for the full set of
tunable fields.

### Switch parameters without recreating Gateways

Because Gateways reference the class by name, you can repoint `parametersRef` at a different
`EdgionGatewayConfig`. Dynamically resolved policies apply without rebuilding the listener.
Compression changes apply to subsequent requests; the first HTTP/1 request on a connection
captures its keepalive request budget for the lifetime of that connection.

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

### Deployment status on shared resources

In Kubernetes, `EdgionPlugins`, `EdgionStreamPlugins`, `EdgionBackend` and
`LinkSys` store each deployment's complete observation in `status.controllers[]`.
Select the entry by `controllerName` and `gatewayClassName`, then inspect its
`status.conditions`. `gatewayClassUid` identifies the current GatewayClass
incarnation. LinkSys credential lifecycle fields also live inside that entry's
`status`, not at the top level.

Each resource holds at most 32 deployment entries. Existing entries can still
update at the limit; a new deployment cannot evict another entry. The Controller
reports the refused status write while continuing local configuration processing.
Inactive entries do not expire: cleanup requires authoritative GatewayClass
deletion or identity-replacement evidence. FileSystem and etcd persistence retain
their native status format. Gateway configuration retains complete `status` and
includes Controller-generated `spec.currentStatus`. Inspect this current view
when troubleshooting this deployment's route acceptance, listener status, or
ACME challenges. Another deployment's successful status does not authorize local
serving. This is internal diagnostic and transport data; do not set it in resource
YAML. A Route with missing current status or no matching `Accepted=True` entry is
not admitted by the data plane. Independent deployments should use distinct
controller names and GatewayClasses; replicas sharing a class must share an election domain and
configuration view.

### GatewayClass caveats

- `controllerName` is immutable in the Gateway API; to change controllers you must delete and
  recreate the GatewayClass.
- If `controllerName` does not exactly match the configured Controller name, Edgion never sets
  an `Accepted` condition and Gateways referencing the class stay unprogrammed.
- A `parametersRef` with the wrong `group` or `kind` is rejected with `Accepted: False`; an
  unset `parametersRef` is valid and falls back to defaults.
- `EdgionGatewayConfig` is cluster-scoped, so `parametersRef` carries no `namespace`.
- Shared `EdgionGatewayConfig.status` reports intrinsic configuration validity.
  If a visible `globalPluginsRef` has an invalid early RequestId source or condition,
  the consuming GatewayClass reports `Accepted=False/InvalidParameters` while
  `ResolvedRefs=True` still means the EGC exists. Check the class status for these
  deployment-local errors. Missing/unwatched plugins are not newly rejected by this check.
- `EdgionGatewayConfig` changes apply dynamically (no Gateway restart); see its page for the
  fields that hot-reload.

## Related documents

- [Gateway overview](overview.md)
- [Listeners](listeners/README.md)
- [EdgionGatewayConfig](../configuration/edgion-gateway-config.md)
- [Preflight policy](preflight-policy.md)
