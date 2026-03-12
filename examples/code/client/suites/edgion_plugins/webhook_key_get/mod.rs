// WebhookKeyGet Plugin Test Module
//
// Tests KeyGet::Webhook variant integration with CtxSet plugin.
// Verifies that webhook-based key resolution works correctly through
// the full gateway pipeline: request → webhook call → response extraction → ctx set.
//
// Required config files (in examples/test/conf/EdgionPlugins/WebhookKeyGet/):
// - 01_LinkSys_edgion-default_webhook-resolver.yaml    # LinkSys Webhook resource
// - 02_LinkSys_edgion-default_webhook-body-resolver.yaml
// - 03_EdgionPlugins_default_webhook-ctx-set.yaml      # CtxSet plugin with webhook key_get
// - HTTPRoute_default_webhook-keyget-test.yaml          # Route binding

mod webhook_key_get;

pub use webhook_key_get::WebhookKeyGetTestSuite;
