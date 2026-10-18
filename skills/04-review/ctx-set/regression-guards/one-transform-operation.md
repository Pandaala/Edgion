---
name: one-transform-operation
description: Use when reviewing CtxSet transform configuration that stacks replace, substring, case, prefix, suffix, or enabled trim on one rule.
knowledge_type: regression-guards
---

# CtxSet accepts at most one transform operation per rule

## Conclusion and scope

`TransformConfig` on a CtxSet `vars[]` rule may configure zero or one of
`replace`, `substring`, `case`, `prefix`, `suffix`, and enabled `trim`.
`trim: false` is not an operation. Two or more configured operations are
rejected by shared `CtxSetConfig` validation before Controller status and
Gateway `compile_plugins` publication.

Chaining remains valid: a later rule may read the previous context variable
(`from.type: ctx`) and apply another single operation. Gateway
`apply_transform` still uses first-wins priority only as defense for a
configuration that bypasses validation.

This rule does not change default ingest trimming. Source values, including
spaces, stay as read unless the operator sets `trim: true` on that rule.

## Rationale and evidence

Authority: the task contract and current shared validation. Older user docs
said "at most one transformation" without counting operations, so a stacked
`transform` still published. `check_fields` / `validate` previously validated
only replace regex and substring bounds, and the Gateway executed the first of
`replace > substring > case > prefix > suffix > trim`.

`TransformConfig::configured_operation_count` now counts configured operations.
Both `check_fields` (Controller `validate_plugins` / `validation_error`) and
`validate` (Gateway compile caches) emit
`vars[{i}] '{name}': transform allows at most one operation`.

- `edgion-resources/src/resources/edgion_plugins/plugin_configs/ctx_set.rs`
- `edgion-gateway/src/plugins/http/ctx_set/plugin.rs` keeps defensive priority
  in `apply_transform` and `malformed_multi_operation_transform_keeps_priority`

## Borrowed validation ownership (ARCH-067)

After the operation-count gate, both validators borrow `replace.pattern` and
copy the substring tuple directly from `TransformConfig`. They do not build an
owned transform projection or copy prefix, suffix, or replacement strings just
to select validation. The unused `TransformType` projection and helper are removed.
`validate` still clears and populates its owned compiled caches; `check_fields`
remains stateless. Gateway runtime configuration and output ownership stay unchanged.
Large-operand tests compare both validators and verify that stateless checks leave
existing caches untouched while `validate` refreshes them after a transform change.

## Alternatives and limits

Trigger: one rule's `transform` sets two or more operations.
Root cause: optional fields were independently deserializable and unchecked.
Correct outcome: shared validation rejects the rule; chained single-operation
rules still compile.

This does not compose operations inside one `transform` object, and it does
not forbid spaces in context values or HTTP header values.

## Verification and re-evaluation

Targeted tests in this change:

- `cargo test -p edgion-resources --lib resources::edgion_plugins::plugin_configs::ctx_set`: 33 passed
- `cargo test -p edgion-gateway --lib plugins::http::ctx_set::plugin`: 8 passed

Re-evaluate if transform fields are added, if `trim: false` is given operation
semantics, or if Gateway `apply_transform` starts applying more than one
operation.

## Resolution references

- `enforce-one-ctx-set-transform-operation`: original todo for this validation
  contract; closed after the 2026-09-19 fix.

## Runtime input ownership

`process_rule` borrows static values and canonical session `key_value` results
through a `Cow<str>` until extraction/transformation needs owned output. Templates
still render an owned value. Mapping values/defaults may borrow configuration;
the rule returns an owned String before `run_request` mutates session context.
Early rule-default returns and failure short circuits retain their semantics.
Present empty and whitespace-only source values do not become missing values.
Sequential rule writes remain visible to later context reads.

Focused tests cover source/static extraction, transform and mapping/default paths,
empty/whitespace preservation, and ordered context write/read/write. A warmed
source-to-mapping fixture checks the final result allocation only; this is not a
whole-callback allocation or throughput claim.
