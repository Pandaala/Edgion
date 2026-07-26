# OpenidConnect SAN Mismatch Fixture

## Test Intent

Verify that OpenID Connect outbound connections fail with `OutboundTlsSanMismatch` error when the server certificate's Subject Alternative Name (SAN) does NOT match the validation policy configured in `tls.validation.subjectAltNames`.

## Test Scenario

- **Plugin**: `EdgionPlugins/openid-connect-san-mismatch-test`
- **Configuration**:
  - `discovery: "https://127.0.0.1:30040/oidc/.well-known/openid-configuration"`
  - `tls.validation.subjectAltNames: [{type: Hostname, hostname: "nonmatching.example.com"}]`
  - `bearerOnly: true`, `verificationMode: IntrospectionOnly`

## Expected Result (Schema Validation)

- YAML parses correctly
- Schema accepts `tls.validation.subjectAltNames` in OpenidConnect plugin config
- No validation errors

## Expected Result (Integration Run)

**NOTE**: The current OIDC test infrastructure uses HTTP (port 30040 without TLS). This fixture configures HTTPS with SAN validation. To execute an integration run:

1. **Stand up an HTTPS OIDC server** with a certificate that does NOT contain SAN `nonmatching.example.com`
   - Typically the test server cert will have SAN `127.0.0.1` or a hostname like `localhost`
   - The policy demands `nonmatching.example.com` → mismatch confirmed

2. **Expected TLS handshake behavior** (once HTTPS server is ready):
   - Gateway validates server certificate (CA check passes)
   - `OutboundSanVerifier` checks for SAN `nonmatching.example.com` → NOT FOUND
   - TLS connection rejected

3. **Expected HTTP response** — the probe request MUST carry a bearer token:
   - **Without** an `Authorization: Bearer <token>` header there is no outbound call at all:
     `bearerOnly: true` short-circuits to `401 Unauthorized - Missing bearer token` and the counter
     does **not** move. `unauthAction: Deny` is what selects that 401; it plays no part in the SAN
     path.
   - **With** a bearer token, verification runs, the handshake fails, and the plugin returns
     **`502`** (`OidcVerifyError::Other { status: 502 }` → `send_plain_error`) — not a 401/403, and
     the auth-failure delay is deliberately skipped for 502.
   - Counter `edgion_gateway_outbound_tls_san_mismatch_total{plugin="openid_connect"}` increments.
   - Access log carries the `PluginLog` tag from `err.log_tag()` for the 502; see below for why it
     carries no `OutboundTlsSanMismatch` status.

   **Which SAN site actually fires here.** With `verificationMode: IntrospectionOnly` and a cold
   cache, `verify_token_via_introspection` calls `get_or_fetch_discovery()` *before* the
   introspection POST, so the handshake fails inside `jwks_provider.rs` — that site increments the
   counter and nothing else. The introspection POST's own SAN site
   (`token_exchange::note_oidc_san_mismatch`) is not reached in this fixture, and it is likewise
   counter-only by design: it owns no `PluginLog`, and the request hot path forbids `tracing::*`
   (log-safety Iron rule 3). Both sites were `tracing::warn!`-free as of 2026-07-26; do not expect
   any gateway log line for this fixture beyond the access-log entry for the 502.

   **No `OutboundTlsSanMismatch` in the access log for this fixture.** Only two frames stamp the
   status (and write the plugin log entries `outbound tls san mismatch` + `san <reason>`):
   `request_filter_impl.rs` (the `UnauthAction::Auth` arm) and `callback.rs` (`handle_callback`).
   Every other frame that reaches `get_or_fetch_discovery()` — including this fixture's
   `verify_token_via_introspection` — `map_err`s the typed `JwksProviderError::TlsSanMismatch` away
   and stamps nothing. The first frame is unreachable here: `bearerOnly: true` short-circuits before
   the `unauth_action` match. The second *is* dispatched under `bearerOnly: true` (the callback-path
   check runs before the bearer-only short-circuit), but it returns at the very first guard — this
   fixture declares no `sessionSecretRef`, so `resolve_session_secret()` fails and it replies
   `401 Unauthorized - Missing session secret` long before the discovery fetch. To assert on the
   access-log `errors` array, use the LDAP fixture or an OIDC fixture with the interactive
   (non-bearer-only) flow.

## Current Limitation

**The OIDC test server cert does not currently contain an SAN extension** matching `127.0.0.1` or another hostname. This fixture validates schema acceptance only; a live integration run requires:

- Generating an SAN-bearing certificate for the OIDC test server
- Ensuring the test server cert SAN does NOT include `nonmatching.example.com` (likely satisfied by any real cert)

## Dependencies

- Pre-existing Secret: `edgion-default/oidc-client-secret`
- HTTPS OIDC test server on `127.0.0.1:30040` with SAN-bearing certificate (not yet available in test infrastructure)

## Reference

- Phase 2 Task 6: `docs/superpowers/plans/2026-06-29-outbound-tls-align-phase2.md`
- SAN mismatch status code: `edgion-resources/src/edgion_status.rs` (OutboundTlsSanMismatch = 999_09)
- SAN validation logic: `edgion-gateway/src/tls/validation/outbound_san_verifier.rs` (OutboundSanVerifier)
