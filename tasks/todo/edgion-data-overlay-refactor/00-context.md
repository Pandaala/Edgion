# 00 — Context (related skills)

Skill paths to load by phase. Load progressively (smallest subtree first).

## Architecture
- `skills/01-architecture/00-common/03-resource-system.md` — resource struct ↔ CRD ↔ conf_sync invariants
- `skills/01-architecture/01-controller/03-config-center/SKILL.md` — controller config center, ref registration, requeue
- `skills/01-architecture/01-controller/09-add-new-resource/00-guide.md` — patterns for resource/field changes
- `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md` — HTTP plugin dev (RegionRoute)
- `skills/01-architecture/02-gateway/13-stream-plugin-dev.md` — Stream plugin dev (GlobalConnIpRestriction)
- `skills/01-architecture/03-controller-gateway-link/SKILL.md` — conf_sync full_set / partial_update

## Features / schema
- `skills/02-features/02-config/SKILL.md` — config schema entry
- `skills/02-features/05-annotations/00-annotations-overview.md` — `edgion.io/*` annotations (for `managed-by`)

## Coding
- `skills/03-coding/00-logging-and-tracing-ids.md` — log IDs
- `skills/03-coding/01-log-safety.md` — log safety
- `skills/03-coding/observability/00-access-log.md` — EdgionStatus / access-log signals (fail-to-base observability)

## Testing
- `skills/05-testing/01-integration-testing.md` — integration + dynamic (hot reload) scenarios

## Cross-cutting invariants (from CLAUDE.md)
- CRD ↔ struct invariant: operator-facing field → CRD; controller-resolved/secret → `#[schemars(skip)]` + redact.
  The 5 tagged-enum CRDs (incl. `plugins`, `stream_plugins`, `plugin_metadata`) use
  `x-kubernetes-preserve-unknown-fields` and are **not** field-guarded — `#[schemars(skip)]` + redact is the
  only safeguard. Re-run `./cicd/checks/check_crd_coverage.sh` after struct changes.
- Data plane must NOT use `tracing::warn!/error!` — observability via access-log EdgionStatus or Prometheus.
- All on-disk content (incl. these task docs) is English.

## Key current code anchors (pre-refactor)
> Rename target (S1): `plugin_metadata` module → `data`; kind `PluginMetaData` → `EdgionData`;
> `PluginMetaDataRef`/`PluginReference` → `DataRef`; `PluginMetaDataGatewayStore` → `EdgionDataStore`.
- PM variants: `edgion-resources/src/resources/plugin_metadata/{mod,key_list,ip_list,region_route,global_connection_ip_restriction}.rs`
- RegionRoute plugin config: `edgion-resources/src/resources/edgion_plugins/plugin_configs/region_route.rs`
- Stream plugins spec: `edgion-resources/src/resources/edgion_stream_plugins/{mod,stream_plugins}.rs`
- Gateway PM store (ArcSwap + typed indexes): `edgion-gateway/src/plugins/runtime/plugin_metadata/conf_handler.rs`
- RegionRoute resolve chain: `edgion-gateway/src/plugins/http/region_route/plugin.rs` (`resolve_config`)
- Stream GlobalConnIpRestriction (fail-closed): `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/plugin.rs`
- Controller handlers: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/{plugin_metadata,edgion_plugins,edgion_stream_plugins}.rs`
- Schema-layer plugin validation: `EdgionPlugins::validate_plugins` in `edgion-resources`
