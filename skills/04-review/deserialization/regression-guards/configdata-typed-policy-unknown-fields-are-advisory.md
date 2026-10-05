---
knowledge_type: regression-guards
---

# Typed ConfigData policy fields use advisory unknown-field handling

## Conclusion

Ordinary operator-facing typed `EdgionConfigData` payloads, including
`RequestAccessUrlAllowList`, must remain deserializable when an otherwise valid
document contains an unknown non-credential field. The shared schema walker
reports the exact path; apply remains warn-not-block, while the read-only Admin
validate endpoint returns `ok: false` with `unknownFieldCheck: "performed"`.

Credential and authentication sub-structures remain strict. In particular,
`SecretObjectReference`, SecretMatch, and HMAC credential shapes keep their
`deny_unknown_fields` boundary.

This advisory rule excludes `ProxyProtocolTrust`, `WafRuleBundle`, and `WafPolicy`:
their security-sensitive payloads reject unknown config fields. It also excludes
unknown extension discriminators. An unsupported `spec.data.type` decodes into an
invalid marker, reports `Accepted=False`, and replaces the old generation so typed
consumer state is withdrawn. The historical resource-wide wording below applies
only to ordinary data fields, not these stricter policy or type boundaries.

## Core rationale

`EdgionConfigData` has one resource-wide unknown-field contract. Making a
single typed variant fail during serde parsing prevents the raw schema finding
from reaching its sidecar, handler condition, Admin warning, and validation
response. It also makes behavior depend on which typed variant carries the
same class of typo.

The advisory boundary covers the URL-list envelope, URL candidates, path
matches, and IP matches. These structs still derive `JsonSchema`, so removing
serde's hard rejection does not make the schema open: the shared walker keeps
detecting their unknown properties.

## Fix suggestions not accepted

- Keep `RequestAccessUrlAllowList` strict because it carries access-policy
  data: the project explicitly selected lenient admission plus visible
  diagnostics for operator-facing policy fields.
- Remove strictness from credential/auth structs: that is a separate security
  boundary preventing inline or misspelled credential material from silently
  degrading authentication.
- Run the schema walk only after typed parsing fails: a failed typed parse does
  not produce a resource that can continue through the normal apply contract.

## Re-evaluation triggers

Re-evaluate if EdgionConfigData adopts a documented resource-wide strict
admission mode, or if operator and runtime credential shapes become separate
wire types.

## Reference case

- UFD-04 in [`review knowledge`](../../SKILL.md)
- Regressions:
  `config_data_url_allow_list_typo_is_advisory_with_gate_off_and_on`,
  `config_data_typed_variants_report_exact_nested_paths`, and
  `url_allow_list_nested_typo_reaches_advisory_validation_result`

## Resolution references

- `UFD-04` — 2026-08-22.
