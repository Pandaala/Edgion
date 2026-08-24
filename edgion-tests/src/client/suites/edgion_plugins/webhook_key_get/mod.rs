// WebhookKeyGet Plugin Test Module
//
// Tests the WebhookKeyGet plugin across the integration fixtures: backend
// resolution (Service+EndpointSlice, ServiceClusterIp), outbound request
// construction (origin/custom/template/secret allow-list), presence policies,
// pathOverride, URL targets, json body, and WebhookKeyGet-then-KeyAuth —
// verified through the full gateway pipeline: request → webhook call →
// response extraction → ctx set → access log (and, for scenario 19, KeyAuth).
//
// Config: one self-contained fixture per scenario in
// edgion-tests/integration/conf/EdgionPlugins/WebhookKeyGet/NN-*.yaml.
// Mixed local+webhook cases (09, 10) keep CtxSet first, then WebhookKeyGet.

mod webhook_key_get;

pub use webhook_key_get::WebhookKeyGetTestSuite;
