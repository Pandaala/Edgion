---
name: multi-certificate-ref
description: Use when reviewing findings that a Gateway listener ignores extra certificateRefs; explains that apply_gateway_tls_cert now applies ALL resolved certs (RSA+ECDSA dual-cert) and rejects re-reports.
---

# tls-04 — Gateway listener serves all certificateRefs (RSA + ECDSA)

**Status:** ✅ resolved · 2026-05-20
**Spec:** RFC 5280 §4.2.1.6 (informative, multi-SAN/multi-algo) + Gateway API listener multi-`certificateRefs` (dual-cert is a supported deployment)

## Conclusion

A Gateway listener with multiple `certificateRefs` now serves the cert that
matches the connecting client. **BoringSSL exposes a single certificate slot per
`SSL` (unlike OpenSSL's per-key-type slots), so the dual-cert case requires
explicit selection in cert_cb** — iterating `SSL_use_certificate` would only
leave the last cert (last-wins). `select_gateway_secret` prefers the ECDSA cert
only for **TLS 1.3** clients that advertise an ECDSA `signature_algorithm`; RSA
otherwise. This mirrors Envoy's `default_tls_certificate_selector.cc`.

Re-reports of the form "only the first `certificateRef` is used / extra refs
silently dropped" are **not accepted** — selection landed in `apply_gateway_tls_cert`.

## Code shape today

- `tls_pingora.rs::apply_gateway_tls_cert` — consumes the matcher-cold-path
  `GatewayTlsEntry.compiled_serving_secrets`; each ordered outcome has already
  been checked against its exact group/kind/namespace/name/index before
  `select_gateway_secret` picks one and `apply_secret_to_ssl` applies it.
- `tls_pingora.rs::select_gateway_secret` (boringssl) — single secret: no-op;
  multiple: prefer ECDSA only when `negotiated_is_tls13(ssl)` **and**
  `client_supports_ecdsa(ssl)` and an ECDSA cert exists, else first RSA, else
  first. Non-boringssl build: first secret.
- `tls_pingora.rs::pick_secret_index` — pure selection rule over key-type kinds
  (`Some(true)`=ECDSA / `Some(false)`=RSA / `None`=unparseable); unit-tested.
- `tls_pingora.rs::negotiated_is_tls13` — safe `SslRef::version2() == Some(SslVersion::TLS1_3)`.
- `tls_pingora.rs::client_supports_ecdsa` — FFI `SSL_get0_peer_verify_algorithms`
  + `SSL_get_signature_algorithm_key_type` == `EVP_PKEY_EC`.
- Controller side (`handlers/gateway.rs`) resolves every declared ref into a
  complete ordered typed outcome. The Gateway has no global SecretStore fallback.

## Why not iterate SSL_use_certificate per cert

On OpenSSL that populates separate RSA/ECDSA slots and the library auto-selects.
On BoringSSL each `SSL_use_certificate` *replaces* the single slot (last-wins),
so iterating serves only one key type. The first attempt at this fix did exactly
that and the `gateway_tls_dual_cert` integration test caught it (RSA-pinned
handshake failed `NO_COMMON_SIGNATURE_ALGORITHMS`). Explicit ClientHello-driven
selection is the BoringSSL-correct approach.

## Why ECDSA preference is gated to TLS 1.3

In TLS 1.2 the server certificate's key type also constrains the usable cipher
suites: BoringSSL derives the cipher auth mask from the installed cert (an EC key
is `SSL_aECDSA`-only), and cipher negotiation happens *after* cert_cb. So
selecting ECDSA from `signature_algorithms` alone could yield
`NO_SHARED_CIPHER` for a TLS 1.2 client that advertised ECDSA sigalgs but offered
only `ECDHE-RSA` ciphers — a regression versus the old RSA-first code. TLS 1.3
decouples auth from the cipher, so sigalg-based selection is safe there. For
TLS 1.2 we therefore keep legacy first-cert (RSA) behavior — identical to the
pre-fix code, so no regression. A faithful TLS 1.2 multi-cert selector would also
have to inspect the client's offered cipher auth masks (not currently wired up).

## Fail-closed semantics (intentional)

If the selected cert/key fails to load, the handshake fails (err_log set,
BoringSSL aborts) rather than silently serving nothing. Consistent with the
cert_cb fail-closed convention and the "do not silently drop" intent.

## Decision rule

- "Gateway listener ignores second/third `certificateRef`" → **rejected**, fixed.
- "Just iterate `SSL_use_certificate` for each cert" → **rejected**: BoringSSL
  single-slot makes that last-wins, not dual-cert. Selection callback required.
- "Prefer ECDSA for all clients (drop the TLS 1.3 gate)" → **rejected**: breaks
  TLS 1.2 clients that offer only ECDHE-RSA ciphers (NO_SHARED_CIPHER). The gate
  is deliberate.
- "Add a `tracing::warn!` when >1 cert is configured" → **rejected**: cert_cb is
  the data-plane handshake hot path; tracing is prohibited (see
  [03-coding/01-log-safety.md](../../03-coding/01-log-safety.md)). Use `err_log`.

## Known limitations / follow-ups (not blocking)

- **TLS 1.2 dual-cert is RSA-only** by design (see gate above). True TLS 1.2
  ECDSA serving would need cipher-auth-mask inspection of the ClientHello.
- **All-ECDSA listener under TLS 1.2**: with no RSA cert to fall back to,
  `pick_secret_index(prefer_ecdsa=false)` returns index 0 (an ECDSA cert), so a
  TLS 1.2 client that offered only ECDHE-RSA ciphers still hits NO_SHARED_CIPHER.
  This is physically unsolvable without an RSA cert and is identical to pre-fix
  behavior — not a regression. (Confirmed: `SSL_version` at cert_cb returns the
  negotiated version by BoringSSL server state-machine ordering — version
  negotiation precedes cert_cb precedes cipher negotiation.)
- **Ed25519/Ed448 leaf certs** classify as non-ECDSA (`secret_is_ecdsa` checks
  `Id::EC` only), so they land in the RSA bucket. Out of scope for the RSA+ECDSA
  feature; revisit if Ed25519 dual-cert is ever requested.
- **Secret sync window**: matcher publication is complete-set atomic. During an
  incomplete resolution window the listener does not serve a partial subset;
  it becomes usable after the owner is requeued with all ordered outcomes.

## Re-evaluation triggers

- Gateway API changes multi-`certificateRefs` semantics.
- BoringSSL changes its single-slot cert behavior or adds an SSL_CREDENTIAL-style
  multi-cert API in the pinned `boring`/`pingora` version.

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a4.md → V07`
- Integration test: `gateway_tls_dual_cert` (TLS1.3 ECDSA/RSA sigalg pinning +
  TLS1.2 RSA guard) in
  `edgion-tests/src/client/suites/gateway/tls/gateway_tls/gateway_tls.rs`
- Unit test: `pick_secret_index` tests in `tls_pingora.rs`
- Architecture: [01-architecture/02-gateway/04-tls/01-downstream-tls.md](../../01-architecture/02-gateway/04-tls/01-downstream-tls.md)
