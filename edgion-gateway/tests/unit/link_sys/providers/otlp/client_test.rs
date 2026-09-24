use super::*;

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
        assert!(OtlpLinkClient::from_linksys_inner(key, &resource, ResolverSpec::System,).is_err());
    }
}

#[tokio::test]
async fn lazy_pipeline_has_single_live_coverage_and_shutdown_clears_it() {
    let resource = resource("otlp", "http://example.test:4317");
    let client = OtlpLinkClient::from_linksys_inner("test/otlp", &resource, ResolverSpec::System).unwrap();
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
    let client = OtlpLinkClient::from_linksys_inner("test/retired", &resource, ResolverSpec::System).unwrap();
    client.publish_lazy_test_pipeline().unwrap();
    client.mark_retired();
    assert!(!client.healthy());
    assert!(client
        .send_with_meta("record".to_string(), &LogMeta::default())
        .is_err());
    client.shutdown().await.unwrap();
}
