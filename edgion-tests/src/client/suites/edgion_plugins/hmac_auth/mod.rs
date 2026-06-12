// HMAC Auth plugin tests
//
// Required config files (in edgion-tests/integration/conf/EdgionPlugins/HmacAuth/):
// - 01_Secret_default_hmac-credentials.yaml
// - EdgionPlugins_default_hmac-auth.yaml
// - HTTPRoute_default_hmac-auth-test.yaml
// - 02_EdgionPlugins_default_hmac-auth-anonymous.yaml
// - 03_HTTPRoute_default_hmac-auth-anonymous.yaml

mod hmac_auth;

pub use hmac_auth::HmacAuthTestSuite;
