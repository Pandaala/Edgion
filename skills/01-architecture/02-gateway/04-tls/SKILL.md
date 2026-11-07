---
name: gateway-tls-overview
description: TLS subsystem overview — TLS Store, SNI matching, certificate management, BoringSSL/rustls build boundaries, and FIPS policy.
---

# TLS subsystem overview

## Unified downstream TLS policy

`ResolvedGatewayConfig.downstream_tls` owns `DownstreamTlsPolicy`: the EGC
minimum, shared cold `CipherPolicy`, and existing numeric `CurvePolicy`.
`CompiledEdgionTls` uses the same cipher compiler. Absent ciphers inherit;
explicit `[]` means backend defaults; a rendered CString or rejection remains
immutable. Resources/Controller validate NUL and curve syntax without linking
BoringSSL. The strict backend cipher setter validates effective expressions.

The early callback first selects the native, EdgionTls or exact frontend-mTLS
overlay identity, then applies policy once through `apply_selected_downstream_policy`.
Local minimum/ciphers replace EGC defaults independently, including a lower local
minimum or a valid local cipher masking an invalid default. Curves have no local
override. Unspecified ciphers inherit active FIPS/backend defaults; an explicit
empty list bypasses only the cipher default, never FIPS curve restrictions.
Any effective-policy error emits one canonical SSL audit and returns select error.
Certificate callbacks configure identity/mTLS only and never reload/reapply policy.
The local EGC Arc lives through application; SSL owns the copied settings afterward,
so no redundant EGC Arc is stored in SslCtx. Identity Arcs retain their existing lifetime.

No Rust handshake cipher joining, parsing or temporary cipher allocation occurs.
BoringSSL's public strict cipher setter internally parses its string; this existing
backend cost remains accepted. No SSL_CTX swapping, new locks or stores are added.
TLS1.3 suites and physical listener ownership/retargeting boundaries remain unchanged.


## Architecture

The Edgion Gateway TLS subsystem is split into two directions plus a storage layer:

```
                    ┌─────────────────────────────┐
                    │         TLS Store            │
                    │  (tls_store + cert_matcher)  │
                    └──────────┬──────────────────┘
                               │
              ┌────────────────┼────────────────┐
              ▼                                 ▼
   ┌──────────────────┐              ┌──────────────────┐
   │  Downstream TLS  │              │   Upstream TLS   │
   │  (client→Gateway)│              │ (Gateway→backend)│
   │                  │              │                  │
   │ - TLS termination│              │ - BackendTLSPolicy│
   │ - SNI cert select│              │ - mTLS to backend│
   │ - mTLS client    │              │ - cert info      │
   │   verification   │              │   extraction     │
   └──────────────────┘              └──────────────────┘
```

## Module structure

```
edgion-gateway/src/tls/
├── mod.rs                          # module entry, feature-flag conditional compilation
├── store/
│   ├── mod.rs
│   ├── tls_store.rs                # TlsStore: certificate storage and validation
│   ├── cert_matcher.rs             # TlsCertMatcher: listener+SNI certificate matching
│   └── conf_handler.rs             # ConfHandler<EdgionTls> implementation
├── runtime/
│   ├── mod.rs
│   ├── gateway/
│   │   ├── mod.rs
│   │   └── tls_pingora.rs          # TlsCallback: Pingora listener TLS callbacks
│   └── backend/
│       ├── mod.rs
│       ├── backend_api.rs           # BoringSSL backend API
│       └── cert_extractor.rs        # client certificate info extraction
├── validation/
│   ├── mod.rs
│   ├── cert.rs                      # certificate validity checks
│   └── mtls.rs                      # mTLS-related validation
├── boringssl/
│   ├── mod.rs
│   └── mtls_verify_callback.rs      # BoringSSL mTLS verification callback
├── fips_profile.rs                 # approved downstream profile + policy resolution
└── fips_host.rs                    # host-kernel FIPS detection
```

## Certificate storage

### TlsStore

`TlsStore` is the canonical certificate storage layer; it stores all `EdgionTls` resources keyed by `namespace/name`:

- Publishes entries, listener/hostname slots, exact attachment candidates, and certificate counts in one immutable `TlsSnapshotData` through one `ArcSwap`.
- Uses structurally shared `rpds` maps: partial updates touch only changed resource keys and their old/new hostname and attachment slots, including when all certificates share one listener.
- Validation runs synchronously on write. An invalid update retains the same key's last-known-good certificate; an add representing delete/create never inherits it. Invalid attachment dimensions revoke old matcher admission.
- The conf-sync cache write lock serializes production writers. Store instances own isolated publication sources; the global store and global matcher share one source. Readers capture one snapshot without a lock.
- Global singleton: `GLOBAL_TLS_STORE` (`LazyLock<Arc<TlsStore>>`).

### TlsCertMatcher

`TlsCertMatcher` is a high-performance matcher used on the TLS handshake hot path:

```rust
pub struct TlsCertMatcher {
    data: ArcSwap<TlsSnapshotData>,
}
```

- Two-level persistent index: `ListenerKey → hostname slot → ordered EdgionTls candidates`.
- Uses `edgion_algo::matcher::rfc6125_hostname::lookup` for exact and single-label wildcard matches, the same algorithm as `Rfc6125HostnameMatcher::get`.
- Uses `ArcSwap` for lock-free reads. The match call (`match_sni`) runs inside the TLS handshake callback and must return quickly.
- Global singleton: `TLS_CERT_MATCHER` (`LazyLock<Arc<TlsCertMatcher>>`), shared with the global TLS store.
- Includes only valid certificates with bounded dimensions. Physical listener slots use `resolved_listeners`; the separate exact attachment index uses resolved Gateway attachment proofs and rejects complete groups above 64 candidates.

### GatewayTlsMatcher

`GatewayTlsMatcher` matches TLS configuration and compiled serving/client-auth state for Gateway listeners. A matched Gateway frontend client-auth decision owns the connection; ordinary certificate-only Gateway entries remain the fallback after EdgionTls matching:

```rust
struct TlsMatcherData {
    listener_map: HashMap<ListenerKey, Rfc6125HostnameMatcher<Vec<GatewayTlsEntry>>>,
}
```

- Extracts certificate references and resolved ordered outcomes from Gateway Listener TLS configuration, and cold-compiles a complete non-empty serving-certificate set before publication.
- Same two-level index `ListenerKey → Rfc6125HostnameMatcher(hostname → GatewayTlsEntry)`.
- `ArcSwap` for lock-free reads; the inner `Option` provides a fast path (returns immediately when no Gateway TLS configuration exists).
- Global singleton: `GATEWAY_TLS_MATCHER`.
- Rebuilt via `rebuild_from_gateways` whenever Gateway configuration changes.

## SNI matching flow (post-Phase 1 refactor)

The TLS handshake is composed of two BoringSSL callbacks chained in state-machine order:

### 1. Early callback `select_certificate_callback` (after ClientHello, before version negotiation)

Registration site: `new_tls_settings_with_callback` (`tls_pingora.rs`).
Trigger: BoringSSL `handshake_server.cc:694`, **earlier** than the version range freeze at `handshake_server.cc:710`.

Responsibilities (this is the only true effective point for `min_tls_version`):

- Extract the SNI from ClientHello (`ch.servername(NameType::HOST_NAME)`).
- Capture both the EdgionTls and Gateway matcher results for the listener/SNI.
- If the Gateway entry has a frontend client-auth decision, process it first. `StrictReject` closes the connection immediately; an EdgionTls certificate may overlay a usable decision only when its resolved attachment identifies that exact winning Gateway listener.
- Otherwise use the ordinary EdgionTls match first, then the certificate-only Gateway entry as fallback.
- Resolve selected EdgionTls minimum over the EGC default, then call `ssl.set_min_proto_version(...)` per connection.
  - On failure, fail-closed: `err_log = "minimum TLS version rejected"` + `log_ssl` + return `Err(SelectCertError::ERROR)`.
- Install the selected policy's session id context (`SSL_set_session_id_context`) so a session never resumes under another selected policy; see the [session resumption isolation guard](../../../04-review/tls/regression-guards/session-resumption-policy-isolation.md).
- Write the selected EdgionTls, Gateway entry, or exact Gateway/EdgionTls overlay pair plus the resolved SNI into SSL ex_data via `store_ssl_ctx` as a snapshot.
- Return `Ok(())` only after a usable match is selected and `store_ssl_ctx` succeeds, allowing the handshake to proceed to cert_cb.
- Return `Err(SelectCertError::ERROR)` after writing the connection audit when any early decision fails: SNI and configured fallback SNI are both absent; the Gateway frontend outcome is `StrictReject`; exact overlay selection finds a client-auth/attachment conflict; applying the selected EdgionTls minimum version fails; installing the session id context fails (`"session context"` / `"session context selection"`); snapshot storage fails; or neither matcher supplies a certificate.

Why `min_tls_version` cannot be set in cert_cb: BoringSSL `handshake_server.cc:710` makes it explicit — the version range is frozen immediately after select_cb; cert_cb (`L759`) is one step too late.

### 2. Certificate callback `certificate_callback` (cert_cb, handshake_server.cc:759)

Implemented as `TlsAccept::certificate_callback`. Pingora uses `SSL_set_cert_cb` to pause the handshake into an async-friendly form.

cert_cb is a **pure snapshot consumer** — all SNI resolution and Layer 1 / 2 matching happen in select_cb. select_cb already fail-closes on `store_ssl_ctx` failure by returning Err, so once cert_cb runs the snapshot must exist and must record exactly one hit.

Responsibilities:

1. `take_ssl_ctx` consumes the snapshot written by select_cb (consume-style read; the ex_data slot is cleared automatically). `take_ssl_ctx` returning None is unreachable in theory (guaranteed by the select_cb contract); if it happens, treat it as a BoringSSL state-machine violation, write `err_log = "no snapshot"` + `log_ssl`, and exit.
2. Snapshot has an exact Gateway/EdgionTls overlay pair: configure the Gateway frontend client-auth decision first, then call `apply_edgion_tls_cert` for the serving identity.
3. Snapshot has EdgionTls: call `apply_edgion_tls_cert` directly.
4. Snapshot has GatewayTls: call `apply_gateway_tls_cert` directly. It requires a complete non-empty `compiled_serving_certificates` set, selects one with `select_gateway_certificate`, configures frontend validation before binding the identity, and installs it with `apply_compiled_certificate_to_ssl`.
5. Snapshot exists but has no usable match (theoretically unreachable): write `err_log = "snapshot empty"` + `log_ssl` and exit.
6. `err_log` was set during apply (cert/key load, install, mTLS configuration failure): `log_ssl` immediately and **skip finalize** — ex_data stays null (the entry-time `take_ssl_ctx` already cleared it), BoringSSL aborts the handshake because the cert was not installed, and `free_ssl_ctx` returns immediately on a null ptr, avoiding double logging.
7. Apply succeeded: call `finalize_ssl_ctx` to stamp `tls_id` and write the SslCtx back to ex_data via `store_ssl_ctx` for `handshake_complete_callback` to consume.

`apply_edgion_tls_cert` does only the following inside cert_cb timing: cert/key install and mTLS verify configuration. **It no longer handles `min_tls_version`**.

### 3. Completion callback `handshake_complete_callback`

`take_ssl_ctx` → take client cert info (if mTLS) → stamp `handshake_complete_time` → `log_ssl` → return `Arc<TlsConnMeta>` as the digest extension for downstream HTTP / TLS proxy code.

### 4. Early-abort audit hook (introduced in Task 10)

When select_cb has stored a snapshot but cert_cb is never invoked (typical case: version negotiation fails and BoringSSL's alert protocol_version terminates the handshake outright), the FFI release callback `free_ssl_ctx` fires when the SSL object is destroyed.

`free_ssl_ctx` checks the private `failure_logged` flag and emits any unclaimed failure, including failures after certificate installation. Without a specific error, it uses `"early abort"` before finalization or `"handshake failed"` afterward. The mTLS rejection callback claims the flag before logging; successful completion consumes the snapshot. A populated `tls_id` does not prove handshake completion or audit emission.

Wrapped in `edgion_common::startup::catch_redacted_panic` so neither a panic nor a dynamic panic payload can cross or leak through the FFI boundary (same pattern as `mtls_verify_callback`).

### Overall priority

```
ClientHello → select_cb (SNI resolve + identity selection + effective TLS policy + session id context + snapshot)
            ├─ no SNI / cert not found / min ver fail / session context fail / store_ssl_ctx fail
            │     → log_ssl once + return Err → BoringSSL abort, cert_cb does not fire
            └─ hit → snapshot stored → return Ok
            → freeze version range
            → version negotiation (may be rejected here by alert; triggers free_ssl_ctx "early abort" audit)
            → cert_cb (consume snapshot + apply cert/mtls; no fallback matching)
            ├─ apply failure → log_ssl once + skip finalize → BoringSSL abort
            └─ success → finalize_ssl_ctx + store_ssl_ctx writes back to ex_data
            → cipher selection (TLS1.2)
            → handshake_complete_callback (writes ssl.log)
```

Match failures are handled in select_cb only (`"cert not found"`); cert_cb performs no fallback matching.

## Certificate sources

| Source | CRD | Match layer | Description |
|--------|-----|-------------|-------------|
| EdgionTls | `edgion.io/EdgionTls` | TlsCertMatcher | Edgion-specific TLS resource, supports advanced configuration: mTLS, minimum version, cipher suites, etc. |
| Gateway Listener | `gateway.networking.k8s.io/Gateway` | GatewayTlsMatcher | Standard Gateway API TLS configuration; references Kubernetes Secrets via `certificateRefs` |

## File list

| File | Topic |
|------|-------|
| [01-downstream-tls.md](01-downstream-tls.md) | Downstream TLS (client→gateway) — Pingora Listener callbacks, certificate selection, client cert verification |
| [02-upstream-tls.md](02-upstream-tls.md) | Upstream TLS (gateway→backend) — mTLS configuration, BackendTLSPolicy, certificate extraction |
| [03-ssl-backends.md](03-ssl-backends.md) | TLS build backends — BoringSSL/rustls capability boundary and opt-in FIPS image policy |

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/tls/store/tls_store.rs` | TlsStore: certificate storage and validation |
| `edgion-gateway/src/tls/store/cert_matcher.rs` | TlsCertMatcher: listener+SNI matching |
| `edgion-gateway/src/tls/store/conf_handler.rs` | ConfHandler<EdgionTls> implementation |
| `edgion-gateway/src/runtime/matching/tls.rs` | GatewayTlsMatcher |
| `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs` | TlsCallback |
| `edgion-gateway/src/tls/validation/cert.rs` | Certificate validation |

## Downstream ECDH policy ownership

`resources::downstream_tls` owns the four classical curve names and pure validation;
Gateway `tls::curve_policy` compiles ordered numeric IDs during EGC publication.
`ResolvedGatewayConfig` is the sole policy owner. There is no listener option,
curve-policy matcher projection or certificate-resource override.

The early select callback loads the existing bound EGC generation once and applies
its policy with numeric `SSL_set1_group_ids` after identity selection, before protocol negotiation.
All SNI names and both native and EdgionTls identities use that same policy. No
Edgion-side parsing, joining, policy allocation or lock is added per handshake;
BoringSSL copies the bounded group slice into per-SSL configuration. FIPS rejects
incompatible explicit groups and preserves valid NIST subsets/order; the implicit
profile uses the same numeric setter. Certificate branches never reapply minimum, cipher or curve policy. Errors use the existing single SSL audit/rejection path.

The physical socket retains its startup-owner EGC binding; GatewayClass reference
retargeting and ownership transfer remain outside this feature. EGC updates affect
new handshakes, while existing TLS sessions retain negotiated state. See
[the regression boundary](../../../04-review/tls/regression-guards/downstream-ecdh-policy-ownership.md).
