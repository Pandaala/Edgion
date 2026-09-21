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

### Runtime EGC snapshot

The existing `EDGION_GATEWAY_CONFIG_STORE` publishes one
`ArcSwap<EdgionGatewayConfigSnapshot>`. Its resource map and
`HashMap<String, Arc<ResolvedGatewayConfig>>` advance together on full and partial
updates. `ResolvedGatewayConfig::resolve` parses durations, body limits and header
names, validates and constructs RealIp extraction, and resolves security/server
values on the configuration path. EGC remains an independently synchronized
resource; Controller does not embed it into Gateway resources.

`bind_gateway_config` resolves the selected name once during listener construction
and returns a shared `Arc<GatewayConfigHandle>`. Publication swaps that handle's
resolved generation. `GatewayConfigHandle::load` performs one atomic Arc load
without hashing a name or reading the resource map. HTTP `new_ctx` pins the result
for request metadata, upstream retries, compression and response processing.
The HTTP/1 keepalive budget is separately retained in Pingora persistent
connection context from the first request. TCP/TLS `process_new` and UDP session
creation capture their own runtime Arc; TLS certificate selection captures at
handshake start. Missing/deleted names and no reference use shared defaults.

This propagation is Edgion's chosen product contract, not a requirement that
Gateway API imposes on all GatewayClass parameter implementations. It does not
retarget a listener's EGC name on GatewayClass reference changes, rebind ports,
transfer shared-listener ownership, or enable Gateway deletion/recreation.
See [EGC publication regression boundaries](../../04-review/listeners/regression-guards/egc-runtime-snapshot-publication.md).

### Existing subsystem publishers

LinkSys, load-balancing defaults, outbound TLS and DNS keep their existing global
dynamic consumers. Plugin policy, global references and access-log masking keep
their scoped publisher. These publications are not an atomic transaction with
the request's runtime EGC Arc.

The plugin-policy publisher resolves `spec.pluginPolicy` only through each
GatewayClass parametersRef. Unreferenced EGC resources do not contribute policy;
classes sharing one EGC share its policy while retaining their own attachment
scope. Policy, global plugin references, source generations and attachments
produce one immutable scope. EGC/class/binding changes rebuild affected views
without restarting listeners. Missing/invalid referenced configuration denies
new admission; no parametersRef selects the explicit default scope.

Missing plugin policy scope still denies admission even though ordinary runtime
EGC lookup falls back to defaults. See
[plugin publication](../../04-review/plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionGatewayConfig → GatewayClass → Gateway | GatewayClass parametersRef | scoped publication | Plugin execution policy affects only bound classes and their Gateways |
