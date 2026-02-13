// LDAP Auth plugin tests
//
// Required config files (in examples/test/conf/EdgionPlugins/LdapAuth/):
// - EdgionPlugins_default_ldap-auth.yaml                  # LDAP auth base config
// - HTTPRoute_default_ldap-auth-test.yaml                 # Route for base tests
// - 02_EdgionPlugins_default_ldap-auth-anonymous.yaml     # LDAP auth anonymous mode
// - 03_HTTPRoute_default_ldap-auth-anonymous.yaml         # Route for anonymous tests
// - 04_EdgionPlugins_default_ldap-auth-basic-scheme.yaml  # LDAP auth with headerType=basic
// - 05_HTTPRoute_default_ldap-auth-basic-scheme.yaml      # Route for basic-scheme tests
// - 06_EdgionPlugins_default_ldap-auth-hide-creds.yaml    # LDAP auth with hideCredentials=true
// - 07_HTTPRoute_default_ldap-auth-hide-creds.yaml        # Route for hideCredentials tests
//
// Also requires base config (in examples/test/conf/EdgionPlugins/base/):
// - Gateway.yaml                                          # Gateway for EdgionPlugins tests

mod ldap_auth;

pub use ldap_auth::LdapAuthTestSuite;
