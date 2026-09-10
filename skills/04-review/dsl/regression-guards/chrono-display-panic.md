---
name: chrono-display-panic
description: Use when a reviewer flags `dt.format(fmt).to_string()` or any other `T: Display` → `to_string()` chain in a DSL builtin as fine; covers why this is a data-plane panic risk and the canonical `write!`-based remediation.
knowledge_type: regression-guards
---

# Chrono `Display::to_string()` Panic on Bad Format — FIXED, PIN-IN-PLACE

**Status**: Resolved — see `BuiltinId::TimeFormat` in `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` for the canonical pattern.
**Test**: `edgion-gateway/src/plugins/http/dsl/tests/p3_utility_tests.rs::test_time_format_invalid_specifier_returns_api_error`.

## Conclusion

In DSL builtins (data-plane code), do not call `.to_string()` on any `T: Display` whose `Display::fmt` is allowed to return `Err`. Use `write!(buf, "{}", x)` and surface the `fmt::Error` as `RuntimeError::ApiError`.

Suggestions of the form "`.to_string()` is fine because chrono usually succeeds" or "wrap it in `catch_unwind`" are **not accepted**:
- `.to_string()` panics any time `Display::fmt` returns `Err(fmt::Error)` (e.g. chrono `DelayedFormat::fmt` for an unknown `%`-specifier).
- The current `execute_safe` boundary catches unwinding VM panics, but discards the payload and records only `vm panic`. The builtin must still use `write!` to preserve its `ApiError` contract and avoid unwinding; the boundary is fallback protection, not a substitute for fallible formatting.

## Core Rationale

**1. `Display::to_string()` panics on `Err`**

The default `Display`-derived `to_string()` calls `fmt(&mut Formatter)` and panics with `"a Display implementation returned an error unexpectedly"` if it returns `Err`. For most types this never happens — but chrono's `DelayedFormat<I>` is one where it routinely does:

| Input to `DelayedFormat::fmt` | Outcome |
|---|---|
| Valid timestamp + valid format specifiers | `Ok(())` |
| Valid timestamp + an unknown specifier like `%Q` | `Err(fmt::Error)` |

Any DSL builtin that lets user-supplied format strings reach `dt.format(fmt).to_string()` is therefore a data-plane panic vector.

**2. Why `write!` is the fix, not `catch_unwind`**

The fix is structural, not defensive. Replace:

```rust
let formatted = dt.format(&format).to_string();      // panics on Err
```

with:

```rust
use std::fmt::Write;
let mut formatted = String::new();
match write!(formatted, "{}", dt.format(&format)) {  // Err is propagated, not panicked
    Ok(()) => { /* check_string_len, return Value::Str */ }
    Err(_) => Err(RuntimeError::ApiError {
        function: "time_format".into(),
        message: format!("invalid format string: {:?}", format),
    }),
}
```

`write!` calls `Display::fmt` directly with a `Formatter` that propagates the `Err` upward; no panic. The partial bytes written to `formatted` before the error are discarded on the `Err` arm.

**3. Why this matters for the data plane**

Both `DslPlugin::run_request` and `run_upstream_response_filter` call
`execute_safe` (`edgion-gateway/src/plugins/http/dsl/lang/vm.rs`). That function
wraps the VM future in `AssertUnwindSafe(...).catch_unwind().await`. A captured
unwinding panic does not escape the VM boundary: its payload is discarded,
`PluginLog` receives only the fixed `vm panic` classification, and
`apply_error_policy` applies `Ignore`, `Deny`, or `DenyWith`. The VM diagnostic
does not include the panic message, script name, or trigger details.

The earlier claim that VM execution had no top-level panic boundary and that
`Ignore` could not handle a captured panic is superseded by this implementation.
The formatting rule remains: invalid chrono formats must produce
`RuntimeError::ApiError` without unwinding. `execute_safe` records the fixed
`api error` classification for that result; it does not copy the internal
error message (which can contain the supplied format) into `PluginLog`.

This distinction preserves the builtin's typed error contract and avoids
relying on panic handling for expected invalid input. The async boundary
catches unwinding panics, not process aborts.

## Other suspect call sites

The fix in `BuiltinId::TimeFormat` plugged the one known panic vector in `lang/builtins.rs`. Future builtins that introduce a `T: Display` value to convert into a `Value::Str` must use the same `write!` pattern when `T::Display::fmt` can fail. Worth checking at review time for any new builtin that touches:

- `chrono::DateTime::format(...)` or any `DelayedFormat`
- Custom `Display` impls whose `fmt` body has fallible operations (file I/O, allocator hits, conditional writes)
- Foreign formatter types pulled in from crates (e.g. `humantime`, `time`, third-party number formatters)

For types whose `Display::fmt` is provably infallible (returns `Ok(())` unconditionally), `to_string()` remains acceptable — the cost is just allocation. Most `String`/`&str`/`i64` impls fall in this group.

## Re-evaluation Triggers

Re-open this decision only if:

- chrono ships a version where `DelayedFormat::fmt` validates the format string up front (turning silent `Err` into a constructor error), making the panic theoretically unreachable for the chrono case specifically.
- VM execution entry points stop calling `execute_safe`, or its panic classification/error-policy contract changes. Recheck the boundary independently; do not replace typed builtin errors with intentional panics.

## Reference Cases

- Fix commit: `BuiltinId::TimeFormat` rewrite (`lang/builtins.rs`), 2026-05.
- Lock-in test: `tests/p3_utility_tests.rs::test_time_format_invalid_specifier_returns_api_error`.
- Related contract: [builtin error semantics](../../../03-coding/dsl/builtin-error-semantics.md) documents which DSL builtins are allowed to raise `ApiError` vs return nil.
- Current boundary evidence (source inspection, not a fresh test run): `plugin.rs` request and response entry points, plus `lang/vm.rs::execute_safe` and `apply_error_policy`.
- The existing invalid-specifier unit test verifies `ApiError`; it does not itself test the outer panic boundary.
- User-facing doc: `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md` `time_format` section, which now states "an invalid format specifier raises a runtime error".
