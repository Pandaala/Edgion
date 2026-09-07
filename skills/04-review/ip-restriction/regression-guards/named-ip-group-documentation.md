---
name: named-ip-group-documentation
description: Use when reviewing IpRestriction documentation or examples for the inline allow and deny schema shared by HTTP, L4 stream, and TLS-route plugins.
knowledge_type: regression-guards
---

# IpRestriction documentation uses named `IpGroup` entries

## Conclusion and scope

Every documented inline `IpRestriction` `allow` or `deny` entry must use the
current named `IpGroup` shape: required `name`, optional `description`, and
non-empty `cidrs`. This applies to HTTP `IpRestrictionConfig`, L4
`ConnectionIpRestrictionRules`, TLS-route IP restriction, related migration
examples, and examples that compose IP restriction with another plugin.

The obsolete `allowlist`/`denylist` keys and flat CIDR arrays under `allow` or
`deny` are not compatibility forms and must not appear in usable examples.

## Rationale and evidence

The canonical grouped-list contract requires atomic replacement of flat fields
without aliases (`skills/01-architecture/00-common/04-grouped-list-pattern.md`).
Current source types independently confirm the wire shape:

- `edgion-resources/src/resources/edgion_config_data/ip_group.rs::IpGroup`
  defines `name`, optional `description`, and `cidrs`.
- `IpRestrictionConfig::allow`/`deny` and
  `ConnectionIpRestrictionRules::allow`/`deny` are
  `Option<Vec<IpGroup>>`.
- HTTP, L4 stream, and TLS-route initialization compile those groups with the
  canonical partial-good matcher path.

D0049 found that source Rustdoc, agent knowledge, migration material, and
bilingual user guides still contained examples from the removed flat schema.
Those examples could not represent the current structs and could mislead an
operator into applying an invalid or ineffective access-control configuration.

## Limits

This guard concerns the inline IP-restriction schema only. Natural-language
terms such as “allowlist”, resource names such as `corp-allowlist`, and unrelated
fields such as Wasm `calloutAllowlist` are not obsolete IP-restriction keys.

Documentation alignment does not change deny-before-allow evaluation,
`defaultAction`, `DirectPeerIp`/`RemoteIp`, partial-good CIDR compilation, or
`allowRefs`/`denyRefs` resolution semantics. Integration fixtures already use the
current schema and are intentionally outside this documentation-only correction.

## Verification and re-evaluation

Search documentation, skills, and source Rustdoc for all of the following when
the schema or examples change:

- `allowlist:` or `denylist:` inside an `IpRestriction` configuration;
- inline `allow: [...]` or `deny: [...]` CIDR arrays;
- an `allow`/`deny` sequence whose entries are scalar IP/CIDR strings rather
  than objects containing `name` and `cidrs`;
- English/Chinese examples that no longer have equivalent schema shapes.

Re-evaluate this rule only if the canonical resource structs or grouped-list
contract change. The D0049 correction passed `cargo fmt --all`, workspace
all-target `cargo check` and Clippy, agent-document validation, and the SSA,
metrics-inventory, and Pingora-source guards. An independent reviewer also ran
`cargo test -p edgion-resources ip_restriction --lib` (65 passed, 0 failed).

## Resolution references

- D0049 — documentation-correctness finding discovered during B0082; corrected
  by aligning Rustdoc, skills, migration docs, and English/Chinese user examples
  without changing schemas, runtime behavior, or integration tests.
