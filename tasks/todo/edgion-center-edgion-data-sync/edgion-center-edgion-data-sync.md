# EdgionCenter — adopt the EdgionData refactor

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-06-16 |
| Status | pending |
| Type | refactor (cross-repo coordination) |
| Priority | P0 — blocks merging `refactor/edgion-data-overlay` branch and any federation deploy |
| Issue | follow-on of `tasks/todo/edgion-data-overlay-refactor` (I1) |

## AI guide summary

> The Edgion repo's `refactor/edgion-data-overlay` branch renames `PluginMetaData` → `EdgionData`, removes
> the `ClusterRegionRoute`/`ServiceRegionRoute` PluginMetaData variants + the `ClusterRegionRouteEntry`/
> `ServiceRegionRouteEntry` types in `edgion-resources`, and adopts a **base + typed overlay** model
> (`RegionRouteOverride`, `GlobalConnIpRestrictionOverride`). EdgionCenter (separate repo, federation
> control plane) depends on the shared `edgion-resources` crate and on the `fed_sync` proto. This task is
> EdgionCenter-side adoption: compile cleanly against the new `edgion-resources`, watch the new `kind:
> EdgionData`, accept the new RegionRoute fed_sync shape (base lives inside `EdgionPlugins`, dynamic
> failover/regions ride a `RegionRouteOverride` EdgionData), and update any aggregated-store / Center UI
> that still assumes the old PM variant chain.

## Why this is its own task
- The Edgion repo branch is complete (10 commits, fmt/check/clippy/docs/CRD/coverage/tests green, 3454 lib
  tests pass) and self-consistent locally. It MUST NOT be merged before EdgionCenter is updated, since both
  consume `edgion-resources` and exchange the fed_sync shape — but the work itself happens in the
  EdgionCenter repo, not here.
- Keeping this as a separate task lets the Edgion branch be reviewed/merged independently once EdgionCenter
  PR is ready, and prevents the Edgion refactor's plan/checklists from getting tangled with cross-repo
  coordination state.

## Hard prerequisites (do these before any coding)
1. Locate / clone the EdgionCenter repository (not present under `/Volumes/ExtStore/ws4/`).
2. Confirm how EdgionCenter pulls `edgion-resources`: workspace path, git dep, or submodule. The current
   `Cargo.toml` comment in this repo notes: "Shared resource/config schema crate (leaf; reused by the
   future EdgionCenter)" — verify the exact dependency form.
3. Confirm how EdgionCenter pulls the `fed_sync.proto` (likely a copy/submodule of
   `edgion-controller/src/fed_sync/proto/fed_sync.proto`). The proto string field
   `FedWatchRequest.kind` (line 9) is already documented as `"EdgionData"` in this repo, but Center may have
   hard-coded the old `"PluginMetaData"` string.

## Scope

### In scope (EdgionCenter side)
Items derived from the Edgion-branch changes (see `tasks/todo/edgion-data-overlay-refactor/02-inventory.md`
"Authoritative rename map" for the full surface):

- **Rename adoption** — all `PluginMetaData`/`PluginMetaDataSpec`/`PluginMetaDataStatus`/`MetaDataEntry`/
  `PluginMetaDataRef`/`PluginReference` → `EdgionData`/`EdgionDataSpec`/`EdgionDataStatus`/`DataEntry`/
  `DataRef`. Module path `edgion-resources/.../plugin_metadata/` → `.../data/`.
- **fed_sync watch kind** — wherever Center watches/subscribes by kind string, use `"EdgionData"`
  (`fed_sync.proto:9` comment).
- **RegionRoute aggregated store** — the old `ClusterRegionRouteEntry`/`ServiceRegionRouteEntry` types and
  the Cluster↔Service `cluster_ref` chain are **gone**. The new shape:
  - Base config (logic + default region table) lives inside `EdgionPlugins.RegionRouteConfig` (already
    visible to Center via the EdgionPlugins resource). New field: `override_ref: Option<DataRef>`.
  - Dynamic overlay = `DataEntry::RegionRouteOverride(RegionRouteOverride)` with all-Optional
    `regions?`/`enable?`/`active?`. `regions` is **whole-table replace**, not per-element merge.
  - Center needs to (a) drop the old entry types & chain code, (b) build whatever aggregated view it needs
    from `EdgionPlugins.RegionRouteConfig` + the `RegionRouteOverride` EdgionData it pushes/aggregates.
- **GlobalConnIp** — old standalone `DataEntry::GlobalConnectionIpRestriction` variant **removed**. Base
  lives inline in `EdgionStreamPlugins` `GlobalConnectionIpRestrictionEntry.base`; dynamic overlay =
  `GlobalConnIpRestrictionOverride` EdgionData. Center updates analogously.
- **Final `DataEntry` taxonomy** Center must handle: `KeyList`, `IpList`, `GlobalConnIpRestrictionOverride`,
  `RegionRouteOverride`, `Misc` (escape hatch). No `ClusterRegionRoute`/`ServiceRegionRoute`/
  `GlobalConnectionIpRestriction` enum variants exist.
- **Envelope** — `EdgionDataSpec` now also has `enable: bool` (default true) + `active: Option<String>`.
- **CRD** — Center may install/manage the EdgionData CRD; ensure it points at the new
  `config/crd/edgion-crd/edgion_data_crd.yaml` (kind `EdgionData`, plural `edgiondata`, shortName
  `edata`). The old `plugin_metadata_crd.yaml` no longer exists.
- **`edgion.io/managed-by` vocabulary** — extended (operator|acme|center|controller); `center` is the
  EdgionCenter-relevant value. Implementation here optional but the constants now exist in
  `edgion-resources/src/constants/labels.rs::managed_by`.

### Out of scope
- Any new EdgionCenter feature.
- Reintroducing or shimming the removed PM variants in Edgion — the Edgion change is intentional.

## Coordination plan

1. Branch in EdgionCenter, pin `edgion-resources` to the Edgion `refactor/edgion-data-overlay` branch
   commit (or local path) to compile against the new API.
2. Resolve all build errors per the rename map; rework the RegionRoute aggregated store.
3. Confirm fed_sync watch/apply round-trip works against an Edgion controller built from
   `refactor/edgion-data-overlay`.
4. Merge order: EdgionCenter PR review + green CI → merge Edgion refactor branch → bump EdgionCenter to the
   merged `edgion-resources` revision → merge EdgionCenter.

## References (this repo)

- Plan/decisions/inventory:
  - `tasks/todo/edgion-data-overlay-refactor/edgion-data-overlay-refactor.md` (Resume + model)
  - `tasks/todo/edgion-data-overlay-refactor/01-design.md` (target model, fail-to-base matrix, taxonomy)
  - `tasks/todo/edgion-data-overlay-refactor/02-inventory.md` (rename map + consumer/runtime anchors)
  - `tasks/todo/edgion-data-overlay-refactor/06-decisions.md` (D1–D7)
- Edgion branch commits: `git log refactor/edgion-data-overlay` — 10 commits, see in particular
  `feat(S1)` (rename), `feat(S2)` (overlay framework), `feat(S3)` (RegionRoute decomposition),
  `feat(S4)` (GlobalConnIp base+overlay), `fix(I3)` (standalone GlobalConnIp variant removal).
- New shared types in `edgion-resources/src/resources/data/`: `EdgionData`, `EdgionDataSpec`,
  `EdgionDataStatus`, `DataEntry`, `DataRef`, `GlobalConnIpRestrictionOverride` + `merge_gcir_override`,
  `RegionRouteOverride` + `merge_region_route_override`, `apply_override` + `OverlaySignal`.
- fed_sync proto: `edgion-controller/src/fed_sync/proto/fed_sync.proto`.
