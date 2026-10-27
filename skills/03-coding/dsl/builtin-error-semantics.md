---
name: dsl-builtin-error-semantics
description: DSL builtin error-handling contract — which functions silently return nil/clamp/false for bad input, which raise RuntimeError. Use when adding a new builtin, modifying an existing one, or reviewing whether a "fix" would change observable error semantics.
---

# DSL builtin error semantics

> User-facing contract: `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md`
> Engine error variants: `edgion-dsl/src/error.rs::RuntimeError`

## Common error-handling shapes

XML's frozen contract: malformed/unsupported parse returns nil (including Body
input); missing getters/indices return nil; incompatible argument types raise
TypeError; invalid construction raises fixed-detail ApiError; structural/payload
bounds use XmlLimitExceeded and work exhaustion uses ValueWorkLimitExceeded.
String result overflow uses StringTooLong with len=limit+1 as a lower bound when
the bounded producer stops early; do not scan the rejected tail. Body XML scan/storage/physical/XML/work
failures and every xml.set_body serialization/publication failure map to
BodyAccess fixed 500, bypassing DslErrorPolicy. Never turn a resource limit into
malformed nil. Xml handles reject implicit string coercion and cannot escape via
ctx.set. See [bounded XML](../../04-review/dsl/regression-guards/bounded-xml-primitives.md).

DSL builtins fall into three groups for invalid / boundary input. The group is the **contract**, not an accident — script authors rely on these shapes for null-safe pipelining, and changing a group is a behavior change that breaks deployed scripts.

| Shape | Examples | When invalid / OOB |
|---|---|---|
| **Silent fallback** (nil / empty / clamped / false) | `substr`, `to_int`, `base64_decode`, `hex_decode`, `url_decode`, `ip_in_cidr`, `time_format` (bad timestamp) | Returns a benign value; script continues |
| **`RuntimeError::TypeError`** | Operations that explicitly require a particular `Value` variant | Scripting-level type mismatch; some builtins deliberately coerce via `into_string` instead |
| **`RuntimeError::ApiError`** | Session-mutation builtins on host failure; `time_format` invalid format specifier; `resp.set_status` out of range | Real I/O or contract failure that scripts cannot avoid |
| **Limit errors** | Output exceeds VM string/list/map limits | `StringTooLong`, `ListTooLong`, or `MapTooLarge`; some I/O builtins instead document truncation |

## Silent-fallback contracts to preserve

Locked in by tests under `edgion-gateway/tests/unit/plugins/http/dsl/mod_test/` (including
`utility_functions.rs`, `p3_utility_tests.rs`, `runtime_errors.rs`, and
`api_errors.rs`). If a reviewer suggests changing one, inspect the current test and
design note before altering the observable contract.

| Builtin | Input class | Returns | Test |
|---|---|---|---|
| `substr(s, i, j)` | start at/past character count, or start after end | `""` | `test_substr`, `test_substr_oob_returns_empty` |
| `substr(s, i, j)` | end past character count with a valid start | suffix from start to string end | `test_substr` |
| `substr(s, i, j)` | `i` or `j` negative | clamped to 0 | `test_substr_negative_start_clamps_to_zero` |
| `to_int(s)` | unparseable / empty / overflow | `nil` | `test_to_int_unparseable_returns_nil`, `_empty_string_returns_nil`, `_overflow_returns_nil` |
| `base64_decode(s)` | non-base64 input | `nil` | `test_base64_decode_invalid_returns_nil` |
| `hex_decode(s)` | non-hex input | `nil` | `test_hex_decode_invalid` |
| `url_decode(s)` | `%` without two following ASCII hex bytes | `%` remains literal; subsequent bytes are processed normally | `test_url_decode_invalid_percent_preserves_literal` covers `%ZZ` |
| `url_decode(s)` | trailing `%` or `%X` (length < 3 tail) | same scanner rule: `%` and `%Z` remain unchanged; `%+` becomes `% ` | `test_url_decode_single_percent_preserves_literal`, `test_url_decode_percent_one_char_preserves_literal` cover `%` and `%Z` |
| `ip_in_cidr(ip, cidr)` | malformed `cidr` (no `/`, bad prefix, bad ip) | `false` | `test_ip_in_cidr_invalid_cidr_returns_false` |
| `time_format(ts, fmt)` | timestamp out of chrono's range | `nil` | `test_time_format_out_of_range_timestamp_returns_nil` |

**Design intent**: these builtins are used in pipelining patterns like `if to_int(req.header("X-Limit")) > 100 { deny }` or `let parts = split(url_decode("a%2Fb"), "/")`. A surprise `ApiError` mid-pipeline would deny every malformed request even when the script's logic could have handled the fallback gracefully (e.g. compared `to_int(x)` to nil). The silent-fallback shape preserves operator-level error handling at the script level.

`req.query(name)` already returns a decoded value from the current Query owner;
another `url_decode` is a second decoding operation, not a required query-read
step. The DSL and resource decoders share ordinary form-style plus handling but
differ on malformed tokens and invalid UTF-8. See the
[decoder boundary](../../04-review/dsl/intentional-design/url-decode-form-semantics.md).
The URL table describes source-traced behavior; the named tests cover the listed
fixtures rather than every malformed-token combination.

## `ApiError` contracts

| Builtin | When it raises `ApiError` | Field `function` |
|---|---|---|
| `req.set_header` / `req.append_header` / `req.remove_header` | Pingora session returns `Err` | `"req.set_header"` / `"req.append_header"` / `"req.remove_header"` |
| `req.set_uri` / `req.set_host` / `req.set_method` | session returns `Err` | `"req.set_uri"` / `"req.set_host"` / `"req.set_method"` |
| `resp.set_header` / `resp.append_header` / `resp.remove_header` | session returns `Err` | `"resp.set_header"` / `"resp.append_header"` / `"resp.remove_header"` |
| `resp.set_status` | final status code outside `200..=599`, or session returns `Err` | `"resp.set_status"` |
| `time_format` | format string exceeds 256 bytes, or contains an invalid chrono specifier | `"time_format"` |
| `ctx.set` / `ctx.remove` | host returns `Err` | `"ctx.set"` / `"ctx.remove"` |

Size-limit errors are their own runtime variants (`StringTooLong`, `ListTooLong`, and
`MapTooLarge`), not `ApiError`. Some I/O builtins intentionally truncate bounded output
instead. Check the individual builtin and its lock-in tests rather than applying a single
“all result lengths” rule.

`ApiError` flows through `DslErrorPolicy` (`lang/vm.rs`):
- `Ignore` (default) — logs `"api error"` in `PluginLog`, returns `GoodNext`.
- `Deny` — returns `ErrResponse { status: 500, .. }`.
- `DenyWith { status, body }` — returns the configured response.

## Request-body outcomes and the hard-failure boundary

The opaque `Body` API separates data outcomes from infrastructure failures:

| Outcome | Contract |
|---|---|
| Invalid JSON, missing JSONPath, or traversal type mismatch | `req.body.json_get()` returns `nil` |
| Search/prefix/suffix/regex does not match | Returns `false` |
| Implicit Body→String coercion, Body ordering, invalid method argument type | `TypeError`, handled by configured `errorPolicy` |
| Oversized returned string or Body.read requested length above maxStringLen | `StringTooLong`, handled by configured `errorPolicy` |
| Storage error, scan-budget exhaustion, invalid byte range, physical-cap or mutation-segment breach | `RuntimeError::BodyAccess` |

`BodyAccess` is an internal typed hard-failure classification. It bypasses
`DslErrorPolicy` and always returns HTTP 500 with the fixed client body
`Request body processing failed`; operational detail stays in the plugin log.
It is deliberately not a new configurable policy or a fifth project-wide
failure-policy shape. Do not convert parse/no-match outcomes into
`BodyAccess`, and do not let `Ignore` soften actual body-storage failures.

## Adding a new builtin — choose the shape deliberately

When implementing a new builtin that can fail:

1. **Does the script author have a way to validate input before calling?** If yes, lean toward `ApiError` (failure is genuinely exceptional). If no, lean toward silent fallback (failure is a normal data condition).
2. **Is the failure deterministic from script-visible state?** Bad regex literal → compile error (handled). Bad runtime string → silent fallback. Host I/O failure → `ApiError`.
3. **Will a chained `if x == nil` / `if x == false` idiom let the script handle it gracefully?** If yes, silent fallback. If no (e.g. `set_header` has no useful Value return), `ApiError`.
4. **Will `Display::to_string()` ever be called on user data?** If yes, see [chrono-display-panic](../../04-review/dsl/regression-guards/chrono-display-panic.md) — use a fallible writer (`write!` for `Display`, or Chrono's direct `write_to`) to surface formatting `Err` as `ApiError`, do not let it panic.
5. **Does it access opaque request-body storage?** Keep normal data outcomes
   (`nil` / `false`) script-visible, but map storage, scan-budget, byte-range,
   and transactional mutation failures to `BodyAccess`.

After implementing, add a lock-in test to the appropriate file (mirroring the existing entries in `tests/utility_functions.rs` etc.) so the contract survives future refactors.

## Cross-references

- Panic-via-Display prevention rule → [chrono-display-panic](../../04-review/dsl/regression-guards/chrono-display-panic.md)
- Engine architecture & error policy → `skills/01-architecture/02-gateway/dsl-engine.md`
- User-facing function reference → `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md`
- Test helpers → `edgion-gateway/tests/unit/plugins/http/dsl/mod_test.rs`

## Strict JSON contract

`json.read`, `get_raw`, `get_string` and `keys` return nil for malformed/duplicate
input; getters also return nil for absent/non-object/type-mismatched JSON values.
Raw JSON null is the explicit string `"null"`, distinct from missing nil.
Wrong DSL argument types raise TypeError. Construction requires a JSON object and
explicitly valid raw values; invalid construction uses fixed ApiError. Structural
bounds use JsonLimitExceeded, output bounds StringTooLong and cumulative work
ValueWorkLimitExceeded. Never turn resource limits into malformed nil.

Body JSON read storage/scan/work failures and every json.set_body failure use
BodyAccess fixed500 regardless of Ignore. Body malformed JSON/UTF-8 is nil so the
caller can deny400. set_body validates before one overwrite, checks the existing
request-stage flag and preserves the canonical frozen-body contract. No response
aggregation is introduced. redis.get_bounded returns the existing non-throwing
map, with fixed result_too_large/invalid_response/timeout classes; old get retains
its separate truncation contract.
