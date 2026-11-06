# gw-infra-01: TLS session resumption crosses SNI / mTLS policy boundaries (client-auth bypass)

- **Category**: design-error
- **Severity**: P0 (correctness/security, fix now)
- **Scope**: local
- **Location**: `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs:490-596` (`new_tls_settings_with_callback`, select-certificate callback), `tls_pingora.rs:299-373` (`certificate_callback`), `tls_pingora.rs:997-1058` (`configure_mtls`), `tls_pingora.rs:375-437` (`handshake_complete_callback`); Pingora fork `pingora-core/src/listeners/tls/boringssl_openssl/mod.rs:73-101` (`SslAcceptor::mozilla_intermediate_v5`)

## Summary
Every HTTPS listener uses one shared `SSL_CTX`; per-SNI policy (EdgionTls / Gateway `frontendValidation`
mTLS, client CA store, `allowedSans`/`allowedCns`, min TLS version) is applied per connection inside the
select-certificate and certificate callbacks. Nothing sets a session-ID context (`SSL_set_session_id_context`)
or disables tickets/session cache, so BoringSSL's defaults (stateless tickets + server session cache, empty
`sid_ctx`) let a session established under one SNI/policy be resumed under any other SNI on the same
listener. A resumed handshake never requests a client certificate, so `Mutual` mTLS, CA pinning and SAN/CN
allow-lists can be bypassed.

## Current behavior (evidence)
- `new_tls_settings_with_callback` builds `TlsSettings::with_callbacks` (Mozilla intermediate acceptor) and
  only adds `set_select_certificate_callback`; no `set_session_id_context`, `set_options(NO_TICKET)`,
  `set_session_cache_mode` or ticket-key callback exists anywhere in `edgion-gateway/src`, `edgion-common/src`
  or the Pingora fork listener code (grep for `sid_ctx|session_id_context|NO_TICKET|session_cache_mode`
  returns nothing).
- mTLS is installed per SSL in `configure_mtls` (`set_verify_cert_store`, `set_verify(PEER|FAIL_IF_NO_PEER_CERT)`,
  optional SAN/CN verify callback) from `certificate_callback`.
- BoringSSL 5.4 (`deps/boringssl/ssl/ssl_session.cc:474-521`): `ssl_session_is_resumable` only checks
  `sid_ctx` equality, endpoint type, time, version, cipher, peer-cred *form* and QUIC-ness. It does not check
  SNI, verify mode, or the trust store. With an empty `sid_ctx` on every connection, all sessions on the
  listener are mutually resumable.
- On resumption, `tls13_server.cc` `do_read_client_certificate` skips certificate request
  (`if (!hs->cert_request) { ... }`, no CertificateRequest in PSK mode); the resumed session inherits the
  original session's peer cert (or none). `FAIL_IF_NO_PEER_CERT` is not re-evaluated.
- `handshake_complete_callback` then reports `is_mtls` from the *current* SNI match and extracts
  `client_cert_info` from the *resumed* session, so downstream consumers (`header_cert_auth`, DSL builtins,
  condition extractors, access log) see either no certificate or a certificate validated against a different
  CA / SAN policy.

Attack paths on one 443 listener hosting `public.example.com` (no client auth) and `admin.example.com`
(`clientAuth.mode: Mutual`):
1. Full handshake to `public` (no client cert) -> receive ticket -> new ClientHello with SNI `admin` +
   that PSK -> resumed, no client cert requested, handshake completes.
2. Two mTLS hosts with different CAs or `allowedSans`: a cert valid for host A resumes on host B, bypassing
   B's CA and SAN allow-list.
3. CA rotation / client cert revocation: a ticket keeps authenticating until ticket lifetime expires.

The integration probe for mTLS fail-close explicitly *disables resumption*
(`skills/04-review/testing/regression-guards/mandatory-integration-gates-fail-closed.md`), so the existing
gates cannot observe this.

## Industry comparison
- Envoy: `ServerContextImpl` derives `SSL_CTX_set_session_id_context` from a SHA-256 over the server cert
  names/SANs plus the full validation context (trusted CA, SAN matchers, cert hashes/SPKI pins, verify mode),
  per filter chain, precisely so a session cannot be resumed across trust contexts; session ticket keys are
  distributed explicitly via SDS (`session_ticket_keys`) or disabled (`disable_stateless_session_resumption`).
- NGINX: `ngx_ssl_session_id_context` hashes the server certificate and `ssl_client_certificate`/
  `ssl_trusted_certificate` into the session id context per server block; OpenSSL additionally refuses to run
  verify-peer with an empty sid_ctx.

## Impact
Client-certificate authentication (and every policy that relies on the verified peer cert) can be bypassed by
any client that can complete a handshake with any other SNI served by the same listener. This is the
canonical multi-tenant listener layout.

## Suggested direction
1. In the select-certificate callback, after the SNI policy is resolved, call `SSL_set_session_id_context`
   with a digest of the selected policy: selected EdgionTls/Gateway attachment identity and generation,
   serving cert fingerprint(s), client-auth mode, client CA store fingerprint, SAN/CN allow-lists, verify
   depth, min TLS version. Compute it on the cold publication path (`CompiledEdgionTls` / `GatewayTlsEntry`)
   and store it next to the compiled identity so the handshake only copies 32 bytes. BoringSSL evaluates
   `sid_ctx` after `select_certificate_cb`, so this is effective for both TLS 1.2 and 1.3.
2. Short term / defense in depth: disable tickets and the server session cache for any SNI whose policy has
   `Mutual`/`OptionalMutual` client auth (`SSL_set_options(SSL_OP_NO_TICKET)` + no-cache for that SSL).
3. Decide ticket-key ownership explicitly (per-process random keys today: no cross-replica resumption, and no
   rotation story). Either keep per-process keys with documented rotation, or add a Secret-backed key list like
   Envoy SDS session_ticket_keys.
4. Add an integration case that *enables* resumption and asserts cross-SNI resumption onto an mTLS host fails
   to resume (full handshake with CertificateRequest).

## Notes
Checked `skills/04-review/tls/` (SKILL index, `intentional-design/revocation-config-not-implemented.md`,
`gateway-tls-candidate-ownership.md`, `mtls-config-reorder.md`, `terminate-mtls-ca-ordering.md`,
`cross-gateway-sni-collision-deterministic.md`). `revocation-config-not-implemented.md` only records that the
reserved session-ticket/cache *schema fields* are unimplemented; no page discusses `sid_ctx` or cross-SNI
resumption. Not tracked under `tasks/`. BoringSSL evidence is from the vendored `boring-sys-5.4.0` sources in
the local cargo registry, which match the locked `boring` 5 dependency.

## Parent verification (2026-11-05)

Re-checked independently: `grep` finds no `set_session_id_context` / ticket-disable call anywhere in
`edgion-gateway/src/{tls,runtime}`; vendored BoringSSL (`boring-sys-5.2.0`) `ssl_session_is_context_valid`
(`ssl/ssl_session.cc:466`) only compares `sid_ctx` spans, and BoringSSL has no OpenSSL-style
"session id context uninitialized" rejection. Reasoning confirmed; still prove it with an integration probe that
enables resumption (resume a no-mTLS SNI ticket against an mTLS-required SNI on the same listener) before fixing.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P0 unchanged. A live resumption probe should still precede the fix (as the parent note says).
- `grep session_id_context|NO_TICKET|session_cache_mode|set_options` over `edgion-gateway/src` returns nothing. `tls_pingora.rs` contains no ticket or resumption handling.
- Vendored `boring-sys-5.4.0/deps/boringssl/ssl/ssl_session.cc:495-522` `ssl_session_is_resumable` checks only sid_ctx, endpoint, time, version, cipher, peer-cred form and QUIC. It does not check SNI, verify mode or the trust store. No hostname check exists in `tls13_server.cc` or `handshake_server.cc` resumption paths.
- `tls13_server.cc:667-669` sets `cert_request` only inside `if (using_certificate(hs))`, which is false for PSK resumption. A resumed handshake onto a `Mutual` SNI therefore never requests a client certificate.
- `tls_pingora.rs:550-580` applies per-SNI policy (EdgionTls/GatewayTls, mTLS, min version) per connection on one shared context. This confirms that cross-SNI sessions share an empty sid_ctx.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P0 unchanged.
- `tls_pingora.rs` changed substantially (downstream policy / curve policy extraction into `tls/downstream_policy.rs`, `tls/curve_policy.rs`), but the diff contains no session, ticket or resumption handling; `grep -e session_id_context -e NO_TICKET -e session_cache_mode -e sid_ctx` over `edgion-gateway/src` and the locked Pingora fork (`e5f4287`, `pingora-core/src`) still returns nothing.
- `new_tls_settings_with_callback` (`tls_pingora.rs:490-596`) still builds one shared context and resolves per-SNI policy in the select-certificate callback; `configure_mtls` (`:997-1058`) still applies mTLS per SSL.
- The only new resumption note (`skills/04-review/tls/regression-guards/boring-v5-fips-build-verification.md:94-106`) concerns TLS 1.3 cipher restriction and callback order, not cross-SNI session isolation.
