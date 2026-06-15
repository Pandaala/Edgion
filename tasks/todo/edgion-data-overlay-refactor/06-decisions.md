# 06 — Decisions

## D1 — IpList / KeyList: keep as standard primitives; delete current orphan plumbing

**Decision:** Keep `IpList` and `KeyList` as first-class standard primitives in the generalized EdgionData
taxonomy. Remove the *current* orphaned gateway indexes + getters (`ip_list_configs` / `key_list_configs`,
`get_ip_list_config` / `get_key_list_config`) — they are dead. Rebuild plumbing only when a real consumer is
wired, under the unified overlay/`dataRef` framework.

**Evidence:** `conf_handler.rs` getters are called only by unit tests (`:351-353`); HTTP `IpRestriction` /
`KeyAuth` use **inline** config, not PM refs; zero manifests use `type: IpList` / `type: KeyList`.

**Follow-on (out of this task's core scope):** optionally let `IpRestriction` / `KeyAuth` reference an
`IpList` / `KeyList` EdgionData via `dataRef` to get hot-swappable allowlists/keysets — this is the natural
first real consumer of these primitives. Tracked, not implemented here.

## D2 — First-party overlays are TYPED variants; `Misc` is an escape hatch only

**Decision:** `RegionRouteOverride` and `GlobalConnIpRestrictionOverride` are **typed enum variants**, not
`Misc`. `Misc` (`x-kubernetes-preserve-unknown-fields`) is reserved strictly for third-party / unknown /
experimental payloads. Principle: **everything first-party is typed** (standard primitives + first-party
overlays); `Misc` is just the open door.

**Evidence:** All PM variants already use `x-kubernetes-preserve-unknown-fields`, so typed-vs-Misc does NOT
change CRD-level validation (there is none for either). Typed buys serde deserialization validation + a place
to hang `validate_and_init` (range/endpoint checks, matcher pre-compile), as `GlobalConnectionIpRestrictionData`
already does (`global_connection_ip_restriction.rs:67-87`). Misc would force ad-hoc `serde_json::Value`
parsing in the plugin. RegionRoute/GlobalConnIp are first-party with known shapes → typed.

> Revises the earlier provisional note that `RegionRouteOverride` might ride `Misc`.

## D3 — Switch is envelope-only; no standalone `Switch` data variant

**Decision:** Put `enable: bool` (default true) + optional `active: String` on the EdgionData **top-level
envelope**, applicable to any variant. Do **not** add a standalone `Switch` data variant. Remove `Switch`
from the first-class variant list.

**Evidence:** Only `enable: bool` has a live consumer today (`stream/.../global_connection_ip_restriction/plugin.rs:45-47`),
and it lives inside a larger struct, not as standalone data. A `Switch` data variant has no consumer →
speculative (YAGNI; "no over-engineering"). Add later if a real "flag-as-data" consumer appears.

## D4 — Hard rename `PluginMetaData` → `EdgionData`, no deprecation/alias

**Decision:** Hard rename. No compatibility alias, no deprecation period.

**Evidence:** `kind: PluginMetaData` appears in 23 places — all integration-test fixtures + 1 CRD + comments
in the controller deploy YAML. **Zero** helm/production manifests; no shipping channel. A compat layer would
be wasted effort and conflicts with the project's "do not forward-port legacy keys" stance. Rename cost =
regenerate CRD + update 22 test YAMLs (folded into S1/S7).

## D5 — Do NOT split StringList / RegexList; keep the unified `KeyList(matchMode)`

**Decision:** No separate `StringList` / `RegexList` variants. `KeyList` already unifies them via
`matchMode: Exact|Regex`. Adding them back would reverse a deliberate prior consolidation. Revisit only if a
real need for a *lightweight flat* string/regex list (without `KeyGroup`/`MetaDataItem`) appears.

**Evidence:** `key_list.rs:1` "Key list metadata — unified StringList + RegexList"; `mod.rs:78` "Merges the
legacy StringList and RegexList into a single type." (Flag F1.)

## D6 — Keep two-level `type` + `config`; `type` is a guard before override

**Decision:** `EdgionData` keeps the current wire shape `data: { type: <Variant>, config: {...} }`
(`#[serde(tag = "type", content = "config")]`). The `type` discriminator additionally acts as a **type guard
at override resolution**: before applying an overlay, the consumer verifies the referenced EdgionData's `type`
matches the expected override variant. **Mismatch → reject the overlay, fail-to-base + signal** (do not coerce
or partially apply). E.g. a RegionRoute plugin's `overrideRef` pointing at a `KeyList` EdgionData is a type
mismatch → use base.

**Evidence:** wire shape `#[serde(tag = "type", content = "config")]` (`mod.rs:75`). (Flag F4.)

---

## Resulting taxonomy (final)

| Group | Variants | Typed? |
|-------|----------|--------|
| Standard primitives | `KeyList` (unified string+regex via `matchMode`), `IpList` | typed |
| First-party overlays | `RegionRouteOverride`, `GlobalConnIpRestrictionOverride` | typed |
| Escape hatch | `Misc` (`x-kubernetes-preserve-unknown-fields`) | opaque |

Envelope (any variant): `enable: bool` (default true), `active: Option<String>`.
Wire shape: `data: { type: <Variant>, config: {...} }`; `type` guards override binding (D6).
