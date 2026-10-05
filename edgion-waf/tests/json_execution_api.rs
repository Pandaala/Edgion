//! External-consumer coverage for the bounded JSON transaction API.
use bytes::Bytes;
use edgion_waf::seclang::*;

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
fn public_owned_default_and_explicit_limits_match() {
    let generation = generation("SecRule ARGS \"@pm attack\" \"id:1,phase:2,deny\"");
    let body = RequestBody::new(
        Bytes::from_static(br#"{"x":"gone","x":"attack","items":[1,true]}"#),
        BodyProcessorSelection::Auto,
    );
    let mut default = transaction(&generation, RuntimeLimits::default());
    let mut explicit = transaction(&generation, RuntimeLimits::default());
    assert_eq!(
        default.preflight_request_body(body.processor()),
        Ok(BodyProcessorSelection::Json)
    );
    default.publish_request_body(body.clone()).unwrap();
    explicit
        .publish_request_body_with_json_limits(body, JsonInspectionLimits::default())
        .unwrap();
    assert_eq!(default.execute_request_body(), explicit.execute_request_body());
    assert_eq!(default.retained_bytes(), explicit.retained_bytes());
}

#[test]
fn physical_failure_is_redacted_and_sticky_without_fallback() {
    let generation = generation("SecRule ARGS \"@pm attack\" \"id:1,phase:2,pass\"");
    let mut tx = transaction(&generation, RuntimeLimits::default());
    let limits = JsonInspectionLimits {
        results: JsonResultLimits {
            retained_bytes: 0,
            ..JsonResultLimits::default()
        },
        ..JsonInspectionLimits::default()
    };
    let error = tx
        .publish_request_body_with_json_limits(
            RequestBody::new(Bytes::from_static(b"{}"), BodyProcessorSelection::Auto),
            limits,
        )
        .unwrap_err();
    let _: &dyn std::error::Error = &error;
    assert!(!format!("{error:?} {error}").contains("attack"));
    assert_eq!(tx.preflight_request_body(BodyProcessorSelection::Raw), Err(error));
    assert_eq!(
        tx.publish_request_body(RequestBody::new(Bytes::new(), BodyProcessorSelection::Raw)),
        Err(error)
    );
    assert_eq!(tx.execute_request_body(), Err(error));
}

#[test]
fn publication_is_atomic_and_duplicate_publication_is_rejected() {
    let generation = generation("");
    let mut tx = transaction(&generation, RuntimeLimits::default());
    tx.publish_request_body(RequestBody::new(
        Bytes::from_static(b"{}"),
        BodyProcessorSelection::Auto,
    ))
    .unwrap();
    assert_eq!(
        tx.preflight_request_body(BodyProcessorSelection::Json),
        Err(RuntimeError::RequestBodyAlreadyPublished)
    );
    assert_eq!(
        tx.publish_request_body(RequestBody::new(Bytes::new(), BodyProcessorSelection::Raw)),
        Err(RuntimeError::RequestBodyAlreadyPublished)
    );
    tx.execute_request_body().unwrap();
}

#[test]
fn scheduled_semantic_failure_retries_through_ordinary_phase_api() {
    let generation = generation(
        "SecAction \"id:1,phase:2,pass,setvar:tx.attempt=+1\"\nSecRule ARGS \"@pm attack\" \"id:2,phase:2,pass\"",
    );
    let mut tx = transaction(
        &generation,
        RuntimeLimits {
            max_selected_values: 0,
            ..RuntimeLimits::default()
        },
    );
    tx.publish_request_body(RequestBody::new(
        Bytes::from_static(b"{}"),
        BodyProcessorSelection::Auto,
    ))
    .unwrap();
    let first = tx.execute_request_body().unwrap_err();
    assert_eq!(tx.tx_value(b"attempt"), Some(b"1".as_slice()));
    assert_eq!(tx.execute_request_body(), Err(first));
    assert_eq!(tx.tx_value(b"attempt"), Some(b"2".as_slice()));
}
