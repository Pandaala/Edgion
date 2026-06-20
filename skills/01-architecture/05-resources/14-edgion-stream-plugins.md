---
name: resource-edgion-stream-plugins
description: EdgionStreamPlugins resource — Stream/TCP-layer plugin configuration, connection-level filters.
---

# EdgionStreamPlugins resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

EdgionStreamPlugins is Edgion's custom-extension resource that defines Stream/TCP-layer plugin configuration. Unlike EdgionPlugins, which works at the HTTP layer, EdgionStreamPlugins runs during connection establishment (ConnectionFilter) and applies to scenarios such as IP restriction and TLS route selection.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs`
- Type definitions: `edgion-resources/src/resources/edgion_stream_plugins/`

## Controller-side processing

### preparse

Scan the Stage 1 `plugins` array and detect legacy HTTP fields written into the L4 `IpRestriction` config — a stale schema. Return an error prefixed with `[LEGACY_SCHEMA_DEPRECATED]`. update_status recognizes the prefix and maps reason to `LegacySchemaDeprecated` rather than the generic `Invalid`, making it easier for operators to find migration guidance.

### parse

- **Cross-PM ref registration**: first `clear_refs_from` clears this resource's old refs in `edgion_config_data_refs`, then iterate each Stage 1 `EdgionStreamPlugin::GlobalConnectionIpRestriction` entry, registering `edgion_config_data_ref.namespace/name` as the cluster_pm_key with value `"EdgionStreamPlugins:<this_ns>/<this_name>"`
- This way, when the referenced GCIR PM changes, the PM handler's `requeue_pm_referrers` will requeue this ESP and refresh ResolvedRefs

### on_delete

- `clear_refs_from` clears all registrations of this resource in `edgion_config_data_refs`

### update_status

Uses `k8s_openapi::Condition`, writing two conditions:

- **`Accepted`**: True (reason=Accepted) when there are no validation_errors; False/`LegacySchemaDeprecated` for legacy-schema errors; False/`Invalid` for other errors
- **`ResolvedRefs`**: iterate each Stage 1 `GlobalConnectionIpRestriction` entry, requiring the PM referenced by `edgion_config_data_ref` to exist with `spec.metadata.type = GlobalConnectionIpRestriction`. Also resolves `allowRefs`/`denyRefs` on every `IpRestriction` entry (Stage 1 and Stage 2) and on each GCIR per-profile `ProfileRules`, requiring each named `IpList` EdgionConfigData to exist and be enabled. Any failure → `ResolvedRefsError::EdgionConfigDataNotFound`, reason `InvalidEdgionConfigDataRef`. Stage 2 `tls_route_plugins` `IpRestriction` entries may also carry `allowRefs`/`denyRefs` and are checked here.

## Gateway-side processing

After EdgionStreamPlugins is synced to the Gateway side, it runs during connection establishment. Typical Stream plugins include:

- **IP restriction**: allowlist/denylist control based on the client IP address
- **TLS route selection**: route selection based on the SNI in the TLS ClientHello

These plugins run at the TCP-connection level and do not require HTTP protocol parsing.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionStreamPlugins ← Route resources | HTTPRoute/GRPCRoute/TCPRoute/TLSRoute | association reference | Route resources may reference Stream plugins |
| EdgionStreamPlugins → EdgionConfigData | EdgionConfigData | `GlobalConnectionIpRestriction.overrideRef` | References a GCIR PM; a reverse-requeue index is built via `edgion_config_data_refs`, so PM changes re-process ESP |
| EdgionStreamPlugins → EdgionConfigData | EdgionConfigData | `IpRestriction.allowRefs` / `IpRestriction.denyRefs` (Stage 1 + Stage 2); GCIR per-profile `ProfileRules.allowRefs` / `ProfileRules.denyRefs` | References an `IpList` EdgionConfigData; resolved at gateway; `ResolvedRefs=False` on this ESP when referenced IpList is missing/disabled/wrong-type |
