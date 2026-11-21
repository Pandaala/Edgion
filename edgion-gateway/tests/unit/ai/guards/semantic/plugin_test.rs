use super::*;

fn config() -> GuardrailConfig {
    yaml_serde::from_str(
        r#"
target:
  url: https://guard.example
timeoutMs: 1000
maxResponseBytes: 4096
request:
  path:
    template: /v1/decision
windowBytes: 16
maxConcurrent: 2
"#,
    )
    .unwrap()
}

#[test]
fn one_snapshot_clone_reuses_client_pool_and_semaphore() {
    let snapshot = build_owned_guardrail();
    let companion = snapshot.clone();
    assert!(Arc::ptr_eq(&snapshot.client, &companion.client));
    assert!(Arc::ptr_eq(&snapshot.permits, &companion.permits));

    let reloaded = build_owned_guardrail();
    assert!(!Arc::ptr_eq(&snapshot.client, &reloaded.client));
    assert!(!Arc::ptr_eq(&snapshot.permits, &reloaded.permits));
}

fn build_owned_guardrail() -> Guardrail {
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"guard","namespace":"tenant"},"spec":{"requestPlugins":[{"type":"Guardrail","config":config()}]}
    })).unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let field = entry.managed_http_config().unwrap().unwrap();
    let defaults = edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot::from_configs([]);
    let input = PreparedManagedHttp::new(&field, &defaults).unwrap();
    Guardrail::build(&input, "test/requestPlugins/#0".to_string()).unwrap()
}

use edgion_resources::resources::edgion_plugins::GuardrailConfig;

#[tokio::test]
async fn only_provider_errors_skip_guardrail_and_release_response_head() {
    use crate::plugins::runtime::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    use pingora_http::ResponseHeader;
    use pingora_proxy::Session;
    use tokio::io::AsyncWriteExt;

    for (fail_open, hold) in [(true, false), (false, false), (false, true)] {
        let mut guard = build_owned_guardrail();
        guard.fail_open = fail_open;
        guard.head_limits = hold.then(|| response_head_hold_limits(std::time::Duration::from_secs(1)));
        for (upstream_status, status, skip) in [
            (Some(400), 400, true),
            (Some(429), 429, true),
            (Some(500), 500, true),
            (Some(200), 429, false),
            (None, 429, false),
            (Some(429), 200, false),
        ] {
            let (mut client, server) = tokio::io::duplex(1024);
            client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
            let mut session = Session::new_h1(Box::new(server));
            session.read_request().await.unwrap();
            let mut ctx = EdgionHttpContext::default();
            ctx.init_backend_context(
                crate::ctx::Rsc::new("Service", "test", "provider"),
                "test/provider".into(),
                crate::backends::ServiceBackendKind::Service,
                None,
            );
            ctx.push_upstream(None, None);
            ctx.get_current_upstream_mut().unwrap().status = upstream_status;
            let mut header = ResponseHeader::build(status, None).unwrap();
            header.insert_header("content-type", "application/json").unwrap();
            if status == 429 {
                header.insert_header("retry-after", "2").unwrap();
            }
            let original = header.headers.clone();
            let mut log = PluginLog::new(PLUGIN_NAME);
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut header);
            assert!(matches!(
                guard.run_request(&mut adapter, &mut log).await,
                PluginRunningResult::GoodNext
            ));
            assert_eq!(
                adapter.ctx().ai_state().unwrap().response_head_hold_claim.is_some(),
                hold
            );
            let result = guard.run_upstream_response(&mut adapter, &mut log).await;
            assert_eq!(header.status.as_u16(), status);
            assert_eq!(header.headers, original);
            let state = ctx.ai_state().unwrap();
            assert!(state.semantic_processor.is_none());
            let owner = &state.attached_guardrail.as_ref().unwrap().1;
            let summary = owner.take_summary_line().unwrap();
            if skip || fail_open {
                assert!(matches!(result, PluginRunningResult::GoodNext));
                assert!(!state.semantic_guard_claimed);
                assert!(state.response_head_hold_claim.is_none());
            } else {
                assert!(matches!(result, PluginRunningResult::ErrResponse { status: 503, .. }));
            }
            let expected = match (skip, fail_open) {
                (true, _) => "outcome=skipped reason=upstream-error",
                (false, true) => "outcome=fail-open reason=response-not-applicable",
                (false, false) => "outcome=fail-close reason=response-not-applicable",
            };
            assert!(summary.contains(expected), "{summary}");
        }
    }
}

#[tokio::test]
async fn response_admission_preserves_first_content_type_text() {
    use crate::ai::{AiUsageShared, UsageExtractionProcessor};
    use crate::plugins::runtime::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    use http::HeaderValue;
    use pingora_http::ResponseHeader;
    use pingora_proxy::Session;
    use tokio::io::AsyncWriteExt;

    let mut guard = build_owned_guardrail();
    guard.fail_open = false;
    let cases: &[(&[&[u8]], bool)] = &[
        (&[], false),
        (&[b""], false),
        (&[b"text/event-stream"], true),
        (&[b"TEXT/EVENT-STREAM; charset=utf-8"], true),
        (&[b"text/event-streaming"], false),
        (&[b"application/json"], false),
        (&[b"text/event-stream", b"application/json"], true),
        (&[b"application/json", b"text/event-stream"], false),
        (&[b"text/event-stream;\xff"], false),
        (&["text/event-stream; note=é".as_bytes()], false),
    ];
    for &(values, accepted) in cases {
        let (mut client, server) = tokio::io::duplex(1024);
        client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
        let mut session = Session::new_h1(Box::new(server));
        session.read_request().await.unwrap();
        let mut ctx = EdgionHttpContext::default();
        ctx.ensure_ai_state().stream_processor = Some(Box::new(UsageExtractionProcessor::new(Arc::new(
            AiUsageShared::default(),
        ))));
        let mut header = ResponseHeader::build(200, None).unwrap();
        for value in values {
            header
                .append_header("Content-Type", HeaderValue::from_bytes(value).unwrap())
                .unwrap();
        }
        let original_headers = header.headers.clone();
        let mut log = PluginLog::new(PLUGIN_NAME);
        let result = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut header);
            assert!(matches!(
                guard.run_request(&mut adapter, &mut log).await,
                PluginRunningResult::GoodNext
            ));
            guard.run_upstream_response(&mut adapter, &mut log).await
        };
        if accepted {
            assert!(matches!(result, PluginRunningResult::GoodNext), "{values:?}");
        } else {
            assert!(
                matches!(
                    result,
                    PluginRunningResult::ErrResponse {
                        status: 503,
                        body: None
                    }
                ),
                "{values:?}"
            );
        }
        assert_eq!(
            ctx.ai_state().unwrap().semantic_processor.is_some(),
            accepted,
            "{values:?}"
        );
        assert_eq!(header.headers, original_headers);
    }
}
