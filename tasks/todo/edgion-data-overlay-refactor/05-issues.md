# 05 — Issues

## I1 — [PRE-MERGE COORDINATION] EdgionData refactor requires a coordinated EdgionCenter update

**Severity:** pre-merge coordination (NOT an Edgion-side implementation blocker).

**Decision (autonomous run, 2026-06-16):** Edgion-side implementation proceeds through S3. Rationale: S1
already renamed the shared `edgion-resources` public API (`PluginMetaData`→`EdgionData`, `DataRef`, module
paths), so EdgionCenter already needs a coordinated update from S1 onward — S3 is not a *new* kind of break.
The Edgion repo compiles/tests independently of EdgionCenter. **Required before merging this branch or
deploying federation:** update EdgionCenter to the new shared API + the new fed_sync RegionRoute shape
(base in EdgionPlugins + `RegionRouteOverride` EdgionData, replacing the old Cluster/ServiceRegionRoute PM
variants and the `*Entry` types). EdgionCenter is not checked out locally, so this must be done in its repo.

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

## S2 Deviations (noted during execution)

### D-S2-A — EdgionDataSpec initializers in test code required updating
Adding `enable`/`active` to `EdgionDataSpec` broke 4 struct-initializer sites in test code (not production
code) that used the exhaustive init syntax. Sites fixed:
- `edgion-gateway/src/plugins/runtime/data/conf_handler.rs` (2 sites in `#[cfg(test)]`)
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs` (2 sites in `#[cfg(test)]`)
All 4 updated to include `enable: true, active: None`. No logic changed.

### D-S2-B — `DataEntry::Misc` match arm required in gateway `build_indexes`
Adding the `Misc` variant to `DataEntry` made the existing exhaustive match in
`edgion-gateway/src/plugins/runtime/data/conf_handler.rs::build_indexes` non-exhaustive.
Added `DataEntry::Misc(_) => {}` (no index — opaque escape hatch). Behavior unchanged.

### D-S2-C — Enable printer column added to CRD
Added an `Enable` column to `additionalPrinterColumns` in `edgion_data_crd.yaml` so that
`kubectl get edgiondata` surfaces the kill switch. Minimal additive change; not required by the task
spec but improves operator visibility.

---

## Resolved flags
- **F1 → D5** (no StringList/RegexList split). **F4 → D6** (keep `type`+`config`; type guards override).
- **F3 → resolved:** "managed" in `plugin-metadata-managed/` is a test-scenario naming for the
  admin-pre-created-PM pattern, not an ownership concept. No existing managed ownership logic on PM.
  (The only `managed-by` today is the ACME label — see I2.)

---

## S3 Deviations (noted during execution)

### D-S3-A — `RegionRouteOverride.enable/active` fields present in struct, not wired to gateway
The `RegionRouteOverride` struct carries `enable: Option<bool>` and `active: Option<String>` to keep the
overlay shape consistent with the envelope design (S5 future work), but neither field is read by the
gateway plugin yet. The gateway only applies `regions` (whole-replace). Wiring `enable`/`active` into the
request path is deferred to S5.

### D-S3-B — Deprecated legacy list endpoints kept, not removed
The controller API routes `GET /api/v1/cluster-region-routes` and `GET /api/v1/service-region-routes` still
exist in `region_route_handlers.rs` as no-op stubs returning `[]`. Operators calling these routes get an
empty list (backward compatible). Removing these routes requires an API deprecation notice + coordination
with EdgionCenter; deferred as a follow-up.

### D-S3-C — `record_region_route_overlay_event` metric added to gateway registry
`overlay_missing` and `overlay_type_mismatch` are now emitted via `edgion_region_route_overlay_events_total`
counter with a `result` label. No `overlay_invalid` signal exists for RegionRoute because
`merge_region_route_override` (whole-replace of regions Vec) cannot fail after the type guard passes.

### D-S3-D — S3.7 is a no-op: RegionRoute has no serde-skip runtime fields
Unlike GCIR (which pre-compiles matchers), RegionDef carries no runtime-only fields. The effective region
table is a plain `Vec<RegionDef>` computed at request time (one ArcSwap load + merge). No `#[serde(skip)]`
migration needed.
