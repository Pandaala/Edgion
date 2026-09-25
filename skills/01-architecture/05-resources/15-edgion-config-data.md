---
name: resource-edgion-config-data
description: EdgionConfigData resource — general-purpose hot-swappable overlay/data primitive. Covers the overlay model (base+overlay, apply_override, fail-to-base), variant taxonomy (KeyList/IpList/typed first-party overlays/Misc), ownership (Center-controlled; the managed-by label is inert here), and controller/gateway processing.
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
- `RequestAccessUrlAllowList` shape: `edgion-resources/src/resources/edgion_config_data/request_access_url_allow_list.rs`
- `IpGroup` (shared primitive): `edgion-resources/src/resources/edgion_config_data/ip_group.rs`
- `Selector` shape: `edgion-resources/src/resources/edgion_config_data/selector.rs`
- First-party overlay shape: `edgion-resources/src/resources/edgion_config_data/region_route.rs` (`RegionRouteOverride`)
- Native WAF bundle shape: `edgion-resources/src/resources/edgion_config_data/waf_rule_bundle.rs` (`WafRuleBundleData`)
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
           Center-controlled data; not an ops-repo artifact (see Ownership)
```

- **Base** (`EdgionPlugins` / `EdgionStreamPlugins`, git-owned): holds complete, safe default config — decision
  logic + default data + default `enable`/`active`. Opt-in `overrideRef` → EdgionConfigData.
- **Overlay** (`EdgionConfigData`): hot-swappable, origin-agnostic. Body is a typed `ConfigEntry` variant. Top-level
  `enable: bool` (default true) + `active: Option<String>`.

### Five Guardrails

1. **Strict replace, never deep-merge.** Field present in overlay → replaces base; collection → whole-table replace.
2. **Only data is overridable; logic never is.** Override shapes are all-Optional, logic fields physically absent.
3. **Effective config is observable.** Admin dump shows base+overlay + per-field source.
4. **Switch unification.** Use the consuming feature's switch contract. RegionRoute consumes only envelope `spec.enable`; its variant `enable` and `active` fields are currently inert.
5. **Fail-to-base.** Base is the complete safe default. Overlay absent → base. Overlay invalid → base + signal.

> **Security invariant:** fail-to-base means the base must be the conservative/safe value. If the base
> `default_action` is `Allow`, an overlay error silently allows all traffic — document and validate this.

### `apply_override` Contract

- Signature (conceptual): `fn apply(base: &Base, overlay: Option<&Override>) -> Effective`
- **Type guard first (D6):** verify the EdgionConfigData `type` matches the expected override variant before applying.
  Mismatch → fail-to-base + signal (`overlay_type_mismatch`). Never coerce across types.
- For each field in `Override` (all `Option`): `Some(v)` → use `v`; `None` → use base value.
- Collections are replaced **whole** (no element merge).
- Invalid RegionRoute overlay → controller `Accepted=False` for standalone table errors; generic ingest still continues through parse/status and conf_sync. The Gateway independently validates and uses base + emits signal. Consumer-specific modulo failures belong to the referring plugin's `ResolvedRefs`.

### Fail-Behavior Matrix

| Situation | Behavior |
|-----------|----------|
| No `overrideRef` set | Base used |
| `overrideRef` set, EdgionConfigData absent | **Base** + `overlay_missing` signal (metric + access log) |
| EdgionConfigData present, **type mismatch** | **Base** + `overlay_type_mismatch` signal |
| RegionRoute EdgionConfigData present, invalid (controller) | `Accepted=False` for standalone errors; still synced by generic ingest; independent Gateway validation uses **base** |
| EdgionConfigData present, valid | `base ⊕ overlay` (field-level replace) |
| RegionRoute envelope `spec.enable=false` | **Base** + `overlay_disabled`; variant `enable`/`active` do not affect RegionRoute |

Signals: gateway uses an access-log `EdgionErrorCode` diagnostic + Prometheus counter. No `tracing::error!` on the data plane.

> Three CI gates guard this contract — see [`skills/09-misc/cicd/02-github-workflow.md`](../../../09-misc/cicd/02-github-workflow.md) "EdgionConfigData overlay gates". In short: forbidden-legacy-refs grep, CRD `type` ↔ Rust `ConfigEntry` parity, and overlay-consumer signal completeness (all four signals above).

## Variant Taxonomy

Wire shape: `data: { type: <Variant>, config: {...} }` (`#[serde(tag="type", content="config")]`).
Envelope (any variant): `enable: bool` (default true), `active: Option<String>`, `visibility: Namespace|Cluster` (default `Namespace` — governs cross-ns reference access; see [Cross-Namespace Reference Grants](#cross-namespace-reference-grants)).

| Group | Variant | Shape | Notes |
|-------|---------|-------|-------|
| std primitive | `KeyList` | `{ matchMode: exact\|regex, items: [KeyGroup] }` | Unified string+regex via `matchMode` |
| std primitive | `IpList` | `{ items: [IpGroup] }` | Radix-tree CIDR matcher; consumed by IpRestriction (HTTP/L4/TLS-route) and GCIR per-profile via `allowRefs`/`denyRefs` |
| typed policy data | `RequestAccessUrlAllowList` | `{ items: [UrlAccessCandidate] }` | URL allow candidates expanded into RequestAccessPolicy by the Controller |
| selector | `Selector` | `{ active?: string, description?: string }` | Active-profile selector; switches among the consumer's git-owned profiles |
| first-party overlay | `RegionRouteOverride` | `{ regions?: [...], enable?: bool, active?: string }` | Typed; whole-replace of region table |
| security policy | `ProxyProtocolTrust` | `{ mode: required }` or `{ mode: trustedSources, trustedCidrs: [...] }` | Non-overlay, all-or-nothing Gateway listener trust policy. Requires envelope `enable=true` and `active` absent. Unknown config fields are fatal rather than advisory. |
| executable policy data | `WafRuleBundle` | `{ version, profile, provenance, digest, roots, rules, phraseAssets? }` | Non-overlay WAF rule and phrase-asset inputs. Structural validation and digest are all-or-nothing; a WAF named profile aggregates ordered, digest-pinned references and strictly compiles the selected generation. Requires `enable=true`, `active` absent, and no unknown config fields. |
| WAF policy | `WafPolicy` | `{ profiles, defaultProfile }` | Shared aggregation of ordered bundle references. EdgionPlugins fixes one `policyRef` and may select a profile with its own `activeProfileRef`; the Controller transports the selected profile's accepted bundles for Gateway compilation. Requires `enable=true`, `active` absent, and no unknown config fields. |
| escape hatch | `Misc` | `x-kubernetes-preserve-unknown-fields` | Third-party / unknown / experimental only |

## Unknown Config Fields

The Controller applies the shared advisory schema walk to the typed `KeyList`, `IpList`,
`RequestAccessUrlAllowList`, `Selector`, and `RegionRouteOverride` payloads. A key that serde would otherwise ignore is
reported through the `UnknownConfigFields=True` condition, a bounded Controller warning,
`GET /api/v1/diagnostics/unknown-fields`, Admin write warnings, and
`POST /api/v1/validate/EdgionConfigData`. It does not change `Accepted` or block conf_sync.

Credential and authentication sub-structures nested inside typed policy data retain their
strict deserialization boundary. For example, `SecretObjectReference`, SecretMatch, and HMAC
credential shapes still reject unknown fields; this prevents inline or misspelled credential
material from degrading into an unauthenticated configuration. The surrounding URL candidate,
path, and IP-match data remains advisory.

`Misc.config` remains deliberately free-form and is never inspected for unknown keys. The
openness is scoped to that config node; raw-document ingresses still report unknown siblings
of `data.config` or unknown fields in the spec envelope. Kubernetes is different because its
structural CRD prunes unknown `spec`/`data` siblings before the watcher sees the object, so the
Kubernetes capture guarantee applies only below the preserved `data.config` node.

Findings live in `EdgionConfigDataSpec::unknown_fields`, an in-process-only sidecar marked
`#[serde(skip)]` and `#[schemars(skip)]`. It is absent from both the CRD and conf_sync wire.

No separate `StringList`/`RegexList` (folded into `KeyList(matchMode)`). No standalone `Switch` variant
(`enable`/`active` live on the envelope). `Selector` replaced the former `GlobalConnIpRestrictionOverride`
overlay: GlobalConnectionIpRestriction now carries its full rule data inline (git-owned plugin config) and
exposes only "which profile is active" as the hot-swappable surface — see the example below.

`ProxyProtocolTrust` and `WafRuleBundle` are exceptions to the ordinary overlay/advisory behavior. `WafRuleBundle`
contains executable rule input, so a wrong or omitted config field invalidates the bundle instead of silently
reducing WAF coverage. Its standalone `Accepted=True` proves structure and content integrity, not that a
consumer's combined rule generation is executable. `WafConfig.policyRef` resolves a `WafPolicy` that selects one named profile through a
default or a `Selector`, then compiles its required pinned references together before publication.
Optional independent references are attempted in order; an invalid optional reference is omitted
and reported through `PartiallyInvalid`, while required failures invalidate that profile. An invalid
or unknown Selector choice returns to a validated default profile. Bundle content is transported through conf_sync, while Admin serialization and
Rust `Debug` redact rule and phrase-asset bodies. Editing a referenced bundle can change WAF policy; enforce
write permissions with that authority in mind.

`ProxyProtocolTrust` is
a security-policy target for `Gateway.metadata.annotations["edgion.io/inbound-proxy-protocol"]`,
not an overlay and never fails to a base. Unknown fields, invalid CIDRs, `enable=false`, any
`active`, or a wrong variant make the referring Gateway explicitly Invalid. Same-namespace
references are allowed; cross-namespace Cluster-visible targets need no grant; cross-namespace
Namespace-visible targets require an exact grant from `gateway.networking.k8s.io/Gateway` to
`edgion.io/EdgionConfigData`.

## Controller-Side Processing

### preparse

- Calls `EdgionConfigData::validate_pre_parse()` for per-variant validation (invalid regex for `KeyList`,
  invalid regions for `RegionRouteOverride`, invalid structure or digest for `WafRuleBundle`) → `Vec<String>`.
  Non-empty → `Accepted=False/Invalid`.
  `Selector` is always structurally valid (the active-name ↔ profile check is a consumer-side,
  connection-time concern, not a controller preparse check).
- **`IpList` is partial-good**: `validate_pre_parse` returns empty (no `Accepted=False`) for bad CIDR
  entries. Bad entries are skipped at compilation time and surfaced via the `PartiallyInvalid` condition
  in `update_status` (see below). Empty `items: []` is valid and compiles to an empty matcher.

### parse

- Stores the EdgionConfigData spec in the controller-internal `EdgionConfigDataStore` (indexed by `ns/name`).
- Validates overlay `type` consistency if a bound consumer is already known.

### on_change

- Looks up resources referencing this EdgionConfigData via `EdgionConfigDataRefManager` and requeues them.
  Consumers include Gateway inbound PROXY policies, EdgionPlugins (via `overrideRef` in `RegionRouteConfig`
  and `WafConfig.policyRef`, plugin `activeProfileRef`, and WafPolicy bundle refs) and EdgionStreamPlugins
  (via `activeProfileRef` in `GlobalConnectionIpRestrictionConfig`). Also requeues EdgionPlugins and
  EdgionStreamPlugins that reference this object as an `IpList` via `allowRefs`/`denyRefs` in
  IpRestriction configs or GCIR per-profile `ProfileRules`.

### on_delete

- Removes from `EdgionConfigDataStore`.
- Requeues all dependents so their `ResolvedRefs` is recomputed.

### update_status

| Condition | Semantics |
|-----------|-----------|
| `Accepted` | Spec passes preparse validation. For `IpList`, always `True` even when entries are bad (partial-good). |
| `ResolvedRefs` | EdgionConfigData itself carries no external cross-resource refs; own `ResolvedRefs` is always `True`. Note: `IpList` variants are ref targets — when `allowRefs`/`denyRefs` in IpRestriction (HTTP/L4/TLS-route) or GCIR per-profile fail to resolve an `IpList`, `ResolvedRefs=False` is set on the *referring* EdgionPlugins/EdgionStreamPlugins, not on the `IpList` EdgionConfigData itself. |
| `PartiallyInvalid` | **`IpList` only**: `status=True`/`reason=RuleInvalid` (with message listing bad entries) when `collect_entry_errors` finds invalid CIDRs, empty groups, duplicate group names, invalid group names, or cross-group duplicate CIDRs (keep-first). `status=False`/`reason=AllRulesValid` when clean. `Accepted` stays `True`; partial-good entries are admitted and the rest still compile. |
| `UnknownConfigFields` | Present as `True/UnknownFieldsDetected` when a typed payload contains keys the generated schema does not recognize; removed entirely after a clean reapply. Advisory for ordinary data variants. `ProxyProtocolTrust` and `WafRuleBundle` reject unknown config fields through `Accepted=False` as well. |

## Gateway-Side Processing

Gateway stores EdgionConfigData in `EdgionConfigDataStore` (ArcSwap + variant-keyed indexes). Live reference resolution is
performed **at request time** (lock-free ArcSwap load). RegionRoute borrows the current raw overlay table;
configuration publication builds a compact `RegionTableValidation` proof in the same snapshot.
The request checks only that proof's maximum hash bound against its base modulo, without repeating
full endpoint, interval, or failover-graph validation or copying the region table.

- **Overlay absent or type-mismatch:** base used; metric counter incremented (`overlay_missing` /
  `overlay_type_mismatch` label on the relevant plugin's event counter).
- **RegionRoute envelope enable=false:** overlay ignored and base table used; the plugin is not disabled.

## Ownership

EdgionConfigData is the shared data resource used for both overlays and strict policy inputs.
The ordinary overlay model treats it as **Center-controlled data**, and its bodies are not an ops-repo
artifact. The split for overlays is deliberate: the git-owned half of the model is the base config
(`EdgionPlugins` and friends), which carries the *logic*; the EdgionConfigData overlay carries
only *data* and exists to be pushed and hot-swapped by EdgionCenter. `WafRuleBundle` is a non-overlay
exception: it may be distributed through the same resource API, but its content changes executable WAF
policy only after a consumer pins its digest. There is no ownership arbitration for this kind.

**The `edgion.io/managed-by` label is inert on EdgionConfigData — nothing reads it.**

- No handler consults it. `resource_processor/handlers/edgion_config_data.rs` has no label logic,
  and the reconcile path only stores the spec, requeues dependents, and writes status (see the
  four sections above). The `operator` value's stated meaning, "controller never mutates spec", was
  therefore not merely unenforced but wrong: the Controller's own Admin API failover endpoint
  (`edgion-controller/src/api/region_route_handlers.rs`) rewrites `spec.data.config.regions` on an
  EdgionConfigData, and consults no label while doing it.
- The `managed_by` constants in `edgion-resources/src/constants/labels.rs` have **no callers**
  anywhere in the workspace. They are declarations, not a mechanism.
- The label's only live use is on `Secret` and `EdgionTls`, never on EdgionConfigData, and it does
  not go through those constants: ACME writes `acme`
  (`edgion-controller/src/services/acme/service.rs`,
  `edgion-controller/src/services/acme/secret_io.rs`) and the conf-sync CA writes `conf-sync-ca`
  (`edgion-controller/src/services/conf_sync_ca/secret_io.rs`), each via its own local string
  literal.

Do not put `edgion.io/managed-by` on an EdgionConfigData manifest or sample: it changes no
behavior and reads as an ownership guarantee that does not exist. See
`skills/02-features/05-annotations/00-annotations-overview.md` (Labels section) for the label's
actual vocabulary, which covers only ACME- and conf-sync-CA-owned `Secret`s plus the
ACME-generated `EdgionTls`.

### Why there is no `center` / `controller` ownership mode

Both values were previously declared here as pending EdgionCenter coordination. The label-based
ownership design that would have consumed them — Center stamping `managed-by: center`, refusing to
take over an unowned object, and surfacing an `unowned-conflict` drift state — lived in
EdgionCenter's `tasks/pending/global-resource-management/` package, which was **superseded and
rejected on 2026-07-26** in favour of
`tasks/pending/center-controller-interaction-convergence/`. The current program authority,
`EdgionCenter/docs/superpowers/specs/2026-07-26-center-controller-write-model-convergence-design.md`,
declares no `managed-by` mechanism at all. Center's cross-cluster `GlobalResources` view is
read-only; Center's two *dedicated* write paths — the pair sharing its convergence-outcome write
core — are RegionRouteOverride failover and row-level copy, while general EdgionConfigData
create / update / delete stays available through the generic Controller Admin proxy, which is what
the default federation policy grants. Write safety throughout is a CAS property — `If-Match` on
`metadata.resourceVersion`, with explicit `converged` / `superseded` / `conflict` outcomes — not an
ownership epoch.

Full analysis and the decision to keep it that way:
[`../../04-review/resources/intentional-design/edgion-config-data-managed-by-is-inert.md`](../../04-review/resources/intentional-design/edgion-config-data-managed-by-is-inert.md).

## Cross-Resource Relationships

| Direction | Mechanism |
|-----------|-----------|
| EdgionConfigData ← EdgionPlugins | `overrideRef` in `RegionRouteConfig` → `RegionRouteOverride`; EP `ResolvedRefs` reflects resolution; cross-ns requires a `ReferenceGrant` or `visibility: Cluster` |
| EdgionConfigData ← EdgionPlugins | `WafConfig.policyRef` → `WafPolicy`; plugin `activeProfileRef` → `Selector`; policy profile `bundleRefs` → `WafRuleBundle`. Controller tracks every reference, chooses one profile, and embeds its accepted uncompiled bundles for Gateway revalidation. Required bundle failures invalidate a profile; explicit optional references may be skipped with `PartiallyInvalid`. Selector failures fall back to the validated default profile. |
| EdgionConfigData ← EdgionPlugins | `RequestAccessPolicy.activeProfileRef` → `Selector`; IP rules/conditions → `IpList`; URL rules → `RequestAccessUrlAllowList` | Selector failures affect `ResolvedRefs`; optional IP/URL data failures, including cross-ns denial, are omitted individually and reported through `PartiallyInvalid`. All refs use the shared reverse index and cross-namespace requeue path. |
| EdgionConfigData ← EdgionStreamPlugins | `activeProfileRef` in `GlobalConnectionIpRestrictionConfig` → `Selector`; ESP `ResolvedRefs` reflects resolution; cross-ns requires a `ReferenceGrant` or `visibility: Cluster` |
| EdgionConfigData ← {EdgionPlugins, EdgionStreamPlugins} | `allowRefs`/`denyRefs` in IpRestriction (HTTP/L4/TLS-route) or GCIR per-profile `ProfileRules` → `IpList`; referrer `ResolvedRefs` reflects resolution (missing/disabled/wrong-type → `ResolvedRefs=False`; cross-ns without grant/visibility → `RefNotPermitted`) |
| EdgionConfigData → {EdgionPlugins, EdgionStreamPlugins} | `EdgionConfigDataRefManager` requeue on change/delete |

> The old `ServiceRegionRoute → ClusterRegionRoute` PM→PM chain and `clusterRef` indirection were
> removed in the S3 refactor. RegionRoute logic now lives entirely in `EdgionPlugins` base config.

## Cross-Namespace Reference Grants

Cross-namespace EdgionConfigData references — all ref types: `allowRefs`/`denyRefs` (IpList),
`overrideRef` (RegionRouteOverride), `activeProfileRef` (Selector, including
`RequestAccessPolicy.activeProfileRef`, `WafConfig.activeProfileRef`), and WafPolicy profile
`bundleRefs` (WafRuleBundle) — are gated by two complementary controls.

### Mechanism

1. **`spec.visibility`** on the EdgionConfigData target (default `Namespace`):
   - `Namespace` — cross-ns refs require a matching Gateway-API `ReferenceGrant` in the target namespace.
   - `Cluster` — any namespace may reference it; no grant is needed.
   - Same-namespace refs are **always** permitted regardless of `visibility`.

2. **Gateway-API `ReferenceGrant`** in the target namespace (required when `visibility: Namespace`).
   Grant shape — `from.kind` must be `EdgionPlugins` or `EdgionStreamPlugins`:

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-team-a-to-blocklist
  namespace: edgion-system          # target namespace (where EdgionConfigData lives)
spec:
  from:
    - group: edgion.io
      kind: EdgionPlugins           # or EdgionStreamPlugins
      namespace: team-a
  to:
    - group: edgion.io
      kind: EdgionConfigData
      name: global-blocklist        # omit `name` to allow all EdgionConfigData in this namespace
```

This enforcement is gated by `is_reference_grant_validation_enabled()` (configured at controller
startup via `validation.enable_reference_grant_validation` in the controller config; restart-scoped;
defaults to `true`). When **disabled** (`false`), all cross-namespace refs are permitted
unconditionally — this is the permissive/legacy mode (behavior before this feature was introduced).

### Data-Plane Semantics When a Cross-Namespace Ref Is Denied

When a cross-ns ref is not permitted, the controller marks it `ref_denied` on the
`EdgionConfigDataRef` struct. The conf_sync payload carries `ref_denied` to the gateway, which
withholds that ref at runtime:

| Ref type | Denied behavior |
|----------|-----------------|
| `allowRefs` (IpList) | **Deny-all fail-closed**: `allow_mode` stays true but the referenced allow set is withheld — no IP is permitted through that ref. Whitelist intent is preserved; this is the safe fail direction. |
| `denyRefs` (IpList) | **Under-block**: the denied deny set is not applied. The plugin continues with remaining inline/permitted deny entries, but the denied external deny set is missing (possible security gap). |
| `overrideRef` (RegionRouteOverride) | **No override**: RegionRoute falls back to the base inline region table. Traffic continues (fail-to-base, not fail-closed). |
| `activeProfileRef` (Selector) | **Fail-to-base** (NOT fail-closed): the plugin uses its base/inline `activeProfile`. Unlike `allowRefs`, a denied selector does NOT block traffic — the Selector only switches among existing git-owned profiles, and the base profile is the safe default. |

### Status Conditions on the Referrer

`ResolvedRefs=False` is set on the **referrer** (`EdgionPlugins` or `EdgionStreamPlugins`) — not on
the EdgionConfigData target itself:

| Reason | Meaning |
|--------|---------|
| `RefNotPermitted` | A cross-ns ref exists but no matching `ReferenceGrant` (target `visibility: Namespace` and no grant). |
| `InvalidEdgionConfigDataRef` | Target EdgionConfigData is missing, disabled, or the wrong type. |

### `visibility: Cluster` — Exposure Footgun

Setting `visibility: Cluster` allows **all current and future namespaces** to reference the
EdgionConfigData with no per-grant audit trail. This is convenient for globally shared data
(shared blocklists, cluster-wide region tables) but bypasses namespace isolation entirely. To
revoke cluster-wide access, flip `visibility` back to `Namespace`; any existing cross-ns refs
without a matching `ReferenceGrant` then immediately resolve as `RefNotPermitted`.

### Example: Shared IpList Published as Cluster-Wide

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: global-blocklist
  namespace: edgion-system
spec:
  visibility: Cluster               # all namespaces may reference without a grant
  data:
    type: IpList
    config:
      items:
        - name: known-bad
          cidrs: ["203.0.113.0/24", "198.51.100.5/32"]
```

### Example: Per-Namespace Grant for a Cross-Namespace IpList

```yaml
# Grant lives in the target namespace (edgion-system), allows team-a EdgionPlugins to ref corp-allowlist
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-team-a-corp-allowlist
  namespace: edgion-system
spec:
  from:
    - group: edgion.io
      kind: EdgionPlugins
      namespace: team-a
  to:
    - group: edgion.io
      kind: EdgionConfigData
      name: corp-allowlist
```

## RegionRoute with EdgionConfigData Overlay

The RegionRoute plugin in `EdgionPlugins` now carries all routing logic inline (base) with an optional
`overrideRef` pointing to a `RegionRouteOverride` EdgionConfigData for dynamic region-table updates:

Base and overlay tables share resources-layer validation: nonblank unique names, strict host/port
syntax, disjoint closed ranges within the consumer modulo, and existing nonself acyclic failover
targets. Gaps remain pass-through and failover execution remains one hop even for legal chains.
The default modulo is 1000 without `hashCalc`. Non-empty `routeRules` require a non-empty base,
regardless of `overrideRef`; only no-rule no-ops may have an empty base. Overlay `regions: None`
inherits, while an explicit empty replacement is invalid. Missing/disabled/wrong-type/invalid
overlays use the valid base; there is no TTL or last-good overlay retention. See
[the RegionRoute regression boundary](../../04-review/routing/regression-guards/region-route-effective-table-validation.md)
for source evidence, management API alignment, and verification limits.

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
