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
That exact-length rule applies when the complete result length is already known.
Bounded producers such as Body reads and XML value/stringify stop at the first
overflow and report `len = limit + 1` as a lower bound. They must not scan or
materialize the rejected tail just to compute an exact diagnostic; no partial
result is returned. XML set_body uses the separate fixed BodyAccess contract.

`BuiltinId::Trim` previously returned both its unchanged and trimmed branches
without calling the shared length check. A string constant may legally exceed
the separately configured runtime limit, so `trim("abcdef")` and
`trim(" abcdef ")` could both return six bytes when `max_string_len` was five.
The corrected branch checks the trimmed slice before choosing whether to retain
the owned input or allocate the trimmed `Arc<str>`.

The digest builtins previously had the same missing-output-check shape. They now
route their fixed-size hexadecimal outputs through `checked_value_str`.

## Borrowed response-header reads

`resp.header` and `resp.content_type` borrow the first live response HeaderValue,
retain its `to_str` eligibility check, and use `checked_value_opt_str_ref` before
allocating the VM's independent Arc string. `resp.has_header` uses the same
first-value text gate without constructing a result string or applying the
string-output limit. An opaque first value is absent for these builtins even if
a later duplicate is textual; empty text is present. No bound response exposes
an empty map, matching the owned session getter, and pending writes are not read.

The response builtin tests use real HeaderMaps for these boundaries. Their
exact/over-limit fixtures use four ASCII bytes: the former multibyte mock values
could not pass the real adapter's HeaderValue text gate. The shared borrowed
helper's multibyte byte-count and rejection-before-allocation coverage remains
in `ctx_value_tests::utf8_limit_is_checked_in_bytes_before_allocating`.
This removes a temporary host String, not the final DSL string or argument
conversion; it makes no throughput or allocation-free execution claim.

`resp.header_names()` traverses the same live map's keys directly, taking at
most 256 unique names in map order before creating the independent Arc list.
Each retained name is length-checked; names beyond that prefix are neither copied
nor checked. The fixed count remains independent of `max_list_len`. Duplicate
values do not duplicate names, and opaque values do not hide names. These rules
differ from request-header enumeration and must not be unified mechanically.
Real-map tests preserve order, duplicate/opaque-value behavior, empty input,
exact/over-limit names and an oversized 257th name outside the retained prefix.
This removes the intermediate full `Vec<String>` and its name copies; output
list/Arc allocation remains.

## Borrowed request scalars and metadata

`req.host`, `req.content_type` and `req.scheme` use the existing `header_str`
accessor and borrowed result helper. Scheme chooses the first text-eligible
X-Forwarded-Proto value, then X-Scheme, then the literal `http`, preserving case
and empty text. A non-text first field value does not search later duplicates.
Check the selected result's length without falling back on a length error.
`req.content_length` parses the borrowed first text value as the same signed i64:
malformed/overflow text returns nil; negative and leading-plus values remain
accepted by this script-level read. It does not apply a string-output limit or
add HTTP input validation. Tests cover selection/length boundaries and numeric
first-value behavior. Final string results still own their Arc storage.

Method, path, entry request path, direct-peer IP and resolved remote IP also pass
their existing borrowed getter result to the same helper without a temporary
String. Client-certificate CN and fingerprint borrow their fields; absent
certificate/CN remains nil. These changes retain each getter's live/frozen and
identity provenance, case and raw spelling. The metadata byte-limit table covers
exact/rejected lengths and returned bytes for these properties, including
multibyte CN, without claiming to eliminate final Arc allocation.

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
