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
    for (prefix, path, openai_path, anthropic_path) in [
        ("", "", "/", "/v1/messages"),
        ("/", "///chat", "/chat", "/v1/messages"),
        ("/api///", "//chat", "/api/chat", "/api/v1/messages"),
        ("api", "chat", "api/chat", "api/v1/messages"),
        ("/café/", "/π", "/café/π", "/café/v1/messages"),
    ] {
        for (provider, native, expected_path, preserves_query) in [
            (AiProvider::OpenAI, false, openai_path, true),
            (AiProvider::OpenAI, true, openai_path, true),
            (AiProvider::OpenAICompatible, false, openai_path, true),
            (AiProvider::OpenAICompatible, true, openai_path, true),
            (AiProvider::Anthropic, false, anthropic_path, false),
            (AiProvider::Anthropic, true, anthropic_path, true),
        ] {
            for query in [None, Some(""), Some("a=hello%20world&b=x%26y")] {
                let mut expected = expected_path.to_owned();
                if preserves_query {
                    if let Some(query) = query {
                        expected.push('?');
                        expected.push_str(query);
                    }
                }
                assert_eq!(provider_uri(provider, prefix, path, query, native), expected);
            }
        }
    }
}

#[test]
fn provider_uri_reserves_the_query_before_building_the_path() {
    let path = "v1/chat/completions";
    let query = "x".repeat(4096);
    let (previous, previous_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        let mut uri = format!("/{path}");
        uri.push('?');
        uri.push_str(&query);
        uri
    });
    let (actual, actual_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        provider_uri(AiProvider::OpenAICompatible, "", path, Some(&query), false)
    });
    assert_eq!(actual, previous);
    assert_eq!(actual_stats.calls, 1);
    assert_eq!(actual_stats.requested_bytes, actual.len());
    assert!(previous_stats.calls > actual_stats.calls);
    assert!(previous_stats.requested_bytes > actual_stats.requested_bytes);
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
            .expect_set_request_header_value()
            .withf(|name, value| name == "accept-encoding" && value == "identity" && !value.is_sensitive())
            .times(1)
            .in_sequence(&mut encoding_order)
            .returning(|_, _| Ok(()));
        session
            .expect_set_request_header_value()
            .withf(|name, value| {
                !value.is_sensitive()
                    && match name {
                        "anthropic-version" => value == ANTHROPIC_VERSION,
                        "content-type" => value == "application/json",
                        _ => false,
                    }
            })
            .times(if provider == AiProvider::Anthropic { 2 } else { 0 })
            .returning(|_, _| Ok(()));
        session
            .expect_set_request_header()
            .withf(|name, _| matches!(name, "authorization" | "x-api-key"))
            .times(1)
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
        Arc::make_mut(&mut ctx.egc).remote_ip_header = http::HeaderName::from_static(name);
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

#[tokio::test]
async fn native_version_projection_preserves_winning_deferred_value_without_mutation() {
    use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    for raw in [b"2023-06-01".as_slice(), b"future", b"\xff", b""] {
        let mut session = native_session().await;
        for _ in 0..2 {
            session
                .req_header_mut()
                .append_header("anthropic-version", ANTHROPIC_VERSION)
                .unwrap();
        }
        let mut ctx = EdgionHttpContext::new();
        assert!(!validate_native_headers(&PingoraSessionAdapter::new(&mut session, &mut ctx)).unwrap());
        let mut unrelated = http::HeaderValue::from_static("fixture-private");
        unrelated.set_sensitive(true);
        let expired = Instant::now() - Duration::from_secs(1);
        ctx.upsert_deferred_credential_mutation(
            http::HeaderName::from_static("x-unrelated"),
            Arc::new(unrelated),
            Some(expired),
        );
        ctx.upsert_deferred_credential_mutation(
            http::HeaderName::from_static("anthropic-version"),
            Arc::new(http::HeaderValue::from_static("previous")),
            None,
        );
        let mut winning = http::HeaderValue::from_bytes(raw).unwrap();
        winning.set_sensitive(true);
        ctx.upsert_deferred_credential_mutation(
            http::HeaderName::from_bytes(b"AnThRoPiC-Version").unwrap(),
            Arc::new(winning),
            None,
        );
        let before = session.req_header().headers.clone();
        for _ in 0..2 {
            assert_eq!(
                validate_native_headers(&PingoraSessionAdapter::new(&mut session, &mut ctx)).unwrap(),
                raw == ANTHROPIC_VERSION.as_bytes()
            );
            assert_eq!(session.req_header().headers, before);
        }
        assert!(!ctx.deferred_credentials_all_unexpired(Instant::now()));
        let mut attempt = session.req_header().clone();
        ctx.apply_deferred_credentials(&mut attempt).unwrap();
        assert_eq!(attempt.headers.get_all("anthropic-version").iter().count(), 1);
        assert_eq!(attempt.headers["anthropic-version"].as_bytes(), raw);
        assert!(attempt.headers["anthropic-version"].is_sensitive());
        assert_eq!(attempt.headers["x-unrelated"], "fixture-private");
        assert!(attempt.headers["x-unrelated"].is_sensitive());
    }
}

#[tokio::test]
async fn native_live_version_cardinality_and_raw_bytes_preserve_observation_policy() {
    use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    use std::sync::Arc;

    let cases: &[(&[&[u8]], bool)] = &[
        (&[], true),
        (&[b"2023-06-01"], true),
        (&[b"future"], false),
        (&[b""], false),
        (&[b"\xff"], false),
        (&[b"2023-06-01", b"2023-06-01"], false),
        (&[b"2023-06-01", b"future"], false),
        (&[b"future", b"2023-06-01"], false),
        (&[b"2023-06-01", b"2023-06-01", b"2023-06-01"], false),
    ];
    for unrelated_deferred in [false, true] {
        for &(raw_values, supported) in cases {
            let mut session = native_session().await;
            for raw in raw_values {
                let mut value = http::HeaderValue::from_bytes(raw).unwrap();
                value.set_sensitive(true);
                session
                    .req_header_mut()
                    .append_header("anthropic-version", value)
                    .unwrap();
            }
            let mut ctx = EdgionHttpContext::new();
            if unrelated_deferred {
                ctx.upsert_deferred_credential_mutation(
                    http::HeaderName::from_static("x-unrelated"),
                    Arc::new(http::HeaderValue::from_static("fixture")),
                    None,
                );
            }
            let before = session.req_header().headers.clone();
            assert_eq!(
                validate_native_headers(&PingoraSessionAdapter::new(&mut session, &mut ctx)).unwrap(),
                supported
            );
            assert_eq!(session.req_header().headers, before);
            assert!(session
                .req_header()
                .headers
                .get_all("anthropic-version")
                .iter()
                .all(|v| v.is_sensitive()));
        }
    }
}
