---
module: tls-secrets
title: OCSP stapling and CRL/OCSP revocation checking are schema-reserved no-ops; revoked certificates are silently accepted
severity: high
envoy_reference: Envoy `transport_socket.tls.common_tls_context.validation_context.crl` (CRL file), Envoy OCSP stapling via `ocsp_staple` field in `TlsCertificate`, Envoy OCSP `must_staple` policy
---

# OCSP stapling and certificate revocation are not implemented

## Envoy's design (the proven baseline)

Envoy provides two revocation mechanisms:

1. **CRL**: `validation_context.crl` accepts a PEM/DER CRL file. BoringSSL
   checks client certificate revocation status against it during the mTLS
   handshake. A revoked client cert causes the handshake to fail, same as an
   untrusted cert.

2. **OCSP stapling**: Each `TlsCertificate` in an Envoy listener can carry an
   `ocsp_staple` containing a DER-encoded OCSP response. Envoy staples it
   during the TLS handshake so clients can verify freshness without an extra
   round trip. The `ocsp_staple_policy` field controls fail-open
   (`LENIENT_STAPLING`) vs fail-closed (`STRICT_STAPLING`).

Both are delivered as first-class config (and via SDS) so they can be rotated
independently of the certificate. Operators running a compliance workload (PCI
DSS, HIPAA) can enforce revocation checking as a real runtime control.

## Edgion's current design

Edgion defines the schema for both features under `EdgionTlsExtend` but
explicitly marks both as **reserved, not implemented**:

- `EdgionTlsExtend.ocsp_stapling: Option<OcspStaplingConfig>` —
  `src/types/resources/edgion_tls.rs:191`. The struct has `enabled`, `refresh_interval_seconds`,
  `fail_open`. The doc comment reads: "NOT IMPLEMENTED (reserved schema): the
  gateway performs NO OCSP stapling at the TLS handshake regardless of these
  values."
- `EdgionTlsExtend.revocation_check: Option<RevocationCheckConfig>` —
  `src/types/resources/edgion_tls.rs:203`. The struct has `mode: off/ocsp/crl`,
  `fail_open`. The doc comment reads: "NOT IMPLEMENTED (reserved schema): the
  gateway performs NO certificate revocation checking regardless of `mode`."

The validation path in `cert.rs:188-205` detects when either field is set and
emits a `tracing::warn!` at config-load time, but then continues. No OCSP
request is ever made, no staple is ever attached, and no CRL is ever consulted.

## The problem / gap

From Envoy's standpoint, both features address different but equally serious
security gaps:

- **OCSP stapling gap**: Clients that implement OCSP Must-Staple (RFC 7633)
  will reject a TLS certificate if the server does not staple a fresh OCSP
  response. Edgion silently serves no staple, which causes those clients to fail
  or fall back to a soft-fail path. More importantly, operators cannot use
  Edgion to deploy `must_staple` certificates, even though the schema suggests
  it is possible.
- **Revocation checking gap** (for mTLS): If a client certificate is revoked
  (compromised key), the CA's CRL or OCSP responder records the revocation, but
  Edgion's mTLS path (`configure_mtls` in `tls_pingora.rs:1017-1073`) builds
  an `X509Store` from the CA bundle and calls `ssl.set_verify_cert_store` —
  with no CRL attached. A revoked client cert that is otherwise structurally
  valid will pass mTLS verification. This is a real security gap for any
  deployment that uses client cert revocation as a security control (e.g., after
  a compromised service account cert).

The warn-but-accept behavior means an operator who reads the schema, sets
`mode: ocsp`, and deploys, receives no runtime error — only a warning in the
gateway trace log — and believes revocation is enforced when it is not.

## Impact

- For mTLS: a revoked client certificate is accepted at the TLS layer. The only
  downstream protection is application-level validation (e.g., `allowed_sans`
  whitelist), which does not check revocation status.
- For server certificates: no OCSP staple means `must_staple` certs cannot be
  deployed behind Edgion, and privacy-aware clients that perform OCSP checks
  must contact the CA directly on every connection.

## Suggested direction

- **Short term**: change the warning to a hard admission error when
  `revocation_check.mode != off` or `ocsp_stapling.enabled == true`, so
  operators are not silently misled. This is safer than "accept and warn."
- **Medium term (OCSP stapling)**: implement background OCSP response fetching
  and BoringSSL stapling via `SSL_CTX_set_tlsext_status_cb` / `SSL_set_tlsext_status_arg`.
  The OCSP response can be stored alongside the `TlsEntry` in `TlsStore` and
  applied in `cert_cb` after the cert is installed.
- **Medium term (CRL for mTLS)**: add a `crl_secret_ref` field to
  `ClientAuthConfig`; the controller resolves it and inlines the CRL bytes into
  `EdgionTls`; `configure_mtls` calls `X509StoreBuilder::add_crl` before
  `build()` and sets `X509VerifyFlags::CRL_CHECK`.
