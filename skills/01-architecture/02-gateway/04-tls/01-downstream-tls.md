---
name: gateway-downstream-tls
description: Downstream TLS (client→gateway) — Pingora Listener TLS callbacks, certificate selection, client certificate verification.
---

# Downstream TLS (client→gateway)

## Overview

Downstream TLS handles the TLS handshake from clients to the Gateway. The core implementation is the `TlsCallback` struct, which implements Pingora's `TlsAccept` trait and injects custom logic at key stages of the TLS handshake.

## Pingora TLS callbacks

`TlsCallback` implements two callbacks of the `TlsAccept` trait, plus an additional early callback registered through BoringSSL FFI. There are 4 ssl.log write stages, fired in BoringSSL state-machine order:

| # | Stage | Trigger location (`tls_pingora.rs`) | Normal / failure path |
|---|-------|------------------------------------|------------------------|
| 1 | `select_certificate_callback` (early cb) | The `set_select_certificate_callback` closure in `new_tls_settings_with_callback` | Failure: each of `no SNI` / `min ver: ...` / `cert not found` calls `log_ssl` once and returns `Err(SelectCertError::ERROR)` so BoringSSL aborts; on `store_ssl_ctx` failure, fail-closed (store_ssl_ctx itself logs ssl, select_cb just returns Err); on success, after `store_ssl_ctx` saves the snapshot, return `Ok(())` |
| 2 | `certificate_callback` (cert_cb) | `TlsAccept::certificate_callback` | Failure (apply set `err_log`): `log_ssl` immediately and skip `finalize_ssl_ctx`; ex_data stays null; BoringSSL aborts naturally because cert wasn't installed. On success, `finalize_ssl_ctx` stamps `tls_id` + `store_ssl_ctx` for stage 3 |
| 3 | `handshake_complete_callback` | `TlsAccept::handshake_complete_callback` | Handshake success → `log_ssl` once; failed handshakes never reach this stage |
| 4 | `free_ssl_ctx` (FFI release fallback) | `ssl_ctx_ex_data::free_ssl_ctx` | Only when ex_data is non-null (select_cb already stored a snapshot) AND `tls_id.is_none()` (finalize did not run): logs ssl belatedly with `err_log = "early abort"` if empty. Covers the early-abort case where "select_cb hit but BoringSSL terminates during version negotiation" |

**Core principle**: each stage logs ssl on failure at the time of failure plus aborts (no propagation to later stages); on success, do not log — let a later stage with more complete fields (`tls_id`, `handshake_complete_time`, `client_cert_info`, etc.) emit the log. A connection produces at most one ssl.log entry.

### select_certificate_callback (early callback, introduced in Phase 1)

Registration site: in `new_tls_settings_with_callback`, calls BoringSSL `SSL_CTX_set_select_certificate_cb` directly via `TlsSettings: DerefMut<SslAcceptorBuilder>` — no pingora patch required.

Trigger: BoringSSL `handshake_server.cc:694`, **earlier** than the version range freeze at `L710`. This is the only true effective point for `min_tls_version`.

Responsibilities:

1. **SNI resolution**: extract the SNI from ClientHello (`ch.servername(NameType::HOST_NAME)`); if missing, fall back to `EdgionGatewayConfig.security_protect.fallback_sni`; if both are missing → `err_log = "no SNI"` + `log_ssl` + return Err.
2. **Layer 1 match**: `TlsCertMatcher::match_sni(listener_key, sni)` — query EdgionTls. On hit:
   - Set `is_mtls = et.is_mtls_enabled()` (true only for Mutual / OptionalMutual; Terminate is not mTLS).
   - If `et.spec.min_tls_version` is set → `ssl.set_min_proto_version(...)` per-connection. On failure, fail-closed: `err_log = "min ver: ..."` + `log_ssl` + return Err.
3. **Layer 2 match** (when Layer 1 misses): `GatewayTlsMatcher::match_sni` — query Gateway listener TLS.
4. **Write snapshot**: write the match result (`Arc<EdgionTls>` or `GatewayTlsEntry` + the resolved SNI) into SSL ex_data via `store_ssl_ctx`. On `store_ssl_ctx` failure it has already done `log_ssl` once internally; select_cb returns Err directly (fail-closed), keeping at most one ssl.log per connection.
5. **Both miss**: `err_log = "cert not found"` + `log_ssl` + return Err.
6. **Success**: return Ok, allowing the handshake to proceed to cert_cb.

Why `min_tls_version` cannot be set in cert_cb: BoringSSL `handshake_server.cc:710` makes it explicit — the version range is frozen immediately after select_cb; by the time cert_cb (`L759`) runs, it can no longer be modified.

### certificate_callback (cert_cb, handshake_server.cc:759)

Invoked during the TLS handshake (after version negotiation, before cipher-suite negotiation).

cert_cb is a **pure snapshot consumer** — all SNI resolution, Layer 1 (EdgionTls) match, Layer 2 (GatewayTls) match, and `fallback_sni` application happen in select_cb in one pass. select_cb fail-closes on `store_ssl_ctx` failure by returning Err and BoringSSL aborts, so cert_cb does not fire. Therefore once cert_cb runs the snapshot must exist and must record exactly one hit (EdgionTls or GatewayTls).

Responsibilities:

1. **Take snapshot**: `take_ssl_ctx` consumes the snapshot written into ex_data by select_cb (auto-clears the slot). Returning None is unreachable in theory (violates the select_cb contract); if it happens, write `err_log = "no snapshot"` + `log_ssl` and exit, treating it as a BoringSSL state-machine violation.

2. **Apply certificate** (two mutually exclusive fast paths):
   - Snapshot has `matched_edgion_tls` → `apply_edgion_tls_cert`.
   - Snapshot has `matched_gateway_tls` → `apply_gateway_tls_cert`.
   - Both None in the snapshot (theoretically unreachable) → write `err_log = "snapshot empty"` + `log_ssl` and exit.

3. **Log failure on the spot**: if `apply_*_cert` set `err_log` during cert/key load, install, or mTLS configuration, cert_cb calls `log_ssl` immediately and **skips finalize**. ex_data stays null (cleared by the entry-time `take_ssl_ctx`); BoringSSL aborts the handshake because the certificate was not installed; `free_ssl_ctx` sees a null ptr and returns directly — single ssl.log emitted.

4. **Wrap up after success**: after a successful apply, call `finalize_ssl_ctx` to stamp `tls_id` (a hex identifier composed of timestamp + random number) and write the updated `SslCtx` back to ex_data via `store_ssl_ctx` for `handshake_complete_callback` to consume.

Principle: **log on failure at the time of failure plus abort, with no propagation**. A connection produces at most one ssl.log entry.

### handshake_complete_callback

Invoked after the TLS handshake completes; responsible for:

1. Retrieving `SslCtx` via `SSL_get_ex_data`.
2. If the connection is mTLS and client-certificate exposure is configured, extract client certificate info.
3. Record the handshake completion time.
4. Output the SSL log.
5. Return `Arc<TlsConnMeta>` as digest extension data for subsequent HTTP/TLS proxy access.

## Certificate selection logic

### Apply EdgionTls certificate

`apply_edgion_tls_cert` handles certificates from EdgionTls resources:

1. Extract cert PEM and key PEM from `EdgionTls`.
2. Parse into `X509` and `PKey` and set them on the SSL connection via the Pingora TLS extension API.
3. Optional configuration (only items effective inside cert_cb timing):
   - **mTLS** (`configure_mtls`): set the CA certificate store, verify mode, and verify depth.
   - **Cipher suite** (`configure_ciphers`): set the TLS1.2 cipher list via BoringSSL FFI (still works in cert_cb because BoringSSL selects the cipher only after cert_cb — `handshake_server.cc:810`).

> **The effective point of `min_tls_version` has moved to `select_certificate_callback`** (see the previous section). By the time cert_cb runs, BoringSSL has frozen the version range; calling `set_min_proto_version` here is silently ineffective, so `apply_edgion_tls_cert` no longer handles this field.

### Apply Gateway TLS certificate

`apply_gateway_tls_cert` handles certificates for Gateway Listeners:

1. Get the Secret from `GatewayTlsEntry`'s inline `secrets` (preferred).
2. Fall back to looking up Secrets by `certificateRefs` from the global SecretStore.
3. Extract the certificate and private key from the Secret's `data["tls.crt"]` and `data["tls.key"]`.
4. Parse and set them on the SSL connection.
5. If `frontend_validation` is present, call `configure_gateway_frontend_mtls()` to configure client certificate verification.

## Gateway frontendValidation (standard mTLS)

Gateway API standard frontend client certificate validation, configured via `spec.tls.frontend`. Completely independent of EdgionTls `client_auth`.

### Verify modes

| Mode | Behavior | Implementation |
|------|----------|----------------|
| `AllowValidOnly` (default) | A certificate signed by the matching CA is required and must be valid | `PEER \| FAIL_IF_NO_PEER_CERT` |
| `AllowInsecureFallback` | Valid / invalid / missing certificates all pass | Custom verify callback always returns 1 (boringssl FFI) |

### Configuration flow (`configure_gateway_frontend_mtls`)

1. Extract CA certificates from `resolved_frontend_ca_secrets` (key: `ca.crt`) and build an `X509Store`.
2. Pick a verify mode based on `mode`: AllowValidOnly uses standard SSL verify; AllowInsecureFallback uses `boring_sys::SSL_set_verify` + `insecure_fallback_verify_cb`.
3. Set `verify_depth(10)`.

The Controller merges per-listener configuration into each listener's `GatewayTlsConfig`; no extra processing is needed on the Gateway side.

## EdgionTls mTLS client verification

When EdgionTls has `spec.client_auth` configured, `configure_mtls` enables client certificate verification:

### Verify modes

| Mode | SslVerifyMode | Description |
|------|---------------|-------------|
| `Terminate` | No verification | Ignores the client certificate and only terminates TLS |
| `Mutual` | `PEER + FAIL_IF_NO_PEER_CERT` | A valid client certificate is required |
| `OptionalMutual` | `PEER` | Verifies if a certificate is present; passes without one |

### Configuration flow

1. Get the CA certificate PEM from EdgionTls and build an `X509Store`.
2. Set the verify depth (`verify_depth`, range 1–9).
3. If `allowed_sans` or `allowed_cns` is configured, set a custom verify callback (`set_mtls_verify_callback`) for allowlist checks.
4. Otherwise use the standard OpenSSL/BoringSSL verification chain.

### Client certificate info extraction

After the handshake completes, `extract_client_cert_info` extracts client-certificate info from the SSL connection:

- Subject DN (Distinguished Name).
- Common Name (CN).
- Subject Alternative Names (DNS, IP, Email, URI).
- SHA256 fingerprint.

The extracted `ClientCertInfo` is stored in `TlsConnMeta.client_cert_info` and is available to plugins and the access log.

## SslCtx and ex_data

Per-connection TLS context is passed between callbacks via the BoringSSL ex_data mechanism:

```
select_certificate_callback   certificate_callback          handshake_complete_callback
       │                              │                                │
       ├── SslCtx::new()              ├── take_ssl_ctx(ssl)            ├── take_ssl_ctx(ssl)
       ├── Layer 1 match              │   (consume snapshot)           ├── extract client cert
       ├── set_min_proto_version      ├── apply cert/mtls/ciphers      ├── stamp handshake_complete_time
       └── store_ssl_ctx(ssl, ctx)    ├── finalize_ssl_ctx (tls_id)    └── log_ssl + return Arc<TlsConnMeta>
                                      └── store_ssl_ctx(ssl, ctx)
                                              │
                                      (if handshake aborted, on SSL destruction)
                                      free_ssl_ctx → tls_id.is_none()?
                                              → log_ssl "early abort"
```

- `SSL_get_ex_new_index`: register a custom ex_data index (once globally) and a `free_ssl_ctx` release callback at the same time.
- `SSL_set_ex_data` / `store_ssl_ctx`: store a `Box<SslCtx>` pointer (select_cb and cert_cb each write once).
- `SSL_get_ex_data` / `take_ssl_ctx`: consume-style reads that transfer ownership (cert_cb and handshake_complete_cb each take once).
- `free_ssl_ctx`: invoked by BoringSSL when the SSL object is destroyed. Frees an SslCtx that was not consumed and, when `tls_id.is_none()` (`finalize_ssl_ctx` didn't run), emits an `"early abort"` audit log (see the section below).

In non-BoringSSL builds, ex_data operations are no-ops, and `handshake_complete_callback` returns a basic `TlsConnMeta`.

## Early-abort audit (introduced in Task 10)

When a connection is terminated by BoringSSL after `select_certificate_callback` but before `certificate_callback` (typical case: min_tls_version rejecting a TLS1.2 client via alert protocol_version), the regular ssl.log path (cert_cb → finalize_ssl_ctx → handshake_complete_callback → log_ssl) does not fire.

To complete the audit, `mod ssl_ctx_ex_data::free_ssl_ctx` (the memory-release callback registered with BoringSSL via `SSL_get_ex_new_index`) checks `tls_id.is_none()` on SSL object destruction — meaning `finalize_ssl_ctx` did not run — and pushes `"early abort"` into err_log (without overriding a more specific err_log) before calling `log_ssl`.

Implementation notes:
- The whole callback body is wrapped in `std::panic::catch_unwind` to prevent panics from crossing FFI (same pattern as `mtls_verify_callback.rs`).
- Sets `"early abort"` only when `meta.err_log.is_none()`, preserving more specific causes set by upstream stages such as select_cb (e.g., `"min ver: ..."`).
- This path is available only on the boringssl backend (depends on the ex_data mechanism).

## Key source files

| File | Responsibility |
|------|----------------|
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | TlsCallback, SslCtx, certificate application |
| `src/core/gateway/tls/store/cert_matcher.rs` | TlsCertMatcher (EdgionTls listener+SNI matching) |
| `src/core/gateway/runtime/matching/tls.rs` | GatewayTlsMatcher (Gateway fallback matching) |
| `src/core/gateway/tls/runtime/backend/cert_extractor.rs` | Client certificate info extraction |
| `src/core/gateway/tls/boringssl/mtls_verify_callback.rs` | BoringSSL mTLS verification callback |
