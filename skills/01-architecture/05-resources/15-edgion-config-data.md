---
name: resource-edgion-config-data
description: EdgionConfigData typed data, ClusterRegionSetting identity and table ownership, consumer failure policies, reference authorization and atomic publication.
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
- Regional setting shape: `edgion-resources/src/resources/edgion_config_data/region_route.rs` (`ClusterRegionSetting`)
- Native WAF bundle shape: `edgion-resources/src/resources/edgion_config_data/waf_rule_bundle.rs` (`WafRuleBundleData`)
- GlobalConnectionIpRestriction plugin config (consumes `Selector` via `activeProfileRef`):
  `edgion-resources/src/resources/edgion_stream_plugins/global_connection_ip_restriction.rs` (`GlobalConnectionIpRestrictionConfig`)
- Live setting selection: `edgion-gateway/src/plugins/http/region_route/plugin.rs` (`compute_effective_regions`, `EffectiveRegions`)
- Controller handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_config_data.rs`
- Gateway store + index: `edgion-gateway/src/plugins/runtime/edgion_config_data/conf_handler.rs`
- Gateway store module: `edgion-gateway/src/plugins/runtime/edgion_config_data/mod.rs`

## Overlay Model

EdgionConfigData supplies typed data to consumers; it does not impose a universal
merge or failure policy. Selector consumers retain their own validated base/profile
behavior. Required regional settings use the distinct contract below.

### ClusterRegionSetting

The required `clusterRegionRef` of RegionRoute names a ClusterRegionSetting with
required clusterName, regionName and regions. Local identity is derived only from
regionName, which must name a row. There is no plugin-local base table or override
model. Names are exact, nonblank, control-free and at most 63 UTF-8 bytes.

The Gateway reads raw data and its compact validation proof from one immutable
snapshot. EffectiveRegions holds that resource Arc and borrows both identity and
table. Full endpoint/interval/graph validation happens at publication; request-time
work checks the proof against the consumer modulo without copying the table.

Missing, denied, disabled, invalid or wrong-type settings follow the plugin's
failOpen (default true) and statusOnError (default 503, 400..599). Continue performs
no partial key extraction/writeback/upstream mutation and runs subsequent plugins.
Reject remains executable even when operational configuration is invalid. Invalid
YAML/policy/structural safety errors retain generic rejection. No last-good setting
or base table is retained. A valid later generation restores routing.

Standalone invalid settings report Accepted=False. Referring plugins report
ResolvedRefs errors and consumer hash errors report Accepted=False. These statuses
are observations, not proof of Gateway application. The Controller effective API
returns an explicit observation DTO; it must not serialize arbitrary processed
plugin config. Required reference identity and grants use the canonical binding
and dependency requeue paths.

Signals use canonical PluginLog and six bounded reasons in
edgion_region_route_config_events_total. See the [user reference](../../../docs/en/user-guide/http-route/filters/edgion-plugins/region-route.md)
and [validation guard](../../04-review/routing/regression-guards/region-route-effective-table-validation.md).

## Variant Taxonomy

Wire shape: `data: { type: <Variant>, config: {...} }` (`#[serde(tag="type", content="config")]`).
Envelope (any variant): `enable: bool` (default true), `active: Option<String>`, `visibility: Namespace|Cluster` (default `Namespace` — governs cross-ns reference access; see [Cross-Namespace Reference Grants](#cross-namespace-reference-grants)).

| Group | Variant | Shape | Notes |
|-------|---------|-------|-------|
| std primitive | `KeyList` | `{ matchMode: exact\|regex, items: [KeyGroup] }` | Unified string+regex via `matchMode` |
| std primitive | `IpList` | `{ items: [IpGroup] }` | Radix-tree CIDR matcher; consumed by IpRestriction (HTTP/L4/TLS-route) and GCIR per-profile via `allowRefs`/`denyRefs` |
| typed policy data | `RequestAccessUrlAllowList` | `{ items: [UrlAccessCandidate] }` | URL allow candidates expanded into RequestAccessPolicy by the Controller |
| selector | `Selector` | `{ active?: string, description?: string }` | Active-profile selector; switches among the consumer's git-owned profiles |
| regional setting | `ClusterRegionSetting` | `{ clusterName, regionName, regions }` | Required local identity and complete table; strict payload |
| security policy | `ProxyProtocolTrust` | `{ mode: required }` or `{ mode: trustedSources, trustedCidrs: [...] }` | Non-overlay, all-or-nothing Gateway listener trust policy. Requires envelope `enable=true` and `active` absent. Unknown config fields are fatal rather than advisory. |
| executable policy data | `WafRuleBundle` | `{ version, profile, provenance, digest?, roots, rules, phraseAssets? }` | Non-overlay WAF rule and phrase-asset inputs. Structural validation is strict; an optional digest self-checks contents. A WAF profile aggregates ordered references with optional `expectedDigest` pins and strictly compiles the selected generation. Requires `enable=true`, `active` absent, and no unknown config fields. |
| WAF policy | `WafPolicy` | `{ profiles, defaultProfile }` | Shared aggregation of ordered bundle references. EdgionPlugins fixes one `policyRef` and may select a profile with `activeProfile` or `activeProfileRef`; the Controller transports the selected profile's accepted bundles for Gateway compilation. Requires `enable=true`, `active` absent, and no unknown config fields. |
| escape hatch | `Misc` | `x-kubernetes-preserve-unknown-fields` | Third-party / unknown / experimental only |

## Open Extension Types

`spec.data.type` is an open CRD string. Adding a supported `ConfigEntry` variant
requires its Rust schema, validation, consumers, and tests, but no CRD type whitelist
update or reinstall. Nested enums and known payload validation remain unchanged.
An unknown type decodes as a schema-hidden `Unsupported` unit marker. The unknown
name and config are discarded, including secret-like fields; conf_sync carries only
the normalized marker with an empty `config: {}` required by the CRD envelope.
Admin writes persist the normalized value, so an upgrade alone cannot recover
discarded input: reapply the original configuration after correcting the type or
upgrading the consuming binaries.

Controller validation sets `Accepted=False/Invalid`, even when `enable=false`.
The resource still replaces the raw store entry, clears outgoing WAF references,
requeues dependents, and reaches the Gateway. Both full and incremental publication
remove stale IpList matchers and RegionRoute proofs. Selector consumers use their base behavior; RegionRoute uses its configured
unavailable-setting policy; strict PROXY trust and required WAF references reject the wrong
type. Existing IP allow/deny reference semantics remain in force. A supported
replacement restores normal validation and runtime publication.

## Unknown Config Fields

The Controller applies the shared advisory schema walk to the typed `KeyList`, `IpList`,
`RequestAccessUrlAllowList` and `Selector` payloads. A key that serde would otherwise ignore is
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
overlay: GlobalConnectionIpRestriction now owns named reference-only profiles (git-owned plugin config) and
exposes only "which profile is active" as the hot-swappable surface — see the example below.

`ProxyProtocolTrust` and `WafRuleBundle` are exceptions to the ordinary overlay/advisory behavior. `WafRuleBundle`
contains executable rule input, so a wrong or omitted required config field invalidates the bundle instead of silently
reducing WAF coverage. Its standalone `Accepted=True` proves structure and any supplied self-check, not that a
consumer's combined rule generation is executable. `WafConfig.policyRef` resolves a `WafPolicy` that selects one named profile through a
fixed plugin choice, default, or `Selector`, then compiles its required references together before publication.
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
  invalid regions for `ClusterRegionSetting`, invalid structure or digest for `WafRuleBundle`) → `Vec<String>`.
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
  Consumers include Gateway inbound PROXY policies, EdgionPlugins (via `clusterRegionRef` in `RegionRouteConfig`
  and `WafConfig.policyRef`, plugin `activeProfileRef`, and WafPolicy bundle refs) and EdgionStreamPlugins
  (via `activeProfileRef` in `GlobalConnectionIpRestrictionConfig`). Also requeues EdgionPlugins and
  EdgionStreamPlugins that reference this object as an `IpList` via `allowRefs`/`denyRefs` in
  IpRestriction configs or GCIR per-profile `ProfileRules`.

### on_delete

- Removes from `EdgionConfigDataStore`.
- Requeues all dependents so their `ResolvedRefs` is recomputed.
- Clears the deleted owner's outgoing references but retains nonempty incoming
  references, so recreation can wake unchanged consumers. Owner replacement or
  deletion removes empty target buckets in the existing reference-manager write
  section; shared targets retain their other dependents. This tracks current
  edges without periodic GC and does not promise shrinking HashMap capacity.

### update_status

| Condition | Semantics |
|-----------|-----------|
| `Accepted` | Spec passes preparse validation. For `IpList`, always `True` even when entries are bad (partial-good). |
| `ResolvedRefs` | WafPolicy owns bundle references: required failures set its own `ResolvedRefs=False` (`RefNotPermitted` or `InvalidEdgionConfigDataRef`); other ConfigData variants have no such outgoing bundle references. Note: `IpList` variants are ref targets — when `allowRefs`/`denyRefs` in IpRestriction (HTTP/L4/TLS-route) or GCIR per-profile fail to resolve an `IpList`, `ResolvedRefs=False` is set on the *referring* EdgionPlugins/EdgionStreamPlugins, not on the `IpList` EdgionConfigData itself. |
| `PartiallyInvalid` | **`IpList`**: `status=True`/`reason=RuleInvalid` (with message listing bad entries) when `collect_entry_errors` finds invalid CIDRs, empty groups, duplicate group names, invalid group names, or cross-group duplicate CIDRs (keep-first). `status=False`/`reason=AllRulesValid` when clean. `Accepted` stays `True`; partial-good entries are admitted and the rest still compile. **WafPolicy** also reports optional bundle failures through `PartiallyInvalid`; required failures belong to `ResolvedRefs`. |
| `UnknownConfigFields` | Present as `True/UnknownFieldsDetected` when a typed payload contains keys the generated schema does not recognize; removed entirely after a clean reapply. Advisory for ordinary data variants. `ProxyProtocolTrust` and `WafRuleBundle` reject unknown config fields through `Accepted=False` as well. |

## Gateway-Side Processing

Gateway stores EdgionConfigData in `EdgionConfigDataStore` (ArcSwap + variant-keyed indexes). Live reference resolution is
performed **at request time** (lock-free ArcSwap load). RegionRoute borrows the current raw setting identity and table;
configuration publication builds a compact `RegionTableValidation` proof in the same snapshot.
The request checks only that proof's maximum hash bound against its configured modulo, without repeating
full endpoint, interval, or failover-graph validation or copying the region table.

HTTP, connection, TLS-route and global-connection IpRestriction capture one
ConfigData snapshot for a decision and borrow referenced matchers lazily through
`ip_list_matchers`. They preserve configured source order without constructing a temporary matcher-reference collection.
GCIR Selector and selected profile matchers use that same snapshot. Matcher borrows
belong to the snapshot generation; key slices are needed only during iteration.
This removes union projections, not all allocations from the surrounding plugin path.

- RegionRoute unavailable settings follow failOpen/statusOnError; disabling the
  setting invokes that policy rather than disabling the plugin entry.
- ClusterRegionSetting rejects unknown payload fields during deserialization.

## Ownership

EdgionConfigData is the shared data resource used for both overlays and strict policy inputs.
The ordinary overlay model treats it as **Center-controlled data**, and its bodies are not an ops-repo
artifact. The split for overlays is deliberate: the git-owned half of the model is the base config
(`EdgionPlugins` and friends), which carries the *logic*; the EdgionConfigData overlay carries
only *data* and exists to be pushed and hot-swapped by EdgionCenter. `WafRuleBundle` is a non-overlay
exception: it may be distributed through the same resource API, but its content changes executable WAF
policy after validation. Consumers may pin the computed digest; an unpinned valid update takes effect automatically. There is no ownership arbitration for this kind.

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
core — are ClusterRegionSetting failover and row-level copy, while general EdgionConfigData
create / update / delete stays available through the generic Controller Admin proxy, which is what
the default federation policy grants. Write safety throughout is a CAS property — `If-Match` on
`metadata.resourceVersion`, with explicit `converged` / `superseded` / `conflict` outcomes — not an
ownership epoch.

Full analysis and the decision to keep it that way:
[`../../04-review/resources/intentional-design/edgion-config-data-managed-by-is-inert.md`](../../04-review/resources/intentional-design/edgion-config-data-managed-by-is-inert.md).

## Cross-Resource Relationships

| Direction | Mechanism |
|-----------|-----------|
| EdgionConfigData ← EdgionPlugins | `clusterRegionRef` in `RegionRouteConfig` → `ClusterRegionSetting`; EP `ResolvedRefs` reflects resolution; cross-ns requires a `ReferenceGrant` or `visibility: Cluster` |
| EdgionConfigData ← EdgionPlugins | `WafConfig.policyRef` → `WafPolicy`; plugin `activeProfileRef` → `Selector`; policy profile `bundleRefs` → `WafRuleBundle`. Controller tracks every reference, chooses one profile from the selector, fixed `activeProfile`, or policy default, and embeds its accepted uncompiled bundles for Gateway revalidation. Required bundle failures invalidate a profile; explicit optional references may be skipped with `PartiallyInvalid`. Selector failures fall back to the validated default profile. |
| EdgionConfigData ← EdgionPlugins | `RequestAccessPolicy.activeProfileRef` → `Selector`; IP rules/conditions → `IpList`; URL rules → `RequestAccessUrlAllowList` | Selector failures affect `ResolvedRefs`; optional IP/URL data failures, including cross-ns denial, are omitted individually and reported through `PartiallyInvalid`. All refs use the shared reverse index and cross-namespace requeue path. |
| EdgionConfigData ← EdgionStreamPlugins | `activeProfileRef` in `GlobalConnectionIpRestrictionConfig` → `Selector`; ESP `ResolvedRefs` reflects resolution; cross-ns requires a `ReferenceGrant` or `visibility: Cluster` |
| EdgionConfigData ← {EdgionPlugins, EdgionStreamPlugins} | `allowRefs`/`denyRefs` in IpRestriction (HTTP/L4/TLS-route) or GCIR per-profile `ProfileRules` → `IpList`; referrer `ResolvedRefs` reflects resolution (missing/disabled/wrong-type → `ResolvedRefs=False`; cross-ns without grant/visibility → `RefNotPermitted`) |
| EdgionConfigData → {EdgionPlugins, EdgionStreamPlugins} | `EdgionConfigDataRefManager` requeue on change/delete |

> The old `ServiceRegionRoute → ClusterRegionRoute` PM→PM chain and `clusterRef` indirection were
> removed in the S3 refactor. RegionRoute logic now lives entirely in `EdgionPlugins` base config.

## Cross-Namespace Reference Grants

Cross-namespace EdgionConfigData references — all ref types: `allowRefs`/`denyRefs` (IpList),
`clusterRegionRef` (ClusterRegionSetting), `activeProfileRef` (Selector, including
`RequestAccessPolicy.activeProfileRef`, `WafConfig.activeProfileRef`), and WafPolicy profile
`bundleRefs` (WafRuleBundle) — are gated by two complementary controls.

### Mechanism

1. **`spec.visibility`** on the EdgionConfigData target (default `Namespace`):
   - `Namespace` — cross-ns refs require a matching Gateway-API `ReferenceGrant` in the target namespace.
   - `Cluster` — any namespace may reference it; no grant is needed.
   - Same-namespace refs are **always** permitted regardless of `visibility`.

2. **Gateway-API `ReferenceGrant`** in the target namespace (required when `visibility: Namespace`).
   Grant source follows the actual owner: `EdgionPlugins` or `EdgionStreamPlugins` for plugin fields, and `EdgionConfigData` for WafPolicy bundle fields. The EP owning policyRef and Selector does not authorize the policy-owned bundles:

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-team-a-to-blocklist
  namespace: edgion-system          # target namespace (where EdgionConfigData lives)
spec:
  from:
    - group: edgion.io
      kind: EdgionPlugins           # EdgionStreamPlugins for stream fields; EdgionConfigData for WafPolicy bundles
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
| `denyRefs` (IpList) | **Under-block**: the denied deny set is not applied. The plugin continues with remaining permitted deny references, but the denied external deny set is missing (possible security gap). |
| `clusterRegionRef` (ClusterRegionSetting) | Required setting unavailable: execute failOpen/statusOnError before request mutation. |
| `activeProfileRef` (Selector) | Consumer-specific: Canary and GlobalConnectionIpRestriction use their validated base profile on invalid, denied or unavailable targets; WAF uses its validated default profile. RequestAccessPolicy fails closed: invalid or denied configured selectors produce an initialization-failed filter, and missing, disabled, wrong-type or unknown selections deny. An absent RAP selector, or an enabled Selector with no active value, uses its default profile. |

### Status Conditions on the Referrer

`ResolvedRefs=False` is set on the **referrer** (`EdgionPlugins`, `EdgionStreamPlugins`, or the WafPolicy `EdgionConfigData`) — not on
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

## RegionRoute with ClusterRegionSetting

See [the configuration reference](../../../docs/en/user-guide/http-route/filters/edgion-plugins/region-route.md)
for a complete setting and plugin example. The required reference preserves exact
namespace/name identity. regions belongs only to the setting; key extraction and
routing rules belong only to the plugin. regionName and regions are read from the
same generation.

## GlobalConnectionIpRestriction with a Selector

Unlike RegionRoute (required regional setting), the L4 `GlobalConnectionIpRestriction` stream plugin carries
named reference-only profiles (git-owned). Their IpList content updates independently; the *active profile* is selected via `activeProfileRef` → a `Selector` EdgionConfigData. The selector can only
switch among the plugin's own `profiles`; it can never inject rules or disable filtering.

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-policy-allow-1
  namespace: edgion-default
spec:
  data:
    type: IpList
    config:
      items:
      - name: office
        cidrs:
        - 10.0.0.0/8
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-policy-allow-2
  namespace: edgion-default
spec:
  data:
    type: IpList
    config:
      items:
      - name: any
        cidrs:
        - 0.0.0.0/0
---
- type: GlobalConnectionIpRestriction
  config:
    enable: true
    activeProfile: locked
    profiles:
      locked:
        defaultAction: deny
        allowRefs:
        - name: example-policy-allow-1
          namespace: edgion-default
        - name: corp-allowlist
      open:
        defaultAction: deny
        allowRefs:
        - name: example-policy-allow-2
          namespace: edgion-default
    activeProfileRef:
      name: gcir-active
      namespace: edgion-system
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: gcir-active
  namespace: edgion-system
spec:
  enable: true
  data:
    type: Selector
    config:
      active: open
      description: temporarily widen access
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

RegionRoute uses the required-setting failure policy described above; Selector
fallback does not imply a regional base table.

## Standalone IpList owning references

HTTP, connection, TLS-route and GCIR profile allowRefs/denyRefs use
`PreparedIpListReferences` at cold construction. The Controller shares the condition
IpList permission decision with fixed EP/ESP source and ConfigData target roles.
An omitted owner namespace resolves targets in `default`; reverse dependencies and
raw source/cache/rate identities retain the original resource key. Invalid target
slots have no read, Grant query or dependency. Authored allow intent survives a
filtered external set; deny-only failed external sets are dropped. Invalid owning
bindings reject reference-bearing construction independently of target availability.
Existing disabled-entry policy and structural config validation remain unchanged.

The Gateway retains only canonical target keys in the compiled plugin. It continues
to capture one immutable ConfigData snapshot per decision, preserving atomic IpList
updates and live content refresh for pinned plugin generations. Selector and other
ConfigData target roles are not migrated by this change.

### WAF reference identity boundary

WAF policyRef and Selector loans borrow the actual EdgionPlugins; bundle loans borrow the captured
WafPolicy ConfigData. Omitted namespaces use the actual owner's effective namespace (`default` when
owner metadata omits it). Raw source storage and dependency owner keys remain unchanged. Invalid
owner/target identities cause no target lookup, Grant query or outgoing edge. Valid missing or denied
references retain recovery edges. Snapshot capture validates these fields under one existing read guard,
with policy bounds checked before child copies. Existing shape failures stay structural; newly invalid
optional targets are unavailable units, while required targets invalidate their profile. Selector target
identity failure keeps the validated default fallback. Gateway selected-bundle transport checks establish
lexical consistency only, never WafPolicy ownership or Grant authority.

### Remaining plugin ConfigData field identity

RAP URL documents and RAP/Canary/GCIR Selectors, plus RegionRoute overrides, borrow
actual authored EP/ESP entries through `refs::config_data_fields`. Identity is
validated before target access, Grant queries or dependency registration. Omitted
owner namespace defaults to `default` for targets; raw owner queue and deletion
keys remain unchanged. Explicit empty namespaces and invalid names have no target
key, even when a malformed raw-store decoy exists. Missing valid targets retain
recovery dependencies. Current readonly Selector/Region diagnostics recompute
permission instead of trusting transported denial markers.

URL documents preserve partial-good expansion and the existing 64-warning bound;
imported conditions retain the consuming EP's authority. URL warnings are captured
during parse and reused by readonly status; status does not re-expand document
material. Same-namespace pure denial marking performs no material read after
identity validation. Availability resolution and cross-namespace marking still
use the existing owned store result; this is not a global zero-copy guarantee.

Gateway cold construction and cache eligibility validate actual owning slots.
Reference-free inline factory construction remains available, while reference-bearing
entries require their actual owner. Consumer fallback and denial retain their
existing policy/condition placement. GCIR still reads its Selector and selected
profile IpLists from one immutable snapshot; RegionRoute retains its paired raw
value and validation-proof lookup. No request-path identity parsing is added.

### Actual inbound PROXY reference field (C3)

Gateway inbound PROXY references now derive a typed GrantQuery from the actual Gateway field before dependency registration/acquisition. The consumer uses `ReferenceGrantStore::check_query` with the canonical `decide_edgion_config_data_ref`; validation flag, same-namespace and Cluster visibility exemptions retain their existing semantics. Identity errors precede every exemption. The bounded diagnostic identity is output metadata, not permission evidence; its failure stops resolution rather than permitting a lookup.

### Live shared IP consumers (ARCH-028 B2)

Shared Conditions and RAP primary IP fields contain `ipRefs` only. IpList groups
own all CIDRs; the central typed index owns their compiled matcher. Conditions
borrow one immutable snapshot through scoped scalar session reads. No list size
threshold or consumer expansion exists. Required availability recovers after a
list-only publication; RAP optional omissions remain a successful empty union.
Permission proofs bind actual source/target/root/leaf/slot and never bypass current
type/enable or Cluster-only visibility. Controller URL expansion and credentials
remain separate cold responsibilities.
