---
name: chrono-display-panic
description: Use when a reviewer flags `dt.format(fmt).to_string()` or any other `T: Display` → `to_string()` chain in a DSL builtin as fine; covers why this is a data-plane panic risk and the canonical `write!`-based remediation.
---

# Chrono `Display::to_string()` Panic on Bad Format — FIXED, PIN-IN-PLACE

**Status**: Resolved — see `BuiltinId::TimeFormat` in `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` for the canonical pattern.
**Test**: `edgion-gateway/src/plugins/http/dsl/tests/p3_utility_tests.rs::test_time_format_invalid_specifier_returns_api_error`.

## Conclusion

In DSL builtins (data-plane code), do not call `.to_string()` on any `T: Display` whose `Display::fmt` is allowed to return `Err`. Use `write!(buf, "{}", x)` and surface the `fmt::Error` as `RuntimeError::ApiError`.

Suggestions of the form "`.to_string()` is fine because chrono usually succeeds" or "wrap it in `catch_unwind`" are **not accepted**:
- `.to_string()` panics any time `Display::fmt` returns `Err(fmt::Error)` (e.g. chrono `DelayedFormat::fmt` for an unknown `%`-specifier).
- `catch_unwind` masks a class of latent crashes but converts them into opaque internal errors — `write!` is shorter, has no overhead, and preserves the original error variant.

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

Gateway worker threads have no top-level `catch_unwind` for VM execution. A panic inside `vm.execute().await` propagates through `block_on` and crashes the worker. Even on `DslErrorPolicy::Ignore` (the default), the panic path is distinct from the `ApiError` path: `Ignore` only swallows the latter. So `to_string()` panics short-circuit the policy contract.

## Other suspect call sites

The fix in `BuiltinId::TimeFormat` plugged the one known panic vector in `lang/builtins.rs`. Future builtins that introduce a `T: Display` value to convert into a `Value::Str` must use the same `write!` pattern when `T::Display::fmt` can fail. Worth checking at review time for any new builtin that touches:

- `chrono::DateTime::format(...)` or any `DelayedFormat`
- Custom `Display` impls whose `fmt` body has fallible operations (file I/O, allocator hits, conditional writes)
- Foreign formatter types pulled in from crates (e.g. `humantime`, `time`, third-party number formatters)

For types whose `Display::fmt` is provably infallible (returns `Ok(())` unconditionally), `to_string()` remains acceptable — the cost is just allocation. Most `String`/`&str`/`i64` impls fall in this group.

## Re-evaluation Triggers

Re-open this decision only if:

- chrono ships a version where `DelayedFormat::fmt` validates the format string up front (turning silent `Err` into a constructor error), making the panic theoretically unreachable for the chrono case specifically.
- A `tracing`-free top-level `catch_unwind` is added around `vm.execute` so the worker-crash concern goes away (then panic-vs-`Err` becomes a code-style question rather than an availability hazard).

## Reference Cases

- Fix commit: `BuiltinId::TimeFormat` rewrite (`lang/builtins.rs`), 2026-05.
- Lock-in test: `tests/p3_utility_tests.rs::test_time_format_invalid_specifier_returns_api_error`.
- Related contract: [[builtin-error-semantics]] documents which DSL builtins are allowed to raise `ApiError` vs return nil.
- User-facing doc: `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md` `time_format` section, which now states "an invalid format specifier raises a runtime error".
