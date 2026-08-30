---
name: resource-edgion-gateway-config
description: EdgionGatewayConfig resource — Edgion's extended global gateway configuration.
---

# EdgionGatewayConfig resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

EdgionGatewayConfig is a cluster-scoped Edgion-extension resource used to configure global gateway behavior parameters. It is a custom extension outside the Gateway API standard.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs`
- Gateway ConfHandler: `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs`
- Type definitions: `edgion-resources/src/resources/edgion_gateway_config.rs`

## Controller-side processing

### parse

`preparse` rejects unsupported outbound TLS SAN pinning and validates that DNS resolver
`linkSysRef` and inline servers are mutually exclusive.

`parse` checks removed/deprecated fields, clears prior Secret registrations, resolves the
cluster-wide `outboundTls.validation.caCertificateRefs` through runtime material stores with
ReferenceGrant enforcement, and embeds `resolved_ca_certificates` for conf_sync. Because the owner
is cluster-scoped, referenced material must carry an explicit namespace.

Change/delete events requeue all GatewayClass resources because their parametersRef status depends
on EdgionGatewayConfig existence; delete also clears Secret registrations.

### update_status

- `Accepted`: True (reason=Accepted) when there are no validation_errors; False (reason=Invalid) when errors exist

## Gateway-side processing

After EdgionGatewayConfig is synced to the Gateway side, its configuration items apply to the global behavior of all Gateway instances (e.g., default timeouts, buffer sizes, and other global parameters).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionGatewayConfig → Gateway | Gateway | global | Global configuration affects the behavior of all Gateways |
