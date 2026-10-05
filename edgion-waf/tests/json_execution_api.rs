//! External-consumer coverage for the bounded JSON transaction API.
use edgion_waf::seclang::{
    compile_with_includes, BodyProcessorSelection, CompiledGeneration, FrontendLimits, IncludeFailure, IncludeRequest,
    IncludeSink, IncludeSource, JsonBodyError, JsonDocumentLimits, JsonExecutionError, JsonExecutionLimit,
    JsonExecutionLimits, JsonFactLimits, JsonPlanLimits, JsonStreamInput, JsonStreamLimits, Phase, PhaseDefaults,
    RequestBody, RequestHead, RequestHeader, RuntimeLimits, SemanticValidationContext, Transaction,
};

struct NoIncludes;
impl IncludeSource for NoIncludes {
    fn load(&mut self, _: IncludeRequest<'_>, _: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

fn generation(source: &str) -> CompiledGeneration {
    let defaults = [PhaseDefaults::pass(Phase::RequestBody, true, true)];
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap();
    compile_with_includes(
        "json-api",
        source.as_bytes(),
        &mut NoIncludes,
        None,
        context,
        FrontendLimits::default(),
    )
    .unwrap()
    .into_parts()
    .0
}

// Deliberately explicit fixture budgets, not suggested deployment defaults.
fn limits() -> JsonExecutionLimits {
    JsonExecutionLimits {
        lexical: JsonStreamLimits {
            max_source_bytes: 4096,
            max_raw_token_bytes: 1024,
            max_decoded_bytes: 4096,
            max_events: 1024,
            max_depth: 16,
            max_retained_bytes: 64 * 1024,
            max_work: 1024 * 1024,
        },
        document: JsonDocumentLimits {
            max_retained_bytes: 1024 * 1024,
            max_key_bytes: 4096,
            max_members: 1024,
            max_events: 1024,
            max_path_bytes: 4096,
            max_depth: 16,
            max_work: 1024 * 1024,
        },
        plan: JsonPlanLimits {
            max_subscribers: 16,
            max_retained_bytes: 4096,
        },
        facts: JsonFactLimits {
            max_facts: 1024,
            max_retained_bytes: 1024 * 1024,
            max_work: 1024 * 1024,
        },
    }
}

fn transaction(generation: &CompiledGeneration, runtime: RuntimeLimits) -> Transaction<'_> {
    let headers = [RequestHeader::new(b"content-type", b"application/json")];
    let head = RequestHead::new(
        b"POST /?q=miss HTTP/1.1",
        b"POST",
        b"/?q=miss",
        b"/?q=miss",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"json-api",
        &headers,
    );
    let mut tx = Transaction::new(generation, head, runtime).unwrap();
    tx.execute_request_headers().unwrap();
    tx
}

#[test]
fn public_streaming_api_matches_complete_publication_at_every_split() {
    let generation = generation("SecRule ARGS \"@pm attack\" \"id:1,phase:2,deny\"");
    let body = br#"{"x":"gone","x":"attack","items":[1,true]}"#;
    let mut ordinary = transaction(&generation, RuntimeLimits::default());
    ordinary
        .publish_request_body(RequestBody::new(body, BodyProcessorSelection::Auto))
        .unwrap();
    let expected = ordinary.execute_request_body().unwrap();
    for split in 0..=body.len() {
        let mut tx = transaction(&generation, RuntimeLimits::default());
        tx.start_json_request_body(BodyProcessorSelection::Auto, limits())
            .unwrap();
        tx.feed_json_request_body(JsonStreamInput::Data(&body[..split]))
            .unwrap();
        tx.feed_json_request_body(JsonStreamInput::Data(&body[split..]))
            .unwrap();
        tx.feed_json_request_body(JsonStreamInput::Complete).unwrap();
        assert_eq!(tx.finish_json_request_body().unwrap(), expected);
        assert_eq!(tx.retained_bytes(), ordinary.retained_bytes());
    }
}

#[test]
fn initial_unsupported_allows_ordinary_publication_but_budget_failure_is_distinct() {
    let unsupported = generation("SecRule REQUEST_BODY \"@contains attack\" \"id:1,phase:2,deny\"");
    let mut tx = transaction(&unsupported, RuntimeLimits::default());
    assert_eq!(
        tx.start_json_request_body(BodyProcessorSelection::Auto, limits()),
        Err(JsonExecutionError::Unsupported)
    );
    tx.publish_request_body(RequestBody::new(b"{}", BodyProcessorSelection::Auto))
        .unwrap();
    tx.execute_request_body().unwrap();

    let supported = generation("SecRule ARGS \"@pm attack\" \"id:1,phase:2,pass\"");
    let mut tx = transaction(&supported, RuntimeLimits::default());
    let mut caps = limits();
    caps.plan.max_subscribers = 0;
    assert_eq!(
        tx.start_json_request_body(BodyProcessorSelection::Auto, caps),
        Err(JsonExecutionError::Limit {
            kind: JsonExecutionLimit::Subscribers,
            limit: 0
        })
    );
}

#[test]
fn sticky_source_error_survives_unsupported_restart_until_new_whole_source() {
    let generation = generation("SecRule ARGS \"@pm attack\" \"id:1,phase:2,pass\"");
    let mut tx = transaction(&generation, RuntimeLimits::default());
    let mut caps = limits();
    caps.lexical.max_source_bytes = 2;
    tx.start_json_request_body(BodyProcessorSelection::Auto, caps).unwrap();
    let error = JsonExecutionError::Body(JsonBodyError::SourceByteLimit { limit: 2 });
    assert_eq!(tx.feed_json_request_body(JsonStreamInput::Data(b"[null]")), Err(error));
    assert_eq!(tx.finish_json_request_body(), Err(error));
    assert_eq!(
        tx.start_json_request_body(BodyProcessorSelection::Xml, limits()),
        Err(JsonExecutionError::Unsupported)
    );
    assert!(tx
        .publish_request_body(RequestBody::new(b"{}", BodyProcessorSelection::Auto))
        .is_err());
    assert_eq!(tx.feed_json_request_body(JsonStreamInput::Complete), Err(error));
    let _: &dyn std::error::Error = &error;
    tx.start_json_request_body(BodyProcessorSelection::Auto, limits())
        .unwrap();
    tx.feed_json_request_body(JsonStreamInput::Data(b"{}")).unwrap();
    tx.feed_json_request_body(JsonStreamInput::Complete).unwrap();
    tx.finish_json_request_body().unwrap();
}

#[test]
fn completion_seals_before_publication_and_rejects_trailing_input() {
    let generation = generation("SecRule ARGS \"@pm attack\" \"id:1,phase:2,pass\"");
    let mut tx = transaction(&generation, RuntimeLimits::default());
    tx.start_json_request_body(BodyProcessorSelection::Auto, limits())
        .unwrap();
    assert_eq!(tx.finish_json_request_body(), Err(JsonExecutionError::State));
    tx.start_json_request_body(BodyProcessorSelection::Auto, limits())
        .unwrap();
    tx.feed_json_request_body(JsonStreamInput::Complete).unwrap();
    assert_eq!(
        tx.feed_json_request_body(JsonStreamInput::Data(b"")),
        Err(JsonExecutionError::AlreadyComplete)
    );
    assert_eq!(tx.finish_json_request_body(), Err(JsonExecutionError::AlreadyComplete));
    tx.start_json_request_body(BodyProcessorSelection::Auto, limits())
        .unwrap();
    tx.feed_json_request_body(JsonStreamInput::Complete).unwrap();
    tx.finish_json_request_body().unwrap();
    let retained = tx.retained_bytes();
    assert_eq!(
        tx.feed_json_request_body(JsonStreamInput::Abandoned),
        Err(JsonExecutionError::AlreadyComplete)
    );
    assert_eq!(tx.finish_json_request_body(), Err(JsonExecutionError::AlreadyComplete));
    assert_eq!(tx.retained_bytes(), retained);
    assert_eq!(tx.next_phase(), Some(Phase::ResponseHeaders));
}

#[test]
fn scheduled_failure_retries_through_ordinary_phase_api() {
    let generation = generation("SecRule ARGS \"@pm attack\" \"id:1,phase:2,pass\"");
    let runtime = RuntimeLimits {
        max_selected_values: 0,
        ..RuntimeLimits::default()
    };
    let mut ordinary = transaction(&generation, runtime);
    ordinary
        .publish_request_body(RequestBody::new(b"{}", BodyProcessorSelection::Auto))
        .unwrap();
    let expected = ordinary.execute_request_body().unwrap_err();
    let mut tx = transaction(&generation, runtime);
    tx.start_json_request_body(BodyProcessorSelection::Auto, limits())
        .unwrap();
    tx.feed_json_request_body(JsonStreamInput::Data(b"{}")).unwrap();
    tx.feed_json_request_body(JsonStreamInput::Complete).unwrap();
    assert_eq!(
        tx.finish_json_request_body(),
        Err(JsonExecutionError::Runtime(expected))
    );
    assert_eq!(tx.execute_request_body(), Err(expected));
    assert_eq!(tx.retained_bytes(), ordinary.retained_bytes());
}
