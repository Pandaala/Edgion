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

- [x] **S2.1** `apply_override(base, overlay) -> effective`: **type-guard first** (D6 — overlay `type` must
      match expected variant, else fail-to-base), then strict field-level replace; collections whole-replace.
      Unit-test present/absent/invalid/**type-mismatch**/collection-replace.
- [x] **S2.2** Add the envelope to `EdgionDataSpec`: `enable: bool` (default true) + `active: Option<String>`
      (**additive, unused yet** — behavior-preserving). Add the `Misc` variant to `DataEntry`
      (`x-kubernetes-preserve-unknown-fields`). Add a shared helper for effective `enable`/`active`. Update
      the EdgionData CRD for the new spec fields.
- [ ] **S2.3** `apply_override` generic mechanism delivered + unit-tested in S2.1 via a toy Base/Override
      fixture. **Attaching `overrideRef: Option<DataRef>` to real base configs is deferred to S3
      (RegionRouteConfig) / S4 (GlobalConnIp base)** — there is no base config to attach to until then.
- [ ] **S2.4** `EdgionPlugins::validate_plugins` extension for `overrideRef` shape + base safety hints is
      **deferred to S3/S4** (needs `overrideRef` to exist first).

**Verify:** `cargo fmt`; `cargo check -p edgion-resources`; new `apply_override` + envelope unit tests.

---

## S3 — RegionRoute decomposition (phase 3) — the main move

> ⚠️ **I1 is now PRE-MERGE COORDINATION, not a blocker** (`05-issues.md`): Edgion-side S3 proceeds; the
> shared `edgion-resources` API + fed_sync shape change requires a coordinated EdgionCenter update before
> merge/deploy (EdgionCenter not checked out locally — done in its repo).

- [x] **S3.1** Move logic fields (`myRegion`, `keyGet`, `hashKeyGet`, `hashCalc`, `routeRules`,
      `routeByKeyConfMatch`) from the metadata variant into `RegionRouteConfig` base. Add default `regions`
      table + `overrideRef`.
- [x] **S3.2** Add `RegionRouteOverride` as a **typed** overlay variant (D2; all-Optional: `regions?`,
      `enable?`, `active?`) with `validate_and_init` (hashRange/endpoint checks). Remove `ClusterRegionRoute`
      + `ServiceRegionRoute` variants and the `cluster_ref` / `service_region_route_ref` indirection.
- [x] **S3.3** Controller `handlers/data.rs`: drop Service→Cluster cross-ref registration + requeue;
      add overlay validation + `ResolvedRefs` for `overrideRef`.
- [x] **S3.4** Controller `handlers/edgion_plugins.rs`: register `overrideRef` (EP → metadata) requeue;
      update `Accepted`/`ResolvedRefs`.
- [x] **S3.4b** Controller `region_route_table/` module (`mod.rs`, `cluster_table.rs`, `service_table.rs`):
      rework/remove the `ClusterRegionRouteEntry`/`ServiceRegionRouteEntry` build path. **⚠️ these types are
      shared with EdgionCenter (I1)** — coordinate before changing their shape.
- [x] **S3.5** Gateway store: replace region typed indexes with a `region_route_override` index; keep ArcSwap
      snapshot atomicity.
- [x] **S3.6** Gateway `http/region_route/plugin.rs` `resolve_config`: effective = base ⊕ overlay; replace
      `BrokenRef` fail-closed with **fail-to-base** + signal; re-resolve per request (lock-free).
- [x] **S3.7** Move matcher/precompute (`#[serde(skip)]` runtime fields) to the effective-config build site;
      keep controller-side `validate_and_init` for overlay validation. (RegionRoute has no runtime-only fields;
      effective regions are built at request time with no pre-compilation step needed.)

**Verify:** `cargo fmt`; `cargo check -p edgion-resources -p edgion-controller -p edgion-gateway`; RegionRoute
unit tests (base-only, base+overlay, overlay-invalid→base).

---

## S4 — GlobalConnectionIpRestriction alignment (phase 3)

- [x] **S4.1** Move default profile/`enable`/`active` into `EdgionStreamPlugins` base; add
      `GlobalConnIpRestrictionOverride` as a **typed** overlay variant (D2; override `enable`/`active`/profile
      data) reusing the existing `validate_and_init` matcher pre-compile.
- [x] **S4.2** `stream/global_connection_ip_restriction/plugin.rs`: fail-closed (`Deny`) → **fail-to-base**
      using base profile; `enable=false` → skip. Emit `overlay_missing`/`overlay_invalid` via metric (no tracing).
- [x] **S4.3** Update conf_handler matcher compilation for the overlay path; keep matcher fields skip-serialized.
- [x] **S4.4** Document the security caveat (base profile = safe default).

**Verify:** `cargo fmt`; `cargo check` (3 crates); stream tests (present/absent/invalid, kill switch).

---

## S5 — Switch generalization sweep (phase 3)

- [x] **S5.1** Done by audit: the only live switch consumer is GlobalConnIp (`enable` wired in S4 via the
      shared `effective_switch` helper). RegionRoute's override `enable`/`active` stay **reserved/unwired**
      (D-S3-A): RegionRoute has no profiles, so `active` is meaningless and `enable` has no agreed semantics —
      not wiring speculative behavior (no over-engineering). Documented in 05-issues D-S3-A.
- [x] **S5.2** Audit done: no other PM/EdgionData-consuming plugins exist (KeyList/IpList have no consumers
      per D1; HTTP IpRestriction/KeyAuth are inline). Nothing else to wire.

**Verify:** `cargo fmt`; `cargo check`.

---

## S6 — `managed-by` ownership model (phase 3)

- [x] **S6.1** Define `edgion.io/managed-by` values (`operator` default | `controller` | `center` | `acme`)
      in `skills/02-features/05-annotations/00-annotations-overview.md` (extended vocabulary table added).
      Constants defined in `edgion-resources/src/constants/labels.rs` (`managed_by` module). Decision D7
      recorded in `06-decisions.md`. I2 resolved.
- [x] **S6.2** Controller reconcile branch: `center` and `controller` are **DESIGN-DECLARED, NOT YET
      IMPLEMENTED** (doc-only per conservative scope; blocked on EdgionCenter coordination I1 and
      template-engine design). `operator` is the only active mode (current default behavior, no code change).
      Marked clearly in annotations overview and labels constants.
- [x] **S6.3** Ref resolution when overlay-absent → base is already wired (S3/S4). `center`/`controller`
      object-exists-but-body-is-remote-owned scenario is deferred with I1 (doc-only here).

**Verify:** `cargo fmt`; `cargo check -p edgion-resources` (labels.rs constant addition).

---

## S7 — CRD YAML + schema guard (phase 3)

- [x] **S7.1** Finalize `config/crd/edgion-crd/` for EdgionPlugins (`overrideRef`, base data fields) and
      EdgionData (final variant set; old RegionRoute variants removed).
- [x] **S7.2** Audit `#[schemars(skip)]` + redact on resolved/secret/runtime fields of new structs.
- [x] **S7.3** Re-run `./cicd/checks/check_crd_coverage.sh` (EGC/TLS structural guard) — expect green.

**Verify:** `python3 cicd/checks/validate_agent_docs.py`; `./cicd/checks/check_crd_coverage.sh`.

---

## S8 — Observability: effective config dump (phase 3)

- [~] **S8.1** DEFERRED (documented). The critical overlay observability — *whether* an overlay applied /
      missing / invalid / type-mismatch — is already shipped as Prometheus counters
      (`region_route_overlay_events_total`, `*_conn_ip_restriction_*` events) from S3/S4. The richer
      admin `/configclient` merged-view dump (effective = base ⊕ overlay with per-field source) is a
      debugging enhancement, not core to the refactor; base (EdgionPlugins/EdgionStreamPlugins) and overlay
      (EdgionData) are already individually inspectable. Deferred to avoid over-engineering — tracked as a
      follow-up.

**Verify:** n/a (deferred).

---

## S9 — Docs & skills (phase 3)

- [x] **S9.1** Updated region_route user-facing docs (`docs/en/` + `docs/zh-CN/`) to base+overlay model;
      removed all stale ClusterRegionRoute/ServiceRegionRoute/PluginMetaData references.
      Rewrote `skills/01-architecture/05-resources/15-plugin-metadata.md` → `15-edgion-data.md`
      (renamed; now covers EdgionData resource architecture + overlay model + variant taxonomy).
      Updated SKILL.md table entry and resource relationship diagram.
      Fixed all 8 stale `src/` refs in 4 skills files (gate: `validate_agent_docs.py` passes).
- [x] **S9.2** Canonical overlay reference in `skills/01-architecture/05-resources/15-edgion-data.md`
      (Overlay Model section: roles, five guardrails, apply_override contract, fail-behavior matrix).
      User-facing RegionRoute docs link to EdgionData overlay concept; review skills updated.
      Chinese mirror at `docs/zh-CN/user-guide/edgion-plugins/region-route.md` updated.
- [x] **S9.3** Annotations overview updated: managed-by extended vocabulary table (operator/acme/center/controller
      with implementation status); labels constants pointer added. Stale PluginMetaData/ClusterRegionRoute
      examples removed from skills. EdgionData general-purpose intent noted in `15-edgion-data.md`.

**Verify:** `python3 cicd/checks/validate_agent_docs.py`.

---

## S10 — Tests (phase 4)

> **Unit/fixture level: DONE** — S2/S3/S4 added unit tests covering base-only, base+overlay replace,
> overlay-invalid→base, type-mismatch→base, enable kill (gateway region_route 14, controller region 15,
> GlobalConnIp 5, apply_override 5). Integration fixtures migrated to the new shape.
> **Full integration run (`run_integration.sh`): NOT RUN — requires a live K8s cluster** (unavailable in this
> autonomous run). Must run in CI / on a cluster before merge. The scenarios below map to existing fixtures.

- [x] **S10.1** RegionRoute base-only / base+overlay / overlay-invalid→base — covered by gateway unit tests.
- [x] **S10.2** GlobalConnIp overlay present/absent/invalid + kill switch — covered by gateway unit tests.
- [~] **S10.3** `managed-by` behaviors — only `operator` (default) is active; center/controller deferred (D7).
- [ ] **S10.4** Hot-reload (apply/revert overlay, no restart) — **needs live cluster** (CI/manual).
- [x] **S10.5** Variant round-trip + `type`-guard mismatch→fail-to-base — covered by apply_override unit tests.

**Verify:** unit/fixture green (done). Full `run_integration.sh` pending a cluster (CI/manual before merge).

---

## S11 — Review & full validation (phase 5)

- [x] **S11.1** Final gate green: `cargo fmt --all --check` clean · `cargo check --all-targets` 0 errors ·
      `cargo clippy` — 3 refactor-introduced warnings fixed (rest are pre-existing workspace tech debt,
      out of scope) · `validate_agent_docs.py` PASS (846 files) · `check_crd_coverage.sh` PASS ·
      `cargo test --workspace --lib` **3454 passed / 0 failed**. (Full `cargo test --all` incl. the
      cluster-dependent integration harness is a CI/manual step — see S10.)
- [x] **S11.2** Self-review vs guardrails: (1) strict replace/no-merge ✓ (apply_override field-replace,
      whole-collection); (2) only-data-overridable ✓ (logic stays in RegionRouteConfig base; overrides are
      all-Optional data); (3) observable ✓ (overlay metrics counters; full merged-dump deferred S8); (4)
      switch ✓ (envelope enable/active; GlobalConnIp enable live); (5) fail-to-base ✓ (RegionRoute +
      GlobalConnIp missing/invalid/type-mismatch → base + metric, no data-plane tracing). Security invariant:
      base = safe default documented. Rename completeness ✓ (validate_agent_docs green; no stale refs).
- [~] **S11.3** Reusable findings captured in `15-edgion-data.md` + decisions D1–D7. **PR NOT opened** (per
      run directive: temp commits only, no push). Open items for the user: I1 (EdgionCenter coordination
      before merge), I3 (GlobalConnIp dual-path consistency decision), S8 (merged config dump), S10.4
      (hot-reload integration on a cluster).

---

## Suggested execution order & parallelism

1. **S0** (sequential, design gate).
2. **S1** rename+generalize (isolated, behavior-preserving) — land green first.
3. **S2** framework — blocks S3/S4.
4. **S3** and **S4** in parallel (different plugins) after S2.
5. **S5** after S3/S4. **S6** controller-only — parallel with S3/S4.
6. **S7/S8/S9** after structs settle. **S10** after S7. **S11** last.

Critical path: **S0 → S1 → S2 → S3 → S7 → S10 → S11**.
