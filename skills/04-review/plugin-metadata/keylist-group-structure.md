---
name: keylist-group-structure
description: Use when reviewing PluginMetaData::KeyList schema or validate_pre_parse changes; documents the KeyGroup grouping contract (mirrors IpListData/IpGroup) and that any regression to flat Vec<MetaDataItem> must be rejected.
---

# PluginMetaData::KeyList uses KeyGroup (mirrors IpListData/IpGroup)

After the 04-keylist-metadata pilot (2026-05-18), `KeyListData.items` is
`Vec<KeyGroup>` — NOT the historical flat `Vec<MetaDataItem>`. Each
`KeyGroup` carries `{ name, description?, items: Vec<MetaDataItem> }` and
mirrors `IpListData::IpGroup`'s name/description/cidrs structure.

## Invariants (enforced by `validate_key_groups_structure`)

1. **Groups list non-empty** — empty `items: []` is rejected
2. **Cross-group `name` uniqueness** — duplicate group name is rejected
3. **Per-group `name` validity** — must match the shared K8s label-value
   charset (`validate_group_name`)
4. **Per-group `items` non-empty** — an empty group is rejected
5. **Per-item `key` non-empty** — empty string keys are rejected
6. **Literal `key` uniqueness across AND within groups** — Regex mode does
   NOT expand to pattern equivalence; operator is responsible for
   non-overlapping literal patterns
7. **Regex mode compile check** — each `key` must compile via
   `regex::Regex::new` (error message carries `group '<g>' item '<key>'`
   prefix)

## What to reject in PR review

- Any reversion to `KeyListData.items: Vec<MetaDataItem>` (flat schema)
- Any new code path that bypasses `validate_pre_parse` for KeyList (e.g.
  inline Regex check duplicating the helper)
- Any "first-wins" / "last-wins" / silent-skip handling of duplicate keys
  — KeyList is declarative; duplicates MUST be preparse-rejected
- Adding `match_mode` per-group (must stay plugin-level; YAGNI)

## Related

- `edgion-resources/src/resources/edgion_data/key_list.rs` (KeyListData/KeyGroup)
- `edgion-resources/src/matcher/key_group_helpers.rs::validate_key_groups_structure`
- `edgion-resources/src/resources/edgion_data/mod.rs` (validate_pre_parse KeyList branch)
- `edgion-resources/src/resources/edgion_data/ip_list.rs` (mirror reference)
- `tasks/todo/plugin-grouping/04-keylist-metadata-spec.md` (design record)
