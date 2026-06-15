# 03 — Subtasks (ordered step-by-step todo)

Status legend: `[ ]` pending · `[~]` in-progress · `[x]` done · `[!]` blocked.
Verify with `cargo fmt` + `cargo check` + targeted test only (no clippy/build in subagents; the integration
script runs clippy/build later).

---

## S0 — Analysis & design lock (phase 1–2)

- [ ] **S0.1** Inventory every `PluginMetaData` reference (CRD kind, struct/module/store names, refs, docs,
      CRD YAML, examples) — this is the rename surface. Confirm live consumers of `IpList` / `KeyList`
      (exploration suggests HTTP IpRestriction/KeyAuth are inline). Output a who-references-what table.
- [x] **S0.2** Decisions resolved — see `06-decisions.md`: D1 keep IpList/KeyList as primitives + delete
      orphan plumbing; D2 first-party overlays are typed (Misc = escape hatch only); D3 envelope-only switch
      (no `Switch` variant); D4 hard rename.
- [ ] **S0.3** Lock `apply_override` signature + override-shape convention (all-Optional; first-party overlays typed).
- [x] **S0.4** Migration: hard rename, no alias (D4). CRD baseline: confirm
      `./cicd/checks/check_crd_coverage.sh` green before changes.

**Gate:** disposition table + fail matrix + rename surface agreed. Deliverable: updated `01-design.md`, `06-decisions.md`.

---

## S1 — Rename `PluginMetaData` → `EdgionData` + generalize variants (phase 3)

> Do the mechanical rename first, as an isolated, behavior-preserving change, to shrink later diffs.

- [x] **S1.1** Rename module `edgion-resources/src/resources/plugin_metadata/` → `.../data/`; struct
      `PluginMetaDataSpec` → `EdgionDataSpec`; `#[kube(kind = "PluginMetaData")]` → `"EdgionData"`.
- [x] **S1.2** Rename refs: `PluginMetaDataRef` / `PluginReference` → `DataRef`; consumer fields → generic
      `dataRef`. Rename gateway store `PluginMetaDataGatewayStore` → `EdgionDataStore` and module
      `plugins/runtime/plugin_metadata/` → `.../data/`.
- [x] **S1.3** Rename controller handler `handlers/plugin_metadata.rs` → `handlers/data.rs` and all
      identifiers; keep behavior identical.
- [x] **S1.4** Rename-only on the variant enum (`MetaDataEntry` → `DataEntry`): keep the current 5 variants
      renamed in place (`KeyList`/`IpList`/`ClusterRegionRoute`/`ServiceRegionRoute`/`GlobalConnectionIpRestriction`),
      keep wire shape `data: { type, config }` (D6). **No** schema/behavior change here — envelope
      (`enable`/`active`), `Misc`, and `overrideRef` land in **S2**; RegionRoute split in S3. Never add
      `StringList`/`RegexList`/`Switch` (D5/D3).
- [x] **S1.5** Remove orphaned `IpList`/`KeyList` gateway indexes + getters (D1, dead code) during the rename.
- [x] **S1.6** CRD YAML kind rename + new variants (`config/crd/edgion-crd/`); update the 22 test YAMLs.

**Verify:** `cargo fmt --all -- --check`; `cargo check --all-targets`; existing metadata tests pass under new
names (rename should be green before any behavior change).

---

## S2 — Overlay framework in `edgion-resources` (phase 3)

- [ ] **S2.1** `apply_override(base, overlay) -> effective`: **type-guard first** (D6 — overlay `type` must
      match expected variant, else fail-to-base), then strict field-level replace; collections whole-replace.
      Unit-test present/absent/invalid/**type-mismatch**/collection-replace.
- [ ] **S2.2** Shared switch helpers: effective `enable`/`active` across consumers.
- [ ] **S2.3** Add `overrideRef: Option<DataRef>` to opt-in plugin base configs.
- [ ] **S2.4** Extend `EdgionPlugins::validate_plugins` (preparse) to validate `overrideRef` shape + base
      safety hints (warn on permissive IP base).

**Verify:** `cargo fmt`; `cargo check -p edgion-resources`; new `apply_override` unit tests.

---

## S3 — RegionRoute decomposition (phase 3) — the main move

> ⚠️ **BLOCKED by I1** (`05-issues.md`): RegionRoute types are shared with EdgionCenter (separate repo).
> Resolve the cross-repo contract before starting S3. S1/S2 are unaffected.

- [ ] **S3.1** Move logic fields (`myRegion`, `keyGet`, `hashKeyGet`, `hashCalc`, `routeRules`,
      `routeByKeyConfMatch`) from the metadata variant into `RegionRouteConfig` base. Add default `regions`
      table + `overrideRef`.
- [ ] **S3.2** Add `RegionRouteOverride` as a **typed** overlay variant (D2; all-Optional: `regions?`,
      `enable?`, `active?`) with `validate_and_init` (hashRange/endpoint checks). Remove `ClusterRegionRoute`
      + `ServiceRegionRoute` variants and the `cluster_ref` / `service_region_route_ref` indirection.
- [ ] **S3.3** Controller `handlers/data.rs`: drop Service→Cluster cross-ref registration + requeue;
      add overlay validation + `ResolvedRefs` for `overrideRef`.
- [ ] **S3.4** Controller `handlers/edgion_plugins.rs`: register `overrideRef` (EP → metadata) requeue;
      update `Accepted`/`ResolvedRefs`.
- [ ] **S3.4b** Controller `region_route_table/` module (`mod.rs`, `cluster_table.rs`, `service_table.rs`):
      rework/remove the `ClusterRegionRouteEntry`/`ServiceRegionRouteEntry` build path. **⚠️ these types are
      shared with EdgionCenter (I1)** — coordinate before changing their shape.
- [ ] **S3.5** Gateway store: replace region typed indexes with a `region_route_override` index; keep ArcSwap
      snapshot atomicity.
- [ ] **S3.6** Gateway `http/region_route/plugin.rs` `resolve_config`: effective = base ⊕ overlay; replace
      `BrokenRef` fail-closed with **fail-to-base** + signal; re-resolve per request (lock-free).
- [ ] **S3.7** Move matcher/precompute (`#[serde(skip)]` runtime fields) to the effective-config build site;
      keep controller-side `validate_and_init` for overlay validation.

**Verify:** `cargo fmt`; `cargo check -p edgion-resources -p edgion-controller -p edgion-gateway`; RegionRoute
unit tests (base-only, base+overlay, overlay-invalid→base).

---

## S4 — GlobalConnectionIpRestriction alignment (phase 3)

- [ ] **S4.1** Move default profile/`enable`/`active` into `EdgionStreamPlugins` base; add
      `GlobalConnIpRestrictionOverride` as a **typed** overlay variant (D2; override `enable`/`active`/profile
      data) reusing the existing `validate_and_init` matcher pre-compile.
- [ ] **S4.2** `stream/global_connection_ip_restriction/plugin.rs`: fail-closed (`Deny`) → **fail-to-base**
      using base profile; `enable=false` → skip. Emit `overlay_missing`/`overlay_invalid` via metric (no tracing).
- [ ] **S4.3** Update conf_handler matcher compilation for the overlay path; keep matcher fields skip-serialized.
- [ ] **S4.4** Document the security caveat (base profile = safe default).

**Verify:** `cargo fmt`; `cargo check` (3 crates); stream tests (present/absent/invalid, kill switch).

---

## S5 — Switch generalization sweep (phase 3)

- [ ] **S5.1** Uniform `enable`/`active` semantics across opt-in consumers (single helper).
- [ ] **S5.2** Audit other candidate plugins for the same switch (consistency pass).

**Verify:** `cargo fmt`; `cargo check`.

---

## S6 — `managed-by` ownership model (phase 3)

- [ ] **S6.1** Define `edgion.io/managed-by` values (`operator` default | `controller` | `center`) in
      `skills/02-features/05-annotations/00-annotations-overview.md`.
- [ ] **S6.2** Controller reconcile branch: `operator` normal; `center` → do not overwrite spec from git
      (placeholder, accept conf_sync/fed_sync data); `controller` → regenerate body from template ref.
- [ ] **S6.3** Ensure refs resolve when object exists but body is remote-owned (overlay-absent → base).

**Verify:** `cargo fmt`; `cargo check -p edgion-controller`; controller test per branch.

---

## S7 — CRD YAML + schema guard (phase 3)

- [ ] **S7.1** Finalize `config/crd/edgion-crd/` for EdgionPlugins (`overrideRef`, base data fields) and
      EdgionData (final variant set; old RegionRoute variants removed).
- [ ] **S7.2** Audit `#[schemars(skip)]` + redact on resolved/secret/runtime fields of new structs.
- [ ] **S7.3** Re-run `./cicd/checks/check_crd_coverage.sh` (EGC/TLS structural guard) — expect green.

**Verify:** `python3 cicd/checks/validate_agent_docs.py`; `./cicd/checks/check_crd_coverage.sh`.

---

## S8 — Observability: effective config dump (phase 3)

- [ ] **S8.1** Admin `/configclient` shows effective = base ⊕ overlay, per-field source (base | overlay) +
      overlay status (applied | absent | invalid→base).

**Verify:** `cargo check`; manual dump inspection.

---

## S9 — Docs & skills (phase 3)

- [ ] **S9.1** Update region_route + EdgionData docs to the overlay model + EdgionData rename.
- [ ] **S9.2** Add an overlay-model + fail-to-base + variant-taxonomy reference doc (one canonical source;
      link from `skills/` and `docs/en/`; Chinese mirror under `docs/zh-CN/` if user-facing).
- [ ] **S9.3** Update annotations overview for `managed-by`. Remove stale ClusterRegionRoute/ServiceRegionRoute
      and `PluginMetaData` examples. Note EdgionData's general-purpose intent (EdgionTls future consumer).

**Verify:** `python3 cicd/checks/validate_agent_docs.py`.

---

## S10 — Tests (phase 4)

- [ ] **S10.1** RegionRoute: base-only; base+overlay replace; overlay invalid → base; `active` flip; `enable` kill.
- [ ] **S10.2** GlobalConnIpRestriction overlay (present/absent/invalid; kill switch).
- [ ] **S10.3** `managed-by` behaviors (operator vs center placeholder).
- [ ] **S10.4** Hot-reload: apply overlay metadata, gateway picks up without restart; revert → base.
- [ ] **S10.5** Variant smoke: `KeyList`/`IpList` + typed overlays + `Misc` round-trip; `type`-guard mismatch
      → fail-to-base.

**Verify:** `./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r <Resource> -i <Item>`,
then a full run.

---

## S11 — Review & full validation (phase 5)

- [ ] **S11.1** `cargo fmt --all` · `cargo check --all-targets` · `cargo clippy --all-targets` ·
      `python3 cicd/checks/validate_agent_docs.py` · `cargo test --all`.
- [ ] **S11.2** Self-review vs the five overlay guardrails + security invariant + rename completeness.
- [ ] **S11.3** Capture reusable findings into `skills/04-review/`; open PR.

---

## Suggested execution order & parallelism

1. **S0** (sequential, design gate).
2. **S1** rename+generalize (isolated, behavior-preserving) — land green first.
3. **S2** framework — blocks S3/S4.
4. **S3** and **S4** in parallel (different plugins) after S2.
5. **S5** after S3/S4. **S6** controller-only — parallel with S3/S4.
6. **S7/S8/S9** after structs settle. **S10** after S7. **S11** last.

Critical path: **S0 → S1 → S2 → S3 → S7 → S10 → S11**.
