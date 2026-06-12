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

Filters by `spec.controllerName`, processing only GatewayClasses whose controllerName matches the configuration (default `edgion.io/gateway-controller`).

### validate

Validates `parametersRef` (if set): `group` must be `edgion.io`, and `kind` must be `EdgionGatewayConfig`. Invalid values produce a validation error → `Accepted: False`. When `parametersRef` is unset, no error is produced.

### parse

No special handling; passes through directly.

### update_status

- `Accepted`: True (reason=Accepted) when there are no validation_errors; False (reason=Invalid) when errors exist
- `SupportedVersion`: always True, indicating support for the current Gateway API version

## Gateway-side processing

After GatewayClass is synced to the Gateway side, it is stored in the GatewayClass cache (`edgion-gateway/src/config/gateway_class/`) and used for class-matching validation of Gateway resources.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| GatewayClass ← Gateway | Gateway | gatewayClassName | Gateway references GatewayClass via the gatewayClassName field |
