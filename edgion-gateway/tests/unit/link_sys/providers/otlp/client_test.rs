use super::*;
fn client(key: &str, resource: &LinkSys, resolver: ResolverSpec) -> anyhow::Result<Arc<OtlpLinkClient>> {
    let candidate = OtlpCandidate::new(Arc::new(resource.clone()))?;
    OtlpLinkClient::from_candidate(key, &candidate, resolver)
}

fn resource(name: &str, endpoint: &str) -> LinkSys {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "LinkSys",
        "metadata": {"namespace": "test", "name": name},
        "spec": {"type": "otlp", "config": {"endpoint": endpoint}}
    }))
    .expect("OTLP fixture")
}

#[test]
fn invalid_keys_never_build_a_generation() {
    let resource = resource("otlp", "http://example.test:4317");
    for key in ["", "raw", "/name", "namespace/", "namespace/a/b", "Bad/name"] {
        assert!(client(key, &resource, ResolverSpec::System,).is_err());
    }
}

#[tokio::test]
async fn lazy_pipeline_has_single_live_coverage_and_shutdown_clears_it() {
    let resource = resource("otlp", "http://example.test:4317");
    let client = client("test/otlp", &resource, ResolverSpec::System).unwrap();
    assert_eq!(
        client.connection_coverage(),
        ConnectionCoverage { configured: 1, live: 0 }
    );
    client.publish_lazy_test_pipeline().unwrap();
    assert_eq!(
        client.connection_coverage(),
        ConnectionCoverage { configured: 1, live: 1 }
    );
    client.shutdown().await.unwrap();
    assert_eq!(client.connection_coverage().live, 0);
    assert!(client.pipeline.load().is_none());
}

#[tokio::test]
async fn retired_generation_stops_serving_and_health_writes() {
    let resource = resource("retired", "http://example.test:4317");
    let client = client("test/retired", &resource, ResolverSpec::System).unwrap();
    client.publish_lazy_test_pipeline().unwrap();
    client.mark_retired();
    assert!(!client.healthy());
    assert!(client
        .send_with_meta("record".to_string(), &LogMeta::default())
        .is_err());
    client.shutdown().await.unwrap();
}

impl OtlpLinkClient {
    pub(in crate::link_sys::providers::otlp) fn assert_retained_transport_input(
        &self,
        input: &Arc<edgion_common::link_sys_core::outbound_tls::PreparedOtlpTls>,
        actual: &LinkSys,
    ) {
        self.transport.assert_retained_input(input, actual);
        let transport = self.transport.clone();
        assert!(Arc::ptr_eq(&transport, &self.transport));
    }
}
