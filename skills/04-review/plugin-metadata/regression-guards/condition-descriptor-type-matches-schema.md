---
name: condition-descriptor-type-matches-schema
description: Use when reviewing compiled Gateway descriptors or cond_skip type tokens. Runtime type tokens must match the public schema discriminator; runtime path descriptors and schema diagnostic summaries have separate owners.
knowledge_type: regression-guards
---

# Condition descriptor type matches the schema discriminator — FIXED (cl-07)

## Conclusion

Fixed on 2026-08-24. The probability condition used to emit the internal
abbreviation `prob` in compiled descriptors and `cond_skip`, although its public
YAML discriminator is `probability`. It now emits `probability` consistently.

## Invariant

The compiled Gateway descriptor uses `<path>:<type>[:<accessor>]`; the outer
condition-skip log prefixes the action (`skip:` or `!run:`). Its type token must
match the public serde/YAML discriminator. `condition_descriptor` in the Gateway
compiled evaluator currently owns this formatting; do not add log-only
abbreviations. For example, `!run:run.allOf[0]:probability` is the current skip
cause for both plain and keyed probability conditions; no probability key value
or accessor is included. `keyMatch` and `keyExist` use `KeyGet::as_log_str`,
including `cookie:session_id` and `method`, not the old `cke`/`mtd` tokens.

The resource test `diagnostic_descriptor_is_value_free_and_ordinal_bearing`
covers schema diagnostic summaries; it does not establish the Gateway runtime
path format. Gateway evaluator tests and the AllConditions exact-cause oracle
cover the runtime side. ARCH-383 corrected stale flat-ordinal/comma-format
guidance while retaining the cl-07 discriminator requirement. Successful
single-leaf decisions do not require match-only log descriptors; see the
[plugin architecture](../../../01-architecture/02-gateway/05-plugin-system.md).

## Resolution references

- `cl-07` — 2026-08-24.
