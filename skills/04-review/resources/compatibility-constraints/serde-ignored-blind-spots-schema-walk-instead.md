---
name: serde-ignored-blind-spots-schema-walk-instead
description: Use when reviewing or proposing an unknown-key / unrecognized-field detector for a serde struct that uses #[serde(flatten)] or an adjacently-tagged enum (#[serde(tag, content)]), and someone suggests wiring in the serde_ignored crate. Records the measured result that serde_ignored is blind through BOTH constructs, why the deserializer's own content-buffering causes it, and the schema-walk approach (edgion-resources::schema_check) that replaces it, including the $ref-with-sibling-keywords pitfall in schemars 1.2.1 output.
status: measured-finding
knowledge_type: compatibility-constraints
---

# `serde_ignored` is blind through flatten AND adjacently-tagged enums — walk the schema instead

## Conclusion

Do not propose `serde_ignored` (or any deserializer-side "collect unvisited keys" approach)
as an unknown-key detector for `EdgionPlugins` / `EdgionStreamPlugins` config, or for any
other struct family using `#[serde(flatten)]` or `#[serde(tag = "type", content = "config")]`
(adjacently-tagged enums). It was measured against the real shapes and found blind through
both — not just flatten, which is the commonly cited limitation. The detector actually
shipped (`edgion-resources::schema_check::unknown_key_paths`) walks the `schemars`-generated
JSON Schema against the raw input instead, which sees through both constructs by construction.
The production implementation is `edgion-resources::schema_check`; its walker
and regression tests are the canonical executable specification.

## Why `serde_ignored` fails here

`serde_ignored` wraps a `Deserializer` and records every map key the target struct's
`Deserialize` impl never visits. That only works if the struct's own `Deserialize` impl
visits (or explicitly skips) each key as it streams past. Two constructs in this schema
defeat that assumption:

- **`#[serde(flatten)]`** — `PluginEntry` embeds the plugin via
  `#[serde(flatten)] pub plugin: EdgionPlugin`. serde's flatten implementation buffers the
  remaining map into an internal `Content`/`Value` representation before handing it to the
  inner type's `Deserialize`, and `serde_ignored`'s tracking wrapper cannot see through that
  buffer.
- **Adjacently-tagged enums** (`#[serde(tag = "type", content = "config")]`, what
  `EdgionPlugin` uses) — the tag-dispatching deserializer must read the whole `content`
  block into a buffer before it knows which variant's `Deserialize` to invoke (it doesn't
  know the type until it has seen `type`, which may arrive before or after `content` in the
  JSON), and that buffering hides the field visits from `serde_ignored` the same way flatten
  does.

## Measured result (probe crate mirroring the real shapes)

| Case | Input | `serde_ignored` reports |
|---|---|---|
| A | `requestPlugin:` (spec-level typo, no flatten/tag involved) | `["requestPlugin"]` |
| B | `config.denys:` inside a plugin entry (through the tag+content dispatch) | **nothing** |
| C | unknown key at entry level, next to `type` | **nothing** |
| D | `successStatusCodes` inside a flattened config | **nothing** |
| E | fully legitimate config (false-positive check) | nothing (correct) |
| F/G | same as B via YAML (`serde_yaml` → `Value`, and direct YAML deserializer) | **nothing** |
| H | adjacently-tagged enum fed directly, no flatten wrapper | **nothing** |
| I | plain nested structs, no flatten, no tagged enum | `["inner.denys", "outerTypo"]` |
| J | the config struct alone (no enum wrapper) | `["denys", "ipSourc"]` |

Case H isolates the effect: the blind spot is **not** limited to `#[serde(flatten)]`
(commonly cited). An adjacently-tagged enum alone is enough to hide everything inside
`content`, because the buffering happens at the tag-dispatch layer, not the flatten layer.
`serde_ignored` would only ever have caught case A — the spec-level layer, which is the
least valuable one, since every real-world typo this task exists to catch lives inside a
plugin's `config` block. There are no false positives from `serde_ignored` either (case E);
the mechanism is content buffering, not key collision, so this isn't fixable by deduplicating
keys.

## The replacement: walk `schema_for!`, not the deserializer

`schemars` 1.2.1's schema for these same shapes has the opposite property: it **inlines**
flattened fields directly into `properties` (no special-casing needed by the walker), and
renders the adjacently-tagged enum as `oneOf` branches, each carrying its own
`"type": {"const": "<Variant>"}` discriminator and a `"config": {"$ref": ...}` pointing at the
variant's field schema. A walker that resolves `$ref`/`$defs`, merges `allOf`, and picks the
matching `oneOf` branch by discriminator constant sees straight through both blind spots —
implemented in `edgion-resources/src/schema_check/{mod.rs,walk.rs}`.

## Pitfall found while implementing the walker: `$ref` with sibling keywords must MERGE, not replace

JSON Schema 2020-12 permits keywords beside `$ref`, and schemars 1.2.1 uses exactly that for
an **internally-tagged** enum branch (`#[serde(tag = "type")]`, no `content` — e.g.
`edgion_plugins::conditions::Condition`): the branch node itself is
`{"$ref": "#/$defs/Variant", "properties": {"type": {"const": "..."}}, ...}` — the
discriminator lives in the referencing node's OWN sibling keywords, while the variant's real
fields live in the `$ref` target.

A naive resolver that treats `$ref` as "replace this node with its target" discards those
sibling keywords. Measured cost: sweeping the 337 `EdgionPlugins` documents in
`edgion-tests/integration/conf/` produced 23 findings, of which **22 were false positives from
this one bug** — every internally-tagged enum reported its own discriminator key (for example
`requestPlugins[0].conditions.run[0].type`). Exactly 1 was real. The bug produced two compounding
effects (pinned by
`real_schema_internally_tagged_conditions_run_has_zero_findings` and the neighboring tests in
`edgion-resources/src/schema_check/tests.rs`):

1. The discriminator matcher never finds a matching branch (the `const` it needs lives only
   in the discarded siblings), so the walker falls into the "no branch matched" path and
   over-reports.
2. The discriminator key itself (`type`) gets reported as unknown, since it was never merged
   into the allowed-property set for that node.

**Fix**: `ref_chain()` in `edgion-resources/src/schema_check/walk.rs` returns every node
along the `$ref` chain (the referencing node first, then each target, up to
`MAX_REF_HOPS`), and the allowed-property set is the union across the whole chain — own
keywords take priority on a key collision, though none is observed in this codebase. Resolve
by merging the chain, never by replacing the node with its final target.

## Reference cases

- `edgion-resources/src/schema_check/walk.rs` module doc (rule 1) and `mod.rs`.
- `edgion-resources/src/schema_check/tests.rs` — the internally-tagged-enum regression tests.
- `skills/04-review/resources/intentional-design/deny-unknown-fields-credential-structs-forward-compat-tradeoff.md`
  — the sibling entry on the credential-injection defense this detector does NOT replace.
- `skills/04-review/deserialization/SKILL.md` — the standing deserialization-review guidance,
  corrected to point here.
