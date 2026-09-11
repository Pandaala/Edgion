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
`linkSysRef` and inline servers are mutually exclusive. It also validates
`spec.pluginPolicy` with the shared pure compiler; invalid names, duplicate
entries, action/status combinations and out-of-range statuses surface as
source validation errors.

`parse` checks removed/deprecated fields, clears prior Secret registrations, resolves the
cluster-wide `outboundTls.validation.caCertificateRefs` through runtime material stores with
ReferenceGrant enforcement, and embeds `resolved_ca_certificates` for conf_sync. Because the owner
is cluster-scoped, referenced material must carry an explicit namespace.

Change/delete events requeue all GatewayClass resources because their parametersRef status depends
on EdgionGatewayConfig existence; delete also clears Secret registrations.

### update_status

- `Accepted`: True (reason=Accepted) when there are no validation_errors; False (reason=Invalid) when errors exist

## Gateway-side processing

The plugin-policy publisher resolves `spec.pluginPolicy` only through each
GatewayClass parametersRef. Unreferenced EGC resources do not contribute policy;
classes sharing one EGC share its policy while retaining their own attachment
scope. Policy, global plugin references, source generations and attachments
produce one immutable scope. EGC/class/binding changes rebuild affected views
without restarting listeners. Missing/invalid referenced configuration denies
new admission; no parametersRef selects the explicit default scope.

This scope rule is specific to plugin execution policy and does not imply that
all existing EGC configuration stores have been redesigned. See
[plugin publication](../../04-review/plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionGatewayConfig → GatewayClass → Gateway | GatewayClass parametersRef | scoped publication | Plugin execution policy affects only bound classes and their Gateways |
