// ResponseRewrite plugin tests
//
// Required config files (in examples/test/conf/EdgionPlugins/ResponseRewrite/):
// - 01_EdgionPlugins_status-code.yaml
// - 02_EdgionPlugins_headers-set.yaml
// - 03_EdgionPlugins_headers-rename.yaml
// - 04_EdgionPlugins_combined.yaml
// - HTTPRoute_default_response-rewrite.yaml

mod response_rewrite;

pub use response_rewrite::ResponseRewriteTestSuite;
