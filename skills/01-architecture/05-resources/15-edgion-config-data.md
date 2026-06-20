---
name: resource-edgion-config-data
description: EdgionConfigData resource — general-purpose hot-swappable overlay/data primitive. Covers the overlay model (base+overlay, apply_override, fail-to-base), variant taxonomy (KeyList/IpList/typed first-party overlays/Misc), managed-by ownership, and controller/gateway processing.
---

# EdgionConfigData Resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)
>
> **Canonical overlay reference**: the overlay model, guardrails, and fail-to-base matrix are documented in
> the [Overlay Model](#overlay-model) section below — link here rather than duplicating.

EdgionConfigData (`kind: EdgionConfigData`) is Edgion's general-purpose, hot-swappable metadata and overlay primitive.
It is **not plugin-specific**: any resource that needs dynamic, origin-agnostic data (plugins today; `EdgionTls`
and others later) may reference it via a generic `dataRef`.

## Source Code Locations

- Type definitions: `edgion-resources/src/resources/edgion_config_data/mod.rs` (main module, `ConfigEntry` enum, `EdgionConfigDataSpec`)
- `KeyList` shape: `edgion-resources/src/resources/edgion_config_data/key_list.rs`
- `IpList` shape: `edgion-resources/src/resources/edgion_config_data/ip_list.rs`
- `IpGroup` (shared primitive): `edgion-resources/src/resources/edgion_config_data/ip_group.rs`
- `Selector` shape: `edgion-resources/src/resources/edgion_config_data/selector.rs`
- First-party overlay shape: `edgion-resources/src/resources/edgion_config_data/region_route.rs` (`RegionRouteOverride`)
- GlobalConnectionIpRestriction plugin config (consumes `Selector` via `activeProfileRef`):
  `edgion-resources/src/resources/edgion_stream_plugins/global_connection_ip_restriction.rs` (`GlobalConnectionIpRestrictionConfig`)
- Overlay framework: `edgion-resources/src/resources/edgion_config_data/overlay.rs` (`apply_override`)
- Controller handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_config_data.rs`
- Gateway store + index: `edgion-gateway/src/plugins/runtime/edgion_config_data/conf_handler.rs`
- Gateway store module: `edgion-gateway/src/plugins/runtime/edgion_config_data/mod.rs`

## Overlay Model

### Roles

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
                │     EdgionConfigData        │  general-purpose, hot-swappable
                │  (overlay / data)     │  KeyList/IpList + typed overlays + Misc
                └──────────────────────┘
           origin-agnostic: operator | controller | center (managed-by label)
```

- **Base** (`EdgionPlugins` / `EdgionStreamPlugins`, git-owned): holds complete, safe default config — decision
  logic + default data + default `enable`/`active`. Opt-in `overrideRef` → EdgionConfigData.
- **Overlay** (`EdgionConfigData`): hot-swappable, origin-agnostic. Body is a typed `ConfigEntry` variant. Top-level
  `enable: bool` (default true) + `active: Option<String>`.

### Five Guardrails

1. **Strict replace, never deep-merge.** Field present in overlay → replaces base; collection → whole-table replace.
2. **Only data is overridable; logic never is.** Override shapes are all-Optional, logic fields physically absent.
3. **Effective config is observable.** Admin dump shows base+overlay + per-field source.
4. **Switch unification.** `enable` is the top kill switch; `active` selects a profile. Both overridable.
5. **Fail-to-base.** Base is the complete safe default. Overlay absent → base. Overlay invalid → base + signal.

> **Security invariant:** fail-to-base means the base must be the conservative/safe value. If the base
> `default_action` is `Allow`, an overlay error silently allows all traffic — document and validate this.

### `apply_override` Contract

- Signature (conceptual): `fn apply(base: &Base, overlay: Option<&Override>) -> Effective`
- **Type guard first (D6):** verify the EdgionConfigData `type` matches the expected override variant before applying.
  Mismatch → fail-to-base + signal (`overlay_type_mismatch`). Never coerce across types.
- For each field in `Override` (all `Option`): `Some(v)` → use `v`; `None` → use base value.
- Collections are replaced **whole** (no element merge).
- Invalid overlay → controller `Accepted=False` (not pushed to gateway); gateway uses base + emits signal.

### Fail-Behavior Matrix

| Situation | Behavior |
|-----------|----------|
| No `overrideRef` set | Base used |
| `overrideRef` set, EdgionConfigData absent | **Base** + `overlay_missing` signal (metric + access log) |
| EdgionConfigData present, **type mismatch** | **Base** + `overlay_type_mismatch` signal |
| EdgionConfigData present, invalid (controller) | `Accepted=False`, not pushed; gateway uses **base** |
| EdgionConfigData present, valid | `base ⊕ overlay` (field-level replace) |
| `enable=false` effective (envelope `spec.enable` or variant `enable`) | **Base** + `overlay_disabled` signal (envelope kill switch) |

Signals: gateway uses access-log `EdgionStatus` + Prometheus counter. No `tracing::error!` on the data plane.

> Three CI gates guard this contract — see [`skills/09-misc/cicd/02-github-workflow.md`](../../../09-misc/cicd/02-github-workflow.md) "EdgionConfigData overlay gates". In short: forbidden-legacy-refs grep, CRD `type` ↔ Rust `ConfigEntry` parity, and overlay-consumer signal completeness (all four signals above).

## Variant Taxonomy

Wire shape: `data: { type: <Variant>, config: {...} }` (`#[serde(tag="type", content="config")]`).
Envelope (any variant): `enable: bool` (default true), `active: Option<String>`.

| Group | Variant | Shape | Notes |
|-------|---------|-------|-------|
| std primitive | `KeyList` | `{ matchMode: Exact\|Regex, items: [KeyGroup] }` | Unified string+regex via `matchMode` |
| std primitive | `IpList` | `{ items: [IpGroup] }` | Radix-tree CIDR matcher; consumed by IpRestriction (HTTP/L4/TLS-route) and GCIR per-profile via `allowRefs`/`denyRefs` |
| selector | `Selector` | `{ active?: string, description?: string }` | Active-profile selector; switches among the consumer's git-owned profiles |
| first-party overlay | `RegionRouteOverride` | `{ regions?: [...], enable?: bool, active?: string }` | Typed; whole-replace of region table |
| escape hatch | `Misc` | `x-kubernetes-preserve-unknown-fields` | Third-party / unknown / experimental only |

No separate `StringList`/`RegexList` (folded into `KeyList(matchMode)`). No standalone `Switch` variant
(`enable`/`active` live on the envelope). `Selector` replaced the former `GlobalConnIpRestrictionOverride`
overlay: GlobalConnectionIpRestriction now carries its full rule data inline (git-owned plugin config) and
exposes only "which profile is active" as the hot-swappable surface — see the example below.

## Controller-Side Processing

### preparse

- Calls `EdgionConfigData::validate_pre_parse()` for per-variant validation (invalid CIDR for `IpList`,
  invalid regex for `KeyList`, invalid regions for `RegionRouteOverride`) → `Vec<String>`.
  Non-empty → `Accepted=False/Invalid`. `Selector` is always structurally valid (the active-name ↔
  profile check is a consumer-side, connection-time concern, not a controller preparse check).

### parse

- Stores the EdgionConfigData spec in the controller-internal `EdgionConfigDataStore` (indexed by `ns/name`).
- Validates overlay `type` consistency if a bound consumer is already known.

### on_change

- Looks up resources referencing this EdgionConfigData via `EdgionConfigDataRefManager` and requeues them.
  Consumers include EdgionPlugins (via `overrideRef` in `RegionRouteConfig`) and EdgionStreamPlugins
  (via `activeProfileRef` in `GlobalConnectionIpRestrictionConfig`). Also requeues EdgionPlugins and
  EdgionStreamPlugins that reference this object as an `IpList` via `allowRefs`/`denyRefs` in
  IpRestriction configs or GCIR per-profile `ProfileRules`.

### on_delete

- Removes from `EdgionConfigDataStore`.
- Requeues all dependents so their `ResolvedRefs` is recomputed.

### update_status

| Condition | Semantics |
|-----------|-----------|
| `Accepted` | Spec passes preparse validation |
| `ResolvedRefs` | EdgionConfigData itself carries no external cross-resource refs; own `ResolvedRefs` is always `True`. Note: `IpList` variants are ref targets — when `allowRefs`/`denyRefs` in IpRestriction (HTTP/L4/TLS-route) or GCIR per-profile fail to resolve an `IpList`, `ResolvedRefs=False` is set on the *referring* EdgionPlugins/EdgionStreamPlugins, not on the `IpList` EdgionConfigData itself. |

## Gateway-Side Processing

Gateway stores EdgionConfigData in `EdgionConfigDataStore` (ArcSwap + variant-keyed indexes). Overlay resolution is
performed **at request time** (lock-free ArcSwap load). The gateway does not cache effective configs — it
re-applies `apply_override` on each request from the current snapshot.

- **Overlay absent or type-mismatch:** base used; metric counter incremented (`overlay_missing` /
  `overlay_type_mismatch` label on the relevant plugin's event counter).
- **Enable=false:** plugin skipped via the shared `effective_switch` helper.

## Managed-By Ownership

The `edgion.io/managed-by` **label** on an EdgionConfigData object declares who owns its lifecycle. See
`skills/02-features/05-annotations/00-annotations-overview.md` (Labels section) for the full vocabulary:

| Value | Meaning | Controller behavior |
|-------|---------|---------------------|
| `operator` (default) | Git/kubectl authoritative | Controller never mutates spec |
| `acme` | ACME-managed (existing usage on Secrets/EdgionTls) | ACME service owns lifecycle |
| `center` | Placeholder; data pushed via fed_sync from EdgionCenter | Controller does not overwrite from git — **DESIGN-DECLARED, NOT YET IMPLEMENTED** |
| `controller` | Body regenerated from a template ref | Controller regenerates spec — **DESIGN-DECLARED, NOT YET IMPLEMENTED** |

`operator` is the only active ownership mode today. `center` and `controller` require EdgionCenter
coordination (I1 in `tasks/todo/edgion-config-data-overlay-refactor/05-issues.md`).

## Cross-Resource Relationships

| Direction | Mechanism |
|-----------|-----------|
| EdgionConfigData ← EdgionPlugins | `overrideRef` in `RegionRouteConfig` → `RegionRouteOverride`; EP `ResolvedRefs` reflects resolution |
| EdgionConfigData ← EdgionStreamPlugins | `activeProfileRef` in `GlobalConnectionIpRestrictionConfig` → `Selector`; ESP `ResolvedRefs` reflects resolution |
| EdgionConfigData ← {EdgionPlugins, EdgionStreamPlugins} | `allowRefs`/`denyRefs` in IpRestriction (HTTP/L4/TLS-route) or GCIR per-profile `ProfileRules` → `IpList`; referrer `ResolvedRefs` reflects resolution (missing/disabled/wrong-type IpList → `ResolvedRefs=False` on referrer) |
| EdgionConfigData → {EdgionPlugins, EdgionStreamPlugins} | `EdgionConfigDataRefManager` requeue on change/delete |

> The old `ServiceRegionRoute → ClusterRegionRoute` PM→PM chain and `clusterRef` indirection were
> removed in the S3 refactor. RegionRoute logic now lives entirely in `EdgionPlugins` base config.

## RegionRoute with EdgionConfigData Overlay

The RegionRoute plugin in `EdgionPlugins` now carries all routing logic inline (base) with an optional
`overrideRef` pointing to a `RegionRouteOverride` EdgionConfigData for dynamic region-table updates:

```yaml
# EdgionPlugins base config (git-owned, logic + default regions)
- type: RegionRoute
  config:
    myRegion: virginia
    keyGet: [{ type: header, name: X-Account-Id }]
    hashCalc: { algorithm: crc32, modulo: 1000 }
    routeRules: [{ type: RouteByHashRange }]
    regions:                    # safe default region table
      - { name: virginia, hashRange: [0, 599], backendEndpoint: virginia.example.com:8443, tls: true }
      - { name: ohio, hashRange: [600, 999], backendEndpoint: ohio.example.com:8443, tls: true }
    overrideRef: { name: region-overlay, namespace: edgion-system }  # optional

---
# EdgionConfigData overlay (hot-swappable region table)
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: region-overlay
  namespace: edgion-system
  labels:
    edgion.io/managed-by: operator
spec:
  enable: true
  data:
    type: RegionRouteOverride
    config:
      regions:                  # whole-replace of region table
        - { name: virginia, hashRange: [0, 499], backendEndpoint: virginia-v2.example.com:8443, tls: true }
        - { name: ohio, hashRange: [500, 999], backendEndpoint: ohio-v2.example.com:8443, tls: true, failoverTo: virginia }
```

## GlobalConnectionIpRestriction with a Selector

Unlike RegionRoute (whole-table overlay), the L4 `GlobalConnectionIpRestriction` stream plugin carries
its **complete** IP rule data inline (git-owned) and exposes only the *active profile* as the
hot-swappable surface via `activeProfileRef` → a `Selector` EdgionConfigData. The selector can only
switch among the plugin's own `profiles`; it can never inject rules or disable filtering.

```yaml
# EdgionStreamPlugins (git-owned: full rule data inline, no base: wrapper)
- type: GlobalConnectionIpRestriction
  config:
    enable: true
    activeProfile: locked          # git default (safe)
    profiles:
      locked:
        defaultAction: deny
        allow: [{ name: office, cidrs: ["10.0.0.0/8"] }]
        # allowRefs (optional): merge additional IpList EdgionConfigData into the allow pool
        allowRefs: [{ name: corp-allowlist }]   # namespace defaults to parent ESP namespace
      open:    { defaultAction: deny, allow: [{ name: any, cidrs: ["0.0.0.0/0"] }] }
    activeProfileRef: { name: gcir-active, namespace: edgion-system }  # optional

---
# EdgionConfigData (hot-swappable active-profile selector)
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata: { name: gcir-active, namespace: edgion-system }
spec:
  enable: true
  data:
    type: Selector
    config:
      active: open                 # switch to the "open" profile; null/absent → base default
      description: "temporarily widen access"   # informational only
```

**Fail-to-base (selector resolution at connection time):**

| Situation | Effective active profile | Signal |
|-----------|--------------------------|--------|
| no `activeProfileRef` | `config.activeProfile` | — |
| ref set, EdgionConfigData absent | `config.activeProfile` | `overlay_missing` |
| present but not a `Selector` variant | `config.activeProfile` | `overlay_type_mismatch` |
| envelope `enable=false` | `config.activeProfile` | `overlay_disabled` |
| `active: null` / absent | `config.activeProfile` (no-op) | — |
| `active: pX`, `pX` ∈ `profiles` | `pX` | — (applied) |
| `active: pX`, `pX` ∉ `profiles` | `config.activeProfile` | `overlay_invalid` |

There is no operator-reachable allow-all switch via the selector: to widen access the operator must
author a permissive profile in git and select it by name (explicit, reviewable, auditable).

**Fail-to-base:** if the overlay is absent or invalid, the gateway uses the default `regions` from the base
config — traffic continues to flow (no BrokenRef fail-closed).
