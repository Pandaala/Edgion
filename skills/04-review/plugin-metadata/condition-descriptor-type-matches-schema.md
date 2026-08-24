---
name: condition-descriptor-type-matches-schema
description: Use when reviewing Condition descriptor or cond_skip type tokens. The token must exactly match the public YAML discriminator returned by the canonical Condition::cond_type mapping.
---

# Condition descriptor type matches the schema discriminator — FIXED (cl-07)

## Conclusion

Fixed on 2026-08-24. The probability condition used to emit the internal
abbreviation `prob` in compiled descriptors and `cond_skip`, although its public
YAML discriminator is `probability`. It now emits `probability` consistently.

## Invariant

`Condition::cond_type` is the single mapping used by both the compiled
`<ordinal>:<type>[:<accessor>]` descriptor and the outer condition-skip log.
Every returned token must exactly match the corresponding serde/YAML `type`
discriminator. Do not add log-only abbreviations or separately patch individual
log formatting sites.

The resource test `descriptor_is_value_free_and_ordinal_bearing` locks both the
plain and keyed probability descriptors. The Gateway test
`test_condition_type_and_detail` locks the shared type token used by `cond_skip`.
