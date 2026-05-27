---
name: shared-apply-auth-failure-delay
description: Use when reviewing HTTP auth plugins for code duplication of the auth-failure delay; documents that the shared `auth_common::apply_auth_failure_delay(delay_ms)` is the single source of truth and lists which plugins must call it directly.
---

# HTTP auth plugins — reuse `auth_common::apply_auth_failure_delay`

The auth-failure delay (brute-force / credential-stuffing mitigation, called
before sending a 401/403) lives in a single shared helper:

```rust
// src/core/gateway/plugins/http/common/auth_common.rs
pub async fn apply_auth_failure_delay(delay_ms: u64) {
    if delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
}
```

**Rule**: every HTTP auth plugin reads `self.config.auth_failure_delay_ms`
and calls `auth_common::apply_auth_failure_delay(...)` directly. No plugin
should declare a private `apply_auth_failure_delay` method that re-implements
the same `if > 0 { sleep }` logic — silent drift hazard if the helper later
adds jitter, exponential back-off, or metrics instrumentation.

## Compliant plugins (call the shared helper directly)

- `basic_auth/plugin.rs`
- `key_auth/plugin.rs`
- `ldap_auth/plugin.rs`
- `forward_auth/plugin.rs`
- `hmac_auth/plugin.rs`         — F-021 (2026-05-27)
- `jwe_decrypt/plugin.rs`       — F-021 (2026-05-27)
- `jwt_auth/plugin.rs`          — F-021 (2026-05-27)

## Known exceptions (not duplicate logic)

- `openid_connect/callback.rs` defines `OpenidConnect::apply_auth_failure_delay`
  as a one-line instance-method wrapper around `auth_delay(...)`
  (= shared helper aliased on import). It does **not** re-implement the
  `if > 0 { sleep }` body — it delegates. This is a stylistic choice (kept
  because the OIDC plugin has many call sites and the receiver-style
  invocation matches the rest of the impl); do not flag.

## Follow-up

- `header_cert_auth/plugin.rs:336` still carries a private copy. It was out
  of F-021's scope; track separately if a future review wants to clean it
  up. Same rule applies: replace with a direct call to
  `auth_common::apply_auth_failure_delay(self.config.auth_failure_delay_ms)`.
