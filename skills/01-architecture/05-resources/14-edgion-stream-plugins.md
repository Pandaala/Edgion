---
name: resource-edgion-stream-plugins
description: EdgionStreamPlugins resource — Stream/TCP-layer plugin configuration, connection-level filters.
---

# EdgionStreamPlugins resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

EdgionStreamPlugins is Edgion's custom-extension resource that defines Stream/TCP-layer plugin configuration. Unlike EdgionPlugins, which works at the HTTP layer, EdgionStreamPlugins runs during connection establishment (ConnectionFilter) and applies to scenarios such as IP restriction and TLS route selection.

## Source code locations

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs`
- Type definitions: `edgion-resources/src/resources/edgion_stream_plugins/`

## Controller-side processing

### preparse

Scan the Stage 1 `plugins` array and detect legacy HTTP fields written into the L4 `IpRestriction` config — a stale schema. Return an error prefixed with `[LEGACY_SCHEMA_DEPRECATED]`. update_status recognizes the prefix and maps reason to `LegacySchemaDeprecated` rather than the generic `Invalid`, making it easier for operators to find migration guidance.

### parse

- **Cross-PM ref registration**: first `clear_refs_from` clears this resource's old refs in `plugin_metadata_refs`, then iterate each Stage 1 `EdgionStreamPlugin::GlobalConnectionIpRestriction` entry, registering `plugin_metadata_ref.namespace/name` as the cluster_pm_key with value `"EdgionStreamPlugins:<this_ns>/<this_name>"`
- This way, when the referenced GCIR PM changes, the PM handler's `requeue_pm_referrers` will requeue this ESP and refresh ResolvedRefs

### on_delete

- `clear_refs_from` clears all registrations of this resource in `plugin_metadata_refs`

### update_status

Uses `k8s_openapi::Condition`, writing two conditions:

- **`Accepted`**: True (reason=Accepted) when there are no validation_errors; False/`LegacySchemaDeprecated` for legacy-schema errors; False/`Invalid` for other errors
- **`ResolvedRefs`**: iterate each Stage 1 `GlobalConnectionIpRestriction` entry, requiring the PM referenced by `plugin_metadata_ref` to exist with `spec.metadata.type = GlobalConnectionIpRestriction`. Any failure → `ResolvedRefsError::PluginMetaDataNotFound`, reason `InvalidPluginMetaDataRef`. Stage 2 `tls_route_plugins` are currently all inline configurations with no external refs

## Gateway-side processing

After EdgionStreamPlugins is synced to the Gateway side, it runs during connection establishment. Typical Stream plugins include:

- **IP restriction**: allowlist/denylist control based on the client IP address
- **TLS route selection**: route selection based on the SNI in the TLS ClientHello

These plugins run at the TCP-connection level and do not require HTTP protocol parsing.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionStreamPlugins ← Route resources | HTTPRoute/GRPCRoute/TCPRoute/TLSRoute | association reference | Route resources may reference Stream plugins |
| EdgionStreamPlugins → PluginMetaData | PluginMetaData | `GlobalConnectionIpRestriction.pluginMetadataRef` | References a GCIR PM; a reverse-requeue index is built via `plugin_metadata_refs`, so PM changes re-process ESP |
