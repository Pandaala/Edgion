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

Current code scans the Stage 1 `plugins` array for removed HTTP fields and maps
the marker to `LegacySchemaDeprecated`. That special legacy reason is
pre-release removal debt: current manifests must use the L4 schema, and new
validation must not add more legacy markers. Remove the detector, reason, and
its tests together when code changes are in scope; ordinary current-schema
errors use `Invalid`.

### parse

- **Cross-PM ref registration**: first `clear_refs_from` clears this resource's old refs in `edgion_config_data_refs`, then iterate each Stage 1 `EdgionStreamPlugin::GlobalConnectionIpRestriction` entry, registering `edgion_config_data_ref.namespace/name` as the cluster_pm_key with value `"EdgionStreamPlugins:<this_ns>/<this_name>"`
- This way, when the referenced GCIR PM changes, the PM handler's `requeue_pm_referrers` will requeue this ESP and refresh ResolvedRefs

### on_delete

- `clear_refs_from` clears all registrations of this resource in `edgion_config_data_refs`

### update_status

Uses `k8s_openapi::Condition`, writing three conditions:

- **`Accepted`**: True (reason=Accepted) when there are no validation errors;
  current code still emits `LegacySchemaDeprecated` for the removal debt above,
  and `Invalid` for current-schema errors. It stays `True` when inline IP lists
  have bad entries (partial-good; see `PartiallyInvalid` below).
- **`ResolvedRefs`**: iterates all EdgionConfigData references:
  - `GlobalConnectionIpRestriction.activeProfileRef` (→ Selector): must exist as a `Selector` EdgionConfigData. If `ref_denied` is set (cross-ns without grant / target `visibility: Namespace` without a `ReferenceGrant`): reason `RefNotPermitted`. If missing or wrong-type: reason `InvalidEdgionConfigDataRef`.
  - `IpRestriction.allowRefs`/`denyRefs` (Stage 1 + Stage 2) and GCIR per-profile `ProfileRules.allowRefs`/`denyRefs` (→ IpList): each named `IpList` EdgionConfigData must exist and be enabled. Denied cross-ns ref → reason `RefNotPermitted`; missing/disabled/wrong-type → reason `InvalidEdgionConfigDataRef`.
  - Stage 2 `tls_route_plugins` `IpRestriction` entries are checked here as well.
- **`PartiallyInvalid`**: set when `IpRestriction` (Stage 1 or Stage 2) or GCIR `ProfileRules` inline `allow`/`deny` groups contain bad entries (invalid CIDR, empty group, cross-group duplicate CIDR, duplicate/invalid group name). `status=True`/`reason=RuleInvalid` when bad entries exist (message lists them); `status=False`/`reason=AllRulesValid` when clean. The plugin still loads — valid entries compile and the connection filter operates normally. A skipped inline **deny** entry from `IpRestriction` (Stage 1 or Stage 2) increments `edgion_ip_deny_entries_skipped_total{scope="stream_l4"|"tls_route"}` (IpRestriction HTTP/L4/TLS-route only; GCIR per-profile deny is not metered by this counter — it uses `record_conn_ip_restriction_event`). Blacklist hole = loud metric. Exception: if the structural guards fire (both allow+deny empty with no refs, or GCIR `ProfileRules` `Some(vec![])` on an inline allow/deny), `Accepted` is set `False/Invalid` — those are structural misconfigurations, not bad-CIDR cases.

## Gateway-side processing

The source store forwards EdgionStreamPlugins to `PluginPublisher`.
`StreamPluginGeneration` shares demanded entry instances across effective
GatewayClass views. Controller validation remains policy-independent; class
policy from `EdgionGatewayConfig.spec.pluginPolicy` masks `stream/` and
`tlsRoute/` entries at publication. Block denies the applicable new admission;
Bypass excludes the entry before construction.

Listener connection views compare all serving Gateway candidates before routing.
Equivalent views run once; incompatible views deny new connections. Matched
TCP/UDP/TLS routes use the owning Gateway's published view. Established sessions
retain their admitted runtime, and policy edits do not rebuild listeners.
`tlsRoutePlugins` remains a separate stage; no Stage-1 fallback is introduced.
See [scoped publication](../../04-review/plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionStreamPlugins ← Route resources | HTTPRoute/GRPCRoute/TCPRoute/TLSRoute | association reference | Route resources may reference Stream plugins |
| EdgionStreamPlugins → EdgionConfigData | EdgionConfigData | `GlobalConnectionIpRestriction.activeProfileRef` (→ Selector) | Cross-ns refs gated by ReferenceGrant + target `visibility`; denied ref → `ref_denied` → `ResolvedRefs=False/RefNotPermitted` |
| EdgionStreamPlugins → EdgionConfigData | EdgionConfigData | `IpRestriction.allowRefs`/`denyRefs` (Stage 1 + Stage 2); GCIR per-profile `ProfileRules.allowRefs`/`denyRefs` (→ IpList) | Cross-ns refs gated by ReferenceGrant + target `visibility`; denied cross-ns ref → `RefNotPermitted`; missing/disabled/wrong-type → `InvalidEdgionConfigDataRef` |
