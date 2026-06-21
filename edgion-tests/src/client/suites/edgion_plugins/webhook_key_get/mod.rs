// WebhookKeyGet Plugin Test Module (LinkSys webhook redesign — Wave E)
//
// Tests the redesigned KeyGet::Webhook variant integration with the CtxSet plugin
// across 11 end-to-end scenarios: backend resolution (Service+EndpointSlice,
// ServiceClusterIp), outbound request construction (origin/custom/template/secret
// allow-list), presence policies, and pathOverride — verified through the full
// gateway pipeline: request → webhook call → response extraction → ctx set →
// access log.
//
// Config: one self-contained fixture per scenario in
// edgion-tests/integration/conf/EdgionPlugins/WebhookKeyGet/NN-*.yaml (Task E1).

mod webhook_key_get;

pub use webhook_key_get::WebhookKeyGetTestSuite;
