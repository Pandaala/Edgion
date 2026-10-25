---
name: string-len-bytes-vs-chars-contract
description: Use when comparing DSL len Unicode-scalar counts with UTF-8 output-byte limits; explains units without claiming a universal heap or CPU bound.
knowledge_type: intentional-design
---

# DSL length units: Unicode scalars and UTF-8 bytes

## Current contract

For Str, `len()` returns `s.chars().count()`: Unicode scalar count, not bytes or
grapheme clusters. List/Map return entry count and nil returns zero. `substr`
also uses scalar-indexed boundaries. Checked runtime string outputs use UTF-8
bytes through `s.len()`, and StringTooLong reports byte length and byte limit.
These intentionally different units should not be aligned as an incidental
optimization. Preserve the script API and its documented examples.

Byte checks measure encoded payload without a scalar scan. Replacing them with
character counts could admit up to four times as many UTF-8 bytes at the same
numeric threshold. Conversely, changing len to bytes would change script
conditions and its relationship with substr. A new length builtin or a unit
change needs separate language/API review; this page authorizes neither.

## Scope of the byte limit

max_string_len applies at explicit operation boundaries, not to every Value::Str
or every intermediate buffer. Constants have independent validator limits;
LoadConst and generic stack push do not apply the runtime output-byte limit.
Split and Range currently do not check each produced element's string length.
Some transformations allocate before checking their final output. These facts
do not make missing checks universally acceptable; inspect the intended owner
and requirements before proposing a guard or relying on an exemption.

The earlier claim that this setting capped every Value::Str heap footprint and
proved a max_steps-times-max_string_len aggregate bound is superseded. Logical
payload length does not include allocation capacity, container/Arc overhead,
shared owners or temporary storage. See [dispatch-budget boundaries](../../cpu-memory/intentional-design/dsl-step-budget-not-cpu-bound.md)
and the [security model](../../../01-architecture/02-gateway/dsl-engine-security-model.md).

Do not assume multibyte input is rare: decoded queries and explicit structured
or body operations can expose UTF-8 text. Nor does StringTooLong universally
mean HTTP 500; execute_safe applies the configured Ignore, Deny or DenyWith
policy. The previous workload-frequency and fixed-500 rationale is withdrawn.

## Evidence and re-evaluation

- `builtins.rs::Len`, `Substr`, `check_string_len`; `edgion-dsl/src/error.rs`
  defines the byte-named error. The owning paths are under Gateway
  `plugins/http/dsl/lang/` unless stated otherwise.
- `test_len_multibyte_returns_char_count_not_byte_count` in the mirrored
  utility_functions.rs pins the scalar/byte distinction.
- `test_runtime_string_limit_is_not_a_universal_input_bound` records current
  literal and container admission below their input/item byte lengths. It is
  evidence of existing behavior, not acceptance of permanent exemptions.
- The English and Chinese DSL API reference `len` entries document the units;
  this correction changes no user-facing API.

The original FB04 resolution established the unit distinction and byte-named
error, not a universal cost proof. Re-evaluate if string representation,
producer admission, scalar indexing or language/API requirements change. Check
current code and the actual input path before closing any resource-bound
finding by reference to this page.
