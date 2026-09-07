---
name: config-struct-field-ordering-padding-waste
description: Use when reviewing findings that suggest reordering config struct fields by size to reduce alignment padding; field order is part of the user-visible JSON schema and readability outweighs negligible padding.
knowledge_type: intentional-design
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# Configuration Struct Field Ordering Causing Padding Waste

**False-positive scenario**: the audit flags configuration structs with `JsonSchema` such as `OpenidConnectConfig` (`edgion-resources/src/resources/edgion_plugins/plugin_configs/openid_connect.rs:78-227`) where bool, String/u64/Option are interleaved; each instance wastes 30-50 B of alignment padding; suggests reordering by field size for tight packing.

**Reality**:

1. **Negligible scale**: plugin configs are instantiated per route; hundreds of instances × 30-50 B = a dozen KB total waste; not measurable in profile.
2. **Not on the hot path**: config is loaded once during reconcile; the request path only reads a few fields; cache-line utilization is unrelated to padding.
3. **Field order is part of the user-visible schema**: such structs typically have `#[derive(JsonSchema)]` + `serde(rename_all = "camelCase")`; field order maps directly to the generated CRD / JSON Schema documentation. Reordering by alignment would result in a counterintuitive "bool first / String later" order in the external schema; new fields would have to choose between "minimum padding" vs "logical grouping".
4. **Readability >> padding**: the current struct is sectioned by Required / Auth behavior / OIDC params / Token verification / Header forwarding / Network / Runtime / Security extras; maintainers can locate field categories at a glance. Breaking up grouping for an unobservable dozen-KB memory benefit is not worth it.

**Verdict**: not a memory issue. Any "config struct field-order optimization for padding / reorder by size" finding is closed per this entry, unless (a) the struct is allocated/copied ≥ a thousand times per second on the request path, or (b) per-instance padding ≥ KB scale.
