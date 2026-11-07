use super::*;

#[test]
fn direct_span_encoding_matches_collected_projection_and_caps() {
    #[derive(Serialize)]
    struct CollectedRequest<'a> {
        version: &'static str,
        spans: &'a [RequestSpan<'a>],
    }
    let content = GuardSpanKey {
        message_index: 2,
        part: GuardPart::Content,
    };
    let part = GuardSpanKey {
        message_index: u32::MAX,
        part: GuardPart::TextPart(u32::MAX),
    };
    for spans in [
        Vec::new(),
        vec![(content, "plain")],
        vec![(part, "café-π\n\"\\"), (content, ""), (part, "duplicate key")],
    ] {
        let collected: Vec<_> = spans
            .iter()
            .map(|(key, current)| RequestSpan {
                message_index: key.message_index,
                part: key.part.into(),
                current,
            })
            .collect();
        let reference = CollectedRequest {
            version: "v1",
            spans: &collected,
        };
        let expected = serde_json::to_vec(&reference).unwrap();
        for cap in [0, expected.len() - 1, expected.len(), expected.len() + 1] {
            let mut writer = CappedWriter::new(cap);
            let legacy = reference
                .serialize(&mut serde_json::Serializer::new(&mut writer))
                .map_err(|_| "evaluator request too large")
                .map(|()| Bytes::from(writer.into_inner()));
            assert_eq!(encode_request(&spans, cap), legacy, "cap={cap}");
        }
        assert_eq!(encode_request(&spans, expected.len()).unwrap().as_ref(), expected);
    }
}

#[test]
fn strict_envelope_rejects_malformed_decisions() {
    assert_eq!(
        parse_decision(br#"{"version":"v1","action":"pass"}"#, 64),
        Ok(GuardDecision::Pass)
    );
    assert!(parse_decision(br#"{"version":"v2","action":"pass"}"#, 64).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"pass","extra":1}"#, 64).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"pass"} {}"#, 64).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"pass","code":"x"}"#, 64).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"replace","replacements":[]}"#, 64).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"nope"}"#, 64).is_err());
}

#[test]
fn replacement_cap_is_inclusive_and_rejects_plus_one() {
    let body = br#"{"version":"v1","action":"replace","replacements":[{"messageIndex":0,"part":{"type":"content"},"current":"abc"}]}"#;
    assert!(parse_decision(body, 3).is_ok());
    assert!(parse_decision(body, 2).is_err());
}

#[test]
fn duplicate_replacement_keys_are_rejected() {
    let body = br#"{"version":"v1","action":"replace","replacements":[{"messageIndex":0,"part":{"type":"content"},"current":"a"},{"messageIndex":0,"part":{"type":"content"},"current":"b"}]}"#;
    assert!(parse_decision(body, 64).is_err());
}

#[test]
fn request_encoding_is_capped_inclusively() {
    let spans = vec![(
        GuardSpanKey {
            message_index: 0,
            part: GuardPart::Content,
        },
        "hello",
    )];
    let encoded = encode_request(&spans, 1024).expect("encodes");
    assert_eq!(encode_request(&spans, encoded.len()).expect("exact cap"), encoded);
    assert!(encode_request(&spans, encoded.len() - 1).is_err());
}

#[test]
fn only_single_json_success_responses_are_accepted() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.append(
        reqwest::header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    );
    assert!(is_json_response(200, &headers));
    assert!(!is_json_response(500, &headers));
    headers.append(reqwest::header::CONTENT_TYPE, "application/json".parse().unwrap());
    assert!(!is_json_response(200, &headers));
}

#[test]
fn request_payload_carries_only_span_text_and_keys() {
    let spans = vec![(
        GuardSpanKey {
            message_index: 2,
            part: GuardPart::TextPart(1),
        },
        "prompt",
    )];
    let encoded = encode_request(&spans, 4096).expect("encodes");
    let text = String::from_utf8(encoded.to_vec()).expect("utf8");
    assert!(text.contains("\"version\":\"v1\""));
    assert!(text.contains("\"messageIndex\":2"));
    assert!(text.contains("\"partIndex\":1"));
    assert!(text.contains("prompt"));
    // No model, provider, backend, header, or credential material.
    assert!(!text.contains("model"));
    assert!(!text.contains("authorization"));
}

#[test]
fn actual_evaluator_constructor_keeps_its_own_limits_and_service_namespace() {
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"guard","namespace":"tenant"},"spec":{"requestPlugins":[{"type":"AiGuard","config":{
            "maxReplacementBytes":1024,"evaluator":{"target":{"name":"decision","port":8080},"maxRequestBytes":2048,"maxConcurrent":3,"request":{"path":{"template":"/decision"}}}
        }}]}
    })).unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let field = entry.managed_http_config().unwrap().unwrap();
    let defaults = edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot::from_configs([]);
    let input = PreparedManagedHttp::new(&field, &defaults).unwrap();
    let evaluator = CompiledEvaluator::build(&input).unwrap();
    assert_eq!(evaluator.max_replacement_bytes, 1024);
    assert_eq!(evaluator.max_request_bytes, 2048);
    assert_eq!(evaluator.permits.available_permits(), 3);
    assert!(evaluator.client.retry.is_none());
    assert!(evaluator.client.compiled_request.method.is_some());
    assert!(
        matches!(&evaluator.client.resolved_target, crate::plugins::http::common::outbound::target::ResolvedTarget::Service {service_key, ..} if service_key == "tenant/decision")
    );
}

#[test]
fn plain_terminal_decisions_do_not_allocate() {
    for (body, expected) in [
        (br#"{"version":"v1","action":"pass"}"#.as_slice(), GuardDecision::Pass),
        (
            br#"{"version":"v1","action":"reject"}"#.as_slice(),
            GuardDecision::Reject,
        ),
    ] {
        let (result, stats) = crate::test_allocation_counter::count_allocation_stats(|| parse_decision(body, 10));
        assert_eq!(result, Ok(expected));
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
}

#[test]
fn escaped_decision_tokens_preserve_decoded_meaning() {
    for (plain, escaped) in [
        (br#"{"version":"v1","action":"pass"}"#.as_slice(),
         br#"{"version":"\u00761","action":"p\u0061ss"}"#.as_slice()),
        (br#"{"version":"v1","action":"reject","code":"policy","reason":"blocked"}"#,
         br#"{"version":"\u00761","action":"r\u0065ject","code":"policy","reason":"blocked"}"#),
        (br#"{"version":"v1","action":"replace","replacements":[{"messageIndex":0,"part":{"type":"content"},"current":"hello"}]}"#,
         br#"{"version":"\u00761","action":"r\u0065place","replacements":[{"messageIndex":0,"part":{"type":"content"},"current":"hello"}]}"#),
        (br#"{"version":"v2","action":"pass"}"#,
         br#"{"version":"\u00762","action":"p\u0061ss"}"#),
        (br#"{"version":"v1","action":"unknown"}"#,
         br#"{"version":"\u00761","action":"\u0075nknown"}"#),
    ] {
        assert_eq!(parse_decision(escaped, 10), parse_decision(plain, 10));
    }
}

#[test]
fn decision_token_errors_keep_their_categories_and_precedence() {
    for body in [
        br#"{"action":"pass"}"#.as_slice(),
        br#"{"version":"v1"}"#,
        br#"{"version":null,"action":"pass"}"#,
        br#"{"version":"v1","action":1}"#,
        br#"{"version":"v1","action":null}"#,
        br#"{"version":1,"action":"pass"}"#,
        br#"{"version":"v1","version":"v1","action":"pass"}"#,
        br#"{"version":"v1","action":"pass","action":"pass"}"#,
        br#"{"version":"v1","action":"pass","extra":1}"#,
        br#"{"version":"v1","action":"\q"}"#,
    ] {
        assert_eq!(parse_decision(body, 10), Err("evaluator invalid json"));
    }
    assert_eq!(
        parse_decision(br#"{"version":"v2","action":"pass"} {}"#, 10),
        Err("evaluator trailing json")
    );
    assert_eq!(
        parse_decision(br#"{"version":"v2","action":"unknown"}"#, 10),
        Err("evaluator bad version")
    );
    assert_eq!(
        parse_decision(br#"{"version":"v1","action":"unknown"}"#, 10),
        Err("evaluator bad action")
    );
    assert_eq!(
        parse_decision(br#"{"version":"v1","action":"pass","code":"policy"}"#, 10),
        Err("evaluator bad fields")
    );
}

#[test]
fn replacement_order_preserves_keys_text_and_budget_precedence() {
    let mut expected = Vec::new();
    let mut replacements = Vec::new();
    let mut total = 0;
    for index in 0..256u32 {
        let part = match index % 4 {
            0 => WirePart::Content,
            value => WirePart::TextPart { part_index: value - 1 },
        };
        let current = format!("text-{index}");
        total += current.len();
        expected.push((part.key(index / 4), current.clone()));
        replacements.push(serde_json::json!({
            "messageIndex": index / 4, "part": part, "current": current,
        }));
    }
    let scrambled: Vec<_> = (0..256).map(|index| &replacements[(index * 73) % 256]).collect();
    let body = serde_json::to_vec(&serde_json::json!({
        "version": "v1", "action": "replace", "replacements": scrambled,
    }))
    .unwrap();
    assert_eq!(parse_decision(&body, total), Ok(GuardDecision::Replace(expected)));
    assert_eq!(parse_decision(&body, total - 1), Err("evaluator replacement too large"));
    for part in [WirePart::Content, WirePart::TextPart { part_index: 7 }] {
        let body = serde_json::to_vec(&serde_json::json!({
            "version": "v1", "action": "replace", "replacements": [
                {"messageIndex": 2, "part": part, "current": "first"},
                {"messageIndex": 2, "part": part, "current": "second"},
            ],
        }))
        .unwrap();
        assert_eq!(parse_decision(&body, 11), Err("evaluator duplicate replacement"));
        assert_eq!(parse_decision(&body, 10), Err("evaluator replacement too large"));
    }
}

fn permit_test_evaluator(permits: usize) -> CompiledEvaluator {
    let mut client = ManagedOutboundClient::test_with_static_target("http://localhost:1");
    client.timeout = std::time::Duration::from_millis(10);
    client.health.force_unhealthy_for_test();
    CompiledEvaluator {
        client: Arc::new(client),
        permits: Arc::new(Semaphore::new(permits)),
        max_request_bytes: 4096,
        max_replacement_bytes: 4096,
    }
}

fn permit_test_spans() -> [(GuardSpanKey, &'static str); 1] {
    [(
        GuardSpanKey {
            message_index: 0,
            part: GuardPart::Content,
        },
        "prompt",
    )]
}

#[tokio::test]
async fn permit_wait_counts_against_the_decision_deadline() {
    let evaluator = permit_test_evaluator(0);
    let session = crate::plugins::runtime::traits::session::MockPluginSession::new();
    assert_eq!(
        evaluator.decide(&session, &permit_test_spans()).await,
        Err("evaluator timeout")
    );
    assert_eq!(evaluator.permits.available_permits(), 0);
}

#[tokio::test]
async fn permit_wait_cancellation_borrows_the_snapshot_semaphore() {
    let evaluator = permit_test_evaluator(0);
    let session = crate::plugins::runtime::traits::session::MockPluginSession::new();
    let spans = permit_test_spans();
    let owners = Arc::strong_count(&evaluator.permits);
    let mut decision = Box::pin(evaluator.decide(&session, &spans));
    assert!(futures::poll!(&mut decision).is_pending());
    assert_eq!(Arc::strong_count(&evaluator.permits), owners);
    drop(decision);
    evaluator.permits.add_permits(1);
    let permit = evaluator.permits.try_acquire().unwrap();
    drop(permit);
    assert_eq!(evaluator.permits.available_permits(), 1);
}

#[tokio::test]
async fn permit_closed_preserves_the_decision_error() {
    let evaluator = permit_test_evaluator(1);
    evaluator.permits.close();
    let session = crate::plugins::runtime::traits::session::MockPluginSession::new();
    assert_eq!(
        evaluator.decide(&session, &permit_test_spans()).await,
        Err("evaluator closed")
    );
}

#[tokio::test]
async fn permit_is_returned_after_health_gate_rejection() {
    let evaluator = permit_test_evaluator(1);
    let session = crate::plugins::runtime::traits::session::MockPluginSession::new();
    assert_eq!(
        evaluator.decide(&session, &permit_test_spans()).await,
        Err("evaluator dependency")
    );
    assert_eq!(evaluator.permits.available_permits(), 1);
}
