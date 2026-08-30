# OpenidConnect SAN Match Fixture

## Test Intent

Verify that OpenID Connect outbound connections succeed when the server certificate's Subject Alternative Name (SAN) matches the validation policy configured in `tls.validation.subjectAltNames`.

## Test Scenario

- **Plugin**: `EdgionPlugins/openid-connect-san-match-test`
- **Configuration**:
  - `discovery: "https://127.0.0.1:30040/oidc/.well-known/openid-configuration"`
  - `tls.validation.subjectAltNames: [{type: Hostname, hostname: "127.0.0.1"}]`
  - `bearerOnly: true`, `verificationMode: IntrospectionOnly`

## Expected Result (Schema Validation)

- YAML parses correctly
- Schema accepts `tls.validation.subjectAltNames` in OpenidConnect plugin config
- No validation errors

## Expected Result (Integration Run)

**NOTE**: The current OIDC test infrastructure uses HTTP (port 30040 without TLS). This fixture configures HTTPS with SAN validation. To execute an integration run:

1. **Stand up an HTTPS OIDC server** with a certificate containing SAN `127.0.0.1` (or equivalent)
   - Either modify the mock OIDC server to support TLS on port 30040
   - Or use a separate TLS-enabled OIDC provider and update the `discovery` URL accordingly

2. **Expected TLS handshake behavior** (once HTTPS server is ready):
   - Gateway validates server certificate
   - `OutboundSanVerifier` confirms server SAN includes `127.0.0.1` → match confirmed
   - TLS connection succeeds

3. **Expected OIDC token flow** (once TLS succeeds):
   - Discovery document retrieved
   - Token introspection succeeds
   - Authorization proceeds normally
   - Note: access-log `diagnostics` must NOT contain the `OutboundTlsSanMismatch` error on this path

## Current Limitation

**The OIDC test server cert does not currently contain an SAN extension** matching `127.0.0.1` or another hostname. This fixture validates schema acceptance only; a live integration run requires:

- Generating an SAN-bearing certificate for the OIDC test server
- Or updating the discovery URL to use a TLS-enabled provider
- Updating this fixture's `subjectAltNames` if the server hostname changes

## Dependencies

- Pre-existing Secret: `edgion-default/oidc-client-secret`
- HTTPS OIDC test server on `127.0.0.1:30040` with SAN-bearing certificate (not yet available in test infrastructure)

## Reference

- Phase 2 Task 6: `docs/superpowers/plans/2026-06-29-outbound-tls-align-phase2.md`
- SAN validation schema: `edgion-resources/src/resources/outbound_tls.rs` (OutboundTls struct)
- OIDC plugin: `edgion-gateway/src/plugins/http/openid_connect/`
