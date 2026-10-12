---
name: gateway-upstream-tls
description: Upstream TLS (gateway→backend) — mTLS configuration, BackendTLSPolicy, certificate extraction.
---

# Upstream TLS (gateway→backend)

## Overview

Upstream TLS handles the TLS connection from the Gateway to backend services. When a backend requires TLS, the Gateway acts as a TLS client to establish a secure connection. Configuration is driven by the Gateway API `BackendTLSPolicy` resource.

## Module structure

```
edgion-gateway/src/tls/runtime/backend/
├── mod.rs               # BoringSSL-gated module entry
├── backend_api.rs       # BoringSSL backend API
└── cert_extractor.rs    # client certificate info extraction utility
```

## BackendTLSPolicy

`BackendTLSPolicy` is a Gateway API standard resource that defines the TLS policy from the Gateway to a specific backend service. After route matching and backend selection complete, `pg_upstream_peer.rs` queries this policy to decide the TLS configuration of the upstream connection.

Key configuration items:

- **Target backend**: the Service to enable TLS against, specified via `targetRef`.
- **Verification**: CA certificate, hostname verification.
- **mTLS**: the Gateway, acting as the client, presents its certificate to the backend.

The BackendTLSPolicy lookup happens inside `pg_upstream_peer.rs`, where the route namespace info is available, so namespace inheritance is handled correctly.

## mTLS to backend

When BackendTLSPolicy requires mTLS, the Gateway must:

1. Verify the backend's server certificate.
2. Present its own client certificate to the backend.

### Verify callback setup

`backend_api.rs` provides the BoringSSL `set_mtls_verify_callback` used by the custom TLS runtime:

```rust
pub fn set_mtls_verify_callback(
    ssl: &mut SslRef,
    verify_mode: SslVerifyMode,
    edgion_tls: &Arc<EdgionTls>,
) -> Result<(), String>
```

It calls `boringssl::mtls_verify_callback::set_verify_callback_with_whitelist` and supports SAN/CN
allowlist verification. The gateway has no OpenSSL backend branch; the custom callback runtime is
not implemented for the rustls build.

## Certificate info extraction

`cert_extractor.rs` provides the `extract_client_cert_info` function, which extracts detailed information about the peer certificate from an SSL connection:

```rust
pub fn extract_client_cert_info(ssl: &SslRef) -> Option<ClientCertInfo>
```

Extracted information:

| Field | Description |
|-------|-------------|
| `subject` | Certificate Subject DN, formatted as `CN=xxx, O=yyy` |
| `cn` | Common Name |
| `sans` | List of Subject Alternative Names |
| `fingerprint` | SHA256 fingerprint, formatted as `xx:xx:xx:...` |

### SAN extraction

Supported SAN types:

- **DNS**: extracted directly as a string.
- **IP**: parses the binary as IPv4 (4 bytes) or IPv6 (16 bytes, formatted via `std::net::Ipv6Addr`); prefixed with `IP:`.
- **Email**: prefixed with `email:`.
- **URI**: prefixed with `uri:`.

The function is mainly used in downstream mTLS scenarios (the `handshake_complete_callback` extracting client certs), but it can also be used to inspect upstream-connection certificates.

## Backend certificate SAN verification

`BackendTLSPolicy.validation.subjectAltNames` configures backend certificate SAN verification, supporting Hostname and URI (SPIFFE) types.

### TLS backend boundary

Additional `subjectAltNames` enforcement is implemented by the supported
BoringSSL Gateway backend. The dormant rustls feature has no BackendTLSPolicy
behavior contract; do not use it for Gateway deployments or interpret its
current mechanics as supported enforcement.

### BoringSSL verification flow

1. Policy publication prepares one immutable hook for non-empty `subjectAltNames`; `create_tls_peer()` clones it into Pingora's `upstream_tls_handshake_complete_hook`.
2. After the TLS handshake, the hook extracts backend certificate SANs via `extract_sans_from_ssl()`.
3. `validate_backend_sans()` compares the certificate SANs with the policy (any match passes). Extraction and validation run inside `catch_redacted_panic`; a panic produces a typed failed result with a fixed reason. Each invocation creates its own result.
4. The verification result is stored in `SslDigest.extension`.
5. `connected_to_upstream()` reads the result and disconnects if it does not match.

### Connection-pool isolation

Under BoringSSL, `create_tls_peer` sets `group_key` from the effective TLS profile
even without `subjectAltNames`. Policy publication fingerprints the trust mode,
complete custom CA payloads, hostname, typed SANs, and policy client Secret data
(full certificate chain and private key). Gateway identity publication prepares a complete Secret-data digest alongside its
native identity and fresh raw Secret Arc. Peer construction combines that selected
fixed-size digest with the profile. Pingora includes `group_key` in the
connection reuse hash, so changed policy material selects another pool bucket.
Existing streams are not forcibly terminated. See the
[pool isolation regression guard](../../../04-review/tls/regression-guards/backend-tls-pool-isolation.md)
for evidence and limits. Any future rustls support must provide equivalent
enforcement through the umbrella design task.

### Match rules

- **Hostname**: exact match or wildcard (`*.example.com` matches a single subdomain level).
- **URI**: exact match (e.g., `spiffe://cluster.local/ns/default/sa/myapp`).
- When `subjectAltNames` is not configured, existing behavior is preserved.

## Certificate validation

The `tls/validation/` module provides certificate validity checks:

- `validate_cert`: checks whether an EdgionTls resource's certificate is valid (PEM format, key match, etc.).
- `CertValidationResult`: validation result, containing an `is_valid` flag and an `errors` list.
- Validation runs synchronously when a certificate is written into TlsStore; invalid certificates are not added to the matcher.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/tls/runtime/backend/backend_api.rs` | Unified backend API, set_mtls_verify_callback |
| `edgion-gateway/src/tls/runtime/backend/cert_extractor.rs` | Certificate info extraction (subject / SAN / fingerprint) + `extract_sans_from_ssl` |
| `edgion-gateway/src/tls/validation/backend_san.rs` | Backend certificate SAN validation (Hostname / URI matching) |
| `edgion-gateway/src/tls/validation/cert.rs` | Certificate validity checks |
| `edgion-gateway/src/tls/validation/mtls.rs` | mTLS-related validation |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs` | BackendTLSPolicy lookup and upstream TLS connection |

## Native preparation ownership

Under the supported BoringSSL build, `backends/prepared_backend_tls.rs` parses
identity chains/private keys and complete custom CA bundles synchronously before
publication. Effective policy generations retain independent identity and CA
outcomes plus an immutable SAN hook. Only winning target/section candidates are
prepared; equivalent resources retain their effective Arc, and metadata-only
changes can share prepared components through exact input equality.

Gateway backend identity entries publish raw Secret Arc, prepared identity outcome
and full Secret-data digest together. Identical data reuses preparation, while
every valid publication still replaces the raw Arc for AllEndpointStatus freshness.
The native builder reads the live Gateway fallback only without policy identity.
It checks selected identity errors before CA errors; failed preparation stays
present and fails closed, and system trust ignores unused custom CA material.
Captured generations and peers retain old objects until their handles are released.
Preparation uses the existing owners, with no extra cache or lazy request parsing.
