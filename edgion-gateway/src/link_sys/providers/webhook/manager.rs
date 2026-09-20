//! Stateless Webhook lookup and invocation through the unified LinkSys runtime table.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::link_sys::runtime::store::get_webhook_client;
use crate::plugins::http::common::outbound::call::{outbound_call, OutboundCallError, OutboundResponse};
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::http::common::outbound::request::OutboundRequestDiagnostics;

/// Global cap for webhook response body size. Initialized to 16KB.
static WEBHOOK_GLOBAL_MAX: AtomicUsize = AtomicUsize::new(16384);

/// Read the current global max response bytes for webhook calls.
pub fn get_webhook_global_max() -> usize {
    WEBHOOK_GLOBAL_MAX.load(Ordering::Relaxed)
}

/// Set the global max response bytes when EdgionGatewayConfig changes.
pub fn configure_webhook_global_max(bytes: usize) {
    WEBHOOK_GLOBAL_MAX.store(bytes, Ordering::Relaxed);
    tracing::info!(bytes, "Webhook global max response bytes updated");
}

/// Handle for calls already admitted through the current Webhook runtime.
/// Removing a LinkSys does not forcibly revoke handles held by existing callers.
pub struct WebhookCallHandle(pub(crate) Arc<ManagedOutboundClient>);

impl WebhookCallHandle {
    /// Execute the resource-defined request with an optional permitted path override.
    /// The caller consumes diagnostics into its canonical log, including on failure.
    pub async fn call(
        &self,
        session: &dyn crate::plugins::runtime::PluginSession,
        path_override: Option<String>,
        diagnostics: &mut OutboundRequestDiagnostics,
    ) -> Result<OutboundResponse, OutboundCallError> {
        outbound_call(
            &self.0,
            session,
            path_override.as_deref(),
            get_webhook_global_max(),
            &[],
            diagnostics,
        )
        .await
    }
}

/// Return a handle only when the current provider is a prepared Webhook.
pub fn get_webhook_call_handle(webhook_ref: &str) -> Option<WebhookCallHandle> {
    get_webhook_client(webhook_ref).map(|client| WebhookCallHandle(client.managed_client()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link_sys::providers::webhook::WebhookLinkClient;
    use crate::link_sys::runtime::store::install_test_webhook;

    #[test]
    fn configure_global_max_changes_value() {
        const CHILD_ENV: &str = "EDGION_WEBHOOK_GLOBAL_MAX_TEST_CHILD";
        const TEST_MODULE: &str = "link_sys::providers::webhook::manager::tests";
        const TEST_NAME: &str = "configure_global_max_changes_value";

        crate::test_subprocess::assert_in_subprocess(TEST_MODULE, TEST_NAME, CHILD_ENV, TEST_NAME, || {
            configure_webhook_global_max(65536);
            assert_eq!(get_webhook_global_max(), 65536);
            configure_webhook_global_max(16384);
        });
    }

    #[test]
    fn webhook_presence_diagnostics_follow_each_call_and_current_generation() {
        use crate::plugins::http::common::outbound::request::test_presence_failure_request;
        use crate::plugins::http::webhook_key_get::WebhookKeyGet;
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
        use crate::plugins::runtime::PluginLog;
        use edgion_resources::common::WebhookExtract;
        use edgion_resources::filters::PluginRunningResult;
        use edgion_resources::resources::common::outbound_request::PresencePolicy;
        use edgion_resources::resources::edgion_plugins::{WebhookKeyGetConfig, WebhookVar};

        // Acquire outside the runtime: no synchronous mutex is held across an
        // await inside an async task. Full-set dispatch tests share this lock.
        let _guard = crate::link_sys::runtime::store::I5_DISPATCH_LOCK.blocking_lock();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let key = "presence-diagnostics/webhook";
                let plugin = WebhookKeyGet::create(&WebhookKeyGetConfig {
                    vars: [Some("fallback"), None]
                        .into_iter()
                        .enumerate()
                        .map(|(index, default)| WebhookVar {
                            name: format!("result{index}"),
                            webhook_ref: key.into(),
                            path_override: None,
                            extract: WebhookExtract::BodyText,
                            default: default.map(str::to_owned),
                        })
                        .collect(),
                });
                for policy in [
                    PresencePolicy::WarnIfMissing,
                    PresencePolicy::SkipIfMissing,
                    PresencePolicy::Required,
                ] {
                    let mut client = ManagedOutboundClient::test_with_static_target("http://example.test");
                    client.compiled_request = test_presence_failure_request(policy);
                    let client = Arc::new(client);
                    // Use one isolated registry key; replacement exercises the resolver's
                    // current-generation lookup without network or background health work.
                    install_test_webhook(key, Some(Arc::new(WebhookLinkClient::from_managed(client.clone()))));
                    let mut session = MockPluginSession::new();
                    setup_header_mocks(&mut session, &[]);
                    session.expect_get_cookie().return_const(None::<String>);
                    session
                        .expect_set_ctx_var()
                        .withf(|name, value| name == "result0" && value == "fallback")
                        .times(1)
                        .returning(|_, _| Ok(()));
                    let mut log = PluginLog::new("WebhookKeyGet");
                    let result = plugin.run_request(&mut session, &mut log).await;
                    assert!(matches!(result, PluginRunningResult::GoodNext));
                    let rendered = log.render_for_test();
                    assert_eq!(
                        rendered.matches("warn outbound field missing").count(),
                        if matches!(policy, PresencePolicy::WarnIfMissing) {
                            2
                        } else {
                            0
                        }
                    );
                    assert!(rendered.contains("default"));
                    assert!(rendered.contains("skip"));
                    assert!(!rendered.contains("private"));

                    // Generic callers receive the same evidence even when the call fails.
                    let mut diagnostics = OutboundRequestDiagnostics::default();
                    assert!(WebhookCallHandle(client)
                        .call(&session, None, &mut diagnostics)
                        .await
                        .is_err());
                    let mut log = PluginLog::new("test");
                    diagnostics.emit(&mut log);
                    assert_eq!(
                        log.render_for_test(),
                        if matches!(policy, PresencePolicy::WarnIfMissing) {
                            "warn outbound field missing"
                        } else {
                            ""
                        }
                    );
                }
                install_test_webhook(key, None);
            });
    }
}
