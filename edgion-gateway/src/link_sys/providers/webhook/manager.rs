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
#[path = "../../../../tests/unit/link_sys/providers/webhook/manager_test.rs"]
mod tests;
