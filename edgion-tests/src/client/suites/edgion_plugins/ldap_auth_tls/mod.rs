// LDAP Auth TLS / CA / mTLS integration test suite.
//
// Required config files (in edgion-tests/integration/conf/EdgionPlugins/LdapAuthTls/):
// - 02_EdgionPlugins_default_ldap-auth-tls-ca.yaml       # LDAPS + CA via SecretRef (port 33636)
// - 03_HTTPRoute_default_ldap-auth-tls-ca.yaml           # Route for CA-only host
// - 04_EdgionPlugins_default_ldap-auth-tls-mtls.yaml     # LDAPS + mTLS (port 33637)
// - 05_HTTPRoute_default_ldap-auth-tls-mtls.yaml         # Route for mTLS host
// - 06_EdgionPlugins_default_ldap-auth-tls-wrong-ca.yaml # LDAPS + mismatched CA (inline)
// - 07_HTTPRoute_default_ldap-auth-tls-wrong-ca.yaml     # Route for wrong-CA host
// - 08_EdgionPlugins_default_ldap-auth-tls-starttls.yaml # STARTTLS + CA (port 33389)
// - 09_HTTPRoute_default_ldap-auth-tls-starttls.yaml     # Route for STARTTLS host
//
// Also requires a running ldap-openldap-tls Docker Compose stack:
// - edgion-tests/integration/conf/Services/ldap-openldap-tls/docker-compose.yml

mod ldap_auth_tls;
pub use ldap_auth_tls::LdapAuthTlsTestSuite;
