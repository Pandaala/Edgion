---
name: gateway-downstream-tls
description: Downstream TLS (client→gateway) — Pingora Listener TLS callbacks, certificate selection, client certificate verification.
---

# Downstream TLS (client→gateway)

## Overview

Downstream TLS handles the TLS handshake from clients to the Gateway. The core implementation is the `TlsCallback` struct, which implements Pingora's `TlsAccept` trait and injects custom logic at key stages of the TLS handshake.

HTTPS listener construction passes the resolved Gateway `edgion.io/enable-http2`
flag to `TlsCallback::new_tls_settings_with_callback`; that factory alone enables
HTTP ALPN. The default and enabled policies advertise h2 with HTTP/1.1 fallback.
The disabled policy leaves HTTP ALPN unset and accepts HTTP/1.1. TLS-to-TCP calls
the same factory with `false`, independently of the Gateway HTTP/2 flag, so it
advertises no HTTP ALPN. These settings are captured at listener startup; changing
the annotation requires a restart. See the
[HTTPS ALPN regression guard](../../../04-review/tls/regression-guards/https-http2-alpn-policy-handoff.md).

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
   - Set `is_mtls = et.tls.is_mtls_enabled()` (true only for Mutual / OptionalMutual; Terminate is not mTLS).
   - If `et.tls.spec.min_tls_version` is set → `ssl.set_min_proto_version(...)` per-connection. On failure, fail-closed: `err_log = "min ver: ..."` + `log_ssl` + return Err.
3. **Layer 2 match** (when Layer 1 misses): `GatewayTlsMatcher::match_sni` — query Gateway listener TLS.
4. **Write snapshot**: write the match result (`Arc<CompiledEdgionTls>` — the accepted resource plus its cold-compiled serving identity, CA store and cipher — or `GatewayTlsEntry`, plus the resolved SNI) into SSL ex_data via `store_ssl_ctx`. On `store_ssl_ctx` failure it has already done `log_ssl` once internally; select_cb returns Err directly (fail-closed), keeping at most one ssl.log per connection.
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

`apply_edgion_tls_cert` handles certificates from EdgionTls resources. The
serving chain, private key, optional mTLS CA store, and operator cipher string
are compiled once when `TlsStore` publishes an accepted generation
(`CompiledEdgionTls`), so this callback copies no Secret bytes and parses no PEM:

1. Apply the independent FIPS cipher/curve and mTLS policy before touching the
   serving identity.
2. Clear any identity inherited by the per-connection SSL.
3. Consume the compiled serving identity. A resource that failed to compile
   stamps its static category and returns here — after the clear in step 2, so
   no inherited identity can remain bound.
4. Recheck the compiled leaf's Unix validity bounds at handshake time.
5. Install the compiled chain and key atomically
   (`install_compiled_serving_identity`: `SSL_certs_clear` +
   `SSL_set_chain_and_key`), which clears again on failure, so neither an
   inherited identity nor a partially installed replacement remains usable.
6. Optional configuration (only items effective inside cert_cb timing):
   - **mTLS** (`configure_mtls`): install the compiled CA certificate store, then set verify mode and verify depth.
   - **Cipher suite** (`configure_ciphers`): apply the compiled TLS1.2 cipher string via BoringSSL FFI (still works in cert_cb because BoringSSL selects the cipher only after cert_cb — `handshake_server.cc:810`).

A resource whose mTLS CA content cannot be compiled keeps its matcher position
and fails the handshake closed; it is never dropped from the index, which would
silently promote the next candidate for that hostname.

Certificate/key read, parse, validity, and install failures use fixed
non-secret diagnostic categories. They never include PEM contents, certificate
dates, key material, or backend error-stack detail.

> **The effective point of `min_tls_version` has moved to `select_certificate_callback`** (see the previous section). By the time cert_cb runs, BoringSSL has frozen the version range; calling `set_min_proto_version` here is silently ineffective, so `apply_edgion_tls_cert` no longer handles this field.

### Apply Gateway TLS certificate

`apply_gateway_tls_cert` handles certificates for Gateway Listeners:

1. Use the complete ordered Secret outcomes resolved by the Controller from all `certificateRefs`. Matcher rebuild verifies each index and source identity, validates each certificate chain and private key with the shared strict parser, checks key/certificate matching, and retains only an immutable compiled Secret set. The Gateway never falls back to its global SecretStore; missing, denied, partial, reordered, or identity-mismatched outcomes remain fail-closed regardless of Secret existence.
2. Select the cert matching the client (`select_gateway_secret`). **BoringSSL exposes only a single certificate slot per `SSL` (unlike OpenSSL's per-key-type slots), so a dual-cert listener (RSA + ECDSA) cannot rely on the library to auto-pick — we select explicitly.** ECDSA is preferred only when **(a) the negotiated version is TLS 1.3** (`negotiated_is_tls13`, via the safe `SslRef::version2()`) **and (b)** the client advertised an ECDSA `signature_algorithm` (`client_supports_ecdsa`, FFI `SSL_get0_peer_verify_algorithms` + `SSL_get_signature_algorithm_key_type`); then the ECDSA cert is served, otherwise the first RSA cert. The TLS 1.3 gate is required because in TLS 1.2 the cert key type also constrains the usable cipher suites (ECDHE-ECDSA vs ECDHE-RSA) — selecting ECDSA purely from sigalgs there could break a client that offered only ECDHE-RSA ciphers, so TLS 1.2 keeps legacy first-cert behavior. TLS 1.3 decouples auth from the cipher, making sigalg-based selection safe. This mirrors BoringSSL's / Envoy's own selection. With a single cert it is a no-op. The pure selection rule is factored into `pick_secret_index` (unit-tested).
3. Install the selected immutable `CompiledServingCertificate` atomically with
   `apply_compiled_certificate_to_ssl`. Invalid Secret material is rejected
   during matcher rebuild; a missing, expired, or failed atomic identity remains
   fail-closed instead of falling back to another Secret.
4. Apply the matcher generation's compiled frontend outcome before binding a server certificate. Strict missing, partial, empty-key, or malformed CA sets are `StrictReject`; a reject candidate remains in the same SNI matcher slot and precedence order as a serving candidate.

> **Why not iterate `SSL_use_certificate` per cert?** On OpenSSL that populates separate RSA/ECDSA slots; on BoringSSL each call *replaces* the single slot (last-wins), so iterating would serve only one key type. Explicit ClientHello-driven selection is the BoringSSL-correct approach.

## Gateway frontendValidation (standard mTLS)

Gateway API standard frontend client certificate validation, configured via `spec.tls.frontend`. Completely independent of EdgionTls `client_auth`.

### Verify modes

| Mode | Behavior | Implementation |
|------|----------|----------------|
| `AllowValidOnly` (default) | A certificate signed by the matching CA is required and must be valid | `PEER \| FAIL_IF_NO_PEER_CERT` |
| `AllowInsecureFallback` | Valid / invalid / missing certificates all pass | Custom verify callback always returns 1 (boringssl FFI) |

### Configuration flow (`configure_gateway_frontend_mtls`)

1. The Controller erases raw listener projection fields, selects `spec.tls.frontend.default` or the authoritative matching `perPort` entry for HTTPS listeners, and resolves the complete Secret/ConfigMap reference set.
2. Matcher publication validates the exact ordered CA source identities, applies shared per-reference and aggregate byte/certificate limits, globally deduplicates certificates, and compiles one immutable outcome: `NotConfigured`, `StrictEffective`, `StrictReject`, or `InsecureFallback`. Strict mode is full-set atomic; no raw CA payload or previous trust store is retained in the published entry.
3. `configure_gateway_frontend_mtls` only installs the already-compiled trust store for an effective strict policy, or installs the insecure fallback callback without parsing or carrying CA payload. Strict rejection occurs before attachment scanning and before any server cert/key binding.
4. An EdgionTls certificate may compose with this policy only when both sides carry resolved, exactly equal Controller proofs covering the winning Gateway namespace, name, listener section, `ListenerKey`, final eligibility, and public certificate/CA fingerprints. Dual client-auth declarations reject as a conflict; a mismatched or stale-generation EdgionTls cannot shadow or conflict with the Gateway winner.

`GatewayTLSConfig.frontend_validation`, `resolved_frontend_ca_refs`, and `frontend_matcher_eligibility` are controller-private conf_sync fields and are excluded from generated schemas. The ordered resolution vector preserves the exact Secret/ConfigMap source identity and an explicit error for every declared reference; strict mode is effective only when the complete set validates. Operators configure only `Gateway.spec.tls.frontend`.

## EdgionTls mTLS client verification

When EdgionTls has `spec.client_auth` configured, `configure_mtls` enables client certificate verification:

### Verify modes

| Mode | SslVerifyMode | Description |
|------|---------------|-------------|
| `Terminate` | No verification | Ignores the client certificate and only terminates TLS |
| `Mutual` | `PEER + FAIL_IF_NO_PEER_CERT` | A valid client certificate is required |
| `OptionalMutual` | `PEER` | Verifies if a certificate is present; passes without one |

### Configuration flow

1. Install the `X509Store` compiled from the EdgionTls CA certificate PEM when
   the generation was published (cloning it is an `X509_STORE_up_ref` refcount
   bump, not a rebuild). `Terminate` mode needs no store and returns first.
2. Set the verify depth (`verify_depth`, range 1–9).
3. If `allowed_sans` or `allowed_cns` is configured, set a custom verify callback (`set_mtls_verify_callback`) for allowlist checks. SAN matching is typed (`AllowedSan`: kind × Exact/Prefix/Suffix/Contains/RegularExpression). RegularExpression is compiled when the EdgionTls is ingested into `TlsStore`, never on the handshake path. BoringSSL invokes that callback at every certificate-chain depth: `preverify_ok == 0` rejects immediately at any depth; client-identity extraction and SAN/CN allowlist evaluation run only at leaf depth 0 (`X509_STORE_CTX_get_error_depth`). Intermediate successful depths return 1 without extraction or a whitelist audit. A terminal mTLS failure emits at most one `ssl.log` record for that handshake.
4. Otherwise use the standard BoringSSL verification chain.

### Client certificate info extraction

After the handshake completes, `extract_client_cert_info` extracts client-certificate info from the SSL connection:

- Subject DN (Distinguished Name).
- Common Name (CN).
- Subject Alternative Names (DNS, IP, Email, URI, OtherName).
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
- The whole callback body is wrapped in `edgion_common::startup::catch_redacted_panic` so neither a panic nor a dynamic panic payload can cross or leak through FFI (same pattern as `mtls_verify_callback.rs`).
- Sets `"early abort"` only when `meta.err_log.is_none()`, preserving more specific causes set by upstream stages such as select_cb (e.g., `"min ver: ..."`).
- This path is available only on the boringssl backend (depends on the ex_data mechanism).

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs` | TlsCallback, SslCtx, certificate application |
| `edgion-gateway/src/tls/store/cert_matcher.rs` | TlsCertMatcher (EdgionTls listener+SNI matching) |
| `edgion-gateway/src/runtime/matching/tls.rs` | GatewayTlsMatcher (Gateway fallback matching) |
| `edgion-gateway/src/tls/runtime/backend/cert_extractor.rs` | Client certificate info extraction |
| `edgion-gateway/src/tls/boringssl/mtls_verify_callback.rs` | BoringSSL mTLS verification callback |
