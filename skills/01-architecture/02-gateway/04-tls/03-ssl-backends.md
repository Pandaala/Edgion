---
name: gateway-ssl-backends
description: SSL backend implementations — two compilation features (BoringSSL and OpenSSL), interface abstraction, recommendations.
---

# SSL backends (BoringSSL / OpenSSL)

## Overview

Edgion Gateway supports two SSL backends through Cargo feature flags. The choice is made at compile time, and runtime behavior dispatches automatically via conditional compilation (`#[cfg(feature = "...")]`).

## Feature flags

```toml
[features]
boringssl = [...]   # use BoringSSL as the TLS backend
openssl = [...]     # use OpenSSL as the TLS backend
```

The two features are mutually exclusive: in conditional compilation, `#[cfg(all(feature = "openssl", not(feature = "boringssl")))]` ensures BoringSSL takes precedence.

## BoringSSL backend

```
src/core/gateway/tls/boringssl/
├── mod.rs
└── mtls_verify_callback.rs   # mTLS custom verify callback
```

### Capability matrix

| Capability | Status | Notes |
|------------|--------|-------|
| TLS termination | Full support | Certificate / key setup, ALPN (HTTP/2) |
| mTLS client verification | Full support | CA verification, verify_depth |
| SAN/CN allowlist | Full support | Custom verify callback |
| SSL ex_data | Full support | Per-connection context passing |
| Minimum TLS version | Full support | TLS 1.0 ~ 1.3 |
| Cipher-suite configuration | Full support | `SSL_set_strict_cipher_list` |
| TLS 1.3 cipher suites | Not configurable | Hardcoded by BoringSSL |

### mTLS verify callback

`mtls_verify_callback.rs` implements a custom certificate-verification callback that supports allowlist verification by SAN (Subject Alternative Name) and CN (Common Name). It activates when EdgionTls is configured with `allowed_sans` or `allowed_cns`.

### ex_data mechanism

BoringSSL's `SSL_set_ex_data` / `SSL_get_ex_data` are used to pass per-connection context (`SslCtx`) between `certificate_callback` and `handshake_complete_callback`. Includes:

- `SSL_get_ex_new_index`: register a custom index (once globally, via `LazyLock` + `Once`).
- Register a `free_ssl_ctx` callback to handle memory release on SSL object destruction.
- Thread safety: the index is stored in `AtomicI32`, initialized via `std::sync::Once`.

## OpenSSL backend

```
src/core/gateway/tls/openssl/
└── mod.rs   # placeholder module (currently empty)
```

### Capability matrix

| Capability | Status | Notes |
|------------|--------|-------|
| TLS termination | Basic support | Through Pingora's OpenSSL bindings |
| mTLS client verification | Basic support | Standard OpenSSL verify chain |
| SAN/CN allowlist | Not implemented | Requires a custom verify callback |
| SSL ex_data | Not implemented | Callbacks return basic TlsConnMeta |
| Minimum TLS version | Supported | Through `set_min_proto_version` |
| Cipher-suite configuration | No-op | Ignored in `configure_ciphers` |

### Non-BoringSSL fallback

When compiled with the OpenSSL backend:

- `store_ssl_ctx` returns `false`, logging `"no boringssl exdata"`.
- `take_ssl_ctx` returns `None`.
- `handshake_complete_callback` returns a `TlsConnMeta` with basic info (SNI, port, timestamp).
- BoringSSL FFI calls inside `configure_ciphers` are removed via conditional compilation.

## Unified interface

`backend_api.rs` provides a unified interface dispatched at compile time:

```rust
pub fn set_mtls_verify_callback(
    ssl: &mut SslRef,
    verify_mode: SslVerifyMode,
    edgion_tls: &Arc<EdgionTls>,
) -> Result<(), String>
```

- `#[cfg(feature = "boringssl")]`: calls `boringssl::mtls_verify_callback::set_verify_callback_with_whitelist`.
- `#[cfg(all(feature = "openssl", not(feature = "boringssl")))]`: returns an error indicating it is not implemented.

Other operations needing conditional compilation use `#[cfg]` attributes directly at the call sites — for example, `configure_ciphers` and `verify_depth` setup in `tls_pingora.rs`.

## Module conditional compilation

The entire `tls/runtime/backend/mod.rs` module is guarded by conditional compilation:

```rust
#![cfg(any(feature = "boringssl", feature = "openssl"))]
```

This ensures backend-related code is not compiled when no SSL backend is enabled.

## Key source files

| File | Responsibility |
|------|----------------|
| `src/core/gateway/tls/mod.rs` | Module entry, feature-flag conditional compilation |
| `src/core/gateway/tls/boringssl/mtls_verify_callback.rs` | BoringSSL mTLS verify callback |
| `src/core/gateway/tls/openssl/mod.rs` | OpenSSL placeholder module |
| `src/core/gateway/tls/runtime/backend/backend_api.rs` | Unified backend API |
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | Conditionally compiled SSL ex_data operations |
