# LdapAuthTls SAN Match Fixture

## Test Intent

Verify that LDAP TLS outbound connections succeed when the server certificate's Subject Alternative Name (SAN) matches the validation policy configured in `tls.validation.subjectAltNames`.

## Test Scenario

- **Plugin**: `EdgionPlugins/ldap-auth-tls-san-match-test`
- **Configuration**: 
  - `ldapHost: "127.0.0.1"`
  - `ldaps: true` (implicit TLS)
  - `tls.validation.subjectAltNames: [{type: Hostname, hostname: "127.0.0.1"}]`
  - `tls.validation.caCertificateRefs: [Secret/ldap-ca-bundle]`

- **LDAP Server**: ldap-openldap-tls (port 33636)
  - Certificate SAN (from `Services/ldap-openldap-tls/make-test-certs.sh`):
    - DNS: `edgion-test-openldap-tls`
    - IP: `127.0.0.1`

## Expected Result (Integration Run)

1. **TLS Handshake**: SUCCESS
   - Gateway verifies the server certificate using the CA (ldap-ca-bundle)
   - `OutboundSanVerifier` checks that server SAN includes the policy hostname `127.0.0.1` → match confirmed
   
2. **LDAP Authentication**: SUCCESS
   - Login request succeeds; no SAN mismatch rejection
   - Access log shows clean LDAP exchange
   - Note: `EdgionStatus: 999_09` (OutboundTlsSanMismatch) is not wired into access log yet; tracked in `tasks/todo/outbound-tls-san-status-wiring.md`

## Dependencies

- Pre-existing Secret: `edgion-default/ldap-ca-bundle` (created by parent LdapAuthTls fixtures)
- LDAP test server cert must have IP SAN `127.0.0.1` (verified via make-test-certs.sh)

## Reference

- Phase 2 Task 6: `docs/superpowers/plans/2026-06-29-outbound-tls-align-phase2.md`
- LDAP cert generation: `edgion-tests/integration/conf/Services/ldap-openldap-tls/make-test-certs.sh` (lines 14-19)
- SAN validation schema: `edgion-resources/src/resources/outbound_tls.rs` (OutboundTls struct)
