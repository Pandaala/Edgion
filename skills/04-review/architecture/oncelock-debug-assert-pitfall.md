---
name: oncelock-debug-assert-pitfall
description: When loud-failing a once-only OnceLock init in debug builds, the .set() call must be evaluated outside debug_assert! — wrapping the side-effecting .set() inside debug_assert!(LOCK.set(x).is_ok()) drops the entire expression in release builds and leaves every OnceLock uninitialized.
---

# OnceLock init: keep `.set()` outside `debug_assert!`

**Audit date**: 2026-05-28
**Linked issue**: infra-cluster F-011 (closed; fix applied)

## Pattern

Five global `OnceLock`s in `src/core/gateway/cli/config.rs` (`REQUEST_DURATION_CONFIG`,
`BACKEND_REQUESTS_CONFIG`, `PATH_NORMALIZATION_CONFIG`, `ACCESS_LOG_OPTIONS`,
`LRU_CACHE_GLOBAL_CONFIG`) are initialized once at Gateway startup and read on the
hot path. The goal is: "if a second init ever happens (test pollution), panic loudly
in debug builds, but keep production behavior unchanged".

## Wrong fix — silently breaks release builds

```rust
pub fn init_request_duration_config(config: RequestDurationConfig) {
    debug_assert!(REQUEST_DURATION_CONFIG.set(config).is_ok());
}
```

`debug_assert!` expands to `if cfg!(debug_assertions) { assert!(...) }`. With
`debug_assertions` off (the default for `--release` / production binaries) **the
entire argument expression is not evaluated** — `.set(config)` is never called.
Consequences in release:

- `REQUEST_DURATION_CONFIG.get().expect(...)` panics on the first request.
- `BACKEND_REQUESTS_CONFIG`, `PATH_NORMALIZATION_CONFIG`, `ACCESS_LOG_OPTIONS`,
  `LRU_CACHE_GLOBAL_CONFIG` all silently fall back to their `Default::default()`
  via `get_or_init` / `unwrap_or`. Operator-provided `[metrics.backend_requests]`,
  `[path_normalization]` (including the security-relevant `reject_escaped_slashes`),
  `[access_log_options]`, and `[lru_cache]` settings are silently ignored.

## Correct fix — evaluate `.set()` unconditionally, assert on the result

```rust
pub fn init_request_duration_config(config: RequestDurationConfig) {
    let result = REQUEST_DURATION_CONFIG.set(config);
    debug_assert!(result.is_ok(), "init_request_duration_config called more than once");
}
```

The `.set()` call runs in every build; only the `is_ok()` check is debug-only.
Release builds get the original "silently drop the Err" behavior (safe because
production calls each `init_*` exactly once in `cli/mod.rs:198-267`); debug/test
builds get a clear panic if duplicate init ever occurs.

## Rule

When a `debug_assert!` argument contains any side-effecting expression (method
calls, `?` propagation, locks, channel sends), bind the side-effecting part to a
local first and assert on the local. Same caution applies to `assert!` if you
later downgrade it to `debug_assert!` and forget the side effect.

## Cross-link

The original "Suggested fix" in `tasks/infra/infra-cluster-F011-*.md` carried the
buggy form. If a future review re-files this finding and proposes the same
`debug_assert!(LOCK.set(x).is_ok())` shape, reject and apply the local-bind
pattern above.
