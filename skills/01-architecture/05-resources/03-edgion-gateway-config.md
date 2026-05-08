---
name: resource-edgion-gateway-config
description: EdgionGatewayConfig resource — Edgion's extended global gateway configuration.
---

# EdgionGatewayConfig resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

EdgionGatewayConfig is a cluster-scoped Edgion-extension resource used to configure global gateway behavior parameters. It is a custom extension outside the Gateway API standard.

## Source code locations

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs`
- Gateway ConfHandler: `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs`
- Type definitions: `src/types/resources/edgion_gateway_config.rs`

## Controller-side processing

### parse

No special handling; passes through directly. The contents of EdgionGatewayConfig's spec are handled by the type itself during preparse.

### update_status

- `Accepted`: True (reason=Accepted) when there are no validation_errors; False (reason=Invalid) when errors exist

## Gateway-side processing

After EdgionGatewayConfig is synced to the Gateway side, its configuration items apply to the global behavior of all Gateway instances (e.g., default timeouts, buffer sizes, and other global parameters).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionGatewayConfig → Gateway | Gateway | global | Global configuration affects the behavior of all Gateways |
