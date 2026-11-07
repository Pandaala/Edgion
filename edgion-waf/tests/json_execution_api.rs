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
    let defaults = [
        PhaseDefaults::pass(Phase::RequestHeaders, true, true),
        PhaseDefaults::pass(Phase::RequestBody, true, true),
    ];
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

#[test]
fn scoped_json_chunks_preserve_captures_order_and_atomic_publication() {
    let generation = generation(concat!(
        "SecRule ARGS \"@rx (?P<word>hit)\" \"id:1,phase:2,pass,capture,chain,t:none\"\n",
        "SecRule TX:word \"@streq hit\" \"setvar:tx.saved=%{TX.word}\"\n",
        "SecRule &ARGS \"@eq 4\" \"id:2,phase:2,pass\"\n",
    ));
    let input = Bytes::from_static(br#"{"a":"gone","a":"hit","x.y":"miss","x":{"y":"hit"}}"#);
    let mut owned = transaction(&generation, RuntimeLimits::default());
    owned
        .publish_request_body(RequestBody::new(input.clone(), BodyProcessorSelection::Json))
        .unwrap();
    let expected = owned.execute_request_body().unwrap();
    for chunk_size in [1, 2, 7, 64] {
        let mut tx = transaction(&generation, RuntimeLimits::default());
        let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
        for chunk in input.chunks(chunk_size) {
            inspection.feed(chunk).unwrap();
        }
        assert_eq!(inspection.usage().inspected_source_bytes, input.len());
        inspection.complete(input.len()).unwrap();
        assert!(!inspection.requires_raw_source());
        assert!(inspection.feed(b"").is_err());
        inspection.finish(None).unwrap();
        assert_eq!(tx.execute_request_body().unwrap(), expected);
        for key in [b"word".as_slice(), b"0", b"1", b"saved"] {
            assert_eq!(tx.tx_value(key), owned.tx_value(key));
        }
        assert_eq!(tx.next_phase(), owned.next_phase());
        assert_eq!(tx.request_body_error(), owned.request_body_error());
    }
}

#[test]
fn scoped_empty_json_is_lazy_and_drop_abandons_unpublished_input() {
    let generation = generation("SecAction \"id:1,phase:2,pass,setvar:tx.done=yes\"");
    let mut tx = transaction(&generation, RuntimeLimits::default());
    let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
    inspection.feed(b"").unwrap();
    assert_eq!(inspection.usage(), JsonInspectionUsage::default());
    inspection.complete(0).unwrap();
    inspection.finish(None).unwrap();
    tx.execute_request_body().unwrap();
    assert_eq!(tx.tx_value(b"done"), Some(b"yes".as_slice()));
    for data in [b"".as_slice(), b"{\"a\":\"hit\""] {
        let mut tx = transaction(&generation, RuntimeLimits::default());
        let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
        inspection.feed(data).unwrap();
        drop(inspection);
        let error = tx.execute_request_body().unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Lexical(JsonStreamError::Abandoned)
            }
        ));
        assert_eq!(tx.preflight_request_body(BodyProcessorSelection::Json), Err(error));
    }
}

#[test]
fn scoped_completion_is_sticky_and_complete_prelude_precedes_latched_syntax() {
    let generation = generation("");
    let input = b"{] later";
    let limits = RuntimeLimits {
        body: BodyLimits {
            max_work_bytes: input.len() - 1,
            ..BodyLimits::default()
        },
        ..RuntimeLimits::default()
    };
    let mut tx = transaction(&generation, limits);
    let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
    assert!(inspection.feed(input).is_err());
    let inspected = inspection.usage().inspected_source_bytes;
    assert!(inspected < input.len());
    assert!(inspection.feed(b"more").is_err());
    assert_eq!(inspection.usage().inspected_source_bytes, inspected);
    let error = inspection.complete(input.len()).unwrap_err();
    assert_eq!(
        error,
        RuntimeError::Body {
            kind: BodyError::WorkLimit { limit: input.len() - 1 }
        }
    );
    assert_eq!(inspection.complete(0), Err(error));
    assert_eq!(inspection.feed(b""), Err(error));
    assert_eq!(inspection.finish(None), Err(error));
    assert_eq!(tx.execute_request_body(), Err(error));
}

#[test]
fn scoped_source_is_required_only_by_actual_raw_visibility_and_has_exact_length() {
    let visible = generation(concat!(
        "SecAction \"id:1,phase:1,pass,ctl:forceRequestBodyVariable=On\"\n",
        "SecRule REQUEST_BODY \"@contains hit\" \"id:2,phase:2,pass\"\n",
    ));
    let input = Bytes::from_static(br#"{"a":"hit"}"#);
    for source in [None, Some(Bytes::from_static(b"short")), Some(input.clone())] {
        let succeeds = source.as_ref().is_some_and(|source| source.len() == input.len());
        let mut tx = transaction(&visible, RuntimeLimits::default());
        let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
        inspection.feed(&input).unwrap();
        inspection.complete(input.len()).unwrap();
        assert!(inspection.requires_raw_source());
        assert_eq!(inspection.finish(source).is_ok(), succeeds);
        if succeeds {
            assert_eq!(tx.execute_request_body().unwrap().findings().len(), 1);
        } else {
            assert_eq!(tx.execute_request_body(), Err(RuntimeError::JsonInspectionState));
        }
    }
    let hidden = generation("SecRule REQUEST_BODY \"@contains hit\" \"id:2,phase:2,pass\"");
    let mut tx = transaction(&hidden, RuntimeLimits::default());
    let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
    inspection.feed(&input).unwrap();
    inspection.complete(input.len()).unwrap();
    assert!(!inspection.requires_raw_source());
    inspection.finish(None).unwrap();
    assert!(tx.execute_request_body().unwrap().findings().is_empty());
}

#[test]
fn scoped_detection_only_syntax_stop_keeps_complete_length_and_raw_fallback() {
    let input = Bytes::from_static(b"{] remaining tail");
    let generation = generation(&format!("SecRule REQUEST_BODY \"@contains tail\" \"id:1,phase:2,pass,setvar:tx.raw=yes\"\nSecRule REQUEST_BODY_LENGTH \"@eq {}\" \"id:2,phase:2,pass,setvar:tx.length=yes\"", input.len()));
    let headers = [RequestHeader::new(b"content-type", b"application/json")];
    let head = RequestHead::new(
        b"POST / HTTP/1.1",
        b"POST",
        b"/",
        b"/",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"json-api",
        &headers,
    );
    let mut tx = Transaction::new_with_policy(
        &generation,
        head,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
        RuleCheckPolicy::default(),
    )
    .unwrap();
    tx.execute_request_headers().unwrap();
    let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
    inspection.feed(&input[..2]).unwrap();
    let inspected = inspection.usage().inspected_source_bytes;
    inspection.feed(&input[2..]).unwrap();
    assert_eq!(inspection.usage().inspected_source_bytes, inspected);
    assert!(inspected < input.len());
    inspection.complete(input.len()).unwrap();
    assert!(inspection.requires_raw_source());
    inspection.finish(Some(input)).unwrap();
    tx.execute_request_body().unwrap();
    assert_eq!(tx.tx_value(b"raw"), Some(b"yes".as_slice()));
    assert_eq!(tx.tx_value(b"length"), Some(b"yes".as_slice()));
    assert!(matches!(tx.request_body_error(), Some(BodyError::Malformed { .. })));
}

fn run_json_chunks<'g>(
    generation: &'g CompiledGeneration,
    input: &[u8],
    chunks: &[usize],
) -> (Transaction<'g>, PhaseOutcome) {
    let mut tx = transaction(generation, RuntimeLimits::default());
    let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
    let mut start = 0;
    for &end in chunks.iter().chain([input.len()].iter()) {
        inspection.feed(&input[start..end]).unwrap();
        start = end;
    }
    inspection.complete(input.len()).unwrap();
    inspection.finish(None).unwrap();
    let outcome = tx.execute_request_body().unwrap();
    (tx, outcome)
}

#[test]
fn duplicate_json_keys_publish_every_occurrence_at_every_chunk_split() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,setvar:'tx.count=0',setvar:'tx.names=0'\"\n",
        "SecRule ARGS \"@detectSQLi\" \"id:2,phase:2,pass,setvar:'tx.sqli=%{MATCHED_VAR}'\"\n",
        "SecRule ARGS:json.q \"@rx .\" \"id:3,phase:2,pass,setvar:'tx.value_%{TX.count}=%{MATCHED_VAR}',setvar:'tx.count=+1'\"\n",
        // The query string contributes `q=miss`, so each count includes it.
        "SecRule &ARGS \"@eq 3\" \"id:4,phase:2,pass\"\n",
        "SecRule &ARGS_NAMES \"@eq 3\" \"id:5,phase:2,pass\"\n",
        "SecRule ARGS_NAMES \"@streq json.q\" \"id:6,phase:2,pass,setvar:'tx.names=+1'\"\n",
    ));
    let payload = b"1' UNION SELECT password FROM users--".as_slice();
    let input = br#"{"q":"1' UNION SELECT password FROM users--","q":"x"}"#;
    let mut owned = transaction(&generation, RuntimeLimits::default());
    owned
        .publish_request_body(RequestBody::new(
            Bytes::from_static(input),
            BodyProcessorSelection::Json,
        ))
        .unwrap();
    let expected = owned.execute_request_body().unwrap();
    let ids = expected
        .findings()
        .iter()
        .map(|finding| finding.rule_id())
        .collect::<Vec<_>>();
    assert_eq!(ids, [2, 3, 4, 5, 6]);
    assert_eq!(owned.tx_value(b"sqli"), Some(payload));
    assert_eq!(owned.tx_value(b"value_0"), Some(payload));
    assert_eq!(owned.tx_value(b"value_1"), Some(b"x".as_slice()));
    assert_eq!(owned.tx_value(b"count"), Some(b"2".as_slice()));
    assert_eq!(owned.tx_value(b"names"), Some(b"2".as_slice()));
    let splits = (0..=input.len()).map(|split| vec![split]);
    let single_bytes = std::iter::once((1..input.len()).collect::<Vec<_>>());
    for chunks in splits.chain(single_bytes) {
        let (tx, outcome) = run_json_chunks(&generation, input, &chunks);
        assert_eq!(outcome, expected, "{chunks:?}");
        for key in [b"sqli".as_slice(), b"value_0", b"value_1", b"count", b"names"] {
            assert_eq!(tx.tx_value(key), owned.tx_value(key), "{chunks:?}");
        }
    }
}

#[test]
fn nested_duplicate_objects_keep_every_member_and_arrays_are_unchanged() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,setvar:'tx.count=0'\"\n",
        "SecRule ARGS \"@rx ^[a-z]+$\" \"id:2,phase:2,pass,setvar:'tx.name_%{TX.count}=%{MATCHED_VAR_NAME}',setvar:'tx.value_%{TX.count}=%{MATCHED_VAR}',setvar:'tx.count=+1'\"\n",
    ));
    let input = br#"{"o":{"a":"first"},"o":{"a":"second","b":"x"},"arr":["s","t"]}"#;
    let expected = [
        ("ARGS:q", "miss"),
        ("ARGS:json.arr.0", "s"),
        ("ARGS:json.arr.1", "t"),
        ("ARGS:json.o.a", "first"),
        ("ARGS:json.o.a", "second"),
        ("ARGS:json.o.b", "x"),
    ];
    // Every two-part split plus single-byte chunks, as for flat duplicates.
    let splits = (0..=input.len()).map(|split| vec![split]);
    let single_bytes = std::iter::once((1..input.len()).collect::<Vec<_>>());
    for chunks in splits.chain(single_bytes) {
        let (tx, _) = run_json_chunks(&generation, input, &chunks);
        assert_eq!(
            tx.tx_value(b"count"),
            Some(expected.len().to_string().as_bytes()),
            "{chunks:?}"
        );
        for (index, (name, value)) in expected.iter().enumerate() {
            assert_eq!(
                tx.tx_value(format!("name_{index}").as_bytes()),
                Some(name.as_bytes()),
                "{chunks:?}"
            );
            assert_eq!(
                tx.tx_value(format!("value_{index}").as_bytes()),
                Some(value.as_bytes()),
                "{chunks:?}"
            );
        }
    }
}
