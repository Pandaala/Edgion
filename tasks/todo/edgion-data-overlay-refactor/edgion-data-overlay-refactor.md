# EdgionData Overlay Refactor

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-06-15 |
| Status | pending |
| Type | refactor |
| Priority | P1 |
| Issue | N/A |

## How to resume (read in this order)

A fresh session has full context by reading: **this file** (meta + locked model + decisions) →
`06-decisions.md` (D1–D6, the locked choices and *why*) → `02-inventory.md` (source-verified current state,
rename map, code anchors) → `01-design.md` (target model, taxonomy, `apply_override` contract, fail matrix) →
`05-issues.md` (I1 blocks S3, I2 is an S6 decision) → `03-subtasks.md` (execute S0→S11; S0 done).
`00-context.md` lists the skill paths to load per phase. **Start work at S1** (S1/S2 are unblocked; S3 waits on I1).

## Background & external references (why this architecture)

The model is grounded in how mature control planes reconcile GitOps (declarative apply) with runtime
dynamism — the core rule is **one field, one owner; no deep-merge**:
- **Envoy ECDS** (Extension Config Discovery Service) — a resource references an extension by *name*; the
  config is delivered/swapped dynamically without touching the referrer. → our base `overrideRef` → EdgionData.
- **Argo Rollouts + K8s Server-Side Apply field ownership** — git owns structure, a controller owns specific
  mutable fields (weights); ArgoCD `ignoreDifferences` avoids fights. → the HTTPRoute-weight follow-on.
- **APISIX one-way sync** — pick one source of truth per resource; never bidirectionally merge.
- **Istio EnvoyFilter MERGE bug** (array-append instead of replace) — the cautionary tale behind guardrail #1
  (strict field-level *replace*, never deep-merge).

## AI guide summary

> Two coupled goals. **(1) Overlay model:** `EdgionPlugins` / `EdgionStreamPlugins` hold the complete, safe
> *base* config (logic + default data); a metadata resource becomes a typed, all-Optional *overlay* that
> overrides only the dynamic data subset. One overlay ref can override multiple fields; overlay errors degrade
> to base. **(2) Generalize + rename:** the old `PluginMetaData` is renamed **`EdgionData`** and is no
> longer plugin-only — it is a general-purpose, hot-swappable metadata/overlay primitive that other resources
> (e.g. `EdgionTls`) can consume later. Its body is no longer "just a list": typed standard primitives
> (`KeyList` / `IpList`) + typed first-party overlays (`RegionRouteOverride`,
> `GlobalConnIpRestrictionOverride`) + a `Misc` escape hatch (third-party/unknown only); `enable`/`active` on
> the envelope. Heavy logic (RegionRoute topology + route rules) moves out of metadata into the plugin.
>
> Core difficulty: the rename touches a CRD kind + every reference; decomposing RegionRoute (logic vs data);
> strict overlay/replace semantics without deep-merge traps; flipping fail-closed → fail-to-base safely.

## Requirements

(From the design discussion, verbatim intent)

1. Move metadata's **complex/logic config** into `EdgionPlugins` / `EdgionStreamPlugins`. Complexity belongs to the plugin.
2. **Simplify metadata** and add a **switch** capability (generalize `enable` + `active` instant-swap).
3. Metadata acts as an **overlay/override** on top of the plugin **base**: one ref can override many fields.
   `base` is the default; **when the overlay errors, fall back to base defaults.**
4. **Generalize the body**: metadata is no longer fixed to a list. Typed standard primitives
   (`KeyList` / `IpList`) and typed first-party overlays are first-class; a
   **`Misc`** variant is the escape hatch for third-party/unknown payloads. `enable`/`active` live on the
   envelope (no standalone `Switch` variant — D3).
5. **Rename `PluginMetaData` → `EdgionData`**. It is no longer limited to plugins; other resources such
   as `EdgionTls` may consume it later. References become a generic `dataRef`.
6. Continue into the ownership model (`managed-by`: operator / controller / center) and HTTPRoute weight ownership.

## Design model (locked)

**Roles**
- `EdgionPlugins` / `EdgionStreamPlugins` (and later other resources) = **base**, git/operator-owned,
  *complete + safe default* config: decision logic + default data + default `enable`/`active`. Opt-in
  `overrideRef` (generic `dataRef`) → EdgionData.
- `EdgionData` = general-purpose **typed overlay / data primitive**, hot-swappable, origin-agnostic
  (later: `managed-by`). Body variants: typed primitives (`KeyList` unified string+regex / `IpList`) +
  typed first-party overlays (`RegionRouteOverride`/`GlobalConnIpRestrictionOverride`) + `Misc` escape hatch.
  Wire shape `data: { type, config }`; `type` guards override binding (D6).
  Top-level envelope: `enable` (+ optional `active`).
- `HTTPRoute` upstream `weight` = **field ownership** (K8s Server-Side Apply), separate follow-on track.

**Five overlay guardrails** (unchanged by the rename)
1. **Strict replace, never deep-merge.** Field present → replaces base; collection → whole-table replace.
2. **Only data is overridable; logic never is.** Opt-in plugin defines a dedicated all-Optional override
   shape (typed, or carried as a `Misc` payload); logic fields are physically excluded.
3. **Effective config is observable** (admin dump shows base+overlay + per-field source).
4. **Switch unification.** base may hold `active` + preloaded `profiles` (instant flip); overlay may override
   `active`/`enable` (flip) or data (push new). `enable` is the top kill switch.
5. **Fail-to-base.** base = complete safe default. overlay absent → base. overlay invalid → base + error
   signal (controller `Accepted=False` + not pushed; gateway metric/access-log, no `tracing::error!`).

> Security note: `base` must be authored as the conservative/safe value, so fail-to-base never widens access.

## Scope

### In scope
- **Rename** `PluginMetaData` → `EdgionData` (CRD kind, struct/module/store names, all references,
  generic `dataRef`). Hard rename (no compat alias) — see decision log.
- **Generalize variants**: keep typed `KeyList` (unified string+regex) / `IpList` + add typed first-party
  overlays + `Misc` escape hatch (D2/D3/D5). No new `StringList`/`RegexList`/`Switch`.
- Overlay framework in `edgion-resources` (override shapes + `apply_override` + `overrideRef`).
- `RegionRoute` decomposition (logic → plugin base; region table → overlay; drop Service→Cluster chain).
- `GlobalConnectionIpRestriction` re-expressed as overlay; fail-closed → fail-to-base.
- Switch generalization; `managed-by` ownership model; CRD YAML; controller validation; gateway
  store/resolution; observability; docs; integration tests.

### Out of scope (tracked as follow-on)
- Wiring `EdgionTls` (or other resources) to actually consume EdgionData — only keep the design
  decoupled/ready; do not implement consumers beyond the existing plugins.
- HTTPRoute upstream `weight` SSA field ownership (separate task; referenced in design only).
- Center-side push implementation (this task only makes EdgionData origin-agnostic + ready to receive).

## Document index

| Document | Status | Notes |
|----------|--------|-------|
| 00-context.md | done | Related skills index + code anchors |
| 01-design.md | done | Overlay model, variant taxonomy, decomposition, fail matrix |
| 02-inventory.md | done | Source-verified current PluginMetaData inventory |
| 03-subtasks.md | done | Ordered step-by-step todo + status |
| 05-issues.md | warning | I1 (S3 cross-repo blocker), I2 (S6 managed-by naming) |
| 06-decisions.md | done | D1–D6 |

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-resources/src/resources/plugin_metadata/` → `.../data/` | Rename module; generalize `DataEntry` variants (keep unified `KeyList`/`IpList`; + typed first-party overlays, `Misc`); remove RegionRoute logic variants; add envelope `enable`/`active` |
| `edgion-resources/.../edgion_plugins/plugin_configs/region_route.rs` | Absorb logic + default region table into base; add `overrideRef` |
| `edgion-resources/.../edgion_stream_plugins/` | GlobalConnIp base + overlay; switch fields |
| `edgion-controller/.../handlers/{plugin_metadata→data,edgion_plugins,edgion_stream_plugins}.rs` | Rename handler; new ref registration; overlay validation; `managed-by`; drop Service→Cluster requeue |
| `edgion-gateway/src/plugins/runtime/plugin_metadata/` → `.../data/` | Rename store; generic variant indexes; effective-config resolution |
| `edgion-gateway/src/plugins/http/region_route/plugin.rs` | base + overlay resolve; fail-to-base |
| `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/plugin.rs` | fail-closed → fail-to-base |
| `config/crd/edgion-crd/` | CRD kind rename + variant schema updates |
| `skills/`, `docs/` | Architecture + schema docs; overlay model; rename; `managed-by` |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-06-15 | Metadata = overlay over plugin base, not sole source | Keeps logic in plugin; enables fail-to-base; one ref overrides many fields |
| 2026-06-15 | Strict field replace, no deep merge | Avoids Istio EnvoyFilter MERGE-class bugs |
| 2026-06-15 | Only data overridable; logic excluded | Physically prevents logic override |
| 2026-06-15 | RegionRoute: logic → base, region table → overlay; drop Service→Cluster chain | RegionRoute was the main "metadata stays simple" violation |
| 2026-06-15 | fail-closed → fail-to-base (base must be safe default) | Missing/broken overlay no longer catastrophic |
| 2026-06-15 | Rename `PluginMetaData` → `EdgionData`; generic `dataRef`; not plugin-only | Reusable primitive for EdgionTls and other resources |
| 2026-06-15 | Generalize variants: standard primitives first-class + `Misc` catch-all | Overlay can cover arbitrary content; plugin-specific shapes go to `Misc`, core schema stays simple |
| 2026-06-15 (assumption — confirm) | Hard rename, no backward-compat alias | Project does not forward-port legacy keys; on feature branch. Revisit if live CRs need migration |
