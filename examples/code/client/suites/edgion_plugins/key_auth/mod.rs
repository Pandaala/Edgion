// Key Auth plugin tests
//
// Required config files (in examples/test/conf/EdgionPlugins/KeyAuth/):
// - 01_Secret_default_api-keys.yaml           # API keys secret
// - EdgionPlugins_default_key-auth.yaml       # KeyAuth plugin config
// - HTTPRoute_default_key-auth-test.yaml      # Route with host: key-auth-test.example.com

mod key_auth;

pub use key_auth::KeyAuthTestSuite;
