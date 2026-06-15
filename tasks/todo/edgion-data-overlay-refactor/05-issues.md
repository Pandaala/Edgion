# 05 — Issues

## I1 — [BLOCKER for S3] RegionRoute decomposition is a cross-repo (EdgionCenter) contract change

**Severity:** blocker (for S3 only; S1/S2 unaffected).

**What:** The RegionRoute parsed-entry types and the `ClusterRegionRoute` / `ServiceRegionRoute` PM variants
live in the shared `edgion-resources` crate and are consumed by **EdgionCenter** (separate repo, federation
control plane) in addition to this repo's controller.

**Evidence (this repo):**
- Types defined: `edgion-resources/src/resources/plugin_metadata/region_route.rs:106` (`ClusterRegionRouteEntry`),
  `:133` (`ServiceRegionRouteEntry`); both `#[derive(Serialize)]`.
- Source comment `region_route.rs:101-103` / `:127-130`: entries "Used by both the Controller's in-memory
  table **and the Center's aggregated store**".
- Built in: `handlers/plugin_metadata.rs:63,79`; `region_route_table/{cluster_table,service_table}.rs`.
- EdgionCenter is **not checked out locally** (only `Edgion` exists under `/Volumes/ExtStore/ws4/`), so its
  exact dependency surface cannot be inspected from here.

**Impact:** S3 removes the `ClusterRegionRoute`/`ServiceRegionRoute` variants, drops the cluster_ref chain,
and moves logic into `EdgionPlugins`. That changes (a) the shared `edgion-resources` API EdgionCenter compiles
against, and (b) the fed_sync data contract Center aggregates. Doing S3 unilaterally will break EdgionCenter.

**Required before S3:**
1. Check out / obtain EdgionCenter and grep its use of `ClusterRegionRouteEntry` / `ServiceRegionRouteEntry`
   / the PM RegionRoute variants and the fed_sync RegionRoute payload.
2. Decide the migration shape jointly: does Center also move to the base+overlay model, or keep a compat
   shim for the entry types during transition?
3. Plan a coordinated cross-repo change (paired PRs) or a versioned `edgion-resources` bump.

**Workaround to keep this task moving:** Execute S1 (rename + keep KeyList/IpList + envelope) and S2 (overlay
framework) first — both are local and non-breaking to the RegionRoute contract. Hold S3 until I1 is closed.

---

## I2 — [S6 decision] `edgion.io/managed-by` already exists as an ACME label

**Severity:** minor (S6-time naming decision; does not block earlier steps).

**What:** `edgion.io/managed-by` is already defined — as a **label** with value `"acme"`, applied to
ACME-managed Secrets and EdgionTls. Our planned ownership model (D-managed-by) wants `edgion.io/managed-by`
as an **annotation** with values `operator|controller|center`.

**Evidence:** `edgion-controller/src/services/acme/secret_io.rs:18,66-68`; `services/acme/service.rs:950,998`;
documented in `skills/02-features/05-annotations/00-annotations-overview.md` (labels section).

**Concern:** same key string, two field types (label vs annotation), two value vocabularies — confusing.
Conceptually both express "who owns this resource's lifecycle", so they could be unified.

**Options for S6 (decide then, not now):**
- (a) Reuse `edgion.io/managed-by` as a **label** consistently; extend the vocabulary to
  `operator|controller|center|acme`. Pro: one concept, label-selectable. Con: label charset limits (fine here).
- (b) Use a distinct annotation key (e.g. `edgion.io/data-source` or `edgion.io/origin`) to avoid the
  label/annotation clash. Pro: no overload. Con: two keys for a similar idea.

Recommendation leans (a) — same concept, keep one key — but confirm at S6.

---

## S1 Deviations (noted during execution)

### D-S1-A — `non_kube_common.rs` import missed in prior session
`edgion-controller/src/conf_mgr/conf_center/non_kube_common.rs` still imported `PluginMetadataHandler` instead
of `DataHandler` when `cargo check` was first run. This file expands `for_each_phase2_resource!` (which embeds
`DataHandler::new()`) but was not included in the rename sweep. Fixed by adding `DataHandler` and removing
`PluginMetadataHandler` from its resource-processor import block.

### D-S1-B — CRD shortName changed
CRD shortName changed from `pmd` to `edata` to match the new kind name (EdgionData). The task instructions did
not specify the old shortName, but `edata` is the natural fit and matches the plural `edgiondata`.

### D-S1-C — API response structs and handler functions renamed
The controller API layer (`api/region_route_handlers.rs`, `api/mod.rs`) exposed public struct names and
handler function names containing `PluginMetadata`. These were renamed to use `EdgionData` (e.g.
`ClusterRegionEdgionDataEntry`, `list_cluster_region_edgion_data`, `cluster_edgion_data_failover`) as part of
the S1 rename sweep. URL path strings exposed via the API were not changed — only Rust identifiers.

### D-S1-D — Internal controller struct names intentionally kept
`PluginMetaDataStore` (in `plugin_metadata_utils/plugin_metadata_store.rs`) and `PluginMetaDataRefManager`
(in `plugin_metadata_utils/ref_manager.rs`) are controller-internal and not part of the wire format or public
API. Their names were kept as-is; only their doc comments were updated to say "EdgionData". Renaming them is
a follow-up quality-of-life item, not a blocker.

---

## Resolved flags
- **F1 → D5** (no StringList/RegexList split). **F4 → D6** (keep `type`+`config`; type guards override).
- **F3 → resolved:** "managed" in `plugin-metadata-managed/` is a test-scenario naming for the
  admin-pre-created-PM pattern, not an ownership concept. No existing managed ownership logic on PM.
  (The only `managed-by` today is the ACME label — see I2.)
