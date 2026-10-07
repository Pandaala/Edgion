//! Actual EGC fixtures for unit tests; never available in production builds.
use super::GlobalOutboundTlsSnapshot;
use edgion_resources::resources::common::OutboundTlsConfig;
use edgion_resources::EdgionGatewayConfig;
use std::sync::Arc;

pub fn snapshot(policy: OutboundTlsConfig) -> Arc<GlobalOutboundTlsSnapshot> {
    let mut owner: EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"global-test-owner"}, "spec":{}
    }))
    .unwrap();
    owner.spec.outbound_tls = Some(policy);
    Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(owner)]))
}
