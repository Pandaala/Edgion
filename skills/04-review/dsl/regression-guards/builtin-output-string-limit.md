---
name: builtin-output-string-limit
description: Use when reviewing UTF-8 byte limits on string-producing DSL builtin results.
knowledge_type: regression-guards
---

# DSL builtin output string limits

## Conclusion and scope

String-producing DSL builtins that use the normal typed limit-error contract must
check their final output against `VmLimits::max_string_len` in UTF-8 bytes before
returning a `Value::Str`. `trim`, `sha256`, `md5`, and `hmac_sha256` enforce this
contract through the shared `check_string_len` or `checked_value_str` helpers.

This rule does not remove the documented exceptions for builtins such as Redis
reads, which truncate values, or `http.call`, which reports bounded-exchange
failure through its result Map. It is also not evidence that every builtin was
audited during either recorded fix.

## Rationale and evidence

The canonical sandbox contract in
`skills/01-architecture/02-gateway/dsl-engine-security-model.md` caps
runtime-produced strings with `max_string_len`. The error contract in
`skills/03-coding/dsl/builtin-error-semantics.md` requires an ordinary output
limit breach to use `RuntimeError::StringTooLong`, with exact UTF-8 byte `len`
and `limit` values.

`BuiltinId::Trim` previously returned both its unchanged and trimmed branches
without calling the shared length check. A string constant may legally exceed
the separately configured runtime limit, so `trim("abcdef")` and
`trim(" abcdef ")` could both return six bytes when `max_string_len` was five.
The corrected branch checks the trimmed slice before choosing whether to retain
the owned input or allocate the trimmed `Arc<str>`.

The digest builtins previously had the same missing-output-check shape. They now
route their fixed-size hexadecimal outputs through `checked_value_str`.

## Alternatives and limits

Checking string constants when they are loaded is not equivalent: constants are
intentionally governed by the separate compile-time constant and pool limits.
The runtime limit applies when a builtin produces its result. Checking after
constructing the final `Value::Str` would also preserve correctness but would do
unnecessary work on rejected trimmed output; checking the borrowed trimmed slice
first preserves the unchanged-input allocation path.

The `trim` defect was observable only when an operator configured
`max_string_len` below an otherwise legal input. With defaults, the 8 KiB runtime
limit exceeds the 4 KiB single-string-constant limit. The bug violated the
sandbox contract but did not create an unbounded string because the independent
compile-time limits still applied.

## Verification and re-evaluation

The focused `edgion-gateway` test selection
`dsl::tests::range_and_string_ops::test_trim` passed after the correction. It
covers an exact-limit trimmed result, a trimmed over-limit result, an unchanged
over-limit result, and UTF-8 multibyte byte counting, in addition to the existing
trim behavior tests. Historical digest regressions remain in
`tests/utility_functions.rs` and `tests/p3_utility_tests.rs`.

Re-evaluate this rule if the project changes the distinction between constant-pool
limits and runtime-produced strings, changes a builtin's documented failure shape,
or replaces `Value::Str` / the shared length helpers. A new string-producing
builtin still needs its own call-chain and boundary-test review; this page is not
a substitute for that audit.

## Resolution references

- `plugins-http-dsl-F005` — digest output checks, resolved 2026-05-26; historical verification evidence was migrated from the earlier review record.
- `enforce-dsl-trim-output-string-limit` — `trim` output checks, corrected in the current task; source finding: `tasks/todo/enforce-dsl-trim-output-string-limit.md`.
