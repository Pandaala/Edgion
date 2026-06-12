// ProxyRewrite plugin tests
//
// Required config files (in examples/test/conf/EdgionPlugins/ProxyRewrite/):
// - EdgionPlugins_default_proxy-rewrite.yaml
// - HTTPRoute_default_proxy-rewrite.yaml

mod proxy_rewrite;

pub use proxy_rewrite::ProxyRewriteTestSuite;
