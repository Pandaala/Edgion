---
name: resource-gateway-class
description: GatewayClass resource — cluster-scoped controller matching and filtering.
---

# GatewayClass resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

GatewayClass is a cluster-scoped resource that identifies the Gateway controller's type. Edgion uses it for controller-matching filtering: only GatewayClasses whose `spec.controllerName` matches `edgion.io/gateway-controller` are processed.

## Design decisions

- **1 Controller : 1 GatewayClass**: consistent with Envoy Gateway and NGINX Gateway Fabric, Edgion explicitly does not support a single instance managing multiple GatewayClasses. Deploy multiple Edgion instances when different configurations are needed.
- **parametersRef is optional**: `parametersRef` is not required. When unset, Gateway uses the built-in defaults of `EdgionGatewayConfigSpec`. When set, it must reference `edgion.io/EdgionGatewayConfig`; otherwise Controller rejects it (`Accepted: False`).

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway_class.rs`
- Gateway ConfHandler: `edgion-gateway/src/config/gateway_class/conf_handler_impl.rs`
- Type definitions: `edgion-resources/src/resources/gateway_class.rs`

## Controller-side processing

### filter

Filters by `spec.controllerName` and, in Kubernetes mode, the configured `gateway_class`
name using the shared `GatewayClassOwnership` value. FileSystem/etcd retain controller-name
matching without a selected-class restriction.

### after_publish / after_unpublish

After the class cache mutation, scan raw Gateway keys and requeue those referencing this
class. Raw keys include previously rejected Gateways; scanning only processed cache keys
would prevent recovery after a missing class is created. Gateway eligibility requires a
matching raw owner and the same nonempty UID in this deployment's published class. It
does not require matching generation or `Accepted=True`. Existing workqueue dirty-key
handling preserves notifications while a Gateway is being processed.

### validate

The validate hook has no separate parameter checks. `update_status` resolves
`parametersRef` against the raw EGC store so existence, reference validity, and
deployment-local attachment validation share one result.

### parse

No special handling; passes through directly.

### update_status

- `parametersRef` must use group `edgion.io` and kind `EdgionGatewayConfig`.
  Invalid group/kind or missing EGC produces `ResolvedRefs=False` and
  `Accepted=False`, both with reason `InvalidParameters`.
- When the EGC exists, `ResolvedRefs=True`. Its visible `globalPluginsRef`
  resources use the shared early RequestId validator against current raw plugin
  generations. Invalid early sources/conditions produce
  `Accepted=False/InvalidParameters` on this owned class, never on shared EGC.
  A missing/unwatched plugin retains the existing unknown/not-rejected behavior.
- With no parametersRef or unwatched EGC kind, remove stale ResolvedRefs and do
  not reject solely for the unknown reference. Other validation errors use
  `Accepted=False/Invalid`; otherwise `Accepted=True/Accepted`.
- `SupportedVersion`: always True, indicating support for the current Gateway API version

Plugin changes/deletes requeue the registered EGC, which requeues the consuming
classes. The class lookup reads raw EGC/plugin stores, so it does not depend on
whether the EGC's new processed object has already been published. See the
[shared status guard](../../04-review/status/regression-guards/shared-config-status-deployment-independence.md).

## Gateway-side processing

After GatewayClass is synced to the Gateway side, it is stored in the GatewayClass cache (`edgion-gateway/src/config/gateway_class/`) and used for class-matching validation of Gateway resources.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| GatewayClass ← Gateway | Gateway | gatewayClassName | Gateway references GatewayClass via the gatewayClassName field |
