// Mock unary drives exercise the pure bounded conversion logic. The real Hold
// wiring is covered by ai_unary_hold_transport; no production fallback exists.
impl AnthropicResponseProcessor {
    pub(crate) fn new_with_quota(
        canonical_model: String,
        requested_streaming: bool,
        effective_usage_chunk: bool,
        created: u64,
        usage_shared: Arc<AiUsageShared>,
        quota: Option<Arc<QuotaAccumulator>>,
        terminate_on_over_limit: bool,
    ) -> Self {
        Self::new_with_quota_and_unary(
            canonical_model,
            requested_streaming,
            effective_usage_chunk,
            created,
            usage_shared,
            quota,
            terminate_on_over_limit,
            (!requested_streaming).then(|| {
                let pool = budget::UnaryBudget::new(1);
                Arc::new(UnaryHead::new(unary_limits(), pool.acquire().unwrap()))
            }),
        )
    }
}
use super::*;
use crate::ai::accounting::usage::QUOTA_EXCEEDED_SSE_EVENT;
use crate::ai::quota::QuotaFinalStatus;
use crate::plugins::runtime::traits::session::MockPluginSession;

fn mock_session() -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_semantic_guard_active().return_const(false);
    session.expect_try_claim_response_terminal().return_const(true);
    session.expect_mark_response_terminal_emission().return_const(());
    session
}

fn processor(streaming: bool, usage_chunk: bool) -> (AnthropicResponseProcessor, Arc<AiUsageShared>) {
    let usage = Arc::new(AiUsageShared::default());
    (
        AnthropicResponseProcessor::new(
            "claude-canonical".to_string(),
            streaming,
            usage_chunk,
            1_785_772_800,
            Arc::clone(&usage),
        ),
        usage,
    )
}

fn header(status: u16, content_type: &str) -> ResponseHeader {
    let mut response = ResponseHeader::build(status, None).expect("valid test status");
    response
        .insert_header("content-type", content_type)
        .expect("valid test content type");
    response
}

async fn run_chunk(
    processor: &mut AnthropicResponseProcessor,
    session: &mut MockPluginSession,
    bytes: Option<&[u8]>,
    eos: bool,
) -> Option<Bytes> {
    let mut chunk = bytes.map(Bytes::copy_from_slice);
    let mut sink = ResponseBodySink::new();
    if matches!(processor.mode, BodyMode::NonStreaming) {
        if processor.sealed {
            return None;
        }
        let input = bytes.unwrap_or_default();
        if !eos && input.len() <= MAX_BODY_BYTES {
            return None;
        }
        processor.sealed = true;
        return match translate_unary(
            input,
            &processor.canonical_model,
            processor.created,
            unary_limits(),
            Some(&processor.usage_shared),
        ) {
            Ok((body, ..)) => Some(Bytes::from(body)),
            Err(_) => {
                processor.record_translation_failure(session);
                processor.usage_shared.mark_json_unavailable();
                Some(Bytes::from(
                    crate::ai::render_openai_error(crate::ai::AiProxyError::UpstreamResponseInvalid).1,
                ))
            }
        };
    }
    processor
        .on_chunk(&mut chunk, eos, ResponseStreamKind::None, &mut sink, session)
        .await;
    chunk
}

fn event(name: &str, value: Value) -> String {
    format!(
        "event: {name}\ndata: {}\n\n",
        serde_json::to_string(&value).expect("test event serializes")
    )
}

fn streaming_fixture() -> String {
    [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"claude-provider","role":"assistant","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":3,"cache_creation_input_tokens":5,"cache_read_input_tokens":7,"output_tokens":0}}}),
            ),
            event(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}),
            ),
            event(
                "content_block_stop",
                json!({"type":"content_block_stop","index":0}),
            ),
            event(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":5}}),
            ),
            event(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":11}}),
            ),
            event("message_stop", json!({"type":"message_stop"})),
        ]
        .concat()
}

#[tokio::test]
async fn header_matrix_normalizes_and_strips_provider_metadata() {
    let cases = [
        (400, 400, "invalid_request_error", "provider_invalid_request"),
        (413, 413, "invalid_request_error", "request_too_large"),
        (429, 429, "rate_limit_error", "rate_limit_exceeded"),
        (201, 503, "api_error", "service_unavailable"),
        (500, 503, "api_error", "service_unavailable"),
    ];
    for (upstream, expected, type_, code) in cases {
        let (mut processor, _) = processor(false, false);
        let mut response = header(upstream, "application/json");
        response
            .insert_header("x-request-id", "secret-id")
            .expect("valid header");
        response
            .insert_header("set-cookie", "private=true")
            .expect("valid header");
        response
            .insert_header("etag", "provider-validator")
            .expect("valid header");
        response
            .insert_header("anthropic-ratelimit-tokens-limit", "1000")
            .expect("valid header");
        response
            .insert_header("x-amzn-requestid", "provider-request")
            .expect("valid header");
        response.insert_header("cf-ray", "edge-request").expect("valid header");
        response
            .insert_header("cf-cache-status", "DYNAMIC")
            .expect("valid header");
        response.insert_header("via", "provider-edge").expect("valid header");
        response.insert_header("alt-svc", "h3=\":443\"").expect("valid header");
        response.insert_header("server", "provider").expect("valid header");
        response
            .insert_header("access-control-allow-origin", "*")
            .expect("valid header");
        response
            .insert_header("cache-control", "public, max-age=86400")
            .expect("valid header");
        response
            .insert_header("x-compatible-private", "opaque")
            .expect("valid header");
        response.insert_header("retry-after", "12").expect("valid header");
        processor.on_response_header(&mut response);
        assert_eq!(response.status.as_u16(), expected);
        assert!(response.headers.get("x-request-id").is_none());
        assert!(response.headers.get("set-cookie").is_none());
        assert!(response.headers.get("etag").is_none());
        assert!(response.headers.get("anthropic-ratelimit-tokens-limit").is_none());
        assert!(response.headers.get("x-amzn-requestid").is_none());
        for name in [
            "cf-ray",
            "cf-cache-status",
            "via",
            "alt-svc",
            "server",
            "access-control-allow-origin",
            "x-compatible-private",
        ] {
            assert!(response.headers.get(name).is_none(), "provider header {name} survived");
        }
        assert_eq!(response.headers["cache-control"], "no-store");
        assert_eq!(response.headers.get("retry-after").is_some(), expected == 429);
        assert!(response
            .headers
            .keys()
            .all(|name| matches!(name.as_str(), "content-type" | "cache-control" | "retry-after")));

        let mut session = mock_session();
        let body = run_chunk(&mut processor, &mut session, Some(b"provider secret body"), false)
            .await
            .expect("fixed body");
        let body: Value = serde_json::from_slice(&body).expect("fixed JSON body");
        assert_eq!(body["error"]["type"], type_);
        assert_eq!(body["error"]["code"], code);
        assert!(!body.to_string().contains("provider secret body"));
    }
}

#[tokio::test]
async fn success_header_requires_media_identity_and_bounded_length() {
    let (mut streaming, _) = processor(true, false);
    let mut good = header(200, "Text/Event-Stream; charset=utf-8");
    good.insert_header("content-encoding", "identity")
        .expect("valid header");
    good.insert_header("content-length", "42").expect("valid header");
    streaming.on_response_header(&mut good);
    assert_eq!(good.status.as_u16(), 200);
    assert_eq!(good.headers["content-type"], "text/event-stream");
    assert!(good.headers.get("content-length").is_none());
    assert!(good.headers.get("content-encoding").is_none());

    for (content_type, encoding, length, expected_code) in [
        ("text/plain", None, None, "service_unavailable"),
        ("application/json", Some("gzip"), None, "service_unavailable"),
        (
            "application/json",
            None,
            Some((MAX_BODY_BYTES + 1).to_string()),
            "response_too_large",
        ),
    ] {
        let (mut p, _) = processor(false, false);
        let mut response = header(200, content_type);
        if let Some(encoding) = encoding {
            response
                .insert_header("content-encoding", encoding)
                .expect("valid encoding");
        }
        if let Some(length) = length {
            response.insert_header("content-length", length).expect("valid length");
        }
        p.on_response_header(&mut response);
        assert_eq!(response.status.as_u16(), 503);
        let mut session = mock_session();
        session
            .expect_record_error()
            .withf(|status| *status == EdgionErrorCode::AiResponseTranslationFailed)
            .times(1)
            .return_const(());
        let body = run_chunk(&mut p, &mut session, None, true).await.expect("fixed body");
        let body: Value = serde_json::from_slice(&body).expect("fixed JSON body");
        assert_eq!(body["error"]["code"], expected_code);
    }
}

#[tokio::test]
async fn non_streaming_text_tools_stop_and_cache_usage_are_canonical() {
    let (mut processor, usage) = processor(false, false);
    let mut response = header(200, "application/json; charset=utf-8");
    processor.on_response_header(&mut response);
    let provider = serde_json::to_vec(&json!({
        "id":"msg_1","type":"message","role":"assistant","model":"provider-alias",
        "content":[
            {"type":"text","text":"Hello "},
            {"type":"tool_use","id":"toolu_1","name":"weather","input":{"city":"Paris"}},
            {"type":"text","text":"world"}
        ],
        "stop_reason":"tool_use","stop_sequence":null,
        "usage":{"input_tokens":3,"cache_creation_input_tokens":5,"cache_read_input_tokens":7,"output_tokens":11}
    }))
    .expect("fixture serializes");
    let split = provider.len() / 2;
    let mut session = mock_session();
    assert!(run_chunk(&mut processor, &mut session, Some(&provider[..split]), false)
        .await
        .is_none());
    let translated = run_chunk(&mut processor, &mut session, Some(&provider), true)
        .await
        .expect("translated response");
    let translated: Value = serde_json::from_slice(&translated).expect("OpenAI response JSON");
    assert_eq!(translated["model"], "claude-canonical");
    assert_eq!(translated["choices"][0]["message"]["content"], "Hello world");
    assert_eq!(translated["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(translated["choices"][0]["message"]["tool_calls"][0]["index"], 0);
    assert_eq!(
        translated["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{\"city\":\"Paris\"}"
    );
    assert_eq!(translated["usage"]["prompt_tokens"], 15);
    assert_eq!(translated["usage"]["completion_tokens"], 11);
    assert_eq!(usage.reported_tokens(), Some((15, 11)));
    let detailed = usage.reported_usage().unwrap();
    assert_eq!(detailed.cache_read_input_tokens, 7);
    assert_eq!(detailed.cache_write_input_tokens, 5);
}

#[tokio::test]
async fn non_streaming_tool_only_message_uses_null_content() {
    let provider = serde_json::to_vec(&json!({
        "id":"msg_tool","type":"message","role":"assistant","model":"provider",
        "content":[{"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}],
        "stop_reason":"tool_use","stop_sequence":null,
        "usage":{"input_tokens":3,"output_tokens":2}
    }))
    .unwrap();
    let (mut processor, _) = processor(false, false);
    processor.on_response_header(&mut header(200, "application/json"));
    let mut session = mock_session();
    let translated = run_chunk(&mut processor, &mut session, Some(&provider), true)
        .await
        .unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    let message = translated["choices"][0]["message"].as_object().unwrap();
    assert!(message.contains_key("content"));
    assert_eq!(message["content"], Value::Null);
    assert_eq!(message["tool_calls"][0]["index"], 0);
    assert_eq!(message["tool_calls"][0]["id"], "toolu_1");
    assert_eq!(message["tool_calls"][0]["function"]["name"], "lookup");
    assert_eq!(message["tool_calls"][0]["function"]["arguments"], "{}");
    assert_eq!(translated["choices"][0]["finish_reason"], "tool_calls");
}

#[tokio::test]
async fn malformed_non_streaming_response_is_fixed_redacted_and_sealed() {
    let (mut processor, _) = processor(false, false);
    processor.on_response_header(&mut header(200, "application/json"));
    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiResponseTranslationFailed)
        .times(1)
        .return_const(());
    let body = run_chunk(
        &mut processor,
        &mut session,
        Some(b"{\"provider_secret\":\"must-not-leak\"}"),
        true,
    )
    .await
    .expect("fixed failure");
    let rendered = String::from_utf8(body.to_vec()).expect("fixed UTF-8");
    assert!(rendered.contains("upstream_response_invalid"));
    assert!(!rendered.contains("must-not-leak"));
    assert!(run_chunk(&mut processor, &mut session, Some(b"later raw bytes"), true)
        .await
        .is_none());
}

#[tokio::test]
async fn streaming_translation_handles_every_byte_split_and_exact_usage() {
    const WIRE_GOLDEN: &str = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"role\":\"assistant\"},\"finish_reason\":null,\"index\":0}],\"created\":1785772800,\"id\":\"msg_1\",\"model\":\"claude-canonical\",\"object\":\"chat.completion.chunk\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null,\"index\":0}],\"created\":1785772800,\"id\":\"msg_1\",\"model\":\"claude-canonical\",\"object\":\"chat.completion.chunk\"}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\",\"index\":0}],\"created\":1785772800,\"id\":\"msg_1\",\"model\":\"claude-canonical\",\"object\":\"chat.completion.chunk\"}\n\n",
            "data: {\"choices\":[],\"created\":1785772800,\"id\":\"msg_1\",\"model\":\"claude-canonical\",\"object\":\"chat.completion.chunk\",\"usage\":{\"completion_tokens\":11,\"prompt_tokens\":15,\"total_tokens\":26}}\n\n",
            "data: [DONE]\n\n",
        );
    let fixture = streaming_fixture();
    for split in 0..=fixture.len() {
        let (mut processor, usage) = processor(true, true);
        processor.on_response_header(&mut header(200, "text/event-stream"));
        let mut session = mock_session();
        let first = run_chunk(&mut processor, &mut session, Some(&fixture.as_bytes()[..split]), false).await;
        let second = run_chunk(&mut processor, &mut session, Some(&fixture.as_bytes()[split..]), true).await;
        let mut output = Vec::new();
        if let Some(first) = first {
            output.extend_from_slice(&first);
        }
        if let Some(second) = second {
            output.extend_from_slice(&second);
        }
        let output = String::from_utf8(output).expect("OpenAI SSE is UTF-8");
        assert_eq!(output, WIRE_GOLDEN, "split={split}");
        assert!(output.contains("\"role\":\"assistant\""), "split={split}");
        assert!(output.contains("\"content\":\"Hello\""), "split={split}");
        assert!(output.contains("\"finish_reason\":\"stop\""), "split={split}");
        assert!(output.contains("\"prompt_tokens\":15"), "split={split}");
        assert!(output.ends_with("data: [DONE]\n\n"), "split={split}");
        assert!(!output.contains("claude-provider"));
        assert_eq!(usage.reported_tokens(), Some((15, 11)));
        let mut oracle = SseDecoder::with_default_limit();
        oracle.enable_data_char_counting();
        oracle.feed_each_bytes(Bytes::copy_from_slice(output.as_bytes()), |_| {});
        assert_eq!(usage.output_chars(), oracle.take_data_chars(), "split={split}");
    }
}

#[tokio::test]
async fn streaming_tools_forward_argument_fragments_without_accumulating() {
    const WIRE_GOLDEN: &str = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"role\":\"assistant\"},\"finish_reason\":null,\"index\":0}],\"created\":1785772800,\"id\":\"msg_tools\",\"model\":\"claude-canonical\",\"object\":\"chat.completion.chunk\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"arguments\":\"\",\"name\":\"weather\"},\"id\":\"toolu_1\",\"index\":0,\"type\":\"function\"}]},\"finish_reason\":null,\"index\":0}],\"created\":1785772800,\"id\":\"msg_tools\",\"model\":\"claude-canonical\",\"object\":\"chat.completion.chunk\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"arguments\":\"{\\\"city\\\":\"},\"index\":0}]},\"finish_reason\":null,\"index\":0}],\"created\":1785772800,\"id\":\"msg_tools\",\"model\":\"claude-canonical\",\"object\":\"chat.completion.chunk\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"arguments\":\"\\\"Paris\\\"}\"},\"index\":0}]},\"finish_reason\":null,\"index\":0}],\"created\":1785772800,\"id\":\"msg_tools\",\"model\":\"claude-canonical\",\"object\":\"chat.completion.chunk\"}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\",\"index\":0}],\"created\":1785772800,\"id\":\"msg_tools\",\"model\":\"claude-canonical\",\"object\":\"chat.completion.chunk\"}\n\n",
            "data: [DONE]\n\n",
        );
    let fixture = [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_tools","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":1}}}),
            ),
            event(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"weather","input":{}}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"city\":"}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"Paris\"}"}}),
            ),
            event("content_block_stop", json!({"type":"content_block_stop","index":0})),
            event(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
            ),
            event("message_stop", json!({"type":"message_stop"})),
        ]
        .concat();
    let (mut processor, usage) = processor(true, false);
    processor.on_response_header(&mut header(200, "text/event-stream"));
    let mut session = mock_session();
    let output = run_chunk(&mut processor, &mut session, Some(fixture.as_bytes()), true)
        .await
        .expect("translated stream");
    let output = String::from_utf8(output.to_vec()).expect("UTF-8 output");
    assert_eq!(output, WIRE_GOLDEN);
    assert_eq!(usage.reported_tokens(), Some((1, 2)));
}

#[tokio::test]
async fn streaming_tool_indexes_are_independent_of_content_block_indexes() {
    let fixture = [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_tools","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":1}}}),
            ),
            event(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Checking tools"}}),
            ),
            event("content_block_stop", json!({"type":"content_block_stop","index":0})),
            event(
                "content_block_start",
                json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"first","input":{}}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"one\":1}"}}),
            ),
            event("content_block_stop", json!({"type":"content_block_stop","index":1})),
            event(
                "content_block_start",
                json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_2","name":"second","input":{}}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"two\":2}"}}),
            ),
            event("content_block_stop", json!({"type":"content_block_stop","index":2})),
            event(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
            ),
            event("message_stop", json!({"type":"message_stop"})),
        ]
        .concat();
    let (mut processor, _) = self::processor(true, false);
    processor.on_response_header(&mut header(200, "text/event-stream"));
    let mut session = mock_session();
    let output = run_chunk(&mut processor, &mut session, Some(fixture.as_bytes()), true)
        .await
        .unwrap();
    let output = String::from_utf8(output.to_vec()).unwrap();
    let mut starts = Vec::new();
    let mut fragments = Vec::new();
    let mut decoder = SseDecoder::with_default_limit();
    decoder.feed_each(output.as_bytes(), |event| {
        let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
            return;
        };
        let Some(call) = value.pointer("/choices/0/delta/tool_calls/0") else {
            return;
        };
        let index = call["index"].as_u64().unwrap();
        if let Some(id) = call.get("id").and_then(Value::as_str) {
            starts.push((id.to_string(), index));
        }
        if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
            if !arguments.is_empty() {
                fragments.push((arguments.to_string(), index));
            }
        }
    });
    assert_eq!(starts, [("toolu_1".to_string(), 0), ("toolu_2".to_string(), 1)]);
    assert_eq!(
        fragments,
        [("{\"one\":1}".to_string(), 0), ("{\"two\":2}".to_string(), 1)]
    );

    let invalid = [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_bad","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":1}}}),
            ),
            event(
                "content_block_start",
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            ),
        ]
        .concat();
    let (mut invalid_processor, _) = self::processor(true, false);
    invalid_processor.on_response_header(&mut header(200, "text/event-stream"));
    let mut session = mock_session();
    session.expect_record_error().times(1).return_const(());
    assert_eq!(
        run_chunk(&mut invalid_processor, &mut session, Some(invalid.as_bytes()), false)
            .await
            .unwrap()
            .as_ref(),
        STREAM_ERROR
    );
}

#[tokio::test]
async fn streaming_quota_tracks_semantic_deltas_and_cumulative_usage_monotonically() {
    let usage = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new_for_test(100));
    let mut processor = AnthropicResponseProcessor::new_with_quota(
        "claude-canonical".to_string(),
        true,
        false,
        1_785_772_800,
        usage.clone(),
        Some(quota.clone()),
        false,
    );
    processor.on_response_header(&mut header(200, "text/event-stream"));
    let mut session = mock_session();
    let events = [
        (
            "message_start",
            json!({"type":"message_start","message":{"type":"message","id":"msg_quota","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":10}}}),
            0,
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            0,
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":""}}),
            0,
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}),
            1,
        ),
        ("content_block_stop", json!({"type":"content_block_stop","index":0}), 1),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}}),
            1,
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}),
            1,
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
            2,
        ),
        ("content_block_stop", json!({"type":"content_block_stop","index":1}), 2),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":5}}),
            5,
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":3}}),
            5,
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":11}}),
            11,
        ),
    ];
    for (type_, value, expected) in events {
        let framed = event(type_, value);
        run_chunk(&mut processor, &mut session, Some(framed.as_bytes()), false).await;
        assert_eq!(quota.observed_output_for_test(), expected);
    }
    let stop = event("message_stop", json!({"type":"message_stop"}));
    run_chunk(&mut processor, &mut session, Some(stop.as_bytes()), true).await;
    assert_eq!(usage.reported_tokens(), Some((10, 11)));
}

#[tokio::test]
async fn malformed_order_clears_same_callback_output_and_emits_one_error() {
    let fixture = [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":1}}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"must-not-leak"}}),
            ),
        ]
        .concat();
    let (mut processor, _) = self::processor(true, false);
    processor.on_response_header(&mut header(200, "text/event-stream"));
    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiResponseTranslationFailed)
        .times(1)
        .return_const(());
    let output = run_chunk(&mut processor, &mut session, Some(fixture.as_bytes()), false)
        .await
        .expect("fixed stream error");
    assert_eq!(output.as_ref(), STREAM_ERROR);
    assert!(!String::from_utf8_lossy(&output).contains("must-not-leak"));
    assert!(run_chunk(&mut processor, &mut session, Some(b"provider secret"), true)
        .await
        .is_none());
}

#[tokio::test]
async fn unknown_events_and_ping_are_ignored_but_known_name_mismatch_fails() {
    let unknown = event("future_event", json!({"type":"future_event","secret":"ignored"}));
    let ping = event("ping", json!({"type":"ping"}));
    let (mut processor, _) = self::processor(true, false);
    processor.on_response_header(&mut header(200, "text/event-stream"));
    let mut session = mock_session();
    assert!(run_chunk(
        &mut processor,
        &mut session,
        Some(format!("{unknown}{ping}").as_bytes()),
        false
    )
    .await
    .is_none());

    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiResponseTranslationFailed)
        .times(1)
        .return_const(());
    let mismatch = event("ping", json!({"type":"message_start"}));
    let output = run_chunk(&mut processor, &mut session, Some(mismatch.as_bytes()), false)
        .await
        .expect("fixed stream error");
    assert_eq!(output.as_ref(), STREAM_ERROR);

    let (mut second_processor, _) = self::processor(true, false);
    second_processor.on_response_header(&mut header(200, "text/event-stream"));
    let mut session = mock_session();
    session.expect_record_error().times(1).return_const(());
    let mismatch = event("error", json!({"type":"future_event"}));
    let output = run_chunk(&mut second_processor, &mut session, Some(mismatch.as_bytes()), false)
        .await
        .expect("known event-name mismatch fails");
    assert_eq!(output.as_ref(), STREAM_ERROR);
}

#[tokio::test]
async fn genuine_midstream_error_is_redacted_and_seals_the_stream() {
    let fixture = [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":1}}}),
            ),
            event(
                "error",
                json!({"type":"error","error":{"type":"overloaded_error","message":"provider-secret"}}),
            ),
        ]
        .concat();
    let (mut processor, _) = processor(true, false);
    processor.on_response_header(&mut header(200, "text/event-stream"));
    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiResponseTranslationFailed)
        .times(1)
        .return_const(());
    let output = run_chunk(&mut processor, &mut session, Some(fixture.as_bytes()), false)
        .await
        .unwrap();
    assert_eq!(output.as_ref(), STREAM_ERROR);
    assert!(!String::from_utf8_lossy(&output).contains("provider-secret"));
    assert!(run_chunk(&mut processor, &mut session, Some(b"later"), true)
        .await
        .is_none());
}

#[tokio::test]
async fn missing_required_usage_fields_fail_instead_of_publishing_zero() {
    let provider = serde_json::to_vec(&json!({
        "id":"msg_1","type":"message","role":"assistant","model":"provider",
        "content":[{"type":"text","text":"Hello"}],"stop_reason":"end_turn",
        "usage":{"input_tokens":3}
    }))
    .expect("fixture serializes");
    let (mut processor, usage) = processor(false, false);
    processor.on_response_header(&mut header(200, "application/json"));
    let mut session = mock_session();
    session.expect_record_error().times(1).return_const(());
    let output = run_chunk(&mut processor, &mut session, Some(&provider), true)
        .await
        .expect("fixed error");
    assert!(String::from_utf8_lossy(&output).contains("upstream_response_invalid"));
    assert_eq!(usage.reported_tokens(), None);

    let stream = [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":3,"output_tokens":0}}}),
            ),
            event(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            ),
            event("content_block_stop", json!({"type":"content_block_stop","index":0})),
            event(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{}}),
            ),
            event("message_stop", json!({"type":"message_stop"})),
        ]
        .concat();
    let (mut processor, usage) = self::processor(true, false);
    processor.on_response_header(&mut header(200, "text/event-stream"));
    let mut session = mock_session();
    session.expect_record_error().times(1).return_const(());
    let output = run_chunk(&mut processor, &mut session, Some(stream.as_bytes()), true)
        .await
        .expect("fixed stream error");
    assert_eq!(output.as_ref(), STREAM_ERROR);
    assert_eq!(usage.reported_tokens(), None);
}

#[tokio::test]
async fn malformed_utf8_and_nonempty_initial_content_fail_closed() {
    let valid_start = event(
        "message_start",
        json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":1}}}),
    );
    let cases = [
            b"event: ping\ndata: \xff\n\n".to_vec(),
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"provider","role":"assistant","content":[{"type":"text","text":"lost"}],"usage":{"input_tokens":1}}}),
            )
            .into_bytes(),
            format!(
                "{valid_start}{}",
                event(
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"lost"}}),
                )
            )
            .into_bytes(),
            format!(
                "{valid_start}{}",
                event(
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"weather","input":{"lost":true}}}),
                )
            )
            .into_bytes(),
        ];

    for input in cases {
        let (mut processor, _) = self::processor(true, false);
        processor.on_response_header(&mut header(200, "text/event-stream"));
        let mut session = mock_session();
        session
            .expect_record_error()
            .withf(|status| *status == EdgionErrorCode::AiResponseTranslationFailed)
            .times(1)
            .return_const(());
        let output = run_chunk(&mut processor, &mut session, Some(&input), false)
            .await
            .expect("fixed stream error");
        assert_eq!(output.as_ref(), STREAM_ERROR);
        assert!(!String::from_utf8_lossy(&output).contains("lost"));
    }
}

#[tokio::test]
async fn message_stop_is_terminal_and_eos_requires_an_idle_decoder() {
    for suffix in [
        event("ping", json!({"type":"ping"})),
        event("future_event", json!({"type":"future_event"})),
        "event: ping\ndata: {".to_string(),
    ] {
        let input = format!("{}{suffix}", streaming_fixture());
        let (mut processor, _) = self::processor(true, false);
        processor.on_response_header(&mut header(200, "text/event-stream"));
        let mut session = mock_session();
        session
            .expect_record_error()
            .withf(|status| *status == EdgionErrorCode::AiResponseTranslationFailed)
            .times(1)
            .return_const(());
        let output = run_chunk(&mut processor, &mut session, Some(input.as_bytes()), true)
            .await
            .expect("terminal violation emits a fixed stream error");
        assert_eq!(output.as_ref(), STREAM_ERROR);
        assert!(!String::from_utf8_lossy(&output).contains("[DONE]"));
    }
}

#[tokio::test]
async fn streaming_translation_terminates_when_quota_is_over_limit() {
    let usage = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    // terminate_on_over_limit = true: the route's `onExceeded` resolved
    // to `Reject`.
    let mut processor = AnthropicResponseProcessor::new_with_quota(
        "claude-canonical".to_string(),
        true,
        false,
        1_785_772_800,
        Arc::clone(&usage),
        Some(quota),
        true,
    );
    processor.on_response_header(&mut header(200, "text/event-stream"));

    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiQuotaOverLimitTerminated)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();
    let message_start = event(
        "message_start",
        json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"claude-provider","role":"assistant","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":0}}}),
    );
    let mut chunk = Some(Bytes::from(message_start.into_bytes()));
    processor
        .on_chunk(&mut chunk, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert!(
        sink.is_terminated(),
        "an over-limit quota must end an Anthropic-backed stream too"
    );
    let extra = sink.peek_extra();
    assert_eq!(extra.len(), 1);
    assert_eq!(
        extra[0], *QUOTA_EXCEEDED_SSE_EVENT,
        "the injected event must reuse the same message/type/code the 429 admission-time \
             rejection renders"
    );
    assert!(chunk.is_some(), "the translated chunk in flight must not be dropped");
    assert_eq!(
        usage.output_chars(),
        0,
        "the terminated chunk never reached the old estimator"
    );
}

#[tokio::test]
async fn streaming_translation_failure_emits_error_then_terminates_once() {
    let (mut processor, usage) = processor(true, false);
    processor.on_response_header(&mut header(200, "text/event-stream"));
    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiResponseTranslationFailed)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();
    let mut chunk = Some(Bytes::from_static(b"event: error\ndata: {}\n\n"));
    processor
        .on_chunk(&mut chunk, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert_eq!(chunk.as_deref(), Some(STREAM_ERROR));
    assert!(sink.is_terminated());
    assert!(sink.peek_extra().is_empty());
    assert_eq!(usage.output_chars(), 0);

    let mut later = Some(Bytes::from_static(b"provider bytes"));
    processor
        .on_chunk(&mut later, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;
    assert!(later.is_none());
    assert!(sink.peek_extra().is_empty());
}

#[tokio::test]
async fn streaming_translation_is_not_terminated_under_throttle_or_downgrade() {
    // `terminate_on_over_limit = false` is what the plugin computes for
    // both `Throttle` and `Downgrade`; the processor only sees the
    // resolved boolean, so a single `false` case covers both.
    let usage = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    let mut processor = AnthropicResponseProcessor::new_with_quota(
        "claude-canonical".to_string(),
        true,
        false,
        1_785_772_800,
        Arc::clone(&usage),
        Some(quota),
        false,
    );
    processor.on_response_header(&mut header(200, "text/event-stream"));

    let mut session = mock_session();
    let mut sink = ResponseBodySink::new();
    let message_start = event(
        "message_start",
        json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"claude-provider","role":"assistant","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":0}}}),
    );
    let mut chunk = Some(Bytes::from(message_start.into_bytes()));
    processor
        .on_chunk(&mut chunk, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert!(
        !sink.is_terminated(),
        "Throttle/Downgrade must let an over-limit Anthropic-backed stream run to completion"
    );
    assert!(sink.peek_extra().is_empty(), "no error event without a terminate");
}

#[tokio::test]
async fn non_streaming_translation_never_terminates_even_when_quota_is_over_limit() {
    let usage = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    let mut processor = AnthropicResponseProcessor::new_with_quota(
        "claude-canonical".to_string(),
        false,
        false,
        1_785_772_800,
        Arc::clone(&usage),
        Some(quota),
        true,
    );
    let pool = budget::UnaryBudget::new(1);
    processor.unary = Some(Arc::new(UnaryHead::new(unary_limits(), pool.acquire().unwrap())));
    processor.on_response_header(&mut header(200, "application/json"));

    let mut session = mock_session();
    let mut sink = ResponseBodySink::new();
    let mut chunk = Some(Bytes::from_static(b"{\"partial"));
    processor
        .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
        .await;

    assert!(
        !sink.is_terminated(),
        "a non-streaming completion has no burn left to stop and must not be cut short"
    );
    assert!(sink.peek_extra().is_empty());
}

#[tokio::test]
async fn bounds_and_stop_reason_mapping_are_stable() {
    for (anthropic, openai) in [
        ("end_turn", "stop"),
        ("stop_sequence", "stop"),
        ("pause_turn", "stop"),
        ("max_tokens", "length"),
        ("model_context_window_exceeded", "length"),
        ("tool_use", "tool_calls"),
        ("refusal", "content_filter"),
    ] {
        assert_eq!(map_stop_reason(anthropic), Some(openai));
    }
    assert_eq!(map_stop_reason("not_a_real_reason"), None);
    assert!(valid_tool_name("tool-name_1"));
    assert!(!valid_tool_name("bad tool"));
    assert!(!valid_tool_name(&"a".repeat(MAX_TOOL_NAME_BYTES + 1)));

    let oversized = vec![b'x'; MAX_BODY_BYTES + 1];
    let (mut processor, _) = processor(false, false);
    processor.on_response_header(&mut header(200, "application/json"));
    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiResponseTranslationFailed)
        .times(1)
        .return_const(());
    let output = run_chunk(&mut processor, &mut session, Some(&oversized), false)
        .await
        .expect("bounded error");
    let output: Value = serde_json::from_slice(&output).expect("fixed JSON");
    assert_eq!(output["error"]["code"], "upstream_response_invalid");
}

fn unary_limits() -> UnaryLimits {
    UnaryLimits::resolve(&crate::ai::config::UnaryConfig::default()).unwrap()
}
fn unary_document() -> Vec<u8> {
    serde_json::to_vec(&json!({"id":"msg_1","type":"message","role":"assistant","model":"provider","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":7,"output_tokens":11}})).unwrap()
}
#[test]
fn unary_preflight_counts_unknown_duplicate_and_empty_container_nodes() {
    let mut limits = unary_limits();
    limits.nodes = 3;
    limits.lexical_events = 9;
    preflight(br#"{"a":{},"b":[]}"#, limits).unwrap();
    assert!(preflight(br#"{"a":{},"a":{},"a":{}}"#, limits).is_err());
    limits = unary_limits();
    limits.depth = 1;
    assert!(preflight(br#"{"unknown":{"deeper":{}}}"#, limits).is_err());
    limits = unary_limits();
    limits.key = 3;
    assert!(preflight(br#"{"long":0}"#, limits).is_err());
    assert!(preflight(
        br#"{"a":18446744073709551616184467440737095516161844674407370955161618446744073709551616}"#,
        unary_limits()
    )
    .is_err());
    assert!(preflight(br#"{} {}"#, unary_limits()).is_err());
    assert!(preflight(br#"{"a":"truncated""#, unary_limits()).is_err());
    let input = br#"{"a":"escaped\ntext"}"#;
    let mut limits = unary_limits();
    limits.input = input.len();
    limits.lexical_work = 4 * limits.input;
    preflight(input, limits).unwrap();
    limits.input -= 1;
    assert!(preflight(input, limits).is_err());
}
#[test]
fn unary_output_failure_preserves_validated_usage_through_cleanup() {
    let budget = budget::UnaryBudget::new(1);
    let reservation = budget.acquire().unwrap();
    let owner = Arc::new(UnaryHead::new(unary_limits(), reservation));
    let usage = Arc::new(AiUsageShared::default());
    usage.mark_json_started();
    let mut limits = unary_limits();
    limits.output = 8;
    assert_eq!(
        translate_unary(&unary_document(), "canonical", 1, limits, Some(&usage)).unwrap_err(),
        NonStreamingError::OutputTooLarge
    );
    assert_eq!(usage.reported_tokens(), Some((7, 11)));
    assert!(!usage.observation_unavailable());
    let mut processor = AnthropicResponseProcessor::new_with_quota_and_unary(
        "canonical".into(),
        false,
        false,
        1,
        usage.clone(),
        None,
        false,
        Some(owner.clone()),
    );
    processor.non_streaming_input = vec![1; 1024];
    processor.finalize_incomplete("terminal", "local-trailer-failure");
    assert_eq!(processor.non_streaming_input.capacity(), 0);
    assert!(!usage.observation_unavailable());
    assert!(budget.acquire().is_none());
    drop(owner);
    assert!(budget.acquire().is_some());
    processor.release_inflight();
    assert_eq!(usage.reported_tokens(), Some((7, 11)));
    assert!(!usage.observation_unavailable());
}
#[test]
fn unary_invalid_document_and_arithmetic_overflow_publish_no_usage_or_error_estimates() {
    let usage = AiUsageShared::default();
    usage.mark_json_started();
    let mut document = unary_document();
    document.extend_from_slice(b" secret");
    assert!(translate_unary(&document, "canonical", 1, unary_limits(), Some(&usage)).is_err());
    assert_eq!(usage.reported_tokens(), None);
    assert_eq!(usage.output_chars(), 0);
    let document=serde_json::to_vec(&json!({"id":"msg","type":"message","role":"assistant","model":"provider","content":[],"stop_reason":"end_turn","usage":{"input_tokens":u64::MAX,"output_tokens":1}})).unwrap();
    assert!(translate_unary(&document, "canonical", 1, unary_limits(), Some(&usage)).is_err());
    assert_eq!(usage.reported_tokens(), None);
}
#[test]
fn unary_duplicate_keys_preserve_last_wins_and_compact_tool_arguments() {
    let document=br#"{"id":"msg","type":"message","role":"assistant","model":"provider","content":[{"type":"tool_use","id":"tool","name":"lookup","input":{"key":"old","key":"new","escaped":"\n\""}}],"stop_reason":"tool_use","usage":{"input_tokens":1,"output_tokens":2}}"#;
    let (body, _, _, _, _) = translate_unary(document, "canonical", 1, unary_limits(), None).unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], Value::Null);
    let arguments = body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
        .as_str()
        .unwrap();
    let parsed: Value = serde_json::from_str(arguments).unwrap();
    assert_eq!(parsed["key"], "new");
    assert_eq!(parsed["escaped"], "\n\"");
    assert!(!arguments.contains(": "));
}
#[test]
fn unary_adversarial_capacity_probe_stays_below_reservation() {
    let mut value: Value = serde_json::from_slice(&unary_document()).unwrap();
    value["unknown"] = Value::Array((0..7000).map(|_| json!({"x":[]})).collect());
    let input = serde_json::to_vec(&value).unwrap();
    let (result, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        translate_unary(&input, "canonical", 1, unary_limits(), None)
    });
    assert!(result.is_ok());
    assert!(
        stats.requested_bytes < unary_limits().reservation,
        "requested {} versus reservation {}",
        stats.requested_bytes,
        unary_limits().reservation
    );
}

#[test]
fn concurrent_adversarial_documents_and_slow_output_owners_stay_reserved() {
    const WORKERS: usize = 4;
    let limits = unary_limits();
    let mut value: Value = serde_json::from_slice(&unary_document()).unwrap();
    value["unknown"] = Value::Array((0..7000).map(|_| json!({"x":[]})).collect());
    let input = Arc::new(serde_json::to_vec(&value).unwrap());
    let pool = Arc::new(budget::UnaryBudget::new(WORKERS));
    let ready = Arc::new(std::sync::Barrier::new(WORKERS));
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (input, pool, ready) = (input.clone(), pool.clone(), ready.clone());
            std::thread::spawn(move || {
                let exchange = pool.acquire().unwrap();
                ready.wait();
                let ((output, clone), stats) = crate::test_allocation_counter::count_allocation_stats(|| {
                    let mut raw = Vec::new();
                    append(&mut raw, &input, limits.input).unwrap();
                    let (body, ..) = translate_unary(&raw, "canonical", 1, limits, None).unwrap();
                    drop(raw);
                    let output = budget::output(body, exchange.clone());
                    let clone = output.clone();
                    (output, clone)
                });
                assert!(stats.requested_bytes < limits.reservation);
                (exchange, output, clone, stats)
            })
        })
        .collect();
    let mut owners: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    assert!(pool.acquire().is_none());
    let sum: usize = owners.iter().map(|entry| entry.3.requested_bytes).sum();
    assert!(sum < WORKERS * limits.reservation);
    eprintln!(
        "unary converter layout: processor={}, borrowed_tool_view={}",
        std::mem::size_of::<AnthropicResponseProcessor>(),
        std::mem::size_of::<UnaryToolCall<'static>>()
    );
    eprintln!("unary concurrent allocation probe: workers={WORKERS}, cumulative_requested={sum}, reservation_sum={}, per_worker={:?}; allocator metadata/slack and transport/TLS excluded",WORKERS*limits.reservation,owners.iter().map(|entry|entry.3.requested_bytes).collect::<Vec<_>>());
    let mut readmitted = Vec::new();
    for (exchange, output, clone, _) in owners.drain(..) {
        // Simulate output replacement and exchange cleanup while a slow consumer
        // still owns a slice of the successfully translated body.
        let retained = clone.slice(..);
        drop(exchange);
        drop(output);
        drop(clone);
        assert!(pool.acquire().is_none());
        drop(retained);
        let replacement = pool.acquire().unwrap();
        readmitted.push(replacement);
    }
}
