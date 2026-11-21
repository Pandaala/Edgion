use super::*;

#[tokio::test]
async fn relaxed_client_without_blocked_cidrs_preserves_ipv6_scope() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::dsl::runtime::tests",
        "relaxed_client_without_blocked_cidrs_preserves_ipv6_scope",
        "EDGION_TEST_DSL_SCOPED_RESOLVER",
        "empty-cidrs",
        || async {
            assert!(!edgion_common::egress::has_blocked_cidrs());
            let resolver = dsl_http_client_config(false, true).dns_resolver.unwrap();
            // The OS resolves this numeric scoped name without DNS traffic or a live peer.
            let sockets = resolver.resolve("fe80::1%1".parse().unwrap()).await.unwrap();
            let expected = "[fe80::1%1]:0".parse::<std::net::SocketAddr>().unwrap();
            assert_eq!(sockets.collect::<Vec<_>>(), [expected]);
        },
    )
    .await;
}

#[tokio::test]
async fn relaxed_client_filters_resolved_destinations() {
    let replayable = dsl_http_client_config(false, true);
    let resolver = replayable.dns_resolver.expect("DSL resolver");
    // Numeric names exercise the configured resolver without relying on external DNS.
    let denial = resolver
        .resolve("224.0.0.1".parse().unwrap())
        .await
        .err()
        .expect("blocked answer");
    assert!(denial
        .to_string()
        .contains("egress policy blocked all resolved addresses"));
    let allowed = resolver.resolve("100.64.0.1".parse().unwrap()).await.unwrap();
    assert_eq!(
        allowed.collect::<Vec<_>>(),
        ["100.64.0.1:0".parse::<std::net::SocketAddr>().unwrap()]
    );
    let opaque = dsl_http_client_config(false, false);
    assert!(opaque.dns_resolver.is_some());
    assert!(matches!(opaque.redirect, OutboundRedirect::Deny));
}

#[tokio::test]
async fn relaxed_client_rejects_non_dialable_redirect_before_connecting() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "http://240.0.0.1/"))
        .expect(1)
        .mount(&server)
        .await;
    let client = build_outbound_client(dsl_http_client_config(false, true)).unwrap();
    let error = client
        .get(server.uri())
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap_err();
    assert!(error.is_redirect(), "expected a redirect policy rejection: {error}");
}
