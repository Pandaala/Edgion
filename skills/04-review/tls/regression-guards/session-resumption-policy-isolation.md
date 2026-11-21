---
name: session-resumption-policy-isolation
description: Use when reviewing downstream TLS session resumption, SSL_set_session_id_context, session tickets or the session cache, or any change to the inputs of CompiledEdgionTls/GatewayTlsEntry client-auth or serving-identity policy.
knowledge_type: regression-guards
---

# gw-infra-01 — Session resumption is isolated per selected downstream TLS policy

## Conclusion and scope

Each downstream HTTPS/TLS listener uses one BoringSSL `SSL_CTX` shared by all of its SNIs, and per-SNI
policy is selected per connection. The select-certificate callback must install a
per-SSL session id context identifying the selected effective policy before the
handshake reaches session lookup. Owner: `apply_selected_downstream_policy` in
`edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`, which selects the id in
the same branch order as `certificate_callback` (overlay → EdgionTls → Gateway) and
calls `SSL_set_session_id_context`; a failure is a select_cb policy failure
(`"session context"`, fail-closed, one ssl.log).

Ids are computed on the cold publication path with the canonical
`FingerprintHasher` under the versioned domain
`edgion.io/downstream-tls-session-context/v1`:

- `CompiledEdgionTls::session_context` (`tls/store/compiled_tls.rs`,
  `edgion_tls_session_context`): namespace, name, serving chain digest, and the
  effective client auth — mode (absent ≡ `Terminate` when selected on its own; an overlay rejects any `clientAuth` and issues no session), and for
  `Mutual`/`OptionalMutual` the verify depth, CA-set digest, typed `allowedSans`
  and `allowedCns` (empty ≡ absent).
- `GatewayTlsEntry::session_context()` (`runtime/matching/tls.rs`,
  `gateway_session_context`, derived once at construction into a private field):
  attachment identity (gateway namespace/name, listener name, listener key), a
  present/failed marker plus count and each serving chain digest, the frontend
  outcome tag (NotConfigured 0, StrictEffective 1, InsecureFallback 2,
  StrictReject 3), and for `StrictEffective` the frontend CA-set digest (the
  matcher always supplies one; a CA failure compiles to `StrictReject`, which
  select_cb rejects before any id is installed). The fixed verify depth and the
  reject reason are not inputs.
- Overlay (Gateway frontend decision + EdgionTls certificate):
  `SessionContextId::overlay` = first 16 bytes of each id, a stack copy, distinct
  from both parts with overwhelming probability.

Chain and CA-set digests come from the original DER (`der_list_digest`, count
plus length-prefixed entries). EdgionTls identity/CA and Gateway serving-chain
compile failures are encoded as markers; the handshake then fails closed in
cert_cb, so no session exists under them. The handshake only copies 32 bytes. A
selection shape select_cb never produces fails closed (`"session context
selection"`); an FFI install failure logs `"session context"`.

## Rationale and evidence

BoringSSL 5.4 `ssl_session_is_resumable` (`ssl/ssl_session.cc:495-522`) compares
only the session id context, endpoint, time, exact version, exact cipher,
peer-credential form and QUIC — never SNI, verify mode or the trust store. A
resumed handshake sends no CertificateRequest (`tls13_server.cc` sets
`cert_request` only for certificate-authenticated handshakes). With an empty
context everywhere, a one-way-TLS session resumed under a `Mutual` SNI and
completed with no client certificate (TLS 1.2 tickets and TLS 1.3 PSK, reproduced
before the fix). `select_certificate_cb` runs before cert_cb and before both the
TLS 1.2 (`handshake_server.cc:744-759`) and TLS 1.3 (`tls13_server.cc:557`)
resumption checks; `SSL_set_session_id_context` writes the per-SSL
`config->cert->sid_ctx`, which `SSL_certs_clear` does not reset, and new
sessions copy it.

Industry practice: Envoy hashes server certificate names/issuer, the complete
client validation context and the filter chain's server names into
`SSL_CTX_set_session_id_context`; NGINX hashes the certificate and client CA per
server block; Traefik/Go isolate by per-`tls.Config` ticket keys.

## Alternatives and limits

- Excluded inputs: minimum version and cipher (BoringSSL requires exact equality,
  chosen fresh from the per-SSL policy); ECDH curves (EGC-global, cannot differ
  between selections); `resourceVersion` (content-addressed so unchanged relists
  keep resumption); `should_expose_client_cert` (output projection only).
- A new authorization input added to EdgionTls client auth or Gateway frontend
  validation belongs in the digest; the sensitivity tests
  (`edgion_tls_session_context_tracks_effective_client_auth_policy`,
  `gateway_session_context_encodes_every_policy_input`) only cover the inputs they
  enumerate, so extend them with the digest.
- [Client leaf expiry under an unchanged policy](../intentional-design/client-certificate-expiry-on-resumption.md)
  is an accepted session lifetime tradeoff.
- Downstream listeners use BoringSSL's automatically rotated, per-`SSL_CTX`
  ticket keys. A replica change or restart costs a full handshake. Revisit if
  operators need cross-replica resumption or coordinated key rotation.
- Tightening `downstreamTls.ecdhCurves` constrains new key exchanges. A TLS
  1.2 abbreviated handshake reuses the original master secret, so an existing
  session can resume until expiry. Revisit if a curve removal must invalidate
  previously negotiated sessions immediately.
- Rejected: disabling tickets for mTLS hosts only (leaves other identities
  mutually resumable), per-policy ticket keys (no TLS 1.2 cache coverage),
  `SSL_CTX` per policy (Pingora listener restructuring).

## Verification and re-evaluation

- Unit: `session_resumption_is_isolated_per_selected_policy` and
  `gateway_frontend_validation_sessions_are_isolated` (TLS 1.2 tickets, TLS 1.2
  session-ID cache, TLS 1.3; same-policy control, cross-policy attack, legitimate
  mTLS; the Gateway pair differs only in the frontend decision),
  `selected_session_context_follows_the_applied_branch` (through
  `apply_selected_downstream_policy`, all three shapes plus fail-closed shapes),
  `edgion_tls_session_context_tracks_effective_client_auth_policy`,
  `compiled_edgion_tls_session_context_follows_ca_bundle_and_serving_chain`,
  `gateway_session_context_tracks_effective_frontend_policy`,
  `gateway_session_context_encodes_every_policy_input`,
  `der_list_digest_is_length_and_boundary_sensitive`. With the install forced to
  a no-op, each resumption mode of both handshake tests fails (verified
  2026-11-06).
- Integration: `mtls_cross_sni_resumption_refused` in the EdgionTls mTLS suite
  (rustls TLS 1.2 and 1.3 with an SNI-agnostic session store; same-SNI resume
  control and valid-client mTLS control; passed 2026-11-06 via
  `run_integration.sh -r EdgionTls -i mTLS`).
- Re-evaluate when: Edgion moves to per-policy `SSL_CTX`s, installs credential-level
  session id contexts, enables 0-RTT or ECH, adds ticket-key management, or
  upgrades BoringSSL across a change to `ssl_session_is_resumable`.

## Resolution references

- gw-infra-01 — 2026-11-06, fixed by the per-SSL session ID context. The
  subsequent review accepted the resumption limits described above.
