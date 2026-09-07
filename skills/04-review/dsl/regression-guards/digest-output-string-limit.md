---
name: digest-output-string-limit
description: Historical review evidence for DSL; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Digest output string limit

## Recorded conclusion

`sha256` / `md5` / `hmac_sha256` builtins (`lang/builtins.rs`) now route their hex output through the shared `checked_value_str(..., max_string_len)` helper, matching all other string-producing builtins — previously they returned `Value::Str` directly, skipping the `max_string_len` cap. Only observable when an operator sets `max_string_len` below the fixed hash-hex length (64 for SHA-256/HMAC, 32 for MD5); the default 8192 never breaches, and the cap is not an exploit boundary (output is bounded by the hash algorithm, not by input). Consistency / sandbox-auditability fix. Unit regressions `test_sha256_result_too_long` + `test_md5_result_too_long` (`tests/utility_functions.rs`), `test_hmac_sha256_result_too_long` (`tests/p3_utility_tests.rs`). Re-reports of the missing-`check_string_len` form are not accepted

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `plugins-http-dsl-F005` — 2026-05-26.
