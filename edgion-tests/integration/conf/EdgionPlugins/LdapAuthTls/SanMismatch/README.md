# LdapAuthTls SAN Mismatch Fixture

## Test Intent

Verify that LDAP TLS outbound connections fail with `OutboundTlsSanMismatch` error when the server certificate's Subject Alternative Name (SAN) does NOT match the validation policy configured in `tls.validation.subjectAltNames`.

## Test Scenario

- **Plugin**: `EdgionPlugins/ldap-auth-tls-san-mismatch-test`
- **Configuration**:
  - `ldapHost: "127.0.0.1"`
  - `ldaps: true` (implicit TLS)
  - `tls.validation.subjectAltNames: [{type: Hostname, hostname: "nonmatching.example.com"}]`
  - `tls.validation.caCertificateRefs: [Secret/ldap-ca-bundle]`

- **LDAP Server**: ldap-openldap-tls (port 33636)
  - Certificate SAN (from `Services/ldap-openldap-tls/make-test-certs.sh`):
    - DNS: `edgion-test-openldap-tls`
    - IP: `127.0.0.1`

## Expected Result (Integration Run)

1. **TLS Handshake**: FAILURE
   - Gateway verifies the server certificate using the CA (ldap-ca-bundle) ✓
   - `OutboundSanVerifier` checks that server SAN includes `nonmatching.example.com` → NOT FOUND
   - Connection rejected with TLS error

2. **LDAP Authentication**: FAILS — no LDAP exchange occurs
   - Plugin reports a generic outbound connection/auth failure
   - The error chain from `OutboundSanVerifier` contains the substring `"SAN mismatch:"` — operator can grep gateway logs for this marker
   - `EdgionStatus: 999_09` (OutboundTlsSanMismatch) is **not yet wired** into the access log; tracked in `tasks/todo/outbound-tls-san-status-wiring.md`

## Dependencies

- Pre-existing Secret: `edgion-default/ldap-ca-bundle` (created by parent LdapAuthTls fixtures)
- LDAP test server cert must have IP SAN `127.0.0.1` (verified via make-test-certs.sh)

## Reference

- Phase 2 Task 6: `docs/superpowers/plans/2026-06-29-outbound-tls-align-phase2.md`
- SAN mismatch status code: `edgion-resources/src/resources/edgion_status.rs` (OutboundTlsSanMismatch = 999_09)
- SAN validation logic: `edgion-common/src/security/tls_validation.rs` (OutboundSanVerifier)
