---
name: grouped-list-pattern
description: Design contract for list-shaped resources that need named groups for source attribution and audit (IP lists, key lists, regex lists, issuer lists, certificate trust stores).
---

# Grouped-list pattern

> Design contract for list-shaped resources (IP lists, key lists, regex lists, issuer lists, certificate trust stores, …) that need **named groups** for source attribution and audit. Read this **before** designing or extending any list field that should support grouping.
>
> Origin: extracted from the `plugin-grouping` pilot series (IpGroup → KeyAuth → HmacAuth → JwtAuth → BasicAuth → KeyList). The per-plugin review notes under [`../../04-review/<plugin>/`](../../04-review/) are the concrete instances of the rules below.

## 1. When grouping applies (and when it doesn't)

Add grouping when **all** are true:

- The field holds a flat list of *anonymous strings* (CIDR, raw key, username, JWT `iss`, certificate fingerprint, origin URL, regex, …) whose only identity at runtime is the value itself.
- Multiple sources (partners / tenants / environments) contribute entries to the same plugin instance.
- Operators want to reverse-engineer **which source** a hit came from for audit / management.

Do **not** add grouping when any of these hold:

- The list element already carries identity (mTLS subject DN/SAN, OIDC `sub`) — server-side grouping is redundant.
- There is no actor concept on a hit (CORS origin is value-match, not actor identity).
- The resource is single-valued (one LDAP server, one CSRF key, one rate, one IdP, one forward-auth URI, one rewrite target). Multi-source is modelled by multiple plugin / resource instances, not by grouping.
- The list already lives in a per-rule wrapper inside a named outer group (for example, `GeoRuleGroup.name` plus each `GeoRule`'s `source` / `key` / `values`). Adding sub-groups inside `GeoRule.values` doubles the schema depth without new attribution.

Closed cases (do not reopen without new requirement): see the "Closed: not grouped" register in [`../../04-review/SKILL.md`](../../04-review/SKILL.md).

## 2. Schema contract

| Rule | Detail |
|------|--------|
| **Field name** | `<thing>_groups: Vec<<Thing>Group>` (e.g. `secret_groups`, `issuer_groups`, `ca_secret_groups`, `origin_groups`). |
| **Group struct** | `{ name: String, items: Vec<…> }`. **Always use `String` for `name`** — `Arc<str>` is not `serde::{Serialize,Deserialize}` (serde `rc` feature only covers `Arc<T: Sized>`). |
| **Replace, do not dual-publish** | When grouping replaces a flat field, remove the flat field and update Controller, Gateway, CRD, fixtures, and docs atomically. Do not retain both fields, add aliases, or validate a legacy/new-field combination. |
| **Transport fields** | Add serde defaults only when the field is genuinely optional in the current schema, not to deserialize payloads from an older Controller. Controller and Gateway changes land together in this pre-release workspace. |
| **Group name validation** | Reuse [`validate_group_name`](../../../edgion-resources/src/resources/common/group_name.rs) (≤ 63 chars, K8s label-value charset `^[a-zA-Z0-9]([-_.a-zA-Z0-9]*[a-zA-Z0-9])?$`). Do not invent a per-plugin variant. |
| **CRD schema shape** | For nested `config:` under `requestPlugins[]`, declare grouped fields with `properties:` + `x-kubernetes-preserve-unknown-fields: true`. Do **not** try to use `oneOf` keyed on the sibling `type` field — CRDs cannot cross-field branch without `x-kubernetes-validations` CEL. |
| **Fixture coverage** | Update both `edgion-tests/integration/conf/EdgionPlugins/<Plugin>/` **and** `edgion-tests/k8s/conf/EdgionPlugins/<Plugin>/`. K8s integration breaks with `unknown field` if one side is missed. |

## 3. Controller resolution contract

- Resolve each group independently in `resolve_credential/<plugin>.rs` (or the equivalent reconcile path) and emit a merged map keyed by the lookup token, with the **group name** stored alongside the value.
- **Cross-group duplicate items**: use **first-wins** (groups are iterated in array order) and emit a `tracing::warn!` carrying both the *existing* and *incoming* group plus the offending item identifier (`secret_key`, `iss`, `cidr`, …). Rationale: grouping is a management / audit aid, **not** a tenant isolation primitive — a single misconfigured partner Secret must not take the whole plugin down. **Forbidden**: fail-fast (kills the resource) and last-wins (order-dependent, operator-unpredictable).
- Cross-group uniqueness validation reuses the helper pattern in `validate_groups_structure` (see `edgion-resources/src/matcher/ip_cidr_helpers.rs`); abstract a `HashSet`-based helper rather than re-implementing per plugin.

### 3.1 Partial-good compilation (IpList and inline IP lists)

`IpList` EdgionConfigData and the inline `allow`/`deny` groups on `IpRestriction` plugins (HTTP, L4 `ConnectionIpRestriction`, GCIR `ProfileRules`, TLS-route `IpRestriction`) use a **partial-good** compilation path instead of the strict all-or-nothing helper:

| Helper | Path | Used by |
|--------|------|---------|
| `validate_groups_structure` | Strict: any bad entry → `Err`, callers typically set `Accepted=False` | `real_ip::trusted_ips` (strict/all-or-nothing; intentional) |
| `build_grouped_matcher` | Strict: same, builds matcher only if all entries valid | Current strict callers that must treat a partial group as invalid |
| `collect_entry_errors(groups) -> Vec<EntryError>` | Partial-good: returns per-entry errors without aborting | Controller `update_status` for `PartiallyInvalid` condition; gateway metric gate |
| `build_grouped_matcher_partial(groups) -> (IpRadixMatcher, Vec<EntryError>)` | Partial-good: skips bad entries, builds matcher from valid ones | `validate_and_init` for all IP-restriction inline lists; `IpList` gateway index build |

**Skipped entry categories** (as produced by `collect_entry_errors`):
- Invalid CIDR string (parse failure)
- Empty `cidrs` list on a group (empty group)
- Cross-group duplicate CIDR: **keep-first** (first occurrence wins by group iteration order); later duplicates are skipped with a `tracing::warn!` at config-compilation/construction time and surfaced as an entry error on the controller side
- Duplicate group name: **keep-first** (same keep-first rule)
- Invalid group name (violates `validate_group_name` charset/length)

**Empty `items` is valid**: an `IpList` with `items: []` compiles to an empty matcher (no entries). This is distinct from a missing `allow`/`deny` field.

**Controller surface**: the controller calls `collect_entry_errors` in `update_status` and writes a `PartiallyInvalid` condition (`status=True`, `reason=RuleInvalid`, message listing the bad entries) when any errors exist; `status=False`/`reason=AllRulesValid` when clean. `Accepted` stays `True` — partial-good entries are admitted.

**Gateway metric**: a skipped inline **deny** entry increments `edgion_ip_deny_entries_skipped_total{scope="http"|"stream_l4"|"tls_route"}` (IpRestriction HTTP/L4/TLS-route only; GCIR per-profile deny is not metered by this counter — it uses `record_conn_ip_restriction_event`). A blacklist hole is the fail-open direction, so it gets a loud counter. Skipped **allow** entries are not metered (over-blocking is the safe direction).

## 4. Gateway runtime / observability contract

- **Hit branch** (`Allow` / `OK`): `PluginLog` must include `group=<name>`. Follow the `<verb>:group=<name>` shape that IpRestriction uses (`allow:group=<name>`, `deny:group=<name>`, `deny:default`, `deny:not-in-allow`) so existing log parsers keep working.
- **Failure / anonymous branch** (`Deny *`, `Allow anon`): **must not** include `group=`. Leaking the group on a failed credential lets an attacker enumerate multi-tenant topology by brute-forcing keys. This is an iron rule; every grouping plugin records it as `skills/04-review/<plugin>/group-in-ok-log.md`.
- **Tracing forbidden on the data plane**: all attribution signal on the gateway path goes through `PluginLog`, access-log diagnostics, or Prometheus metrics. `tracing::warn!/error!/info!` is reserved for the controller (resolution path, duplicate warnings). See [`../../03-coding/01-log-safety.md`](../../03-coding/01-log-safety.md) iron rule 3.

## 5. Access-log skill must be updated in the same PR

When `PluginLog` output changes shape (e.g. `OK user <X>` → `OK group=<g> user <X>`), update the corresponding plugin example in [`../../03-coding/observability/00-access-log.md`](../../03-coding/observability/00-access-log.md) **in the same commit**, and add a one-liner noting that the failure branch deliberately omits `group=`. Otherwise the next review run will treat the change as a regression against the stale skill.

## 6. Test contract

At minimum each grouped plugin / list must add:

- A **two-group integration case** (e.g. `partner-a` and `partner-b`) confirming both groups resolve and both hit lines carry the correct `group=` attribution.
- An **anonymous-only case** if the plugin supports anonymous (`*_groups` absent, anonymous policy unchanged).
- **Cross-group first-wins** covered at the **unit-test** level (integration is not worth the echo-upstream-header wiring).

For `DynamicTest` mode, new subdirectories (`initial_grouped/`, `updates_grouped/`, …) must be added to **both** skip lists:
`edgion-tests/integration/scripts/utils/start_all_with_conf.sh` *and* `edgion-tests/integration/scripts/utils/load_conf.sh`. Missing the second causes `updates_grouped/` to be applied during the initial phase and silently shadow the initial fixture.

## 7. Existing instances (templates to copy)

| Resource | Group field | Review notes |
|----------|-------------|--------------|
| `IpRestriction` / `IpList` / `GlobalConnectionIpRestriction` | `IpGroup` (original archetype) | [`../../04-review/ip-restriction/`](../../04-review/) |
| `KeyAuth` (pilot) | `secret_groups` | [`../../04-review/key-auth/`](../../04-review/key-auth/) |
| `HmacAuth` | `secret_groups` | [`../../04-review/hmac-auth/`](../../04-review/hmac-auth/) |
| `JwtAuth` | `issuer_groups` | [`../../04-review/jwt-auth/`](../../04-review/jwt-auth/) |
| `BasicAuth` | `secret_groups` | [`../../04-review/basic-auth/`](../../04-review/basic-auth/) |
| `EdgionConfigData::KeyList` | `KeyGroup` | [`../../04-review/plugin-metadata/keylist-group-structure.md`](../../04-review/plugin-metadata/keylist-group-structure.md) |

When adding a new grouped list, open a `skills/04-review/<plugin>/` directory and copy the `group-in-ok-log.md` + `dup-key-first-wins.md` templates from the closest existing instance.
