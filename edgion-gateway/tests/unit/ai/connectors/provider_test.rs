use super::*;

#[test]
fn provider_uri_uses_canonical_path_and_query() {
    assert_eq!(
        provider_uri(
            AiProvider::OpenAICompatible,
            "/openai/v1",
            "/chat/completions",
            Some("trace=1")
        ),
        "/openai/v1/chat/completions?trace=1"
    );
    assert_eq!(
        provider_uri(AiProvider::Anthropic, "/native", "/chat/completions", Some("ignored=1")),
        "/native/v1/messages"
    );
}

#[test]
fn every_provider_requests_identity_encoding_after_removing_client_values() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use mockall::Sequence;
    for provider in [AiProvider::OpenAI, AiProvider::OpenAICompatible, AiProvider::Anthropic] {
        let mut session = MockPluginSession::new();
        let mut encoding_order = Sequence::new();
        session
            .expect_remove_request_header()
            .withf(|name| name == "accept-encoding")
            .times(1)
            .in_sequence(&mut encoding_order)
            .returning(|_| Ok(()));
        session
            .expect_remove_request_header()
            .withf(|name| name != "accept-encoding")
            .times(5)
            .returning(|_| Ok(()));
        session
            .expect_set_request_header()
            .withf(|name, value| name == "accept-encoding" && value == "identity")
            .times(1)
            .in_sequence(&mut encoding_order)
            .returning(|_, _| Ok(()));
        session
            .expect_set_request_header()
            .withf(|name, _| name != "accept-encoding")
            .returning(|_, _| Ok(()));
        session.expect_set_upstream_uri().returning(|_| Ok(()));
        session.expect_set_upstream_host().returning(|_| Ok(()));
        let endpoint = RuntimeAiEndpoint {
            host: "fixture.example.com".to_string(),
            path_prefix: String::new(),
            domain: "fixture.example.com".to_string(),
            port: 443,
            use_tls: true,
            sni: "fixture.example.com".to_string(),
        };
        prepare_provider_request(
            &mut session,
            provider,
            &endpoint,
            "fixture-test-credential",
            "/v1/chat/completions",
            None,
            true,
        )
        .unwrap();
    }
}
