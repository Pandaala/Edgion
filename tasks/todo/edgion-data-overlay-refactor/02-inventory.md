# 02 — Current PluginMetaData inventory (pre-refactor, source-verified)

Snapshot of the **current** `PluginMetaData` resource as it exists in source (2026-06-15), to drive S1.
All facts quoted from source; file:line included.

## Top-level shape

`edgion-resources/src/resources/plugin_metadata/mod.rs`
- Kind `PluginMetaData`, group `edgion.io`, version `v1`, plural `pluginmetadata`, **namespaced**, status `PluginMetaDataStatus` (`:34-42`).
- `PluginMetaDataSpec { metadata: MetaDataEntry }` (`:44-48`) — spec field is `metadata`.
- `MetaDataEntry` is `#[serde(tag = "type", content = "config")]` (`:75`). **Wire shape:**
  ```yaml
  spec:
    metadata:
      type: <Variant>
      config: { ... }
  ```
- Status conditions (Gateway-API style): `Accepted` (per-variant validation) + `ResolvedRefs` (only
  meaningful for `ServiceRegionRoute.clusterRef`; all others always `True`) (`:50-62`).
- `validate_pre_parse()` (`:113-142`): IpList → structure check; KeyList → structure check;
  GlobalConnectionIpRestriction → `validate_and_init` (matcher precompile); **RegionRoute variants → no
  validation** (cross-ref check lives in controller `ResolvedRefs`).

## The five variants

### 1. `KeyList` (`key_list.rs`) — **already the unified StringList + RegexList**
> File header `:1`: "Key list metadata — **unified StringList + RegexList**". mod.rs `:78`: "Merges the legacy
> StringList and RegexList into a single type." ← **StringList/RegexList were deliberately removed and folded
> into KeyList(matchMode).** See flag F1.
- `KeyListData { match_mode: KeyMatchMode(Exact|Regex), items: Vec<KeyGroup> }` (`:49-56`).
- `KeyGroup { name, description?, items: Vec<MetaDataItem> }` (`:28-40`).
- `MetaDataItem { key (redacted via redact_required), code?, priority?, id?, behavior? }` (`:61-81`).
- Validation: `validate_key_groups_structure` (group name charset/uniqueness, cross-group key uniqueness,
  regex compile when Regex mode).
- **Referenced by:** doc comment claims "KeyAuth, IpRestriction (future use)" — **no actual consumer**.
  Gateway store builds `key_list_configs` index + `get_key_list_config` getter, called **only by unit tests**.
- **YAML fixtures:** 0.

### 2. `IpList` (`ip_list.rs`) — simple CIDR groups
- `IpListData { items: Vec<IpGroup> }`. `IpGroup { name, description?, cidrs: Vec<String> }` (shared from
  `global_connection_ip_restriction.rs:51-62`).
- Validation: `validate_groups_structure`.
- **Referenced by:** **no actual consumer**. `ip_list_configs` index + `get_ip_list_config`, unit-test only.
- **YAML fixtures:** 0.

### 3. `ClusterRegionRoute` (`region_route.rs:19-51`) — heavy logic + data
- `ClusterRegionRouteConfig { regions: Vec<RegionDef>, my_region, key_get: Vec<KeyGet>, hash_key_get?,
  hash_calc?, route_rules: Vec<RouteRule>, route_by_key_conf_match? }`.
- `RouteByKeyConfMatchMeta { match_map: HashMap<String,String>, bypass_cluster_keys: Vec<String> }` (`:91-98`).
- **Referenced by:** the chain — RegionRoute plugin (`service_region_route_ref`) → ServiceRegionRoute PM →
  its `cluster_ref` → this ClusterRegionRoute PM.
- **Parsed into** `ClusterRegionRouteEntry` (`:104-116`) stored in the RegionRoute index — **"Used by both the
  Controller's in-memory table and the Center's aggregated store."** ← cross-repo, see flag F2.
- **YAML fixtures:** 6 (`type: ClusterRegionRoute`).

### 4. `ServiceRegionRoute` (`region_route.rs:60-74`) — per-service failover + cross-PM ref
- `ServiceRegionRouteConfig { cluster_ref: Option<PluginMetaDataRef>, regions: Vec<ServiceRegionDef> }`.
- `ServiceRegionDef { name, failover_to? }` (`:80-86`) — only `failover_to` is service-specific.
- **Referenced by:** RegionRoute plugin config `service_region_route_ref` (the entry point of the chain).
- **Parsed into** `ServiceRegionRouteEntry { pm_namespace, pm_name, cluster_pm_ref?, regions, ref_plugins:
  Vec<RefPluginEntry> }` (`:131-139`). `RefPluginEntry { kind, namespace, name }` (`:121-125`) = reverse map
  (which EdgionPlugin uses this) for requeue. Controller-only; Center leaves `ref_plugins` empty.
- **YAML fixtures:** 10 (`type: ServiceRegionRoute`).

### 5. `GlobalConnectionIpRestriction` (`global_connection_ip_restriction.rs:13-28`) — the switch model
- `GlobalConnectionIpRestrictionData { enable: bool, active_profile: String, profiles: HashMap<String,
  ProfileRules>, description? }`.
- `ProfileRules { allow?: Vec<IpGroup>, deny?: Vec<IpGroup>, default_action: DefaultAction,
  allow_matcher (#[serde(skip)+schemars(skip)]), deny_matcher (skip) }` (`:30-49`).
- `enable=false` → data plane **completely skips** (allow) (`:16-18`). `active_profile` must exist (validated).
- `validate_and_init` (`:67-87`) pre-compiles matchers per profile; `ProfileRules::validate_and_init` (`:91-111`)
  is **deliberately stricter** than HTTP/Stream IpRestriction on empty allow/deny (documented `:98-103` — do
  not harmonize).
- **Referenced by:** stream plugin `GlobalConnectionIpRestrictionEntry.plugin_metadata_ref` (EdgionStreamPlugins).
  Stream plugin **fails closed** (Deny) on PM/profile missing.
- **YAML fixtures:** 10 (`type: GlobalConnectionIpRestriction`).

## Reference graph

```
EdgionPlugins.RegionRouteConfig.serviceRegionRouteRef ──► PM(ServiceRegionRoute)
                                                              │ cluster_ref
                                                              ▼
                                                          PM(ClusterRegionRoute)
   (both parsed → Cluster/ServiceRegionRouteEntry index; consumed by Controller table AND EdgionCenter)

EdgionStreamPlugins.GlobalConnectionIpRestrictionEntry.pluginMetadataRef ──► PM(GlobalConnectionIpRestriction)

KeyList / IpList ──► (no live consumer; orphan gateway indexes + getters, unit-test only)
```

## YAML fixture inventory (kind: PluginMetaData)

23 files total: 1 CRD (`config/crd/edgion-crd/plugin_metadata_crd.yaml`) + 22 integration fixtures under
`edgion-tests/integration/conf/` + 1 Rust test suite (`tcp_route/global_connection_ip_restriction.rs`).
By variant: ClusterRegionRoute 6, ServiceRegionRoute 10, GlobalConnectionIpRestriction 10, IpList/KeyList 0.
Notable dirs: `PluginMetaDataStatus/` (+ `RefFlap/DynamicTest`), `plugin-metadata-managed/` (see flag F3).

## Flags (impact prior decisions)

- **F1 — RESOLVED (D5):** keep the unified `KeyList(matchMode)`; do **not** re-split StringList/RegexList.
  Revisit only if a lightweight flat list is genuinely needed later.
- **F2 → tracked as I1 (BLOCKER for S3):** confirmed cross-repo. RegionRoute entry types are shared via
  `edgion-resources` and consumed by EdgionCenter (not checked out locally). See `05-issues.md` I1.
- **F3 → RESOLVED:** "managed" is test-scenario naming for the admin-pre-created-PM pattern, not an ownership
  concept; no controller ownership branch exists. BUT `edgion.io/managed-by` already exists as an **ACME
  label** — naming clash to resolve at S6, tracked as I2 in `05-issues.md`.
- **F4 — RESOLVED (D6):** keep the two-level `data: { type, config }` shape. `type` additionally acts as a
  **guard at override resolution** — verify it matches the expected override variant before applying, else
  fail-to-base. Affects all 22 fixtures + CRD (mechanical `metadata` → `data` rename, shape unchanged).

## Authoritative rename map (S1)

| Old (current) | New (`EdgionData`) | Where |
|---------------|--------------------|-------|
| kind `PluginMetaData` / `PLUGIN_METADATA_KIND` | `EdgionData` | `plugin_metadata/mod.rs:31,38` |
| `PluginMetaDataSpec` | `EdgionDataSpec` | `mod.rs:44` |
| `PluginMetaDataStatus` | `EdgionDataStatus` | `mod.rs:59` |
| spec field `metadata: MetaDataEntry` | `data: DataEntry` | `mod.rs:47` (keep `#[serde(tag="type",content="config")]`) |
| `MetaDataEntry` | `DataEntry` | `mod.rs:76` |
| module `resources/plugin_metadata/` | `resources/data/` | edgion-resources |
| `PluginMetaDataRef` (+ stream `PluginReference`) | `DataRef` | `plugin_configs/region_route.rs:137`; `edgion_stream_plugins/stream_plugins.rs` |
| consumer fields `serviceRegionRouteRef` / `pluginMetadataRef` | generic `dataRef` / `overrideRef` | see consumers below |
| gateway `PluginMetaDataGatewayStore` | `EdgionDataStore` | `conf_handler.rs:73` |
| gateway module `plugins/runtime/plugin_metadata/` | `plugins/runtime/data/` | edgion-gateway |
| controller `handlers/plugin_metadata.rs` | `handlers/data.rs` | edgion-controller |
| plural `pluginmetadata` / CRD `plugin_metadata_crd.yaml` | `edgiondata` / `edgion_data_crd.yaml` | `config/crd/edgion-crd/` |

Enumerate all fixtures to update (22 + CRD):
`grep -rln 'kind:\s*PluginMetaData' edgion-tests/ config/` and the Rust suite
`edgion-tests/src/client/suites/tcp_route/global_connection_ip_restriction/`.

## Consumer & runtime code anchors (S2 / S3 / S4)

**RegionRoute (S3):**
- Plugin config (base absorb target): `edgion-resources/src/resources/edgion_plugins/plugin_configs/region_route.rs`
  — `RegionRouteConfig` (`:28`, holds `service_region_route_ref` `:34`), `RegionDef` (`:40`), `HashCalcConfig`
  (`:70`), `PluginMetaDataRef` (`:137`), `RouteRule` (imported, defined under `plugin_configs`).
- PM-side types to remove/relocate: `plugin_metadata/region_route.rs` (`ClusterRegionRouteConfig` `:21`,
  `ServiceRegionRouteConfig` `:62`, `*Entry` `:106/:133`, `RefPluginEntry` `:121`).
- Controller index to rework: `edgion-controller/.../resource_processor/region_route_table/`
  (`mod.rs`, `cluster_table.rs`, `service_table.rs`) — builds the `*Entry`; **shared with EdgionCenter (I1)**.
- Controller handler: `handlers/plugin_metadata.rs` (builds entries `:63,:79`; cross-ref + requeue),
  `handlers/edgion_plugins.rs` (register `overrideRef` requeue).
- Gateway store: `edgion-gateway/src/plugins/runtime/plugin_metadata/conf_handler.rs` —
  `PluginMetaDataGatewayStore` (`:73`), getters `get_cluster_region_route_config[_by_key]` (`:122/:187`),
  `get_service_region_route_config[_by_key]` (`:134/:182`); replace with overlay index.
- Gateway plugin resolve (fail-closed → fail-to-base): `edgion-gateway/src/plugins/http/region_route/plugin.rs`
  `resolve_config` (the ServiceRR → cluster_ref → ClusterRR chain).

**GlobalConnectionIpRestriction (S4):**
- Stream entry/ref: `edgion-resources/src/resources/edgion_stream_plugins/stream_plugins.rs` —
  `EdgionStreamPlugin` enum (`:13`), `GlobalConnectionIpRestrictionEntry` (`:26`, `plugin_metadata_ref` `:29`),
  `PluginReference`.
- Gateway getter: `conf_handler.rs` `get_global_connection_ip_restriction[_by_key]` (`:162/:174`).
- Gateway plugin (fail-closed → fail-to-base): `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/plugin.rs`.
- Orphan getters to delete (D1): `get_key_list_config` (`:146`), `get_ip_list_config` (`:154`) + their indexes.

**Overlay framework / validation (S2):**
- Schema-layer preparse validation: `EdgionPlugins::validate_plugins` at
  `edgion-resources/src/resources/edgion_plugins/validate.rs:124` (extend for `overrideRef` + base safety hints).
- `apply_override` (new) — place in `edgion-resources` alongside the override structs.
