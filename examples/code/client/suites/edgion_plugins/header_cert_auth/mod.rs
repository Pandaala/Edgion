// Header Cert Auth plugin tests
//
// Required config files (in examples/test/conf/EdgionPlugins/HeaderCertAuth/):
// - 01_Secret_default_header-cert-ca.yaml
// - EdgionPlugins_default_header-cert-auth.yaml
// - HTTPRoute_default_header-cert-auth-test.yaml

mod header_cert_auth;

pub use header_cert_auth::HeaderCertAuthTestSuite;
