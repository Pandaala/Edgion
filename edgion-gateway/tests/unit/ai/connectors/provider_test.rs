use super::*;

#[test]
fn provider_uri_uses_canonical_path_and_query() {
    assert_eq!(
        provider_uri(
            AiProvider::OpenAICompatible,
            "/openai/v1",
            "/chat/completions",
            Some("trace=1"),
            false
        ),
        "/openai/v1/chat/completions?trace=1"
    );
    assert_eq!(
        provider_uri(
            AiProvider::Anthropic,
            "/native",
            "/chat/completions",
            Some("ignored=1"),
            false
        ),
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
            .times(7)
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
            AiApi::OpenaiChat,
            &endpoint,
            "fixture-test-credential",
            "/v1/chat/completions",
            None,
            true,
        )
        .unwrap();
    }
}

async fn native_session() -> pingora_proxy::Session {
    use tokio::io::AsyncWriteExt;
    let (mut client, server) = tokio::io::duplex(4096);
    let mut session = pingora_proxy::Session::new_h1(Box::new(server));
    client
        .write_all(b"POST /v1/messages?q=native HTTP/1.1\r\nHost: test\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    drop(client);
    assert!(session.read_request().await.unwrap());
    session
}
fn native_endpoint() -> RuntimeAiEndpoint {
    RuntimeAiEndpoint {
        host: "fixture.example.com".into(),
        path_prefix: "/native".into(),
        domain: "fixture.example.com".into(),
        port: 443,
        use_tls: true,
        sni: "fixture.example.com".into(),
    }
}
#[tokio::test]
async fn native_headers_preserve_all_beta_and_effective_deferred_version_but_isolate_workspace() {
    use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    use std::sync::Arc;
    for (live, deferred, known) in [
        (None, None, true),
        (Some("2023-06-01"), None, true),
        (Some("future"), None, false),
        (Some("future"), Some("2023-06-01"), true),
        (None, Some("future"), false),
    ] {
        let mut session = native_session().await;
        session
            .req_header_mut()
            .append_header("anthropic-beta", "first-beta")
            .unwrap();
        session
            .req_header_mut()
            .append_header("anthropic-beta", "second-beta")
            .unwrap();
        if let Some(version) = live {
            session
                .req_header_mut()
                .insert_header("anthropic-version", version)
                .unwrap();
        }
        session
            .req_header_mut()
            .insert_header("anthropic-workspace-id", "client-workspace")
            .unwrap();
        session
            .req_header_mut()
            .insert_header("authorization", "Bearer client-key")
            .unwrap();
        let mut ctx = EdgionHttpContext::new();
        for (name, value) in [
            ("anthropic-workspace-id", "deferred-workspace"),
            ("x-api-key", "deferred-key"),
        ] {
            ctx.upsert_deferred_credential_mutation(
                http::HeaderName::from_static(name),
                Arc::new(http::HeaderValue::from_static(value)),
                None,
            );
        }
        if let Some(version) = deferred {
            ctx.upsert_deferred_credential_mutation(
                http::HeaderName::from_static("anthropic-version"),
                Arc::new(http::HeaderValue::from_static(version)),
                None,
            );
        }
        let known_actual = {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            prepare_provider_request(
                &mut adapter,
                AiProvider::Anthropic,
                AiApi::AnthropicMessages,
                &native_endpoint(),
                "fixed-key",
                "/v1/messages",
                Some("q=native"),
                true,
            )
            .unwrap()
        };
        assert_eq!(known_actual, known);
        let mut upstream = session.req_header().clone();
        ctx.apply_deferred_credentials(&mut upstream).unwrap();
        assert_eq!(upstream.uri.to_string(), "/native/v1/messages?q=native");
        assert_eq!(
            upstream
                .headers
                .get_all("anthropic-beta")
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["first-beta", "second-beta"]
        );
        assert_eq!(
            upstream.headers["anthropic-version"],
            deferred.or(live).unwrap_or("2023-06-01")
        );
        assert_eq!(upstream.headers["x-api-key"], "fixed-key");
        assert!(!upstream.headers.contains_key("authorization"));
        assert!(!upstream.headers.contains_key("anthropic-workspace-id"));
        assert_eq!(upstream.headers["accept-encoding"], "identity");
    }
}
#[tokio::test]
async fn native_profile_and_final_remote_ip_collisions_fail_before_materialization() {
    use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    use std::sync::Arc;
    for name in [
        "authorization",
        "api-key",
        "x-api-key",
        "anthropic-version",
        "anthropic-beta",
        "anthropic-workspace-id",
        "anthropic-user-profile-id",
    ] {
        let mut session = native_session().await;
        let mut ctx = EdgionHttpContext::new();
        ctx.remote_ip_header = http::HeaderName::from_static(name);
        let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            validate_native_headers(&adapter),
            Err(AiDispatchError::Client(AiProxyError::UnsupportedParameter))
        );
    }
    for deferred in [false, true] {
        let mut session = native_session().await;
        let mut ctx = EdgionHttpContext::new();
        if deferred {
            ctx.upsert_deferred_credential_mutation(
                http::HeaderName::from_static("anthropic-user-profile-id"),
                Arc::new(http::HeaderValue::from_static("profile")),
                None,
            );
        } else {
            session
                .req_header_mut()
                .insert_header("anthropic-user-profile-id", "profile")
                .unwrap();
        }
        let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            validate_native_headers(&adapter),
            Err(AiDispatchError::Client(AiProxyError::UnsupportedParameter))
        );
    }
}
