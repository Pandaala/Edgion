# 01 — Design

## 1. The overlay model

```
                 git / operator (declarative, complete)
                              │
                   ┌──────────▼───────────┐
                   │  EdgionPlugins (base) │   logic + default data + enable/active
                   │   overrideRef: <md>?  │   (later: EdgionTls and others too)
                   └──────────┬───────────┘
                              │ resolve at runtime
        ┌─────────────────────▼─────────────────────┐
        │  effective = apply_override(base, overlay)  │
        │  overlay absent  → base                     │
        │  overlay present & valid → base ⊕ overlay   │  (field-level REPLACE)
        │  overlay present & invalid → base + signal  │  (fail-to-base)
        └─────────────────────▲─────────────────────┘
                              │ generic dataRef (ECDS-style)
                   ┌──────────┴───────────┐
                   │  EdgionData       │  general-purpose, hot-swappable
                   │  (overlay / data)     │  KeyList/IpList + typed overlays + Misc
                   └──────────────────────┘
              origin-agnostic: operator | controller | center (Step S6)
```

`EdgionData` is **not plugin-specific**. Any resource (plugins today; `EdgionTls` and others later) may
reference it via a generic `dataRef`. Consumers decide how to interpret the variant.

### `apply_override` contract
- Signature (conceptual): `fn apply(base: &Base, overlay: Option<&Override>) -> Effective`.
- **Type guard first (D6):** before applying, verify the referenced EdgionData's `type` matches the override
  variant the consumer expects. Mismatch (e.g. `overrideRef` → a `KeyList`) → treat as no valid overlay →
  **fail-to-base + signal**. Never coerce or partially apply across types.
- For each field in `Override` (all `Option`): `Some(v)` → use `v`; `None` → use `base`'s value.
- Collections are replaced **whole** (no element merge). Selective element patch = explicit, separately-named
  operation only — never implicit merge.
- `Override` contains **only the data subset**. Logic fields are not present → always from base.
- Validation: `Override` validates standalone (CIDR parse, regex compile, range bounds). Invalid overlay →
  controller rejects (status), gateway ignores overlay + uses base + emits a signal.

## 2. EdgionData variant taxonomy (generalized)

The body is a tagged enum `data: DataEntry`. Top-level envelope fields apply to any variant:
`enable: bool` (default true), `active: Option<String>` (profile selector, optional).

Wire shape (D6): `data: { type: <Variant>, config: {...} }` (`#[serde(tag="type", content="config")]`).

| Group | Variant | Shape | Notes |
|-------|---------|-------|-------|
| std primitive | `KeyList` | `{ matchMode: Exact\|Regex, items: [KeyGroup] }` | **keep** — already unifies string+regex (D5); orphan plumbing removed (D1) |
| std primitive | `IpList` | `{ items: [IpGroup] }` | **keep** (radix matcher); orphan plumbing removed (D1) |
| first-party overlay | `RegionRouteOverride` | all-Optional (`regions?` …) | **typed** + `validate_and_init` (D2) |
| first-party overlay | `GlobalConnIpRestrictionOverride` | all-Optional (`enable?`/`active?`/profiles?) | **typed** + `validate_and_init` (D2) |
| escape hatch | `Misc` | `x-kubernetes-preserve-unknown-fields` | third-party / unknown / experimental only (D2) |

No separate `StringList` / `RegexList` (D5 — folded into `KeyList(matchMode)`). No standalone `Switch` variant
(D3). Envelope (any variant): `enable: bool` (default true), `active: Option<String>`.

**Design intent (D2):** everything first-party is **typed** — standard primitives *and* first-party overlays.
Typed buys serde validation + a `validate_and_init` hook (range/endpoint checks, matcher pre-compile). `Misc`
is strictly the open door for non-first-party payloads. All variants are CRD-level
`x-kubernetes-preserve-unknown-fields` anyway, so "typed" is a Rust-layer guarantee, not a CRD one.

## 3. Per-resource disposition

| Current PM variant | Disposition | Where logic goes | Where data goes |
|--------------------|-------------|------------------|-----------------|
| `KeyList` | **keep** as simple data (+ envelope) | n/a | stays EdgionData |
| `IpList` | **keep** as simple data (+ envelope) | n/a | stays EdgionData |
| `ClusterRegionRoute` | **split + remove variant** | `route_rules`, `hash_calc`, `hash_key_get`, `key_get`, `my_region`, `route_by_key_conf_match` → EdgionPlugins `RegionRouteConfig` base | `regions` → base default + `RegionRouteOverride` (**typed** overlay) |
| `ServiceRegionRoute` | **remove variant + drop chain** | n/a | per-service `failoverTo` → fold into `RegionRouteOverride.regions` (whole-table replace) |
| `GlobalConnectionIpRestriction` | **re-express as overlay** | (already data-only) | base in `EdgionStreamPlugins`; `GlobalConnIpRestrictionOverride` (**typed** overlay) overrides `enable`/`active`/profile data |

> Resolved (D1): `IpList` / `KeyList` have no live consumer (HTTP IpRestriction/KeyAuth are inline) — keep as
> standard primitives, delete the orphan gateway indexes/getters, wire real consumers later.

### RegionRoute target shape

```
# EdgionPlugins (base, git)
RegionRouteConfig:
  myRegion: cn
  keyGet / hashKeyGet / hashCalc / routeRules / routeByKeyConfMatch   # logic — not overridable
  regions:                 # DEFAULT region table (safe default)
    - { name: cn, hashRange: [0,499],   backendEndpoint: a:443, tls: true }
    - { name: us, hashRange: [500,999], backendEndpoint: b:443, tls: true, failoverTo: cn }
  overrideRef: { name: region-overlay, namespace: ... }   # optional -> EdgionData

# EdgionData (typed variant RegionRouteOverride, all-Optional)
enable: true
data:
  type: RegionRouteOverride
  regions:                 # optional WHOLE-TABLE replace (endpoint/hashRange/failover)
    - { name: cn, hashRange: [0,499],   backendEndpoint: a2:443, tls: true }
    - { name: us, hashRange: [500,999], backendEndpoint: b2:443, tls: true, failoverTo: cn }
  # no routeRules/hashCalc/keyGet — logic cannot be overridden
```

Removed: `service_region_route_ref` indirection and the `cluster_ref` chain. The plugin references at most one
overlay via `overrideRef`. ServiceRegionRoute's only payload (per-service `failoverTo`) = overriding `regions`.

## 4. Fail-behavior matrix (replaces today's fail-closed)

| Situation | Today | New |
|-----------|-------|-----|
| No `overrideRef` | base (RegionRoute) / N/A | base used |
| `overrideRef` set, metadata absent | RegionRoute BrokenRef / GlobalConnIp Deny (fail closed) | **base** + `overlay_missing` signal |
| metadata present, **type mismatch** (D6) | n/a | **base** + `overlay_type_mismatch` signal |
| metadata present, invalid | rejected/var | controller `Accepted=False` (not pushed); gateway **base** + `overlay_invalid` signal |
| metadata present, valid | overlay applied | overlay applied (field-level replace) |
| `enable=false` effective | skip (GlobalConnIp) | skip (uniform) |

Signals: gateway uses access-log EdgionStatus + Prometheus counter (extend `record_conn_ip_restriction_event`);
**no `tracing::error!` in the data plane**. Controller sets `Accepted`/`ResolvedRefs` and may use tracing.

**Security invariant:** overlay error falls back to base, so `base` MUST be the safe value. Document loudly;
validate where feasible (e.g. warn if base IP default action is `Allow` with empty allow list).

## 5. Switch (`enable` / `active`) generalization
- `enable: bool` (default true) — top kill switch on the envelope; overridable.
- `active: Option<String>` + base `profiles` — preloaded alternatives, instant atomic flip by overriding
  `active`. Optional per consumer (RegionRoute may skip profiles initially; GlobalConnIp keeps them).
- Resolution: effective `enable` gates everything → `active` selects a profile → field overrides apply.

## 6. `managed-by` ownership model (Step S6)
Annotation `edgion.io/managed-by` on EdgionData (and optionally consumers):
- `operator` (default) — git authoritative; controller never mutates spec.
- `controller` — operator sets a template ref; controller regenerates the body.
- `center` — local object is a placeholder; real data via conf_sync/fed_sync; controller does not overwrite
  from git. Refs always resolve (object exists) → overlay-absent fallback yields base.

This makes "apply only a reference" / "wait for remote" / "annotation-controlled" expressible through one
declarative knob layered on the overlay model.

## 7. HTTPRoute weight ownership (follow-on, design-only)
K8s Server-Side Apply field ownership: git owns backend structure, a progressive-delivery controller/Center
owns `weight`; ArgoCD `ignoreDifferences` on weights (mirrors Argo Rollouts). Separate task.
