# Rustls mTLS Support Evaluation — Analysis Based on Pingora 0.8.0 Changes

> Source: in-depth investigation of native mTLS support during the Pingora 0.8.0 upgrade (`06-mtls-native-support`).
> This document records the conclusions of that investigation as reference material for future Rustls backend support.

## Background

The Pingora 0.8.0 release notes state:

> Add support for client certificate verification in mTLS configuration.

Source diff verification shows that **this change only affects the Rustls backend**. There were zero changes on the BoringSSL/OpenSSL and s2n paths between 0.7.0 and 0.8.0.

## mTLS Scope

**Downstream (client -> gateway)**, meaning client certificate verification on the listener side for incoming connections. This does not involve upstream (gateway -> backend).

## What Changed

### Rustls Backend: From Unsupported to Basically Usable

**0.7.0** — hardcoded `with_no_client_auth()`, making client certificate verification completely unavailable:

```rust
// pingora-core-0.7.0/src/listeners/tls/rustls/mod.rs L57-60
// TODO - Add support for client auth & custom CA support
let mut config = ServerConfig::builder_with_protocol_versions(&[&version::TLS12, &version::TLS13])
    .with_no_client_auth()
    .with_single_cert(certs, key)
```

**0.8.0** — adds the `client_cert_verifier` field, allowing injection of a custom `ClientCertVerifier`:

```rust
// pingora-core-0.8.0/src/listeners/tls/rustls/mod.rs L59-65
let builder = ServerConfig::builder_with_protocol_versions(&[&version::TLS12, &version::TLS13]);
let builder = if let Some(verifier) = self.client_cert_verifier {
    builder.with_client_cert_verifier(verifier)
} else {
    builder.with_no_client_auth()
};
```

New API:

```rust
// pingora-core-0.8.0/src/listeners/tls/rustls/mod.rs L93-96
pub fn set_client_cert_verifier(&mut self, verifier: Arc<dyn ClientCertVerifier>) {
    self.client_cert_verifier = Some(verifier);
}
```

### BoringSSL/OpenSSL Path: No Changes

`boringssl_openssl/mod.rs` is identical in 0.7.0 and 0.8.0. Edgion currently uses the `boringssl` feature and follows this path, so **the 0.8.0 mTLS change has no effect on the current Edgion version**.

## Impact on Edgion

### Current State (BoringSSL): No Action Required

Edgion's mTLS implementation is entirely based on custom BoringSSL logic. That path has no breaking API changes in 0.8.0, so the existing code remains fully compatible.

### Future State (If Rustls Support Is Added): This Is a Prerequisite

The `set_client_cert_verifier()` API in 0.8.0 is a **necessary prerequisite** for mTLS support on the Rustls backend. However, fully porting Edgion's mTLS capabilities to Rustls still requires addressing the following gaps:

## Edgion mTLS Features vs. Rustls Path Capabilities

| Edgion Feature | BoringSSL (Current) | Rustls (0.8.0) | Gap Analysis |
|----------------|---------------------|----------------|--------------|
| CA certificate verification | `ssl.set_verify_cert_store()` | `ClientCertVerifier` | ✅ Can be implemented through `WebPkiClientVerifier` |
| Mutual mode | `PEER \| FAIL_IF_NO_PEER_CERT` | Determined by verifier | ✅ Feasible |
| OptionalMutual mode | `PEER` | Determined by verifier | ✅ Can be implemented through `allow_unauthenticated()` |
| verify_depth | `ssl.set_verify_depth()` | Not exposed | ⚠️ Depth limit must be implemented manually in `ClientCertVerifier` |
| SAN/CN allowlist | BoringSSL FFI verify callback | Not provided | ⚠️ Must be implemented in a custom `ClientCertVerifier` trait implementation |
| SAN wildcard matching | `matches_pattern()` | Not provided | ⚠️ Validation logic can be reused, but must be wrapped in `ClientCertVerifier` |
| per-SNI+port config | `certificate_callback` dispatches by SNI | **Unsupported** | ❌ Rustls `with_callbacks()` returns an error directly |
| Dynamic certificate loading | `TlsAcceptCallbacks` | **Unsupported** | ❌ Same limitation; Rustls does not support certificate callbacks |
| Client certificate info propagation | `handshake_complete_callback` -> `ClientCertInfo` | **Unsupported** | ❌ Rustls cannot write this information into `digest.extension` after the handshake |

## Rustls mTLS Adaptation Approach (Future Reference)

If support for the Rustls backend is added in the future, the mTLS adaptation will need to be handled in two layers:

### Layer 1: Directly Implementable (Depends on 0.8.0 API)

By implementing the `rustls::server::danger::ClientCertVerifier` trait:

```rust
use rustls::server::WebPkiClientVerifier;

// Basic CA verification
let roots = Arc::new(load_ca_roots(ca_pem)?);
let verifier = WebPkiClientVerifier::builder(roots)
    .build()?;

// Or a custom verifier for SAN/CN allowlist + verify_depth
struct EdgionClientCertVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    allowed_sans: Option<Vec<String>>,
    allowed_cns: Option<Vec<String>>,
    verify_depth: u8,
}

impl ClientCertVerifier for EdgionClientCertVerifier {
    fn verify_client_cert(&self, ...) -> Result<...> {
        // 1. Use inner for basic CA chain validation
        // 2. Check certificate chain depth
        // 3. Extract SAN/CN and perform allowlist matching (reuse existing mtls.rs logic)
    }
}
```

### Layer 2: Blocked by Pingora Rustls Limitations

The following features require either upstream Pingora support or custom adaptation by Edgion:

1. **Dynamic certificate loading + per-SNI configuration** — Rustls `with_callbacks()` currently returns an error.
   Pingora upstream would need to add `TlsAcceptCallbacks` support for Rustls, or Edgion would need to implement SNI routing itself using the rustls `ResolvesServerCert` trait.

2. **Client certificate info propagation** — there is no `handshake_complete_callback`.
   The certificate information would need to be extracted at request-processing time (for example in `request_filter`) using rustls `ServerConnection::peer_certificates()` instead of handling it in a handshake callback.

## Action Items

- [ ] Continue tracking whether future Pingora versions add `TlsAcceptCallbacks` support for Rustls
- [ ] If a Rustls backend support project starts, prioritize evaluating an SNI-based dynamic routing approach (`ResolvesServerCert`)
- [ ] Evaluate whether the validation logic in `mtls.rs` can be abstracted into a TLS-backend-agnostic shared module
- [ ] Evaluate rustls `ServerConnection::peer_certificates()` as an alternative approach for propagating certificate information

## Related Files

| File | Description |
|------|-------------|
| `tasks/working/pingora-0.8.0-upgrade/06-mtls-native-support.md` | Original task (already completed, no action needed) |
| `src/core/gateway/tls/boringssl/mtls_verify_callback.rs` | BoringSSL FFI verify callback |
| `src/core/gateway/tls/validation/mtls.rs` | SAN/CN allowlist validation (reusable) |
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | mTLS configuration entry point |
| `src/types/resources/edgion_tls.rs` | `ClientAuthConfig` configuration struct |
| `src/types/ctx.rs` | `ClientCertInfo` definition |
