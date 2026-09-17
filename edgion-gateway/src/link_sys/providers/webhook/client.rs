//! Prepared Webhook client owned by the ordered LinkSys store transition.
//!
//! Webhook manager publication is deliberately not implemented through the
//! generic `LinkSysLifecycle` trait. A Webhook candidate must be fully built
//! before publication, then committed through the manager's sequence-checked
//! mutation guard before the runtime bookkeeping map is updated. Keeping this
//! type outside `LinkSysLifecycle` keeps Webhook off the persistent-runner
//! `ApplyOwner` path used by Redis, Etcd, Elasticsearch, Kafka, and OTLP.

use crate::link_sys::providers::webhook::manager::PreparedWebhookEntry;
use crate::link_sys::runtime::apply::RuntimeFingerprint;

pub struct WebhookLinkClient {
    prepared: PreparedWebhookEntry,
    fingerprint: RuntimeFingerprint,
}

impl WebhookLinkClient {
    pub(crate) fn from_prepared(prepared: PreparedWebhookEntry, fingerprint: RuntimeFingerprint) -> Self {
        Self { prepared, fingerprint }
    }

    pub(crate) fn prepared_entry(&self) -> PreparedWebhookEntry {
        self.prepared.clone()
    }

    pub(crate) fn runtime_fingerprint(&self) -> &RuntimeFingerprint {
        &self.fingerprint
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link_sys::providers::webhook::manager::WebhookManager;
    use crate::link_sys::runtime::apply::RuntimeFingerprint;
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
        let client = WebhookLinkClient::from_prepared(prepared, RuntimeFingerprint::unspecified());

        let first = client.prepared_entry();
        let second = client.prepared_entry();
        assert!(Arc::ptr_eq(first.registration(), second.registration()));
        assert!(Arc::ptr_eq(first.client(), second.client()));
    }
}
