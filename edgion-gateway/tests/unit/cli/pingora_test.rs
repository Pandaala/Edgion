use std::future::pending;

use edgion_resources::resources::gateway::{GatewaySpec, Listener};
use edgion_resources::resources::proxy_protocol::{
    CanonicalProxyProtocolTrustPolicy, ProxyProtocolPolicySourceKind, ResolvedInboundProxyProtocol,
};

use super::*;

#[derive(Debug)]
struct ExternalFast;

#[async_trait]
impl ShutdownSignalWatch for ExternalFast {
    async fn recv(&self) -> ShutdownSignal {
        ShutdownSignal::FastShutdown
    }
}

#[derive(Debug)]
struct ExternalPending;

#[async_trait]
impl ShutdownSignalWatch for ExternalPending {
    async fn recv(&self) -> ShutdownSignal {
        pending().await
    }
}

fn gateway(state: ResolvedInboundProxyProtocol) -> edgion_resources::Gateway {
    let mut gateway = edgion_resources::Gateway::new(
        "shutdown",
        GatewaySpec {
            current_status: None,
            gateway_class_name: "edgion".to_string(),
            listeners: Some(vec![Listener {
                name: "tcp".to_string(),
                hostname: None,
                port: 19090,
                protocol: "TCP".to_string(),
                tls: None,
                allowed_routes: None,
            }]),
            addresses: None,
            tls: None,
            allowed_listeners: None,
            infrastructure: None,
            default_scope: None,
            resolved_inbound_proxy_protocol: state,
            resolved_attachment_proof: None,
        },
    );
    gateway.metadata.namespace = Some("default".to_string());
    gateway
}

#[tokio::test]
async fn external_shutdown_semantics_are_preserved() {
    let watcher = PolicyShutdownSignalWatch::new(ExternalFast, Arc::new(ProxyProtocolPolicyRuntime::new()));
    assert!(matches!(watcher.recv().await, ShutdownSignal::FastShutdown));
}

#[tokio::test]
async fn internal_policy_fatal_requests_fast_shutdown() {
    let runtime = Arc::new(ProxyProtocolPolicyRuntime::new());
    let initial = gateway(ResolvedInboundProxyProtocol::Disabled);
    runtime.observe_snapshot([&initial]);
    runtime.preflight_and_freeze([&initial]).unwrap();

    let changed = gateway(ResolvedInboundProxyProtocol::Ready {
        listener_names: vec!["tcp".to_string()],
        policy: CanonicalProxyProtocolTrustPolicy::Required {},
        source: ProxyProtocolPolicySourceKind::Inline,
    });
    runtime.observe_snapshot([&changed]);

    let watcher = PolicyShutdownSignalWatch::new(ExternalPending, runtime);
    assert!(matches!(watcher.recv().await, ShutdownSignal::FastShutdown));
}
