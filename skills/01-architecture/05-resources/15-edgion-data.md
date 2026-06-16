---
name: resource-edgion-data
description: EdgionData resource — general-purpose hot-swappable overlay/data primitive. Covers the overlay model (base+overlay, apply_override, fail-to-base), variant taxonomy (KeyList/IpList/typed first-party overlays/Misc), managed-by ownership, and controller/gateway processing.
---

# EdgionData Resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)
>
> **Canonical overlay reference**: the overlay model, guardrails, and fail-to-base matrix are documented in
> the [Overlay Model](#overlay-model) section below — link here rather than duplicating.

EdgionData (`kind: EdgionData`) is Edgion's general-purpose, hot-swappable metadata and overlay primitive.
It is **not plugin-specific**: any resource that needs dynamic, origin-agnostic data (plugins today; `EdgionTls`
and others later) may reference it via a generic `dataRef`.

## Source Code Locations

- Type definitions: `edgion-resources/src/resources/data/mod.rs` (main module, `DataEntry` enum, `EdgionDataSpec`)
- `KeyList` shape: `edgion-resources/src/resources/data/key_list.rs`
- `IpList` shape: `edgion-resources/src/resources/data/ip_list.rs`
- First-party overlay shapes: `edgion-resources/src/resources/data/region_route.rs` (`RegionRouteOverride`),
  `edgion-resources/src/resources/data/global_connection_ip_restriction.rs` (`GlobalConnIpRestrictionOverride`)
- Overlay framework: `edgion-resources/src/resources/data/overlay.rs` (`apply_override`)
- Controller handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/data.rs`
- Gateway store + index: `edgion-gateway/src/plugins/runtime/data/conf_handler.rs`
- Gateway store module: `edgion-gateway/src/plugins/runtime/data/mod.rs`

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
                │     EdgionData        │  general-purpose, hot-swappable
                │  (overlay / data)     │  KeyList/IpList + typed overlays + Misc
                └──────────────────────┘
           origin-agnostic: operator | controller | center (managed-by label)
```

- **Base** (`EdgionPlugins` / `EdgionStreamPlugins`, git-owned): holds complete, safe default config — decision
  logic + default data + default `enable`/`active`. Opt-in `overrideRef` → EdgionData.
- **Overlay** (`EdgionData`): hot-swappable, origin-agnostic. Body is a typed `DataEntry` variant. Top-level
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
- **Type guard first (D6):** verify the EdgionData `type` matches the expected override variant before applying.
  Mismatch → fail-to-base + signal (`overlay_type_mismatch`). Never coerce across types.
- For each field in `Override` (all `Option`): `Some(v)` → use `v`; `None` → use base value.
- Collections are replaced **whole** (no element merge).
- Invalid overlay → controller `Accepted=False` (not pushed to gateway); gateway uses base + emits signal.

### Fail-Behavior Matrix

| Situation | Behavior |
|-----------|----------|
| No `overrideRef` set | Base used |
| `overrideRef` set, EdgionData absent | **Base** + `overlay_missing` signal (metric + access log) |
| EdgionData present, **type mismatch** | **Base** + `overlay_type_mismatch` signal |
| EdgionData present, invalid (controller) | `Accepted=False`, not pushed; gateway uses **base** |
| EdgionData present, valid | `base ⊕ overlay` (field-level replace) |
| `enable=false` effective (envelope `spec.enable` or variant `enable`) | **Base** + `overlay_disabled` signal (envelope kill switch) |

Signals: gateway uses access-log `EdgionStatus` + Prometheus counter. No `tracing::error!` on the data plane.

> Three CI gates guard this contract — see [`skills/09-misc/cicd/02-github-workflow.md`](../../../09-misc/cicd/02-github-workflow.md) "EdgionData overlay gates". In short: forbidden-legacy-refs grep, CRD `type` ↔ Rust `DataEntry` parity, and overlay-consumer signal completeness (all four signals above).

## Variant Taxonomy

Wire shape: `data: { type: <Variant>, config: {...} }` (`#[serde(tag="type", content="config")]`).
Envelope (any variant): `enable: bool` (default true), `active: Option<String>`.

| Group | Variant | Shape | Notes |
|-------|---------|-------|-------|
| std primitive | `KeyList` | `{ matchMode: Exact\|Regex, items: [KeyGroup] }` | Unified string+regex via `matchMode` |
| std primitive | `IpList` | `{ items: [IpGroup] }` | Radix-tree CIDR matcher |
| first-party overlay | `RegionRouteOverride` | `{ regions?: [...], enable?: bool, active?: string }` | Typed; whole-replace of region table |
| first-party overlay | `GlobalConnIpRestrictionOverride` | all-Optional profiles/enable/active | Typed; pre-compiles matchers at validate_and_init |
| escape hatch | `Misc` | `x-kubernetes-preserve-unknown-fields` | Third-party / unknown / experimental only |

No separate `StringList`/`RegexList` (folded into `KeyList(matchMode)`). No standalone `Switch` variant
(`enable`/`active` live on the envelope).

## Controller-Side Processing

### preparse

- Calls `EdgionData::validate_pre_parse()` for per-variant validation (invalid CIDR, invalid regex,
  `GlobalConnIpRestrictionOverride::validate_and_init` matcher-compile failures) → `Vec<String>`.
  Non-empty → `Accepted=False/Invalid`.

### parse

- Stores the EdgionData spec in the controller-internal `PluginMetaDataStore` (indexed by `ns/name`).
- Validates overlay `type` consistency if a bound consumer is already known.

### on_change

- Looks up resources referencing this EdgionData via `PluginMetaDataRefManager` and requeues them.
  Consumers include EdgionPlugins (via `overrideRef` in `RegionRouteConfig`) and EdgionStreamPlugins
  (via `overrideRef` in `GlobalConnectionIpRestrictionEntry`).

### on_delete

- Removes from `PluginMetaDataStore`.
- Requeues all dependents so their `ResolvedRefs` is recomputed.

### update_status

| Condition | Semantics |
|-----------|-----------|
| `Accepted` | Spec passes preparse validation |
| `ResolvedRefs` | No current typed variant carries external cross-resource refs; always `True` |

## Gateway-Side Processing

Gateway stores EdgionData in `EdgionDataStore` (ArcSwap + variant-keyed indexes). Overlay resolution is
performed **at request time** (lock-free ArcSwap load). The gateway does not cache effective configs — it
re-applies `apply_override` on each request from the current snapshot.

- **Overlay absent or type-mismatch:** base used; metric counter incremented (`overlay_missing` /
  `overlay_type_mismatch` label on the relevant plugin's event counter).
- **Enable=false:** plugin skipped via the shared `effective_switch` helper.

## Managed-By Ownership

The `edgion.io/managed-by` **label** on an EdgionData object declares who owns its lifecycle. See
`skills/02-features/05-annotations/00-annotations-overview.md` (Labels section) for the full vocabulary:

| Value | Meaning | Controller behavior |
|-------|---------|---------------------|
| `operator` (default) | Git/kubectl authoritative | Controller never mutates spec |
| `acme` | ACME-managed (existing usage on Secrets/EdgionTls) | ACME service owns lifecycle |
| `center` | Placeholder; data pushed via fed_sync from EdgionCenter | Controller does not overwrite from git — **DESIGN-DECLARED, NOT YET IMPLEMENTED** |
| `controller` | Body regenerated from a template ref | Controller regenerates spec — **DESIGN-DECLARED, NOT YET IMPLEMENTED** |

`operator` is the only active ownership mode today. `center` and `controller` require EdgionCenter
coordination (I1 in `tasks/todo/edgion-data-overlay-refactor/05-issues.md`).

## Cross-Resource Relationships

| Direction | Mechanism |
|-----------|-----------|
| EdgionData ← EdgionPlugins | `overrideRef` in `RegionRouteConfig`; EP `ResolvedRefs` reflects resolution |
| EdgionData ← EdgionStreamPlugins | `overrideRef` in `GlobalConnectionIpRestrictionEntry`; ESP `ResolvedRefs` reflects resolution |
| EdgionData → {EdgionPlugins, EdgionStreamPlugins} | `PluginMetaDataRefManager` requeue on change/delete |

> The old `ServiceRegionRoute → ClusterRegionRoute` PM→PM chain and `clusterRef` indirection were
> removed in the S3 refactor. RegionRoute logic now lives entirely in `EdgionPlugins` base config.

## RegionRoute with EdgionData Overlay

The RegionRoute plugin in `EdgionPlugins` now carries all routing logic inline (base) with an optional
`overrideRef` pointing to a `RegionRouteOverride` EdgionData for dynamic region-table updates:

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
# EdgionData overlay (hot-swappable region table)
apiVersion: edgion.io/v1
kind: EdgionData
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

**Fail-to-base:** if the overlay is absent or invalid, the gateway uses the default `regions` from the base
config — traffic continues to flow (no BrokenRef fail-closed).
