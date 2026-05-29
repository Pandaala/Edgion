---
name: dsl-builtin-error-semantics
description: DSL builtin error-handling contract — which functions silently return nil/clamp/false for bad input, which raise RuntimeError. Use when adding a new builtin, modifying an existing one, or reviewing whether a "fix" would change observable error semantics.
---

# DSL builtin error semantics

> User-facing contract: `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md`
> Engine error variants: `src/core/gateway/plugins/http/dsl/lang/error.rs::RuntimeError`

## Three error-handling shapes

DSL builtins fall into three groups for invalid / boundary input. The group is the **contract**, not an accident — script authors rely on these shapes for null-safe pipelining, and changing a group is a behavior change that breaks deployed scripts.

| Shape | Examples | When invalid / OOB |
|---|---|---|
| **Silent fallback** (nil / empty / clamped / false) | `substr`, `to_int`, `base64_decode`, `hex_decode`, `url_decode`, `ip_in_cidr`, `time_format` (bad timestamp) | Returns a benign value; script continues |
| **`RuntimeError::TypeError`** | Any builtin given the wrong `Value` variant (e.g. `substr(int, 0, 1)`) | Bytecode bug or scripting-level type confusion |
| **`RuntimeError::ApiError`** | Session-mutation builtins on host failure; `time_format` invalid format specifier; `resp.set_status` out of range | Real I/O or contract failure that scripts cannot avoid |

## Silent-fallback contracts to preserve

Locked in by tests under `tests/utility_functions.rs`, `tests/p3_utility_tests.rs`, `tests/runtime_errors.rs`, `tests/api_errors.rs`. If a reviewer suggests changing any of these to raise `ApiError` or `TypeError`, decline: see `skills/04-review/dsl-builtins/` for the design rationale.

| Builtin | Input class | Returns | Test |
|---|---|---|---|
| `substr(s, i, j)` | `i` or `j` past `char_count` | `""` | `test_substr_oob_returns_empty` |
| `substr(s, i, j)` | `i` or `j` negative | clamped to 0 | `test_substr_negative_start_clamps_to_zero` |
| `to_int(s)` | unparseable / empty / overflow | `nil` | `test_to_int_unparseable_returns_nil`, `_empty_string_returns_nil`, `_overflow_returns_nil` |
| `base64_decode(s)` | non-base64 input | `nil` | `test_base64_decode_invalid_returns_nil` |
| `hex_decode(s)` | non-hex input | `nil` | `test_hex_decode_invalid` |
| `url_decode(s)` | `%XX` where `XX` is not hex | literal `%XX` preserved | `test_url_decode_invalid_percent_preserves_literal` |
| `url_decode(s)` | trailing `%` or `%X` (length < 3 tail) | literal preserved | `test_url_decode_single_percent_preserves_literal`, `test_url_decode_percent_one_char_preserves_literal` |
| `ip_in_cidr(ip, cidr)` | malformed `cidr` (no `/`, bad prefix, bad ip) | `false` | `test_ip_in_cidr_invalid_cidr_returns_false` |
| `time_format(ts, fmt)` | timestamp out of chrono's range | `nil` | `test_time_format_out_of_range_timestamp_returns_nil` |

**Design intent**: these builtins are used in pipelining patterns like `if to_int(req.header("X-Limit")) > 100 { deny }` or `let parts = split(url_decode(req.query("path")), "/")`. A surprise `ApiError` mid-pipeline would deny every malformed request even when the script's logic could have handled the fallback gracefully (e.g. compared `to_int(x)` to nil). The silent-fallback shape preserves operator-level error handling at the script level.

## `ApiError` contracts

| Builtin | When it raises `ApiError` | Field `function` |
|---|---|---|
| `req.set_header` / `req.append_header` / `req.remove_header` | Pingora session returns `Err` | `"req.set_header"` / `"req.append_header"` / `"req.remove_header"` |
| `req.set_uri` / `req.set_host` / `req.set_method` | session returns `Err` | `"req.set_uri"` / `"req.set_host"` / `"req.set_method"` |
| `resp.set_header` / `resp.append_header` / `resp.remove_header` | session returns `Err` | `"resp.set_header"` / `"resp.append_header"` / `"resp.remove_header"` |
| `resp.set_status` | status code outside `100..=599`, or session returns `Err` | `"resp.set_status"` |
| `time_format` | format string exceeds 256 bytes, or contains an invalid chrono specifier | `"time_format"` |
| `ctx.set` / `ctx.remove` | host returns `Err` | `"ctx.set"` / `"ctx.remove"` |
| Result length check (any builtin) | `check_string_len` exceeds `VmLimits::max_string_len` | varies |

`ApiError` flows through `DslErrorPolicy` (`lang/vm.rs`):
- `Ignore` (default) — logs `"api error"` in `PluginLog`, returns `GoodNext`.
- `Deny` — returns `ErrResponse { status: 500, .. }`.
- `DenyWith { status, body }` — returns the configured response.

## Adding a new builtin — choose the shape deliberately

When implementing a new builtin that can fail:

1. **Does the script author have a way to validate input before calling?** If yes, lean toward `ApiError` (failure is genuinely exceptional). If no, lean toward silent fallback (failure is a normal data condition).
2. **Is the failure deterministic from script-visible state?** Bad regex literal → compile error (handled). Bad runtime string → silent fallback. Host I/O failure → `ApiError`.
3. **Will a chained `if x == nil` / `if x == false` idiom let the script handle it gracefully?** If yes, silent fallback. If no (e.g. `set_header` has no useful Value return), `ApiError`.
4. **Will `Display::to_string()` ever be called on user data?** If yes, see [chrono-display-panic](../../04-review/dsl-builtins/chrono-display-panic.md) — use `write!` to surface fallible-formatter `Err` as `ApiError`, do not let it panic.

After implementing, add a lock-in test to the appropriate file (mirroring the existing entries in `tests/utility_functions.rs` etc.) so the contract survives future refactors.

## Cross-references

- Panic-via-Display prevention rule → [chrono-display-panic](../../04-review/dsl-builtins/chrono-display-panic.md)
- Engine architecture & error policy → `skills/01-architecture/02-gateway/dsl-engine.md`
- User-facing function reference → `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md`
- Test helpers → `tests/mod.rs::assert_sets_request_header`, `tests/mod.rs::run_expect_runtime_err`
