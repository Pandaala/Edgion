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
- The list already lives in a per-rule wrapper that itself acts as the group (e.g. `RestrictionRule.name/source/key`). Adding sub-groups doubles the schema depth without new attribution.

Closed cases (do not reopen without new requirement): see the "Closed: not grouped" register in [`../../04-review/SKILL.md`](../../04-review/SKILL.md).

## 2. Schema contract

| Rule | Detail |
|------|--------|
| **Field name** | `<thing>_groups: Vec<<Thing>Group>` (e.g. `secret_groups`, `issuer_groups`, `ca_secret_groups`, `origin_groups`). |
| **Group struct** | `{ name: String, items: Vec<…> }`. **Always use `String` for `name`** — `Arc<str>` is not `serde::{Serialize,Deserialize}` (serde `rc` feature only covers `Arc<T: Sized>`). |
| **Mutual exclusion with the flat field** | The legacy flat field (`secret_refs`, `issuer_refs`, …) and the new `*_groups` must be **preparse-validated as exclusive** in `validate_and_check` / `detect_validation_error`. Emit a precise error pointing at which two fields collided. |
| **Rolling-upgrade safety** | Any new field added to a struct that crosses the controller → gateway gRPC boundary (`ResolvedKey`, `HmacCredential`, `ResolvedJwtCredential`, `ResolvedUser`, `KeyMetadata`, …) **must** carry `#[serde(default = "…")]` with a reasonable default. Otherwise a new gateway deserializing an old controller's payload fails for the whole `EdgionPlugins` resource. |
| **Group name validation** | Reuse [`validate_group_name`](../../../src/types/resources/common/group_name.rs) (≤ 63 chars, K8s label-value charset `^[a-zA-Z0-9]([-_.a-zA-Z0-9]*[a-zA-Z0-9])?$`). Do not invent a per-plugin variant. |
| **CRD schema shape** | For nested `config:` under `requestPlugins[]`, declare grouped fields with `properties:` + `x-kubernetes-preserve-unknown-fields: true`. Do **not** try to use `oneOf` keyed on the sibling `type` field — CRDs cannot cross-field branch without `x-kubernetes-validations` CEL. |
| **Fixture coverage** | Update both `examples/test/conf/EdgionPlugins/<Plugin>/` **and** `examples/k8stest/conf/EdgionPlugins/<Plugin>/`. K8s integration breaks with `unknown field` if one side is missed. |

## 3. Controller resolution contract

- Resolve each group independently in `resolve_credential/<plugin>.rs` (or the equivalent reconcile path) and emit a merged map keyed by the lookup token, with the **group name** stored alongside the value.
- **Cross-group duplicate items**: use **first-wins** (groups are iterated in array order) and emit a `tracing::warn!` carrying both the *existing* and *incoming* group plus the offending item identifier (`secret_key`, `iss`, `cidr`, …). Rationale: grouping is a management / audit aid, **not** a tenant isolation primitive — a single misconfigured partner Secret must not take the whole plugin down. **Forbidden**: fail-fast (kills the resource) and last-wins (order-dependent, operator-unpredictable).
- Cross-group uniqueness validation reuses the helper pattern in `validate_groups_structure` (see `src/core/common/matcher/ip_cidr_helpers.rs`); abstract a `HashSet`-based helper rather than re-implementing per plugin.

## 4. Gateway runtime / observability contract

- **Hit branch** (`Allow` / `OK`): `PluginLog` must include `group=<name>`. Follow the `<verb>:group=<name>` shape that IpRestriction uses (`allow:group=<name>`, `deny:group=<name>`, `deny:default`, `deny:not-in-allow`) so existing log parsers keep working.
- **Failure / anonymous branch** (`Deny *`, `Allow anon`): **must not** include `group=`. Leaking the group on a failed credential lets an attacker enumerate multi-tenant topology by brute-forcing keys. This is an iron rule; every grouping plugin records it as `skills/04-review/<plugin>/group-in-ok-log.md`.
- **Tracing forbidden on the data plane**: all attribution signal on the gateway path goes through `PluginLog`, the access log's `EdgionStatus`, or Prometheus metrics. `tracing::warn!/error!/info!` is reserved for the controller (resolution path, duplicate warnings). See [`../../03-coding/01-log-safety.md`](../../03-coding/01-log-safety.md) iron rule 3.

## 5. Access-log skill must be updated in the same PR

When `PluginLog` output changes shape (e.g. `OK user <X>` → `OK group=<g> user <X>`), update the corresponding plugin example in [`../../03-coding/observability/00-access-log.md`](../../03-coding/observability/00-access-log.md) **in the same commit**, and add a one-liner noting that the failure branch deliberately omits `group=`. Otherwise the next review run will treat the change as a regression against the stale skill.

## 6. Test contract

At minimum each grouped plugin / list must add:

- A **two-group integration case** (e.g. `partner-a` and `partner-b`) confirming both groups resolve and both hit lines carry the correct `group=` attribution.
- An **anonymous-only case** if the plugin supports anonymous (`*_groups` absent, anonymous policy unchanged).
- **Cross-group first-wins** covered at the **unit-test** level (integration is not worth the echo-upstream-header wiring).

For `DynamicTest` mode, new subdirectories (`initial_grouped/`, `updates_grouped/`, …) must be added to **both** skip lists:
`examples/test/scripts/utils/start_all_with_conf.sh` *and* `examples/test/scripts/utils/load_conf.sh`. Missing the second causes `updates_grouped/` to be applied during the initial phase and silently shadow the initial fixture.

## 7. Existing instances (templates to copy)

| Resource | Group field | Review notes |
|----------|-------------|--------------|
| `IpRestriction` / `IpList` / `GlobalConnectionIpRestriction` | `IpGroup` (original archetype) | [`../../04-review/ip-restriction/`](../../04-review/) |
| `KeyAuth` (pilot) | `secret_groups` | [`../../04-review/key-auth/`](../../04-review/key-auth/) |
| `HmacAuth` | `secret_groups` | [`../../04-review/hmac-auth/`](../../04-review/hmac-auth/) |
| `JwtAuth` | `issuer_groups` | [`../../04-review/jwt-auth/`](../../04-review/jwt-auth/) |
| `BasicAuth` | `secret_groups` | [`../../04-review/basic-auth/`](../../04-review/basic-auth/) |
| `PluginMetaData::KeyList` | `KeyGroup` | [`../../04-review/plugin-metadata/keylist-group-structure.md`](../../04-review/plugin-metadata/keylist-group-structure.md) |

When adding a new grouped list, open a `skills/04-review/<plugin>/` directory and copy the `group-in-ok-log.md` + `dup-key-first-wins.md` templates from the closest existing instance.
