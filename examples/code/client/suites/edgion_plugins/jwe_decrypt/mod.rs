// JWE Decrypt plugin tests
//
// Required config files (in examples/test/conf/EdgionPlugins/JweDecrypt/):
// - 01_Secret_default_jwe-secret.yaml
// - EdgionPlugins_default_jwe-decrypt.yaml
// - HTTPRoute_default_jwe-decrypt-test.yaml

mod jwe_decrypt;

pub use jwe_decrypt::JweDecryptTestSuite;
