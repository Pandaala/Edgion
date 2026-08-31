//! Prepared Webhook client owned by the ordered LinkSys store transition.
//!
//! Webhook manager publication is deliberately not implemented through the
//! generic `LinkSysLifecycle` trait. A Webhook candidate must be fully built
//! before publication, then committed through the manager's sequence-checked
//! mutation guard before the runtime bookkeeping map is updated. Keeping this
//! type outside `LinkSysLifecycle` makes accidental use of the generic spawned
//! init/shutdown helpers a compile-time error.

use crate::link_sys::providers::webhook::manager::PreparedWebhookEntry;

pub struct WebhookLinkClient {
    prepared: PreparedWebhookEntry,
}

impl WebhookLinkClient {
    pub(crate) fn from_prepared(prepared: PreparedWebhookEntry) -> Self {
        Self { prepared }
    }

    pub(crate) fn prepared_entry(&self) -> PreparedWebhookEntry {
        self.prepared.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link_sys::providers::webhook::manager::WebhookManager;
    use edgion_resources::resources::link_sys::webhook::{
        ExternalHttpConnection, WebhookServiceConfig, WebhookServiceRef, WebhookTarget,
    };
    use std::sync::Arc;

    fn config() -> WebhookServiceConfig {
        WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        name: Some("example-svc".into()),
                        namespace: Some("default".into()),
                        port: Some(80),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn prepared_entry_keeps_registration_identity() {
        let manager = WebhookManager::new_for_test();
        let mutation = manager.begin_mutation(1).await.expect("first sequence is accepted");
        let prepared = mutation
            .prepare("ns/wh", config(), None)
            .expect("valid webhook prepares");
        let client = WebhookLinkClient::from_prepared(prepared);

        let first = client.prepared_entry();
        let second = client.prepared_entry();
        assert!(Arc::ptr_eq(first.registration(), second.registration()));
        assert!(Arc::ptr_eq(first.client(), second.client()));
    }
}
