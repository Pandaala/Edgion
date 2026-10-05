use super::*;
use crate::seclang::{
    compile_with_includes, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink, IncludeSource, PhaseDefaults,
    SemanticValidationContext,
};

struct NoIncludes;

impl IncludeSource for NoIncludes {
    fn load(&mut self, _request: IncludeRequest<'_>, _sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

fn generation(source: &str) -> CompiledGeneration {
    generation_with_frontend_limits(source, FrontendLimits::default())
}

fn generation_with_frontend_limits(source: &str, limits: FrontendLimits) -> CompiledGeneration {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap();
    compile_with_includes(
        "runtime-fixture",
        source.as_bytes(),
        &mut NoIncludes,
        None,
        context,
        limits,
    )
    .unwrap()
    .into_parts()
    .0
}

fn request<'a>(headers: &'a [RequestHeader<'a>], uri: &'a [u8]) -> RequestHead<'a> {
    RequestHead::new(
        b"GET /search?a=one&a=two%20words HTTP/1.1",
        b"GET",
        uri,
        uri,
        b"HTTP/1.1",
        b"192.0.2.1",
        b"request-1",
        headers,
    )
}

#[test]
fn shared_generation_transaction_outlives_the_callers_handle() {
    let generation = Arc::new(generation(
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:1,phase:1,pass\"\n",
    ));
    let mut transaction =
        Transaction::new_shared(Arc::clone(&generation), request(&[], b"/"), RuntimeLimits::default()).unwrap();
    drop(generation);

    assert_eq!(
        transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
        1
    );
}

#[test]
fn finding_storage_is_lazy_and_grows_only_after_matches() {
    let mut clean_source = String::new();
    let mut matching_source = String::new();
    for id in 1..=64 {
        clean_source.push_str(&format!(
            "SecRule REQUEST_URI \"@streq /never-{id}\" \"id:{id},phase:1,pass\"\n"
        ));
        matching_source.push_str(&format!(
            "SecRule REQUEST_URI \"@contains /\" \"id:{id},phase:1,pass\"\n"
        ));
    }

    let clean_generation = generation(&clean_source);
    let mut clean = Transaction::new(&clean_generation, request(&[], b"/clean"), RuntimeLimits::default()).unwrap();
    let clean_outcome = clean.execute_request_headers().unwrap();
    assert!(clean_outcome.findings.is_empty());
    assert_eq!(clean_outcome.findings.capacity(), 0);

    let matching_generation = generation(&matching_source);
    let limits = RuntimeLimits {
        max_findings: 17,
        ..RuntimeLimits::default()
    };
    let mut matching = Transaction::new(&matching_generation, request(&[], b"/match"), limits).unwrap();
    let matching_outcome = matching.execute_request_headers().unwrap();
    assert_eq!(matching_outcome.findings.len(), 17);
    assert_eq!(matching_outcome.omitted_findings, 47);
    assert_eq!(matching_outcome.omitted_loggable_findings, 47);
    assert!(matching_outcome.findings.capacity() >= matching_outcome.findings.len());
    assert!(matching_outcome.findings.capacity() <= 32);

    let silent_generation =
        generation("SecRule REQUEST_URI \"@contains /\" \"id:100,phase:1,pass,nolog,noauditlog\"\n");
    let silent_limits = RuntimeLimits {
        max_findings: 0,
        ..RuntimeLimits::default()
    };
    let mut silent = Transaction::new(&silent_generation, request(&[], b"/match"), silent_limits).unwrap();
    let silent_outcome = silent.execute_request_headers().unwrap();
    assert_eq!(silent_outcome.omitted_findings, 1);
    assert_eq!(silent_outcome.omitted_loggable_findings, 0);
    assert_eq!(silent_outcome.findings.capacity(), 0);
}

#[test]
fn request_state_preserves_query_header_and_cookie_order() {
    let headers = [
        RequestHeader::new(b"X-Test", b"first"),
        RequestHeader::new(b"Cookie", b"a=one; b=two"),
        RequestHeader::new(b"X-Test", b"second"),
        RequestHeader::new(b"cookie", b"a=three"),
    ];
    let state = RequestState::build(
        request(&headers, b"/search?a=one&a=two%20words"),
        RuntimeLimits::default(),
    )
    .unwrap();
    assert_eq!(
        state.collection(CollectionKind::ArgsGet).collect::<Vec<_>>(),
        [
            (Some(b"a".as_slice()), b"one".as_slice()),
            (Some(b"a".as_slice()), b"two words".as_slice()),
        ]
    );
    assert_eq!(
        state.collection(CollectionKind::RequestCookies).collect::<Vec<_>>(),
        [
            (Some(b"a".as_slice()), b"one".as_slice()),
            (Some(b"b".as_slice()), b"two".as_slice()),
            (Some(b"a".as_slice()), b"three".as_slice()),
        ]
    );
    assert_eq!(
        state.collection(CollectionKind::RequestHeaders).collect::<Vec<_>>(),
        [
            (Some(b"X-Test".as_slice()), b"first".as_slice()),
            (Some(b"Cookie".as_slice()), b"a=one; b=two".as_slice()),
            (Some(b"X-Test".as_slice()), b"second".as_slice()),
            (Some(b"cookie".as_slice()), b"a=three".as_slice()),
        ]
    );
    let decoded_path = RequestState::build(
        request(&[], b"/get/php%20call%28%24x%29+tail?value=1"),
        RuntimeLimits::default(),
    )
    .unwrap();
    assert_eq!(
        decoded_path
            .collection(CollectionKind::RequestFilename)
            .collect::<Vec<_>>(),
        [(None, b"/get/php call($x)+tail".as_slice())]
    );
    assert_eq!(
        decoded_path
            .collection(CollectionKind::RequestBasename)
            .collect::<Vec<_>>(),
        [(None, b"php call($x)+tail".as_slice())]
    );
}

#[test]
fn every_count_and_byte_limit_rejects_first_excess() {
    let headers = [RequestHeader::new(b"X", b"1"), RequestHeader::new(b"Y", b"2")];
    assert!(matches!(
        RequestState::build(
            request(&headers, b"/?a=1"),
            RuntimeLimits {
                max_headers: 1,
                ..RuntimeLimits::default()
            }
        ),
        Err(RuntimeError::HeaderLimit { actual: 2, limit: 1 })
    ));
    assert!(matches!(
        RequestState::build(
            request(&[], b"/?a=1&b=2"),
            RuntimeLimits {
                max_query_arguments: 1,
                ..RuntimeLimits::default()
            }
        ),
        Err(RuntimeError::QueryArgumentLimit { limit: 1 })
    ));
    let cookies = [RequestHeader::new(b"Cookie", b"a=1;b=2")];
    assert!(matches!(
        RequestState::build(
            request(&cookies, b"/"),
            RuntimeLimits {
                max_cookies: 1,
                ..RuntimeLimits::default()
            }
        ),
        Err(RuntimeError::CookieLimit { limit: 1 })
    ));
    let exact = RequestState::build(request(&headers[..1], b"/"), RuntimeLimits::default()).unwrap();
    assert!(matches!(
        RequestState::build(
            request(&headers[..1], b"/"),
            RuntimeLimits {
                max_retained_bytes: exact.retained_bytes - 1,
                ..RuntimeLimits::default()
            }
        ),
        Err(RuntimeError::RetainedByteLimit { .. })
    ));
}

#[test]
fn invalid_headers_and_errors_are_content_free() {
    let private = [RequestHeader::new(b"Bad Name", b"private-value")];
    let Err(error) = RequestState::build(request(&private, b"/private-path"), RuntimeLimits::default()) else {
        panic!("private fixture must fail header validation");
    };
    assert_eq!(error, RuntimeError::InvalidHeaderName { ordinal: 0 });
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("private"));
    assert!(!rendered.contains("Bad Name"));
}

#[test]
fn phase_cursor_is_monotonic() {
    assert_eq!(phase_from_number(1), Some(Phase::RequestHeaders));
    assert_eq!(phase_from_number(5), Some(Phase::Logging));
    assert_eq!(phase_from_number(6), None);
}

#[test]
fn request_header_phase_executes_targets_transforms_and_operators_in_order() {
    let generation = generation(concat!(
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:1,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Test \"@pm first other\" \"id:2,phase:1,pass,t:lowercase\"\n",
        "SecRule &REQUEST_HEADERS \"@ge 2\" \"id:3,phase:1,pass\"\n",
        "SecRule REQUEST_URI \"@rx ^/private\" \"id:4,phase:1,deny,status:451\"\n",
    ));
    let headers = [
        RequestHeader::new(b"x-test", b"FIRST"),
        RequestHeader::new(b"Host", b"example.test"),
    ];
    let mut transaction = Transaction::new(
        &generation,
        request(&headers, b"/private?a=1"),
        RuntimeLimits::default(),
    )
    .unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    assert_eq!(outcome.intervention().map(Intervention::status), Some(451));
    assert_eq!(outcome.evaluated_units(), 4);
    assert_eq!(transaction.next_phase(), Some(Phase::RequestBody));
    assert!(matches!(
        transaction.execute_request_headers(),
        Err(RuntimeError::PhaseOrder {
            expected: Some(Phase::RequestBody),
            actual: Phase::RequestHeaders,
        })
    ));
}

#[test]
fn detection_only_reports_deny_rules_without_short_circuiting_the_phase() {
    let generation = generation(concat!(
        "SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,deny,status:451\"\n",
        "SecRule REQUEST_URI \"@contains private\" \"id:2,phase:1,deny\"\n",
        "SecRule REQUEST_URI \"@contains private\" \"id:3,phase:1,pass\"\n",
    ));
    let mut transaction = Transaction::new_with_modes(
        &generation,
        request(&[], b"/private"),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
    )
    .unwrap();

    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(outcome.intervention(), None);
    assert_eq!(outcome.evaluated_units(), 3);
}

#[test]
fn negation_selector_exclusion_and_validation_operators_are_exact() {
    let generation = generation(concat!(
        "SecRule REQUEST_HEADERS|!REQUEST_HEADERS:Ignore \"!@contains forbidden\" \"id:10,phase:1,pass\"\n",
        "SecRule REQUEST_URI \"@validateUrlEncoding\" \"id:11,phase:1,pass\"\n",
        "SecRule REMOTE_ADDR \"@ipMatch 192.0.2.0/24\" \"id:12,phase:1,pass\"\n",
    ));
    let headers = [
        RequestHeader::new(b"Ignore", b"forbidden"),
        RequestHeader::new(b"Keep", b"safe"),
    ];
    let mut transaction = Transaction::new(
        &generation,
        request(&headers, b"/path?bad=%Q0"),
        RuntimeLimits::default(),
    )
    .unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [10, 11, 12]
    );
}

#[test]
fn setvar_actions_are_ordered_visible_and_case_insensitive() {
    let generation = generation(concat!(
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:20,phase:1,pass,setvar:'tx.score=1',setvar:'tx.score=+2',setvar:'tx.copy=%{tx.score}'\"\n",
            "SecRule REQUEST_HEADERS:X-Score \"@eq %{TX.score}\" \"id:21,phase:1,pass,setvar:'!tx.copy'\"\n",
        ));
    let headers = [RequestHeader::new(b"X-Score", b"3")];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [20, 21]
    );
    assert_eq!(transaction.tx_value(b"SCORE"), Some(b"3".as_slice()));
    assert_eq!(transaction.tx_value(b"copy"), None);
}

#[test]
fn failed_chain_and_failed_action_batch_do_not_leak_tx_mutations() {
    let failed_chain = generation(concat!(
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:22,phase:1,pass,chain,setvar:'tx.leak=1'\"\n",
        "SecRule REQUEST_URI \"@streq /never\"\n",
    ));
    let mut transaction = Transaction::new(&failed_chain, request(&[], b"/actual"), RuntimeLimits::default()).unwrap();
    assert!(transaction.execute_request_headers().unwrap().findings().is_empty());
    assert_eq!(transaction.tx_value(b"leak"), None);

    let overflow = generation(
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:23,phase:1,pass,setvar:'tx.score=1',setvar:'tx.score=+9223372036854775807'\"\n",
        );
    let mut transaction = Transaction::new(&overflow, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    assert!(matches!(
        transaction.execute_request_headers(),
        Err(RuntimeError::IntegerConversion)
    ));
    assert_eq!(transaction.tx_value(b"score"), None);
    assert_eq!(transaction.next_phase(), Some(Phase::RequestHeaders));
}

#[test]
fn matcher_scratch_limit_is_checked_before_transaction_publication() {
    let generation = generation("SecRule REQUEST_URI \"@pm one two\" \"id:30,phase:1,pass\"\n");
    let baseline = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    assert!(baseline.scratch_bytes > 0);
    assert!(matches!(
        Transaction::new(
            &generation,
            request(&[], b"/"),
            RuntimeLimits {
                max_matcher_scratch_bytes: baseline.scratch_bytes - 1,
                ..RuntimeLimits::default()
            },
        ),
        Err(RuntimeError::MatcherScratchByteLimit { .. })
    ));
}

#[test]
fn skip_and_skip_after_move_only_the_phase_schedule_cursor() {
    let generation = generation(concat!(
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:40,phase:1,pass,skip:1\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:41,phase:1,pass\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:42,phase:1,pass,skipAfter:END\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:43,phase:1,pass\"\n",
        "SecMarker END\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:44,phase:1,pass\"\n",
    ));
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [40, 42, 44]
    );
    assert_eq!(outcome.evaluated_units(), 3);
}

#[test]
fn regex_and_phrase_captures_publish_bounded_tx_values_and_replace_prior_keys() {
    let generation = generation(concat!(
            "SecRule REQUEST_URI \"@rx ^/user/([a-z]+)-(?P<num>[0-9]+)\" \"id:50,phase:1,pass,capture,setvar:'tx.copy=%{TX.1}'\"\n",
            "SecRule REQUEST_HEADERS:X-Token \"@pm token\" \"id:51,phase:1,pass,capture\"\n",
        ));
    let headers = [RequestHeader::new(b"X-Token", b"prefix TOKEN suffix")];
    let mut transaction = Transaction::new(
        &generation,
        request(&headers, b"/user/alice-42"),
        RuntimeLimits::default(),
    )
    .unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [50, 51]
    );
    assert_eq!(transaction.tx_value(b"copy"), Some(b"alice".as_slice()));
    assert_eq!(transaction.tx_value(b"0"), Some(b"TOKEN".as_slice()));
    assert_eq!(transaction.tx_value(b"1"), None);
    assert_eq!(transaction.tx_value(b"num"), None);
}

#[test]
fn injection_operators_compile_transform_capture_and_chain_atomically() {
    let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:101,phase:1,pass,t:urlDecodeUni,capture,chain,setvar:'tx.fingerprint=%{TX.0}'\"\n",
            "SecRule TX:0 \"@streq 1&1\"\n",
            "SecRule REQUEST_HEADERS:X-XSS \"@detectXSS\" \"id:102,phase:1,pass,capture,setvar:'tx.previous=%{TX.0}'\"\n",
        ));
    let headers = [
        RequestHeader::new(b"X-SQL", b"1%20OR%201%3D1"),
        RequestHeader::new(b"X-XSS", b"<script>alert(1)</script>"),
    ];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [101, 102]
    );
    assert_eq!(transaction.tx_value(b"0"), Some(b"1&1".as_slice()));
    assert_eq!(transaction.tx_value(b"fingerprint"), Some(b"1&1".as_slice()));
    assert_eq!(transaction.tx_value(b"previous"), Some(b"1&1".as_slice()));

    let failed = self::generation(concat!(
        "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:103,phase:1,pass,capture,chain\"\n",
        "SecRule REQUEST_METHOD \"@streq POST\"\n",
    ));
    let mut transaction = Transaction::new(&failed, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    assert!(transaction.execute_request_headers().unwrap().findings().is_empty());
    assert_eq!(transaction.tx_value(b"0"), None);
}

#[test]
fn negated_detection_does_not_capture_and_detection_work_is_bounded() {
    let generation = generation("SecRule REQUEST_HEADERS:X-SQL \"!@detectSQLi\" \"id:104,phase:1,pass,capture\"\n");
    let headers = [RequestHeader::new(b"X-SQL", b"safe")];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    assert_eq!(
        transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
        104
    );
    assert_eq!(transaction.tx_value(b"0"), None);

    let budget_generation = self::generation("SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:106,phase:1,pass\"\n");
    let exact_work = 4096 + 10 * b"safe".len();
    let mut transaction = Transaction::new(
        &budget_generation,
        request(&headers, b"/"),
        RuntimeLimits {
            max_operator_work_bytes: exact_work,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    transaction.execute_request_headers().unwrap();
    let mut transaction = Transaction::new(
        &budget_generation,
        request(&headers, b"/"),
        RuntimeLimits {
            max_operator_work_bytes: exact_work - 1,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        transaction.execute_request_headers(),
        Err(RuntimeError::OperatorWorkLimit { .. })
    ));
}

#[test]
fn decoded_query_high_byte_does_not_hide_later_xss() {
    let generation = generation("SecRule ARGS_GET:x \"@detectXSS\" \"id:107,phase:1,pass\"\n");
    let mut transaction = Transaction::new(
        &generation,
        request(&[], b"/?x=%3Ca%20%FFid%3Dx%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E"),
        RuntimeLimits::default(),
    )
    .unwrap();
    assert_eq!(
        transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
        107
    );

    let generation = self::generation("SecRule ARGS_GET:x \"@detectSQLi\" \"id:108,phase:1,pass\"\n");
    let mut transaction = Transaction::new(
        &generation,
        request(&[], b"/?x=%27-(f=q%27%BF"),
        RuntimeLimits::default(),
    )
    .unwrap();
    assert_eq!(
        transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
        108
    );

    let mut transaction = Transaction::new(
        &generation,
        request(&[], b"/?x=1%20OR%20q%27%BF%C2%BF%27"),
        RuntimeLimits::default(),
    )
    .unwrap();
    assert_eq!(
        transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
        108
    );
}

#[test]
fn detector_work_accumulates_across_rules_and_phases() {
    let detector_work = crate::injection::work_cost(crate::injection::Kind::Sqli, b"safe".len()).unwrap();
    let one_rule_work = detector_work + 2 * b"safe".len();
    let headers = [RequestHeader::new(b"X-SQL", b"safe")];
    for rule_count in [1, 2, 100] {
        let source = (1..=rule_count)
            .map(|id| format!("SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:{id},phase:1,pass\"\n"))
            .collect::<String>();
        let generation = generation(&source);
        let limit = if rule_count == 1 {
            one_rule_work
        } else {
            (rule_count - 1) * detector_work + 2 * b"safe".len()
        };
        let mut transaction = Transaction::new(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits {
                max_operator_work_bytes: limit,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        let result = transaction.execute_request_headers();
        if rule_count == 1 {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert_eq!(result, Err(RuntimeError::OperatorWorkLimit { limit }));
        }
    }

    let generation = generation(concat!(
        "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:201,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:202,phase:2,pass\"\n",
    ));
    let mut transaction = Transaction::new(
        &generation,
        request(&headers, b"/"),
        RuntimeLimits {
            max_operator_work_bytes: one_rule_work,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    transaction.execute_request_headers().unwrap();
    assert_eq!(
        transaction.execute_request_body_prefix(b"", 1, RequestBodyPrefixCompletion::NotRequired),
        Err(RuntimeError::OperatorWorkLimit { limit: one_rule_work })
    );
}

#[test]
fn detector_work_accumulates_across_chain_members() {
    let generation = generation(concat!(
        "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:203,phase:1,pass,chain\"\n",
        "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\"\n",
    ));
    let value = b"1 OR 1=1";
    let headers = [RequestHeader::new(b"X-SQL", value)];
    let limit = crate::injection::work_cost(crate::injection::Kind::Sqli, value.len()).unwrap() + 2 * value.len();
    let mut transaction = Transaction::new(
        &generation,
        request(&headers, b"/"),
        RuntimeLimits {
            max_operator_work_bytes: limit,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        transaction.execute_request_headers(),
        Err(RuntimeError::OperatorWorkLimit { limit })
    );
    assert_eq!(transaction.tx_value(b"0"), None);
}

#[test]
fn detector_input_and_capture_failures_do_not_publish_tx_state() {
    let generation = generation("SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:105,phase:1,pass,capture\"\n");
    let headers = [RequestHeader::new(b"X-SQL", b"1 OR 1=1")];
    let mut transaction = Transaction::new(
        &generation,
        request(&headers, b"/"),
        RuntimeLimits {
            max_capture_entries: 0,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        transaction.execute_request_headers(),
        Err(RuntimeError::CaptureEntryLimit { .. })
    ));
    assert_eq!(transaction.tx_value(b"0"), None);

    let large = vec![b'a'; crate::injection::MAX_INPUT_BYTES + 1];
    let headers = [RequestHeader::new(b"X-SQL", &large)];
    let mut transaction = Transaction::new(
        &generation,
        request(&headers, b"/"),
        RuntimeLimits {
            max_operator_work_bytes: usize::MAX,
            max_header_bytes: crate::injection::MAX_INPUT_BYTES + 1024,
            max_value_bytes: crate::injection::MAX_INPUT_BYTES + 1,
            max_transformation_output_bytes: crate::injection::MAX_INPUT_BYTES + 1,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    let result = transaction.execute_request_headers();
    assert!(
        matches!(
            &result,
            Err(RuntimeError::DetectorInputLimit { limit }) if *limit == crate::injection::MAX_INPUT_BYTES
        ),
        "{result:?}"
    );
    assert_eq!(transaction.tx_value(b"0"), None);
}

#[test]
fn crs_extension_capture_is_visible_to_its_chain_child() {
    let direct = crate::matcher::CompiledCaptureRegexProgram::compile_one(
        r"\.([^.]+)$",
        crate::matcher::RegexProgramLimits::default(),
    )
    .unwrap();
    let mut direct_scratch = direct.create_scratch();
    let direct_captures = direct.captures(b"foo.bak", &mut direct_scratch).unwrap();
    assert_eq!(direct_captures.overall(), Some(3..7));
    assert_eq!(direct_captures.group(1), Some(4..7));
    let generation = generation(concat!(
            "SecAction \"id:1,phase:1,pass,nolog,setvar:'tx.restricted_extensions=.bak/ .sql/'\"\n",
            "SecRule REQUEST_BASENAME \"@rx \\.([^.]+)$\" \"id:2,phase:1,pass,capture,setvar:'tx.extension=.%{tx.1}/',chain\"\n",
            "SecRule TX:EXTENSION \"@within %{tx.restricted_extensions}\" \"t:none,t:lowercase\"\n",
        ));
    let mut transaction =
        Transaction::new(&generation, request(&[], b"/get/foo.bak"), RuntimeLimits::default()).unwrap();

    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(transaction.tx_value(b"extension"), Some(b".bak/".as_slice()));
}

#[test]
fn chain_retries_later_root_values_after_an_earlier_child_failure() {
    let generation = generation(concat!(
            "SecAction \"id:1,phase:1,pass,nolog,setvar:'tx.restricted=/content-range/'\"\n",
            "SecRule REQUEST_HEADERS_NAMES \"@rx ^.*$\" \"id:2,phase:1,pass,capture,t:lowercase,setvar:'tx.header_%{tx.0}=/%{tx.0}/',chain\"\n",
            "SecRule TX:/^header_/ \"@within %{tx.restricted}\"\n",
        ));
    let headers = [
        RequestHeader::new(b"Accept", b"text/plain"),
        RequestHeader::new(b"Content-Range", b"bytes 0-1/2"),
    ];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();

    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(transaction.tx_value(b"header_accept"), None);
    assert_eq!(
        transaction.tx_value(b"header_content-range"),
        Some(b"/content-range/".as_slice())
    );
}

#[test]
fn tx_regex_selectors_are_case_insensitive_like_literal_tx_selectors() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,nolog,setvar:'tx.lowercase=value'\"\n",
        "SecRule TX:/^LOWERCASE$/ \"@streq value\" \"id:2,phase:1,pass\"\n",
    ));
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();

    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn seclang_regex_hex_escapes_match_raw_bytes_without_unicode_reencoding() {
    let generation = generation(concat!(
        "SecRule REQUEST_HEADERS:X-Braced \"@rx \\x{bc}[^\\x{be}]*\\x{be}\" \"id:1,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Magic \"@rx \\xac\\xed\" \"id:2,phase:1,pass\"\n",
    ));
    let headers = [
        RequestHeader::new(b"X-Braced", b"\xbcvalue\xbe"),
        RequestHeader::new(b"X-Magic", b"\xac\xedpayload"),
    ];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();

    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn standalone_rule_applies_nondisruptive_actions_for_every_matching_collection_value() {
    let generation = generation(concat!(
            "SecAction \"id:1,phase:1,pass,nolog,setvar:'tx.counter=0'\"\n",
            "SecRule REQUEST_HEADERS_NAMES \"@rx ^X-Collect-.+$\" \"id:2,phase:1,pass,capture,setvar:'tx.name_%{tx.counter}=%{tx.0}',setvar:'tx.counter=+1'\"\n",
        ));
    let headers = [
        RequestHeader::new(b"X-Collect-One", b"first"),
        RequestHeader::new(b"X-Collect-Two", b"second"),
    ];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();

    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(transaction.tx_value(b"counter"), Some(b"2".as_slice()));
    assert_eq!(transaction.tx_value(b"name_0"), Some(b"X-Collect-One".as_slice()));
    assert_eq!(transaction.tx_value(b"name_1"), Some(b"X-Collect-Two".as_slice()));
}

#[test]
fn chain_children_observe_staged_captures_and_setvars_before_atomic_commit() {
    let generation = generation(concat!(
        "SecRule REQUEST_URI \"@rx ^/user/([a-z]+)$\" \"id:52,phase:1,pass,capture,chain,setvar:'tx.root=%{TX.1}'\"\n",
        "SecRule REQUEST_HEADERS:X-User \"@streq %{TX.root}\"\n",
    ));
    let headers = [RequestHeader::new(b"X-User", b"alice")];
    let mut transaction =
        Transaction::new(&generation, request(&headers, b"/user/alice"), RuntimeLimits::default()).unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(outcome.findings().len(), 1);
    assert_eq!(transaction.tx_value(b"root"), Some(b"alice".as_slice()));
    assert_eq!(transaction.tx_value(b"1"), Some(b"alice".as_slice()));
}

#[test]
fn matched_variable_macros_use_transformed_value_and_are_visible_inside_chains() {
    let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-User \"@streq alice\" \"id:55,phase:1,pass,t:lowercase,chain,setvar:'tx.value=%{MATCHED_VAR}',setvar:'tx.name=%{MATCHED_VAR_NAME}'\"\n",
            "SecRule TX:value \"@streq alice\" \"setvar:'tx.child_name=%{MATCHED_VAR_NAME}'\"\n",
        ));
    let headers = [RequestHeader::new(b"X-User", b"ALICE")];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(outcome.findings().len(), 1);
    assert_eq!(transaction.tx_value(b"value"), Some(b"alice".as_slice()));
    assert_eq!(
        transaction.tx_value(b"name"),
        Some(b"REQUEST_HEADERS:X-User".as_slice())
    );
    assert_eq!(transaction.tx_value(b"child_name"), Some(b"TX:value".as_slice()));
}

#[test]
fn negated_operator_matches_the_first_nonmatching_selected_value() {
    let generation = generation(
            "SecRule REQUEST_HEADERS:X-Test \"!@streq forbidden\" \"id:56,phase:1,pass,setvar:'tx.value=%{MATCHED_VAR}'\"\n",
        );
    let headers = [
        RequestHeader::new(b"X-Test", b"forbidden"),
        RequestHeader::new(b"X-Test", b"allowed"),
    ];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(outcome.findings().len(), 1);
    assert_eq!(transaction.tx_value(b"value"), Some(b"allowed".as_slice()));

    let mut empty = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    assert!(empty.execute_request_headers().unwrap().findings().is_empty());
}

#[test]
fn message_and_logdata_templates_are_validated_without_exposing_payloads() {
    let generation = generation(
            "SecRule REQUEST_HEADERS:X-Secret \"@contains token\" \"id:57,phase:1,pass,msg:'matched %{MATCHED_VAR_NAME}',logdata:'value %{MATCHED_VAR}'\"\n",
        );
    let headers = [RequestHeader::new(b"X-Secret", b"private-token")];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(outcome.findings().len(), 1);
    let rendered = format!("{outcome:?} {transaction:?}");
    assert!(!rendered.contains("private"));
    assert!(!rendered.contains("X-Secret"));
}

#[test]
fn matched_vars_collection_accumulates_only_within_the_current_chain() {
    let generation = generation(concat!(
        "SecRule ARGS_GET:first \"@streq one\" \"id:58,phase:1,pass,chain\"\n",
        "SecRule ARGS_GET:second \"@streq two\" \"chain\"\n",
        "SecRule MATCHED_VARS:ARGS_GET:first \"@streq one\"\n",
        "SecRule MATCHED_VARS \"@streq one\" \"id:59,phase:1,pass\"\n",
    ));
    let mut transaction = Transaction::new(
        &generation,
        request(&[], b"/?first=one&second=two"),
        RuntimeLimits::default(),
    )
    .unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [58]
    );
}

#[test]
fn multi_match_evaluates_raw_and_each_changed_transformation_prefix() {
    let generation = generation(
        "SecRule REQUEST_HEADERS:X-Test \"@streq ABC\" \"id:53,phase:1,pass,multiMatch,t:lowercase,t:length\"\n",
    );
    let headers = [RequestHeader::new(b"X-Test", b"ABC")];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(outcome.findings().len(), 1);
    assert_eq!(outcome.findings()[0].rule_id(), 53);
}

#[test]
fn findings_expose_only_bounded_safe_metadata_shape() {
    let generation = generation(
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:54,phase:1,pass,severity:'CRITICAL',tag:'private-category',tag:'second'\"\n",
        );
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    let outcome = transaction.execute_request_headers().unwrap();
    assert_eq!(
        outcome.findings()[0].severity(),
        Some(super::super::super::Severity::Critical)
    );
    assert_eq!(outcome.findings()[0].tag_count(), 2);
    let rendered = format!("{:?}", outcome.findings()[0]);
    assert!(!rendered.contains("private-category"));
}

#[test]
fn request_body_publication_extends_args_and_executes_phase_two_once() {
    let generation = generation(concat!(
        "SecRule REQUEST_URI \"@contains item=from-query\" \"id:60,phase:1,pass\"\n",
        "SecRule ARGS:item \"@streq from-body\" \"id:61,phase:2,pass\"\n",
        "SecRule REQUEST_BODY \"@contains item=from-body\" \"id:62,phase:2,pass\"\n",
        "SecRule REQBODY_PROCESSOR \"@streq URLENCODED\" \"id:63,phase:2,pass\"\n",
        "SecRule ARGS_COMBINED_SIZE \"@eq 27\" \"id:68,phase:2,pass\"\n",
    ));
    let headers = [RequestHeader::new(
        b"Content-Type",
        b"application/x-www-form-urlencoded",
    )];
    let mut transaction = Transaction::new(
        &generation,
        request(&headers, b"/?item=from-query"),
        RuntimeLimits::default(),
    )
    .unwrap();
    assert_eq!(
        transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
        60
    );
    transaction
        .publish_request_body(RequestBody::new(
            b"item=from-body",
            super::super::super::BodyProcessorSelection::Auto,
        ))
        .unwrap();
    let outcome = transaction.execute_request_body().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [61, 62, 63, 68]
    );
    assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
}

#[test]
fn prefix_mixes_header_and_body_rules_in_schedule_order() {
    let headers = [RequestHeader::new(b"X-Test", b"blocked")];
    for targets in [
        ["REQUEST_HEADERS:X-Test", "REQUEST_BODY"],
        ["REQUEST_BODY", "REQUEST_HEADERS:X-Test"],
    ] {
        let rules = format!("SecRule {} \"@contains blocked\" \"id:1,phase:2,deny,status:409\"\nSecRule {} \"@contains blocked\" \"id:2,phase:2,deny,status:403\"\n", targets[0], targets[1]);
        let generation = generation(&rules);
        assert!(generation.requires_complete_request_body());
        assert!(generation.request_body_execution_summary().requires_request_body());
        for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
            let mut transaction = Transaction::new_with_modes(
                &generation,
                request(&headers, b"/"),
                RuntimeLimits::default(),
                EvaluationMode::Accelerated,
                mode,
            )
            .unwrap();
            transaction.execute_request_headers().unwrap();
            let outcome = transaction
                .execute_request_body_prefix(b"blocked", 7, RequestBodyPrefixCompletion::EndOfStream)
                .unwrap();
            if mode == RuleEngineMode::On {
                assert_eq!(
                    outcome.intervention,
                    Some(Intervention {
                        rule_id: 1,
                        status: 409
                    })
                );
                assert_eq!(outcome.evaluated_units, 1);
            } else {
                assert_eq!(
                    outcome
                        .findings
                        .iter()
                        .map(|finding| finding.rule_id)
                        .collect::<Vec<_>>(),
                    vec![1, 2]
                );
                assert_eq!(outcome.intervention, None);
            }
        }
    }
}

#[test]
fn prefix_not_required_still_executes_phase_two_header_rules() {
    let generation = generation("SecRule REQUEST_HEADERS:X-Test \"@streq blocked\" \"id:1,phase:2,deny\"\n");
    let headers = [RequestHeader::new(b"X-Test", b"blocked")];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    let outcome = transaction
        .execute_request_body_prefix(b"", 64, RequestBodyPrefixCompletion::NotRequired)
        .unwrap();
    assert_eq!(
        outcome.intervention,
        Some(Intervention {
            rule_id: 1,
            status: 403
        })
    );
    assert!(transaction.request.body.is_none());
    assert_eq!(
        transaction.request_prefix_completion(),
        Some(RequestBodyPrefixCompletion::NotRequired)
    );
    assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
}

#[test]
fn prefix_short_circuits_deny_before_unreachable_work() {
    let rules: String = (1..=33)
        .map(|id| format!("SecRule REQUEST_BODY \"@contains blocked\" \"id:{id},phase:2,deny,status:403\"\n"))
        .collect();
    let generation = generation(&rules);
    let mut payload = vec![b'x'; MAX_REQUEST_BODY_PREFIX_BYTES];
    payload[..7].copy_from_slice(b"blocked");
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    let outcome = transaction
        .execute_request_body_prefix(&payload, payload.len(), RequestBodyPrefixCompletion::WindowFilled)
        .unwrap();
    assert_eq!(outcome.evaluated_units, 1);
    let mut full = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    full.execute_request_headers().unwrap();
    full.publish_request_body(RequestBody::new(
        &payload,
        super::super::super::BodyProcessorSelection::Auto,
    ))
    .unwrap();
    let full_outcome = full.execute_request_body().unwrap();
    assert_eq!(outcome.intervention, full_outcome.intervention);
    assert_eq!(outcome.findings, full_outcome.findings);
    assert_eq!(
        outcome.intervention,
        Some(Intervention {
            rule_id: 1,
            status: 403
        })
    );

    for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
        let mut transaction = Transaction::new_with_modes(
            &generation,
            request(&[], b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            mode,
        )
        .unwrap();
        transaction.execute_request_headers().unwrap();
        if mode == RuleEngineMode::On {
            payload[..7].fill(b'x');
        } else {
            payload[..7].copy_from_slice(b"blocked");
        }
        assert!(matches!(
            transaction.execute_request_body_prefix(&payload, payload.len(), RequestBodyPrefixCompletion::WindowFilled),
            Err(RuntimeError::OperatorWorkLimit { .. })
        ));
        assert_eq!(transaction.request_prefix_completion(), None);
    }
}

#[test]
fn prefix_borrowed_input_preserves_output_limit_and_detection_findings() {
    let generation = generation(concat!(
        "SecRule REQUEST_BODY \"@contains blocked\" \"id:1,phase:2,deny\"\n",
        "SecRule REQUEST_BODY \"@contains blocked\" \"id:2,phase:2,deny\"\n",
    ));
    let limits = RuntimeLimits {
        max_transformation_output_bytes: 6,
        ..RuntimeLimits::default()
    };
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), limits).unwrap();
    transaction.execute_request_headers().unwrap();
    assert!(matches!(
        transaction.execute_request_body_prefix(b"blocked", 7, RequestBodyPrefixCompletion::EndOfStream),
        Err(RuntimeError::Transformation {
            kind: RuntimeTransformError::OutputLimit
        })
    ));
    let mut transaction = Transaction::new_with_modes(
        &generation,
        request(&[], b"/"),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
    )
    .unwrap();
    transaction.execute_request_headers().unwrap();
    let outcome = transaction
        .execute_request_body_prefix(b"blocked", 7, RequestBodyPrefixCompletion::EndOfStream)
        .unwrap();
    assert_eq!(
        outcome
            .findings
            .iter()
            .map(|finding| finding.rule_id)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(outcome.evaluated_units, 2);
    assert_eq!(outcome.intervention, None);
}

#[test]
fn prefix_configurable_window_matches_at_the_boundary_up_to_one_mib() {
    let generation = generation("SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,t:lowercase\"\n");
    for limit in [16 * 1024, 100 * 1024, MAX_REQUEST_BODY_PREFIX_BYTES] {
        let mut payload = vec![b'x'; limit];
        payload[limit - 9..].copy_from_slice(b"FORBIDDEN");
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        let outcome = transaction
            .execute_request_body_prefix(&payload, limit, RequestBodyPrefixCompletion::WindowFilled)
            .unwrap();
        assert!(outcome.intervention().is_some());
        assert_eq!(outcome.findings().len(), 1);
    }
    for limit in [0, MAX_REQUEST_BODY_PREFIX_BYTES + 1] {
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        assert!(transaction
            .execute_request_body_prefix(b"", limit, RequestBodyPrefixCompletion::EndOfStream)
            .is_err());
    }
}

#[test]
fn prefix_completion_preserves_body_absence_and_advances_phase_once() {
    let generation = generation(concat!(
        "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,status:403\"\n",
        "SecRule RESPONSE_STATUS \"@eq 200\" \"id:2,phase:3,pass\"\n",
    ));
    assert_eq!(generation.validate_request_body_prefix(), Ok(()));
    for completion in [
        RequestBodyPrefixCompletion::EndOfStream,
        RequestBodyPrefixCompletion::WindowFilled,
    ] {
        let mut bytes = vec![b'x'; DEFAULT_REQUEST_BODY_PREFIX_BYTES];
        bytes[DEFAULT_REQUEST_BODY_PREFIX_BYTES - 9..].copy_from_slice(b"forbidden");
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        let outcome = transaction
            .execute_request_body_prefix(&bytes, DEFAULT_REQUEST_BODY_PREFIX_BYTES, completion)
            .unwrap();
        assert_eq!(outcome.findings().len(), 1);
        assert!(outcome.intervention().is_some());
        assert!(transaction.request.body.is_none());
        assert_eq!(transaction.request_prefix_completion(), Some(completion));
        assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
        assert!(transaction
            .execute_request_body_prefix(&bytes, DEFAULT_REQUEST_BODY_PREFIX_BYTES, completion)
            .is_err());
        transaction.publish_response_head(ResponseHead::new(200, &[])).unwrap();
        assert_eq!(
            transaction.execute_response_headers().unwrap().findings()[0].rule_id(),
            2
        );
    }
}

#[test]
fn prefix_detection_only_uses_pm_transform_and_short_body_completion() {
    let generation = generation("SecRule REQUEST_BODY \"@pm forbidden blocked\" \"id:1,phase:2,deny,t:lowercase\"\n");
    let mut transaction = Transaction::new_with_modes(
        &generation,
        request(&[], b"/"),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
    )
    .unwrap();
    transaction.execute_request_headers().unwrap();
    let outcome = transaction
        .execute_request_body_prefix(
            b"BLOCKED",
            DEFAULT_REQUEST_BODY_PREFIX_BYTES,
            RequestBodyPrefixCompletion::EndOfStream,
        )
        .unwrap();
    assert_eq!(outcome.findings().len(), 1);
    assert!(outcome.intervention().is_none());
    assert!(transaction.request.body.is_none());
}

#[test]
fn prefix_rejects_false_window_completion_and_supports_no_body_rules() {
    let generation = generation("SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,deny\"\n");
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    assert!(transaction
        .execute_request_body_prefix(
            b"short",
            DEFAULT_REQUEST_BODY_PREFIX_BYTES,
            RequestBodyPrefixCompletion::WindowFilled
        )
        .is_err());
    assert!(transaction
        .execute_request_body_prefix(
            &[],
            DEFAULT_REQUEST_BODY_PREFIX_BYTES,
            RequestBodyPrefixCompletion::NotRequired
        )
        .is_ok());
    assert!(transaction.request.body.is_none());
}

#[test]
fn prefix_admission_checks_only_the_selected_window_and_enforces_limits() {
    let generation = generation("SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny\"\n");
    let mut payload = vec![b'x'; DEFAULT_REQUEST_BODY_PREFIX_BYTES];
    payload.extend_from_slice(b"forbidden");
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    assert!(transaction
        .execute_request_body_prefix(
            &payload,
            DEFAULT_REQUEST_BODY_PREFIX_BYTES,
            RequestBodyPrefixCompletion::EndOfStream
        )
        .is_err());
    assert!(transaction
        .execute_request_body_prefix(
            &[],
            DEFAULT_REQUEST_BODY_PREFIX_BYTES,
            RequestBodyPrefixCompletion::NotRequired
        )
        .is_err());
    let outcome = transaction
        .execute_request_body_prefix(
            &payload[..DEFAULT_REQUEST_BODY_PREFIX_BYTES],
            DEFAULT_REQUEST_BODY_PREFIX_BYTES,
            RequestBodyPrefixCompletion::WindowFilled,
        )
        .unwrap();
    assert!(outcome.findings().is_empty());
    assert!(outcome.intervention().is_none());
    assert!(transaction.request.body.is_none());
}

#[test]
fn raw_phrase_body_stream_is_chunk_exact_and_constant_memory() {
    let generation = generation(concat!(
        "SecRule REQUEST_BODY \"@pm blocked\" \"id:70,phase:2,deny,status:406,t:lowercase\"\n",
        "SecRule REQUEST_BODY \"!@pm present\" \"id:71,phase:2,pass\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:72,phase:2,pass\"\n",
    ));
    let mut transaction = Transaction::new_with_modes(
        &generation,
        request(&[], b"/"),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
    )
    .unwrap();
    transaction.execute_request_headers().unwrap();
    transaction.start_request_body_stream().unwrap();
    let retained = transaction.retained_bytes();
    assert_eq!(
        transaction
            .execute_request_body_stream_chunk(b"prefix-BLO", false)
            .unwrap(),
        None
    );
    assert_eq!(transaction.retained_bytes(), retained);
    assert_eq!(
        transaction.execute_request_body_stream_chunk(b"CKED-", false).unwrap(),
        None
    );
    assert_eq!(
        transaction
            .execute_request_body_stream_chunk(&vec![b'z'; 1024 * 1024], false)
            .unwrap(),
        None
    );
    assert_eq!(transaction.retained_bytes(), retained);
    let outcome = transaction
        .execute_request_body_stream_chunk(b"suffix", true)
        .unwrap()
        .expect("EOS completes Phase 2");
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [70, 71, 72]
    );
    assert_eq!(outcome.intervention(), None);
    assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
}

#[test]
fn raw_phrase_body_stream_preserves_blocking_status() {
    let generation = generation("SecRule REQUEST_BODY \"@pm cross-boundary\" \"id:73,phase:2,deny,status:409\"\n");
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    transaction.start_request_body_stream().unwrap();
    transaction.execute_request_body_stream_chunk(b"cross-", false).unwrap();
    let outcome = transaction
        .execute_request_body_stream_chunk(b"boundary", false)
        .unwrap()
        .expect("the sole positive raw rule completes Phase 2 immediately");
    assert_eq!(outcome.intervention().map(Intervention::status), Some(409));
    assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
}

#[test]
fn raw_phrase_body_stream_enforces_body_and_work_limits_before_scanning_excess() {
    let generation = generation("SecRule REQUEST_BODY \"@pm bounded\" \"id:74,phase:2,pass\"\n");
    let mut body_limited = Transaction::new(
        &generation,
        request(&[], b"/"),
        RuntimeLimits {
            body: super::super::super::BodyLimits {
                max_body_bytes: 3,
                ..super::super::super::BodyLimits::default()
            },
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    body_limited.execute_request_headers().unwrap();
    body_limited.start_request_body_stream().unwrap();
    assert_eq!(
        body_limited.execute_request_body_stream_chunk(b"four", false),
        Err(RuntimeError::Body {
            kind: BodyError::ByteLimit { actual: 4, limit: 3 }
        })
    );

    let mut work_limited = Transaction::new(
        &generation,
        request(&[], b"/"),
        RuntimeLimits {
            max_operator_work_bytes: 3,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    work_limited.execute_request_headers().unwrap();
    work_limited.start_request_body_stream().unwrap();
    assert_eq!(
        work_limited.execute_request_body_stream_chunk(b"four", false),
        Err(RuntimeError::OperatorWorkLimit { limit: 3 })
    );
}

#[test]
fn request_body_processor_is_available_from_content_type_in_phase_one() {
    let generation = generation("SecRule REQBODY_PROCESSOR \"@streq MULTIPART\" \"id:1,phase:1,pass\"\n");
    let headers = [RequestHeader::new(
        b"Content-Type",
        b"multipart/form-data; boundary=example",
    )];
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();

    assert_eq!(
        transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
        1
    );
}

#[test]
fn multipart_boundary_search_preserves_phase_two_decisions_in_both_modes() {
    let generation = generation("SecRule ARGS:field \"@streq value\" \"id:80,phase:2,deny,status:403\"\n");
    let headers = [RequestHeader::new(
        b"Content-Type",
        b"multipart/form-data; boundary=edge",
    )];
    let body = b"--edge\r\nContent-Disposition: form-data; name=\"field\"\r\n\r\nvalue\r\n--edge--\r\n";
    for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
        let mut transaction = Transaction::new_with_modes(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            mode,
        )
        .unwrap();
        transaction.execute_request_headers().unwrap();
        transaction
            .publish_request_body(RequestBody::new(
                body,
                super::super::super::BodyProcessorSelection::Auto,
            ))
            .unwrap();
        let outcome = transaction.execute_request_body().unwrap();
        assert_eq!(outcome.findings()[0].rule_id(), 80);
        assert_eq!(
            outcome.intervention().map(Intervention::status),
            (mode == RuleEngineMode::On).then_some(403)
        );
    }
}

#[test]
fn json_and_xml_body_views_feed_args_and_selected_xml_paths() {
    let json_generation = generation(concat!(
        "SecRule ARGS:json.user.name \"@streq alice\" \"id:64,phase:2,pass\"\n",
        "SecRule ARGS:json.roles \"@eq 2\" \"id:65,phase:2,pass\"\n",
    ));
    let mut json = Transaction::new(&json_generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    json.execute_request_headers().unwrap();
    json.publish_request_body(RequestBody::new(
        br#"{"user":{"name":"alice"},"roles":["a","b"]}"#,
        super::super::super::BodyProcessorSelection::Json,
    ))
    .unwrap();
    assert_eq!(json.execute_request_body().unwrap().findings().len(), 2);

    let xml_generation = generation(concat!(
        "SecRule XML:/* \"@contains attack\" \"id:66,phase:2,pass\"\n",
        "SecRule XML://@* \"@streq dangerous\" \"id:67,phase:2,pass\"\n",
    ));
    let mut xml = Transaction::new(&xml_generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    xml.execute_request_headers().unwrap();
    xml.publish_request_body(RequestBody::new(
        br#"<root mode="dangerous"><item>attack</item></root>"#,
        super::super::super::BodyProcessorSelection::Xml,
    ))
    .unwrap();
    let outcome = xml.execute_request_body().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [66, 67]
    );
}

#[test]
fn failed_body_publication_is_atomic_and_can_be_retried() {
    let generation = generation("SecRule ARGS:a \"@streq ok\" \"id:69,phase:2,pass\"\n");
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    assert!(matches!(
        transaction.publish_request_body(RequestBody::new(
            b"{private",
            super::super::super::BodyProcessorSelection::Json,
        )),
        Err(RuntimeError::Body {
            kind: BodyError::Malformed {
                processor: super::super::super::BodyProcessorSelection::Json
            }
        })
    ));
    assert!(transaction.request.body.is_none());
    assert_eq!(transaction.next_phase(), Some(Phase::RequestBody));
    transaction
        .publish_request_body(RequestBody::new(
            b"a=ok",
            super::super::super::BodyProcessorSelection::UrlEncoded,
        ))
        .unwrap();
    assert_eq!(transaction.execute_request_body().unwrap().findings()[0].rule_id(), 69);
}

#[test]
fn detection_only_retains_malformed_body_signal_and_executes_raw_phase_two_rules() {
    let generation = generation(concat!(
        "SecRule REQBODY_PROCESSOR \"@streq JSON\" \"id:1,phase:2,pass,chain\"\n",
        "SecRule REQUEST_BODY \"@contains private\"\n",
    ));
    let mut transaction = Transaction::new_with_modes(
        &generation,
        request(&[], b"/"),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
    )
    .unwrap();
    transaction.execute_request_headers().unwrap();
    transaction
        .publish_request_body(RequestBody::new(
            b"{private",
            super::super::super::BodyProcessorSelection::Json,
        ))
        .unwrap();

    assert!(matches!(
        transaction.request_body_error(),
        Some(BodyError::Malformed {
            processor: super::super::super::BodyProcessorSelection::Json
        })
    ));
    assert_eq!(transaction.execute_request_body().unwrap().findings()[0].rule_id(), 1);
}

#[test]
fn response_headers_windows_and_logging_advance_without_body_aggregation() {
    let generation = generation(concat!(
        "SecRule RESPONSE_STATUS \"@eq 404\" \"id:70,phase:3,pass\"\n",
        "SecRule RESPONSE_HEADERS:X-Test \"@streq value\" \"id:71,phase:3,pass\"\n",
        "SecAction \"id:72,phase:4,pass,setvar:'tx.windows=+1'\"\n",
        "SecRule TX:windows \"@eq 2\" \"id:73,phase:5,pass\"\n",
    ));
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    transaction
        .publish_request_body(RequestBody::new(b"", super::super::super::BodyProcessorSelection::Raw))
        .unwrap();
    transaction.execute_request_body().unwrap();
    let response_headers = [ResponseHeader::new(b"x-test", b"value")];
    transaction
        .publish_response_head(ResponseHead::new(404, &response_headers))
        .unwrap();
    let headers = transaction.execute_response_headers().unwrap();
    assert_eq!(
        headers
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [70, 71]
    );
    let first = transaction
        .execute_response_window(ResponseWindow::new(b"private-window", false))
        .unwrap();
    assert_eq!(first.findings()[0].rule_id(), 72);
    assert_eq!(transaction.next_phase(), Some(Phase::ResponseBody));
    assert!(transaction.response.as_ref().unwrap().window.is_none());
    let last = transaction
        .execute_response_window(ResponseWindow::new(b"", true))
        .unwrap();
    assert_eq!(last.findings()[0].rule_id(), 72);
    assert_eq!(transaction.next_phase(), Some(Phase::Logging));
    assert!(transaction.response.as_ref().unwrap().window.is_none());
    assert_eq!(transaction.execute_logging().unwrap().findings()[0].rule_id(), 73);
    assert_eq!(transaction.next_phase(), None);
    assert!(!format!("{transaction:?}").contains("private-window"));
}

#[test]
fn response_limits_are_content_free_and_window_failure_does_not_retain_bytes() {
    let generation = generation("");
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    transaction
        .publish_request_body(RequestBody::new(b"", super::super::super::BodyProcessorSelection::Raw))
        .unwrap();
    transaction.execute_request_body().unwrap();
    let private = [ResponseHeader::new(b"X-Private", b"private-value")];
    assert!(matches!(
        transaction.publish_response_head(ResponseHead::new(99, &private)),
        Err(RuntimeError::InvalidResponseStatus { status: 99 })
    ));
    transaction
        .publish_response_head(ResponseHead::new(200, &private))
        .unwrap();
    transaction.execute_response_headers().unwrap();
    let error = transaction
        .execute_response_window(ResponseWindow::new(b"private", false))
        .unwrap();
    assert!(error.findings().is_empty());
    assert!(transaction.response.as_ref().unwrap().window.is_none());

    let limited = RuntimeLimits {
        max_response_window_bytes: 6,
        ..RuntimeLimits::default()
    };
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), limited).unwrap();
    transaction.execute_request_headers().unwrap();
    transaction
        .publish_request_body(RequestBody::new(b"", super::super::super::BodyProcessorSelection::Raw))
        .unwrap();
    transaction.execute_request_body().unwrap();
    transaction.publish_response_head(ResponseHead::new(200, &[])).unwrap();
    transaction.execute_response_headers().unwrap();
    let error = transaction
        .execute_response_window(ResponseWindow::new(b"private", false))
        .unwrap_err();
    assert_eq!(error, RuntimeError::ResponseWindowByteLimit { actual: 7, limit: 6 });
    assert!(!format!("{error:?} {error}").contains("private"));
    assert!(transaction.response.as_ref().unwrap().window.is_none());
}

#[test]
fn accelerated_and_reference_transactions_are_outcome_equivalent() {
    let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-Probe \"@pm alpha beta gamma\" \"id:80,phase:1,pass,t:lowercase,setvar:'tx.score=+1'\"\n",
            "SecRule REQUEST_HEADERS:X-Probe \"@pm delta omega theta\" \"id:81,phase:1,pass,t:lowercase,setvar:'tx.score=+2'\"\n",
            "SecRule REQUEST_HEADERS:X-Probe \"@rx (alpha|delta)[0-9]+\" \"id:82,phase:1,pass,t:lowercase,setvar:'tx.score=+4'\"\n",
            "SecRule REQUEST_HEADERS:X-Probe \"@rx (omega|theta)-[a-z]+\" \"id:83,phase:1,pass,t:lowercase,setvar:'tx.score=+8'\"\n",
            "SecRule REQUEST_HEADERS:X-Probe \"@pm alpha delta\" \"id:84,phase:1,pass,t:lowercase,capture,setvar:'tx.capture=%{TX.0}'\"\n",
        ));
    let mut seed = 0x07c3_19e2_d830_2026_u64;
    for case in 0..512 {
        let mut value = Vec::with_capacity(192);
        for _ in 0..128 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            value.push(b'a' + ((seed >> 33) % 26) as u8);
        }
        match case % 8 {
            0 => value.extend_from_slice(b" ALPHA42"),
            1 => value.extend_from_slice(b" delta7"),
            2 => value.extend_from_slice(b" omega-value"),
            3 => value.extend_from_slice(b" theta-test"),
            4 => value.extend_from_slice(b" gamma"),
            5 => value.extend_from_slice(b" ALPHA delta"),
            _ => {}
        }
        let headers = [RequestHeader::new(b"X-Probe", &value)];
        let mut accelerated = Transaction::new_with_evaluation(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
        )
        .unwrap();
        let mut reference = Transaction::new_with_evaluation(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Reference,
        )
        .unwrap();
        let accelerated_outcome = accelerated.execute_request_headers().unwrap();
        let reference_outcome = reference.execute_request_headers().unwrap();
        assert_eq!(accelerated_outcome, reference_outcome, "case={case}");
        assert_eq!(
            accelerated.tx_value(b"score"),
            reference.tx_value(b"score"),
            "case={case}"
        );
        assert_eq!(
            accelerated.tx_value(b"capture"),
            reference.tx_value(b"capture"),
            "case={case}"
        );
    }
}

#[test]
fn ctl_rule_removal_is_transaction_local_and_applies_after_the_matching_rule() {
    let generation = Arc::new(generation(concat!(
        "SecRule REQUEST_HEADERS:X-Exempt \"@streq yes\" \"id:1,phase:1,pass,ctl:ruleRemoveByTag=blocklist\"\n",
        "SecRule REQUEST_URI \"@streq /evil\" \"id:2,phase:1,deny,tag:blocklist\"\n",
    )));
    let run = |exempt: bool| {
        let headers = exempt.then_some([RequestHeader::new(b"X-Exempt", b"yes")]);
        let mut tx = Transaction::new_shared(
            Arc::clone(&generation),
            request(headers.as_ref().map_or(&[][..], |headers| &headers[..]), b"/evil"),
            RuntimeLimits::default(),
        )
        .unwrap();
        tx.execute_request_headers().unwrap().intervention().is_some()
    };
    assert!(!run(true));
    assert!(run(false));
    std::thread::scope(|scope| {
        let blocked = scope.spawn(|| run(false));
        let exempt = scope.spawn(|| run(true));
        assert!(blocked.join().unwrap());
        assert!(!exempt.join().unwrap());
    });
}

#[test]
fn ctl_target_removal_filters_only_the_named_value_in_chain_children() {
    let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-Exempt \"@streq yes\" \"id:1,phase:1,pass,ctl:ruleRemoveTargetByTag=blocklist;ARGS:/^allowed$/\"\n",
            "SecRule REQUEST_FILENAME \"@streq /\" \"id:2,phase:2,deny,chain,tag:blocklist\"\n",
            "SecRule ARGS \"@streq blocked\" \"t:none\"\n",
        ));
    let headers = [RequestHeader::new(b"X-Exempt", b"yes")];
    for (uri, blocked) in [
        (b"/?allowed=blocked".as_slice(), false),
        (b"/?other=blocked".as_slice(), true),
    ] {
        let mut tx = Transaction::new(&generation, request(&headers, uri), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(b"", super::super::super::BodyProcessorSelection::Auto))
            .unwrap();
        assert_eq!(tx.execute_request_body().unwrap().intervention().is_some(), blocked);
    }
}

#[test]
fn ctl_processor_and_force_raw_body_apply_before_publication() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,ctl:requestBodyProcessor=JSON,ctl:forceRequestBodyVariable=On\"\n",
        "SecRule ARGS:json.name \"@streq blocked\" \"id:2,phase:2,deny\"\n",
        "SecRule REQUEST_BODY \"@contains blocked\" \"id:3,phase:2,pass\"\n",
    ));
    let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(
        b"{\"name\":\"blocked\"}",
        super::super::super::BodyProcessorSelection::Auto,
    ))
    .unwrap();
    let outcome = tx.execute_request_body().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [2]
    );
    assert_eq!(outcome.intervention().unwrap().rule_id(), 2);
    assert!(tx
        .request
        .body
        .as_ref()
        .unwrap()
        .materialized()
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.collection == CollectionKind::RequestBody));
}

#[test]
fn ctl_force_raw_body_off_hides_only_the_raw_value() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,ctl:forceRequestBodyVariable=Off\"\n",
        "SecRule REQUEST_BODY \"@contains blocked\" \"id:2,phase:2,deny\"\n",
        "SecRule REQUEST_BODY_LENGTH \"@streq 7\" \"id:3,phase:2,pass\"\n",
    ));
    let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(
        b"blocked",
        super::super::super::BodyProcessorSelection::Raw,
    ))
    .unwrap();
    let outcome = tx.execute_request_body().unwrap();
    assert!(outcome.intervention().is_none());
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [3]
    );
}

#[test]
fn ctl_entry_limit_fails_without_affecting_another_transaction() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,ctl:ruleRemoveByTag=blocklist\"\n",
        "SecRule REQUEST_URI \"@streq /\" \"id:2,phase:1,deny,tag:blocklist\"\n",
    ));
    let limits = RuntimeLimits {
        max_ctl_entries: 0,
        ..RuntimeLimits::default()
    };
    let mut limited = Transaction::new(&generation, request(&[], b"/"), limits).unwrap();
    assert!(matches!(
        limited.execute_request_headers(),
        Err(RuntimeError::CtlEntryLimit { limit: 0 })
    ));
    let mut normal = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    assert!(normal.execute_request_headers().unwrap().intervention().is_none());
}

#[test]
fn ctl_target_removal_supports_xml_attributes_without_removing_text() {
    let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-Exempt \"@streq yes\" \"id:1,phase:1,pass,ctl:ruleRemoveTargetByTag=xml-check;XML://@*\"\n",
            "SecRule XML:/*|XML://@* \"@streq blocked\" \"id:2,phase:2,deny,tag:xml-check\"\n",
        ));
    for (headers, body, blocked) in [
        (
            vec![RequestHeader::new(b"X-Exempt", b"yes")],
            b"<root key=\"blocked\">clean</root>".as_slice(),
            false,
        ),
        (vec![], b"<root key=\"blocked\">clean</root>".as_slice(), true),
        (
            vec![RequestHeader::new(b"X-Exempt", b"yes")],
            b"<root key=\"clean\">blocked</root>".as_slice(),
            true,
        ),
    ] {
        let mut tx = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(body, super::super::super::BodyProcessorSelection::Xml))
            .unwrap();
        assert_eq!(tx.execute_request_body().unwrap().intervention().is_some(), blocked);
    }
}

#[test]
fn ctl_in_failed_chain_does_not_remove_a_later_rule() {
    let generation = generation(concat!(
        "SecRule REQUEST_URI \"@streq /evil\" \"id:1,phase:1,pass,chain,ctl:ruleRemoveByTag=blocklist\"\n",
        "SecRule REQUEST_HEADERS:X-Allow \"@streq yes\" \"t:none\"\n",
        "SecRule REQUEST_URI \"@streq /evil\" \"id:2,phase:1,deny,tag:blocklist\"\n",
    ));
    let mut tx = Transaction::new(&generation, request(&[], b"/evil"), RuntimeLimits::default()).unwrap();
    assert_eq!(
        tx.execute_request_headers().unwrap().intervention().unwrap().rule_id(),
        2
    );
}

#[test]
fn ctl_body_actions_reject_late_phase_and_streaming_bypass() {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap();
    let late = compile_with_includes(
        "ctl-late",
        b"SecAction \"id:1,phase:2,pass,ctl:requestBodyProcessor=JSON\"\n",
        &mut NoIncludes,
        None,
        context,
        FrontendLimits::default(),
    );
    assert!(matches!(
        late,
        Err(crate::seclang::FrontendError::Compiler(
            super::super::CompilerError::CtlPhase { .. }
        ))
    ));

    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,ctl:ruleRemoveByTag=body\"\n",
        "SecRule REQUEST_BODY \"@contains blocked\" \"id:2,phase:2,deny,tag:body\"\n",
    ));
    let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    assert!(matches!(
        tx.start_request_body_stream(),
        Err(RuntimeError::RequestBodyStreamCtlUnavailable)
    ));
}

#[test]
fn ctl_rejects_unsupported_xml_exclusion_path_during_compilation() {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap();
    let result = compile_with_includes(
            "ctl-xml-path",
            b"SecAction \"id:1,phase:1,pass,ctl:ruleRemoveTargetByTag=check;XML:/root/node\"\nSecRule XML:/* \"@contains blocked\" \"id:2,phase:2,deny,tag:check\"\n",
            &mut NoIncludes,
            None,
            context,
            FrontendLimits::default(),
        );
    assert!(matches!(
        result,
        Err(crate::seclang::FrontendError::Compiler(
            super::super::CompilerError::CtlXmlSelector { .. }
        ))
    ));
}

#[test]
fn ctl_repeated_tag_activation_consumes_work_budget() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,ctl:ruleRemoveByTag=check,ctl:ruleRemoveByTag=check\"\n",
        "SecRule REQUEST_URI \"@streq /\" \"id:2,phase:1,deny,tag:check\"\n",
    ));
    let limits = RuntimeLimits {
        max_operator_work_bytes: 3,
        ..RuntimeLimits::default()
    };
    let mut tx = Transaction::new(&generation, request(&[], b"/"), limits).unwrap();
    assert!(matches!(
        tx.execute_request_headers(),
        Err(RuntimeError::OperatorWorkLimit { limit: 3 })
    ));
}

#[test]
fn ctl_mismatched_target_exclusions_consume_work_budget() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,ctl:ruleRemoveTargetByTag=check;REQUEST_HEADERS:X-Ignored\"\n",
        "SecRule ARGS \"@streq x\" \"id:2,phase:2,deny,tag:check\"\n",
    ));
    let limits = RuntimeLimits {
        max_operator_work_bytes: 2,
        ..RuntimeLimits::default()
    };
    let mut tx = Transaction::new(&generation, request(&[], b"/?a=x"), limits).unwrap();
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(b"", super::super::super::BodyProcessorSelection::Auto))
        .unwrap();
    assert!(matches!(
        tx.execute_request_body(),
        Err(RuntimeError::OperatorWorkLimit { limit: 2 })
    ));
}

impl RequestState {
    #[cfg(test)]
    pub(super) fn collection(&self, collection: CollectionKind) -> impl Iterator<Item = (Option<&[u8]>, &[u8])> {
        self.collections[collection_index(collection)].iter().map(|record| {
            let name = record.name.map(|range| self.bytes_at(range));
            (name, self.bytes_at(record.value))
        })
    }
}

#[test]
fn body_macro_cannot_start_raw_stream_and_keeps_complete_body_value() {
    let generation = generation("SecRule REQUEST_URI \"@streq %{ARGS.json.key}\" \"id:1,phase:2,deny,status:403\"\n");
    let mut transaction = Transaction::new(&generation, request(&[], b"/blocked"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    assert!(matches!(
        transaction.start_request_body_stream(),
        Err(RuntimeError::GenerationInvariant)
    ));
    transaction
        .publish_request_body(RequestBody::new(
            br#"{"key":"/blocked"}"#,
            super::super::super::BodyProcessorSelection::Json,
        ))
        .unwrap();
    let outcome = transaction.execute_request_body().unwrap();
    assert_eq!(outcome.findings()[0].rule_id(), 1);
    assert!(outcome.intervention().is_some());
}

#[test]
fn structured_reference_duplicate_replacement_removes_earlier_hits_before_rules_run() {
    let generation = generation("SecRule ARGS \"@contains attack\" \"id:1,phase:2,deny,status:403\"\n");
    for (body, blocked) in [
        (br#"{"key":"attack","key":"safe"}"#.as_slice(), false),
        (br#"{"key":{"removed":"attack"},"key":{"kept":"safe"}}"#, false),
        (br#"{"key":"safe","key":"attack"}"#, true),
    ] {
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        transaction
            .publish_request_body(RequestBody::new(
                body,
                super::super::super::BodyProcessorSelection::Json,
            ))
            .unwrap();
        let outcome = transaction.execute_request_body().unwrap();
        let ids: Vec<_> = outcome.findings().iter().map(|finding| finding.rule_id()).collect();
        assert_eq!(ids, if blocked { vec![1] } else { vec![] });
        assert_eq!(
            outcome.intervention().map(|intervention| intervention.rule_id()),
            blocked.then_some(1)
        );
    }
}

#[test]
fn structured_reference_standalone_actions_count_each_matching_value_in_reference_order() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,setvar:'tx.count=0'\"\n",
        "SecRule ARGS \"@contains attack\" \"id:2,phase:2,pass,setvar:'tx.name_%{TX.count}=%{MATCHED_VAR_NAME}',setvar:'tx.value_%{TX.count}=%{MATCHED_VAR}',setvar:'tx.count=+1'\"\n",
        "SecRule TX:count \"@eq 2\" \"id:3,phase:2,pass,setvar:'tx.observed=yes'\"\n",
    ));
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    transaction
        .publish_request_body(RequestBody::new(
            br#"{"z":"attack-last","m":"safe","a":"attack-first"}"#,
            super::super::super::BodyProcessorSelection::Json,
        ))
        .unwrap();
    let outcome = transaction.execute_request_body().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert!(outcome.intervention().is_none());
    assert_eq!(transaction.tx_value(b"count"), Some(b"2".as_slice()));
    assert_eq!(transaction.tx_value(b"name_0"), Some(b"ARGS:json.a".as_slice()));
    assert_eq!(transaction.tx_value(b"value_0"), Some(b"attack-first".as_slice()));
    assert_eq!(transaction.tx_value(b"name_1"), Some(b"ARGS:json.z".as_slice()));
    assert_eq!(transaction.tx_value(b"value_1"), Some(b"attack-last".as_slice()));
    assert_eq!(transaction.tx_value(b"name_2"), None);
    assert_eq!(transaction.tx_value(b"observed"), Some(b"yes".as_slice()));
}

#[test]
fn structured_reference_negation_is_per_value_and_empty_collection_does_not_match() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,setvar:'tx.count=0'\"\n",
        "SecRule ARGS \"!@streq forbidden\" \"id:2,phase:2,pass,setvar:'tx.count=+1',setvar:'tx.last=%{MATCHED_VAR}'\"\n",
    ));
    for (body, count, last) in [
        (
            br#"{"a":"forbidden","b":"safe","c":"other"}"#.as_slice(),
            b"2".as_slice(),
            Some(b"other".as_slice()),
        ),
        (br#"{"a":"forbidden","b":"safe"}"#, b"1", Some(b"safe".as_slice())),
        (br#"{"a":"forbidden","b":"forbidden"}"#, b"0", None),
        (b"{}", b"0", None),
    ] {
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        transaction
            .publish_request_body(RequestBody::new(
                body,
                super::super::super::BodyProcessorSelection::Json,
            ))
            .unwrap();
        let outcome = transaction.execute_request_body().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            if last.is_some() { vec![2] } else { vec![] }
        );
        assert!(outcome.intervention().is_none());
        assert_eq!(transaction.tx_value(b"count"), Some(count));
        assert_eq!(transaction.tx_value(b"last"), last);
    }
}

#[test]
fn structured_reference_chain_children_can_observe_matches_and_captures() {
    let generation = generation(concat!(
        "SecRule ARGS:json.a \"@rx ^alpha-([0-9]+)$\" \"id:1,phase:2,pass,capture,chain,setvar:'tx.root_name=%{MATCHED_VAR_NAME}'\"\n",
        "SecRule MATCHED_VAR \"@streq alpha-1\" \"chain,setvar:'tx.child_scalar=%{MATCHED_VAR}'\"\n",
        "SecRule MATCHED_VARS:ARGS:json.a \"@streq alpha-1\" \"setvar:'tx.child_collection=%{MATCHED_VAR}',setvar:'tx.capture_seen=%{TX.1}'\"\n",
    ));
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    transaction
        .publish_request_body(RequestBody::new(
            br#"{"a":"alpha-1"}"#,
            super::super::super::BodyProcessorSelection::Json,
        ))
        .unwrap();
    let outcome = transaction.execute_request_body().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [1]
    );
    assert_eq!(transaction.tx_value(b"root_name"), Some(b"ARGS:json.a".as_slice()));
    assert_eq!(transaction.tx_value(b"child_scalar"), Some(b"alpha-1".as_slice()));
    assert_eq!(transaction.tx_value(b"child_collection"), Some(b"alpha-1".as_slice()));
    assert_eq!(transaction.tx_value(b"capture_seen"), Some(b"1".as_slice()));
}

#[test]
fn structured_reference_later_units_keep_captures_but_clear_match_observers() {
    let generation = generation(concat!(
        "SecRule ARGS \"@rx ^([a-z]+)-([0-9]+)$\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule MATCHED_VAR \"@streq beta-2\" \"id:2,phase:2,pass,setvar:'tx.scalar_leak=yes'\"\n",
        "SecRule MATCHED_VARS \"@streq alpha-1\" \"id:3,phase:2,pass,setvar:'tx.collection_leak=yes'\"\n",
        "SecAction \"id:4,phase:2,pass,setvar:'tx.persisted_capture=%{TX.1}',setvar:'tx.cleared=%{MATCHED_VAR}'\"\n",
        "SecRule TX:1 \"@streq beta\" \"id:5,phase:2,deny,status:403\"\n",
    ));
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    transaction
        .publish_request_body(RequestBody::new(
            br#"{"z":"beta-2","a":"alpha-1"}"#,
            super::super::super::BodyProcessorSelection::Json,
        ))
        .unwrap();
    let outcome = transaction.execute_request_body().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [1, 4, 5]
    );
    assert_eq!(outcome.intervention().unwrap().rule_id(), 5);
    assert_eq!(transaction.tx_value(b"0"), Some(b"beta-2".as_slice()));
    assert_eq!(transaction.tx_value(b"1"), Some(b"beta".as_slice()));
    assert_eq!(transaction.tx_value(b"2"), Some(b"2".as_slice()));
    assert_eq!(transaction.tx_value(b"persisted_capture"), Some(b"beta".as_slice()));
    assert_eq!(transaction.tx_value(b"scalar_leak"), None);
    assert_eq!(transaction.tx_value(b"collection_leak"), None);
    assert_eq!(transaction.tx_value(b"cleared"), Some(b"".as_slice()));
}

#[test]
fn structured_reference_pure_silent_rule_still_enforces_matching_value_count_limit() {
    let generation = generation("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,nolog,noauditlog\"\n");
    for (body, succeeds) in [
        (br#"["hit","hit"]"#.as_slice(), true),
        (br#"["hit","hit","hit"]"#, false),
    ] {
        let limits = RuntimeLimits {
            max_tx_variables: 2,
            ..RuntimeLimits::default()
        };
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), limits).unwrap();
        transaction.execute_request_headers().unwrap();
        transaction
            .publish_request_body(RequestBody::new(
                body,
                super::super::super::BodyProcessorSelection::Json,
            ))
            .unwrap();
        let result = transaction.execute_request_body();
        if succeeds {
            let outcome = result.unwrap();
            assert_eq!(
                outcome
                    .findings()
                    .iter()
                    .map(|finding| finding.rule_id())
                    .collect::<Vec<_>>(),
                [1]
            );
            assert!(outcome.intervention().is_none());
        } else {
            assert_eq!(result.unwrap_err(), RuntimeError::TxVariableLimit { limit: 2 });
        }
    }
}

#[test]
fn structured_reference_pure_silent_rule_checks_intermediate_matched_byte_limit() {
    let generation = generation("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,nolog,noauditlog\"\n");
    let large = format!("hit{}", "x".repeat(1000));
    let body = format!(r#"{{"a":"{large}","z":"hit"}}"#);
    // A short final match cannot erase the earlier, larger matched-value state.
    // No action or later rule reads MATCHED_VAR(S), but their limits still apply.
    for (limit, succeeds) in [(1500, false), (2500, true)] {
        let limits = RuntimeLimits {
            max_tx_bytes: limit,
            ..RuntimeLimits::default()
        };
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), limits).unwrap();
        transaction.execute_request_headers().unwrap();
        transaction
            .publish_request_body(RequestBody::new(
                body.as_bytes(),
                super::super::super::BodyProcessorSelection::Json,
            ))
            .unwrap();
        let result = transaction.execute_request_body();
        if succeeds {
            let outcome = result.unwrap();
            assert_eq!(
                outcome
                    .findings()
                    .iter()
                    .map(|finding| finding.rule_id())
                    .collect::<Vec<_>>(),
                [1]
            );
            assert!(outcome.intervention().is_none());
        } else {
            assert_eq!(result.unwrap_err(), RuntimeError::TxByteLimit { limit });
        }
    }
}

#[test]
fn structured_reference_committed_match_charge_survives_nonmatching_next_phase() {
    let generation = generation(concat!(
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,nolog,noauditlog\"\n",
        "SecRule REQUEST_URI \"@streq /never\" \"id:2,phase:3,pass\"\n",
    ));
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    transaction
        .publish_request_body(RequestBody::new(
            br#"{"a":"hit","z":"hit-again"}"#,
            super::super::super::BodyProcessorSelection::Json,
        ))
        .unwrap();
    let before_match = transaction.retained_bytes();
    let outcome = transaction.execute_request_body().unwrap();
    assert_eq!(outcome.findings()[0].rule_id(), 1);
    assert!(transaction.retained_bytes() > before_match);
    transaction.publish_response_head(ResponseHead::new(200, &[])).unwrap();
    let before_nonmatch = transaction.retained_bytes();
    let outcome = transaction.execute_response_headers().unwrap();
    assert_eq!(outcome.evaluated_units(), 1);
    assert!(outcome.findings().is_empty());
    assert_eq!(transaction.retained_bytes(), before_nonmatch);
}

#[test]
fn structured_reference_enforced_deny_hides_later_error_but_detection_only_does_not() {
    let generation = generation(concat!(
        "SecRule ARGS:json.a \"@pm hit\" \"id:1,phase:2,deny,status:403\"\n",
        "SecRule ARGS:json.z \"@pm hit\" \"id:2,phase:2,pass\"\n",
    ));
    let body = format!(r#"{{"z":"{}","a":"hit"}}"#, "x".repeat(512));
    for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
        let mut transaction = Transaction::new_with_modes(
            &generation,
            request(&[], b"/"),
            RuntimeLimits {
                max_operator_work_bytes: 256,
                ..RuntimeLimits::default()
            },
            EvaluationMode::Accelerated,
            mode,
        )
        .unwrap();
        transaction.execute_request_headers().unwrap();
        transaction
            .publish_request_body(RequestBody::new(
                body.as_bytes(),
                super::super::super::BodyProcessorSelection::Json,
            ))
            .unwrap();
        let result = transaction.execute_request_body();
        if mode == RuleEngineMode::On {
            let outcome = result.unwrap();
            assert_eq!(outcome.evaluated_units(), 1);
            assert_eq!(outcome.intervention().unwrap().rule_id(), 1);
        } else {
            assert_eq!(result.unwrap_err(), RuntimeError::OperatorWorkLimit { limit: 256 });
        }
    }
}

#[test]
fn json_match_summary_retention_matches_complete_transaction_thresholds() {
    use crate::seclang::JsonMatchSummary;

    for seed in ["", "existing-state"] {
        let prefix = if seed.is_empty() {
            String::new()
        } else {
            format!("SecAction \"id:1,phase:1,pass,setvar:'tx.seed={seed}'\"\n")
        };
        let source = format!("{prefix}SecRule ARGS \"@pm hit\" \"id:2,phase:2,pass,nolog,noauditlog\"\n");
        let generation = generation(&source);
        // Enumerate all sequences of up to three short/medium/long matches.
        // Reverse source keys so reference sorting, rather than arrival order,
        // determines the retention peak.
        for length in 0..=3_u32 {
            for mut choice in 0..3_usize.pow(length) {
                let mut fields = Vec::new();
                let mut summary = JsonMatchSummary::new(size_of::<TxEntry>());
                for index in 0..length {
                    let value_len = [3, 80, 512][choice % 3];
                    choice /= 3;
                    let key = format!("k{index}");
                    let value = format!("hit{}", "x".repeat(value_len - 3));
                    summary.append(format!("ARGS:json.{key}").len(), value.len()).unwrap();
                    fields.push(format!("\"{key}\":\"{value}\""));
                }
                fields.reverse();
                let body = format!("{{{}}}", fields.join(","));
                let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
                let before_headers = tx.retained_bytes();
                tx.execute_request_headers().unwrap();
                let base = tx.retained_bytes() - before_headers;
                tx.publish_request_body(RequestBody::new(
                    body.as_bytes(),
                    super::super::super::BodyProcessorSelection::Json,
                ))
                .unwrap();
                let before_body = tx.retained_bytes();
                let expected = summary.retention(base).unwrap();
                let outcome = tx.execute_request_body().unwrap();
                assert_eq!(outcome.findings().len(), usize::from(length != 0));
                assert_eq!(tx.retained_bytes() - before_body + base, expected.final_bytes);

                if length == 0 {
                    assert_eq!(expected.peak_bytes, base);
                    continue;
                }
                for (limit, succeeds) in [(expected.peak_bytes, true), (expected.peak_bytes - 1, false)] {
                    let mut tx = Transaction::new(
                        &generation,
                        request(&[], b"/"),
                        RuntimeLimits {
                            max_tx_bytes: limit,
                            ..RuntimeLimits::default()
                        },
                    )
                    .unwrap();
                    tx.execute_request_headers().unwrap();
                    tx.publish_request_body(RequestBody::new(
                        body.as_bytes(),
                        super::super::super::BodyProcessorSelection::Json,
                    ))
                    .unwrap();
                    let result = tx.execute_request_body();
                    if succeeds {
                        assert_eq!(result.unwrap().findings()[0].rule_id(), 2);
                    } else {
                        assert_eq!(result.unwrap_err(), RuntimeError::TxByteLimit { limit });
                    }
                }
            }
        }
    }
}

fn json_execution_limits() -> json_rules::JsonExecutionLimits {
    use crate::seclang::{JsonDocumentLimits, JsonStreamLimits};
    json_rules::JsonExecutionLimits {
        lexical: JsonStreamLimits::default(),
        document: JsonDocumentLimits {
            max_retained_bytes: 16 * 1024 * 1024,
            max_key_bytes: 1024 * 1024,
            max_members: 65_536,
            max_events: 131_072,
            max_path_bytes: 16 * 1024,
            max_depth: 64,
            max_work: 128 * 1024 * 1024,
        },
        plan: json_rules::JsonPlanLimits {
            max_subscribers: 128,
            max_retained_bytes: 1024 * 1024,
        },
        facts: json_rules::JsonFactLimits {
            max_facts: 131_072,
            max_retained_bytes: 32 * 1024 * 1024,
            max_work: 128 * 1024 * 1024,
        },
    }
}

fn json_runtime_error(error: json_rules::JsonExecutionError) -> RuntimeError {
    match error {
        json_rules::JsonExecutionError::Runtime(error) => error,
        json_rules::JsonExecutionError::Body(crate::seclang::JsonBodyError::Body(kind)) => RuntimeError::Body { kind },
        error => panic!("unexpected independent streaming failure: {error:?}"),
    }
}

fn compare_json_transaction(source: &str, body: &[u8], split: usize, limits: RuntimeLimits, mode: RuleEngineMode) {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation(source);
    let mut reference = Transaction::new_with_modes(
        &generation,
        request(&[], b"/?q=hit&q=miss"),
        limits,
        EvaluationMode::Accelerated,
        mode,
    )
    .unwrap();
    let mut streamed = Transaction::new_with_modes(
        &generation,
        request(&[], b"/?q=hit&q=miss"),
        limits,
        EvaluationMode::Accelerated,
        mode,
    )
    .unwrap();
    assert_eq!(reference.execute_request_headers(), streamed.execute_request_headers());
    let expected = reference
        .publish_request_body(RequestBody::new(body, BodyProcessorSelection::Json))
        .and_then(|()| reference.execute_request_body());
    let actual = streamed
        .execute_json_body_inputs(
            BodyProcessorSelection::Json,
            [
                JsonStreamInput::Data(&body[..split]),
                JsonStreamInput::Data(&body[split..]),
                JsonStreamInput::Complete,
            ],
            json_execution_limits(),
        )
        .map_err(json_runtime_error);
    assert_eq!(actual, expected);
    assert_eq!(streamed.next_phase(), reference.next_phase());
    assert_eq!(streamed.request_body_error(), reference.request_body_error());
    assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
    assert_eq!(streamed.request.entry_count, reference.request.entry_count);
    assert_eq!(streamed.request.body.is_some(), reference.request.body.is_some());
    assert_eq!(streamed.tx_value(b"seed"), reference.tx_value(b"seed"));
    let entries = |tx: &TxState| {
        tx.entries
            .iter()
            .map(|entry| (entry.key.clone(), entry.value.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(entries(&streamed.tx), entries(&reference.tx));
    assert_eq!(streamed.capture_keys, reference.capture_keys);
    // Runtime errors leave Phase 2 and earlier commits available to a retry.
    if expected.is_ok() {
        assert!(streamed.json_body_state.is_empty());
        assert_eq!(streamed.json_body_state.capacity(), 0);
    }
    if expected.is_err() && reference.request.body.is_some() {
        assert_eq!(streamed.json_body_state.len(), 1);
        assert_eq!(streamed.execute_request_body(), reference.execute_request_body());
        assert_eq!(entries(&streamed.tx), entries(&reference.tx));
        assert_eq!(streamed.capture_keys, reference.capture_keys);
        assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
        assert_eq!(streamed.next_phase(), reference.next_phase());
    }
}

#[test]
fn json_transaction_schedule_matches_complete_body_at_every_split() {
    let source = concat!(
        "SecAction \"id:9,phase:1,pass,setvar:tx.seed=5\"\n",
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,tag:'test',severity:NOTICE\"\n",
        "SecRule ARGS \"@pm absent\" \"id:2,phase:2,pass,nolog,noauditlog\"\n",
        "SecRule ARGS_NAMES \"!@pm absent\" \"id:3,phase:2,deny,status:418\"\n",
        "SecRule ARGS:json.a \"@pm hit\" \"id:4,phase:2,pass,t:lowercase\"\n",
    );
    for input in [
        b"".as_slice(),
        b"{}",
        b"null",
        b"[]",
        b" ",
        b"[",
        br#"{"a":"HIT","a":"hit","z":[null,"miss"]}"#,
        br#"{"a.b":"hit","a":{"b":"miss"}}"#,
    ] {
        for split in 0..=input.len() {
            for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                for max_findings in [0, 1, 8] {
                    compare_json_transaction(
                        source,
                        input,
                        split,
                        RuntimeLimits {
                            max_findings,
                            ..RuntimeLimits::default()
                        },
                        mode,
                    );
                }
            }
        }
    }
}

#[test]
fn json_transaction_deny_hides_deferred_errors_and_detection_mode_retries_them() {
    let source = concat!(
        "SecRule ARGS:json.a \"@pm hit\" \"id:1,phase:2,deny,status:403\"\n",
        "SecRule ARGS:json.z \"@pm hit\" \"id:2,phase:2,pass\"\n",
    );
    let body = format!(r#"{{"z":"{}","a":"hit"}}"#, "x".repeat(512));
    for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
        compare_json_transaction(
            source,
            body.as_bytes(),
            19,
            RuntimeLimits {
                max_transformation_output_bytes: 32,
                ..RuntimeLimits::default()
            },
            mode,
        );
        compare_json_transaction(
            source,
            body.as_bytes(),
            31,
            RuntimeLimits {
                max_operator_work_bytes: 256,
                ..RuntimeLimits::default()
            },
            mode,
        );
    }
}

#[test]
fn json_transaction_preserves_publication_and_runtime_limit_order() {
    let source =
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\nSecRule ARGS_NAMES \"@pm json\" \"id:2,phase:2,pass\"\n";
    let body = br#"{"a":"hit","z":"hit-again"}"#;
    for max_operator_evaluations in [0, 1, 2, 8] {
        for max_selected_values in [0, 1, 3, 8] {
            for max_tx_variables in [0, 1, 8] {
                compare_json_transaction(
                    source,
                    body,
                    9,
                    RuntimeLimits {
                        max_operator_evaluations,
                        max_selected_values,
                        max_tx_variables,
                        ..RuntimeLimits::default()
                    },
                    RuleEngineMode::On,
                );
            }
        }
    }
    for max_tx_bytes in [0, 32, 96, 160, 1024] {
        compare_json_transaction(
            source,
            body,
            0,
            RuntimeLimits {
                max_tx_bytes,
                ..RuntimeLimits::default()
            },
            RuleEngineMode::On,
        );
    }
    for max_body_entries in [0, 1, 2, 3, 6, 7] {
        compare_json_transaction(
            source,
            body,
            4,
            RuntimeLimits {
                body: crate::seclang::BodyLimits {
                    max_entries: max_body_entries,
                    ..crate::seclang::BodyLimits::default()
                },
                ..RuntimeLimits::default()
            },
            RuleEngineMode::On,
        );
    }
}

#[test]
fn json_transaction_publication_admission_matches_exact_collection_and_retention_caps() {
    use crate::seclang::BodyProcessorSelection;
    let source = concat!(
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:1,phase:1,pass\"\n",
        "SecRule ARGS \"@pm hit\" \"id:2,phase:2,pass\"\n",
    );
    let generation = generation(source);
    let body = br#"{"a":"hit"}"#;
    let mut probe = Transaction::new(&generation, request(&[], b"/?q=hit&q=miss"), RuntimeLimits::default()).unwrap();
    probe.execute_request_headers().unwrap();
    probe
        .publish_request_body(RequestBody::new(body, BodyProcessorSelection::Json))
        .unwrap();
    for max_collection_entries in [probe.request.entry_count - 1, probe.request.entry_count] {
        for max_retained_bytes in [
            probe.retained_bytes() - 1,
            probe.retained_bytes(),
            probe.retained_bytes() + 1024,
        ] {
            compare_json_transaction(
                source,
                body,
                2,
                RuntimeLimits {
                    max_collection_entries,
                    max_retained_bytes,
                    ..RuntimeLimits::default()
                },
                RuleEngineMode::On,
            );
        }
    }
}

#[test]
fn json_transaction_retention_matches_response_admission_and_later_match_replacement() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation(concat!(
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n",
        "SecRule REQUEST_URI \"@streq /never\" \"id:2,phase:3,pass\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:3,phase:4,pass\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:4,phase:5,pass\"\n",
    ));
    let body = br#"{"a":"hit","z":"hit-again"}"#;
    for extra_retention in [0, 1, 8, 64, 4096] {
        let mut reference = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        let mut streamed = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        reference.execute_request_headers().unwrap();
        streamed.execute_request_headers().unwrap();
        reference
            .publish_request_body(RequestBody::new(body, BodyProcessorSelection::Json))
            .unwrap();
        let expected = reference.execute_request_body().unwrap();
        let actual = streamed
            .execute_json_body_inputs(
                BodyProcessorSelection::Json,
                [JsonStreamInput::Data(body), JsonStreamInput::Complete],
                json_execution_limits(),
            )
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
        assert!(streamed.tx.virtual_matches.is_some());
        assert!(streamed.json_body_state.is_empty());
        let cap = reference.retained_bytes() + extra_retention;
        reference.limits.max_retained_bytes = cap;
        streamed.limits.max_retained_bytes = cap;
        let expected = reference.publish_response_head(ResponseHead::new(200, &[]));
        assert_eq!(streamed.publish_response_head(ResponseHead::new(200, &[])), expected);
        assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
        if expected.is_err() {
            continue;
        }
        assert_eq!(
            streamed.execute_response_headers(),
            reference.execute_response_headers()
        );
        assert!(streamed.tx.virtual_matches.is_some());
        assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
        let expected = reference.execute_response_window(ResponseWindow::new(b"hit", true));
        assert_eq!(
            streamed.execute_response_window(ResponseWindow::new(b"hit", true)),
            expected
        );
        assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
        assert!(streamed.response.as_ref().unwrap().window.is_none());
        assert_eq!(streamed.next_phase(), reference.next_phase());
        if expected.is_ok() {
            assert!(streamed.tx.virtual_matches.is_none());
            assert_eq!(streamed.execute_logging(), reference.execute_logging());
            assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
        }
    }
}

#[test]
fn json_transaction_rejects_unsupported_admission_before_consuming_input() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let source = "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n";
    let compiled = generation(source);
    let mut tx = Transaction::new(&compiled, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    let input =
        || std::iter::from_fn(|| -> Option<JsonStreamInput<'static>> { panic!("input consumed before admission") });
    assert!(matches!(
        tx.execute_json_body_inputs(BodyProcessorSelection::Json, input(), json_execution_limits()),
        Err(json_rules::JsonExecutionError::Runtime(RuntimeError::PhaseOrder { .. }))
    ));
    tx.execute_request_headers().unwrap();
    assert!(tx
        .execute_json_body_inputs(BodyProcessorSelection::Xml, input(), json_execution_limits())
        .is_err());
    assert!(tx.request.body.is_none());
    tx.publish_request_body(RequestBody::new(b"{}", BodyProcessorSelection::Json))
        .unwrap();
    assert!(matches!(
        tx.execute_json_body_inputs(BodyProcessorSelection::Json, input(), json_execution_limits()),
        Err(json_rules::JsonExecutionError::Runtime(
            RuntimeError::RequestBodyAlreadyPublished
        ))
    ));

    for target in [
        "ARGS",
        "ARGS_NAMES",
        "ARGS_COMBINED_SIZE",
        "REQUEST_BODY",
        "REQUEST_BODY_LENGTH",
        "REQBODY_PROCESSOR",
        "FILES_NAMES",
    ] {
        let source = format!("{source}SecRule {target} \"@pm hit\" \"id:2,phase:3,pass\"\n");
        let generation = generation(&source);
        let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        assert_eq!(
            tx.execute_json_body_inputs(BodyProcessorSelection::Json, input(), json_execution_limits()),
            Err(json_rules::JsonExecutionError::Unsupported)
        );
        assert!(tx.request.body.is_none());
    }
}

#[test]
fn json_transaction_incomplete_abandoned_and_trailing_input_install_no_publication() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    for inputs in [
        vec![JsonStreamInput::Data(b"{}")],
        vec![JsonStreamInput::Data(b"{"), JsonStreamInput::Abandoned],
        vec![JsonStreamInput::Complete, JsonStreamInput::Data(b"x")],
    ] {
        let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        let before = tx.retained_bytes();
        assert!(tx
            .execute_json_body_inputs(BodyProcessorSelection::Json, inputs, json_execution_limits())
            .is_err());
        assert!(tx.request.body.is_none());
        assert!(tx.json_body_state.is_empty());
        assert_eq!(tx.retained_bytes(), before);
        assert_eq!(tx.next_phase(), Some(Phase::RequestBody));
        assert!(tx
            .execute_json_body_inputs(
                BodyProcessorSelection::Json,
                [JsonStreamInput::Data(b"{}"), JsonStreamInput::Complete],
                json_execution_limits()
            )
            .is_ok());
    }
}

#[test]
fn json_transaction_later_matched_readers_clear_inherited_state_and_chains_see_new_matches() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation(concat!(
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n",
        "SecRule MATCHED_VARS \"@contains hit\" \"id:2,phase:3,pass,setvar:tx.inherited=1\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:3,phase:3,pass,chain\"\n",
        "SecRule MATCHED_VARS \"@contains GET\" \"setvar:tx.chain=1\"\n",
    ));
    let mut reference = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    let mut streamed = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    reference.execute_request_headers().unwrap();
    streamed.execute_request_headers().unwrap();
    let body = br#"{"a":"hit"}"#;
    reference
        .publish_request_body(RequestBody::new(body, BodyProcessorSelection::Json))
        .unwrap();
    assert_eq!(
        streamed
            .execute_json_body_inputs(
                BodyProcessorSelection::Json,
                [JsonStreamInput::Data(body), JsonStreamInput::Complete],
                json_execution_limits()
            )
            .unwrap(),
        reference.execute_request_body().unwrap()
    );
    reference.publish_response_head(ResponseHead::new(200, &[])).unwrap();
    streamed.publish_response_head(ResponseHead::new(200, &[])).unwrap();
    let actual = streamed.execute_response_headers().unwrap();
    assert_eq!(actual, reference.execute_response_headers().unwrap());
    assert_eq!(
        actual
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [3]
    );
    assert_eq!(streamed.tx_value(b"inherited"), None);
    assert_eq!(streamed.tx_value(b"chain"), Some(b"1".as_slice()));
    assert!(streamed.tx.virtual_matches.is_none());
    assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
}

#[test]
fn json_transaction_resolves_auto_processor_and_rejects_raw_lane_conflicts() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let json_generation = generation("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    let headers = [RequestHeader::new(b"content-type", b"application/problem+json")];
    let mut tx = Transaction::new(&json_generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    tx.execute_json_body_inputs(
        BodyProcessorSelection::Auto,
        [JsonStreamInput::Data(b"{}"), JsonStreamInput::Complete],
        json_execution_limits(),
    )
    .unwrap();
    assert!(matches!(
        tx.start_request_body_stream(),
        Err(RuntimeError::PhaseOrder { .. })
    ));
    assert!(matches!(
        tx.execute_request_body(),
        Err(RuntimeError::PhaseOrder { .. })
    ));

    let raw_generation = generation("SecRule REQUEST_BODY \"@pm hit\" \"id:1,phase:2,pass\"\n");
    let mut tx = Transaction::new(&raw_generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    tx.start_request_body_stream().unwrap();
    let before = tx.retained_bytes();
    let inputs =
        std::iter::from_fn(|| -> Option<JsonStreamInput<'static>> { panic!("raw lane conflict consumed input") });
    assert_eq!(
        tx.execute_json_body_inputs(BodyProcessorSelection::Json, inputs, json_execution_limits()),
        Err(json_rules::JsonExecutionError::State)
    );
    assert!(tx.request.body.is_none());
    assert!(tx.request_body_stream.is_some());
    assert_eq!(tx.retained_bytes(), before);
}

#[test]
fn json_transaction_source_and_fact_failures_leave_request_unpublished() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    for source_failure in [false, true] {
        let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        let before = tx.retained_bytes();
        let mut limits = json_execution_limits();
        if source_failure {
            limits.lexical.max_source_bytes = 0;
        } else {
            limits.facts.max_facts = 0;
        }
        assert!(tx
            .execute_json_body_inputs(
                BodyProcessorSelection::Json,
                [JsonStreamInput::Data(br#"{"a":"hit"}"#), JsonStreamInput::Complete],
                limits
            )
            .is_err());
        assert!(tx.request.body.is_none());
        assert!(tx.json_body_state.is_empty());
        assert_eq!(tx.retained_bytes(), before);
        assert_eq!(tx.next_phase(), Some(Phase::RequestBody));
    }
}

#[test]
fn json_persistent_interleaved_chunks_match_reference_without_replacing_scratch() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation(concat!(
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,t:lowercase\"\n",
        "SecRule ARGS_NAMES \"!@pm absent\" \"id:2,phase:2,deny,status:418\"\n",
    ));
    for body in [
        br#"{"a":"HIT","a":"miss","z":["hit",null]}"#.as_slice(),
        b"{}",
        b"[",
        b"",
    ] {
        for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
            let limits = RuntimeLimits::default();
            let make = || {
                Transaction::new_with_modes(
                    &generation,
                    request(&[], b"/?q=hit"),
                    limits,
                    EvaluationMode::Accelerated,
                    mode,
                )
                .unwrap()
            };
            let mut reference = make();
            let mut streamed = make();
            reference.execute_request_headers().unwrap();
            streamed.execute_request_headers().unwrap();
            let scratch = (
                streamed.group_scratch.as_ptr(),
                streamed.group_scratch.capacity(),
                streamed.scratch_bytes,
            );
            let before = streamed.retained_bytes();
            streamed
                .start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
                .unwrap();
            for byte in body.chunks(1) {
                streamed.feed_json_request_body(JsonStreamInput::Data(b"")).unwrap();
                let mut callback_buffer = byte.to_vec();
                streamed
                    .feed_json_request_body(JsonStreamInput::Data(&callback_buffer))
                    .unwrap();
                callback_buffer.fill(0);
                assert!(streamed.request.body.is_none());
                assert_eq!(streamed.retained_bytes(), before);
                assert_eq!(
                    scratch,
                    (
                        streamed.group_scratch.as_ptr(),
                        streamed.group_scratch.capacity(),
                        streamed.scratch_bytes
                    )
                );
            }
            let actual = streamed
                .feed_json_request_body(JsonStreamInput::Complete)
                .and_then(|()| {
                    assert!(streamed.request.body.is_none());
                    streamed.finish_json_request_body()
                })
                .map_err(json_runtime_error);
            let expected = reference
                .publish_request_body(RequestBody::new(body, BodyProcessorSelection::Json))
                .and_then(|()| reference.execute_request_body());
            assert_eq!(actual, expected);
            assert_eq!(streamed.next_phase(), reference.next_phase());
            assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
            assert_eq!(streamed.request_body_error(), reference.request_body_error());
            assert_eq!(
                scratch,
                (
                    streamed.group_scratch.as_ptr(),
                    streamed.group_scratch.capacity(),
                    streamed.scratch_bytes
                )
            );
        }
    }
}

#[test]
fn json_persistent_abandonment_is_sticky_and_new_source_restart_is_explicit() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    let before = tx.retained_bytes();
    tx.start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
        .unwrap();
    tx.feed_json_request_body(JsonStreamInput::Data(br#"{"a":"hit","b":"unfinished"#))
        .unwrap();
    let error = tx.feed_json_request_body(JsonStreamInput::Abandoned).unwrap_err();
    assert_eq!(tx.feed_json_request_body(JsonStreamInput::Complete), Err(error));
    assert_eq!(tx.finish_json_request_body(), Err(error));
    assert_eq!(tx.retained_bytes(), before);
    assert!(tx.request.body.is_none());
    assert_eq!(tx.json_body_state.capacity(), 0);
    assert!(tx
        .publish_request_body(RequestBody::new(b"{}", BodyProcessorSelection::Json))
        .is_err());
    assert!(tx.start_request_body_stream().is_err());
    assert!(tx.execute_request_body_stream_chunk(b"ignored", true).is_err());
    assert!(tx
        .execute_request_body_prefix(b"", 1, RequestBodyPrefixCompletion::EndOfStream)
        .is_err());
    assert!(tx
        .start_json_request_body(BodyProcessorSelection::Xml, json_execution_limits())
        .is_err());
    assert_eq!(tx.feed_json_request_body(JsonStreamInput::Data(b"{}")), Err(error));
    // Restart supplies the complete replacement document, never the old suffix.
    tx.start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
        .unwrap();
    tx.feed_json_request_body(JsonStreamInput::Data(br#"{"a":"hit"}"#))
        .unwrap();
    tx.feed_json_request_body(JsonStreamInput::Complete).unwrap();
    assert_eq!(tx.finish_json_request_body().unwrap().findings.len(), 1);
    assert_eq!(tx.json_body_state.capacity(), 0);
}

#[test]
fn json_persistent_sealed_and_collecting_states_reject_competing_entrypoints() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    tx.start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
        .unwrap();
    tx.feed_json_request_body(JsonStreamInput::Data(br#"{"a":"hit"}"#))
        .unwrap();
    for sealed in [false, true] {
        if sealed {
            tx.feed_json_request_body(JsonStreamInput::Complete).unwrap();
        }
        assert!(tx
            .start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
            .is_err());
        assert!(tx
            .publish_request_body(RequestBody::new(b"{}", BodyProcessorSelection::Json))
            .is_err());
        assert!(tx.start_request_body_stream().is_err());
        assert!(tx.execute_request_body_stream_chunk(b"ignored", true).is_err());
        assert!(tx
            .execute_request_body_prefix(b"", 1, RequestBodyPrefixCompletion::EndOfStream)
            .is_err());
        assert!(tx.execute_request_body().is_err());
        assert!(tx.request.body.is_none());
    }
    let outcome = tx.finish_json_request_body().unwrap();
    assert_eq!(outcome.findings.len(), 1);
    let retained = tx.retained_bytes();
    let phase = tx.next_phase();
    for input in [
        JsonStreamInput::Complete,
        JsonStreamInput::Data(b"x"),
        JsonStreamInput::Abandoned,
    ] {
        assert!(tx.feed_json_request_body(input).is_err());
        assert_eq!(tx.retained_bytes(), retained);
        assert_eq!(tx.next_phase(), phase);
        assert!(tx.request.body.is_some());
    }
    assert!(tx.finish_json_request_body().is_err());
    assert_eq!(tx.retained_bytes(), retained);
    assert_eq!(tx.json_body_state.capacity(), 0);
}

#[test]
fn json_persistent_unfinished_or_trailing_inputs_release_provisional_state() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    for trailing in [None, Some(JsonStreamInput::Complete), Some(JsonStreamInput::Data(b"x"))] {
        let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        let before = tx.retained_bytes();
        tx.start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
            .unwrap();
        tx.feed_json_request_body(JsonStreamInput::Data(br#"{"a":"hit"}"#))
            .unwrap();
        let error = if let Some(input) = trailing {
            tx.feed_json_request_body(JsonStreamInput::Complete).unwrap();
            tx.feed_json_request_body(input).unwrap_err()
        } else {
            tx.finish_json_request_body().unwrap_err()
        };
        assert_eq!(tx.finish_json_request_body(), Err(error));
        assert_eq!(tx.feed_json_request_body(JsonStreamInput::Complete), Err(error));
        assert_eq!(tx.json_body_state.capacity(), 0);
        assert!(tx.request.body.is_none());
        assert_eq!(tx.retained_bytes(), before);
    }
}

#[test]
fn json_persistent_owned_transaction_drop_releases_generation_in_each_lifecycle() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = Arc::new(generation("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n"));
    for stage in 0..4 {
        let mut tx =
            Transaction::new_shared(Arc::clone(&generation), request(&[], b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        tx.start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
            .unwrap();
        tx.feed_json_request_body(JsonStreamInput::Data(br#"{"a":"hit"}"#))
            .unwrap();
        if stage > 0 {
            tx.feed_json_request_body(JsonStreamInput::Complete).unwrap();
        }
        if stage == 2 {
            tx.finish_json_request_body().unwrap();
        }
        if stage == 3 {
            assert!(tx.feed_json_request_body(JsonStreamInput::Abandoned).is_err());
        }
        assert_eq!(Arc::strong_count(&generation), 2);
        drop(tx);
        assert_eq!(Arc::strong_count(&generation), 1);
    }
}

#[test]
fn json_persistent_transport_misuse_preserves_published_retry_facts() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation(concat!(
        "SecRule ARGS:json.a \"@pm hit\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS:json.z \"@pm hit\" \"id:2,phase:2,pass\"\n",
    ));
    let limits = RuntimeLimits {
        max_transformation_output_bytes: 16,
        ..RuntimeLimits::default()
    };
    let body = format!(r#"{{"a":"hit","z":"{}"}}"#, "x".repeat(32));
    let mut tx = Transaction::new(&generation, request(&[], b"/"), limits).unwrap();
    let mut reference = Transaction::new(&generation, request(&[], b"/"), limits).unwrap();
    tx.execute_request_headers().unwrap();
    reference.execute_request_headers().unwrap();
    reference
        .publish_request_body(RequestBody::new(body.as_bytes(), BodyProcessorSelection::Json))
        .unwrap();
    let expected = reference.execute_request_body();
    tx.start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
        .unwrap();
    tx.feed_json_request_body(JsonStreamInput::Data(body.as_bytes()))
        .unwrap();
    tx.feed_json_request_body(JsonStreamInput::Complete).unwrap();
    assert_eq!(tx.finish_json_request_body().map_err(json_runtime_error), expected);
    assert!(expected.is_err());
    assert_eq!(tx.json_body_state.len(), 1);
    let retained = tx.retained_bytes();
    let capacity = tx.json_body_state.capacity();
    for input in [
        JsonStreamInput::Data(b"x"),
        JsonStreamInput::Abandoned,
        JsonStreamInput::Complete,
    ] {
        assert!(tx.feed_json_request_body(input).is_err());
        assert!(tx.finish_json_request_body().is_err());
        assert!(tx
            .start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
            .is_err());
        assert_eq!(tx.json_body_state.len(), 1);
        assert_eq!(tx.json_body_state.capacity(), capacity);
        assert_eq!(tx.retained_bytes(), retained);
        assert_eq!(tx.execute_request_body(), reference.execute_request_body());
        assert_eq!(tx.retained_bytes(), reference.retained_bytes());
    }
}

#[test]
fn json_cached_plan_is_shared_while_transaction_facts_remain_isolated() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = Arc::new(generation(
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,deny,status:418\"\n",
    ));
    let shared_plan = generation.json_capability.subscribers().unwrap().as_ptr();
    let mut first =
        Transaction::new_shared(Arc::clone(&generation), request(&[], b"/"), RuntimeLimits::default()).unwrap();
    let mut second =
        Transaction::new_shared(Arc::clone(&generation), request(&[], b"/"), RuntimeLimits::default()).unwrap();
    first.execute_request_headers().unwrap();
    second.execute_request_headers().unwrap();
    first
        .start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
        .unwrap();
    second
        .start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
        .unwrap();
    assert_eq!(
        first.generation.json_capability.subscribers().unwrap().as_ptr(),
        shared_plan
    );
    assert_eq!(
        second.generation.json_capability.subscribers().unwrap().as_ptr(),
        shared_plan
    );
    assert_ne!(first.group_scratch.as_ptr(), second.group_scratch.as_ptr());
    first
        .feed_json_request_body(JsonStreamInput::Data(br#"{"a":"hit"}"#))
        .unwrap();
    second
        .feed_json_request_body(JsonStreamInput::Data(br#"{"a":"miss"}"#))
        .unwrap();
    second.feed_json_request_body(JsonStreamInput::Complete).unwrap();
    assert!(second.finish_json_request_body().unwrap().findings().is_empty());
    first.feed_json_request_body(JsonStreamInput::Complete).unwrap();
    let first_result = first.finish_json_request_body().unwrap();
    assert_eq!(first_result.findings().len(), 1);
    assert!(first_result.intervention().is_some());
    assert_eq!(Arc::strong_count(&generation), 3);
    assert_eq!(generation.json_capability.subscribers().unwrap().as_ptr(), shared_plan);
}

#[test]
fn json_cached_plan_keeps_inflight_generation_when_a_new_generation_is_compiled() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let old = Arc::new(generation(
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,deny,status:418\"\n",
    ));
    let mut inflight = Transaction::new_shared(Arc::clone(&old), request(&[], b"/"), RuntimeLimits::default()).unwrap();
    inflight.execute_request_headers().unwrap();
    inflight
        .start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
        .unwrap();
    inflight
        .feed_json_request_body(JsonStreamInput::Data(br#"{"a":"hi"#))
        .unwrap();
    let replacement = Arc::new(generation(
        "SecRule ARGS \"@pm miss\" \"id:2,phase:2,deny,status:419\"\n",
    ));
    assert_ne!(
        old.json_capability.subscribers().unwrap().as_ptr(),
        replacement.json_capability.subscribers().unwrap().as_ptr()
    );
    let mut fresh =
        Transaction::new_shared(Arc::clone(&replacement), request(&[], b"/"), RuntimeLimits::default()).unwrap();
    fresh.execute_request_headers().unwrap();
    let fresh_result = fresh
        .execute_json_body_inputs(
            BodyProcessorSelection::Json,
            [JsonStreamInput::Data(br#"{"a":"hit"}"#), JsonStreamInput::Complete],
            json_execution_limits(),
        )
        .unwrap();
    assert!(fresh_result.findings().is_empty());
    inflight
        .feed_json_request_body(JsonStreamInput::Data(br#"t"}"#))
        .unwrap();
    inflight.feed_json_request_body(JsonStreamInput::Complete).unwrap();
    assert_eq!(inflight.finish_json_request_body().unwrap().findings().len(), 1);
    assert_eq!(Arc::strong_count(&old), 2);
    assert_eq!(Arc::strong_count(&replacement), 2);
}

#[test]
fn json_mixed_actions_predicates_and_match_replacement_follow_reference_order() {
    let source = concat!(
        "SecAction \"id:1,phase:2,pass,setvar:tx.seed=1\"\n",
        "SecRule TX:seed \"@eq 1\" \"id:2,phase:2,pass,setvar:tx.seed=+2\"\n",
        "SecRule ARGS:json.a \"@pm hit\" \"id:3,phase:2,pass\"\n",
        "SecRule REQUEST_METHOD \"@streq POST\" \"id:4,phase:2,pass,setvar:tx.seed=99\"\n",
        "SecRule ARGS_NAMES \"@pm json\" \"id:5,phase:2,pass\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:6,phase:2,pass,setvar:tx.seed=-1\"\n",
        "SecAction \"id:7,phase:2,pass,setvar:tx.extra=value\"\n",
        "SecRule ARGS:json.a \"@pm hit\" \"id:8,phase:2,deny,status:418\"\n",
        "SecAction \"id:9,phase:2,pass,setvar:!tx.extra\"\n",
    );
    for body in [br#"{"a":"hit","b":"miss"}"#.as_slice(), br#"{"a":"miss"}"#, b"{}", b"["] {
        for split in 0..=body.len() {
            for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                compare_json_transaction(source, body, split, RuntimeLimits::default(), mode);
            }
        }
    }
}

#[test]
fn json_mixed_captures_and_ordinary_chains_preserve_tx_across_json_replay() {
    let source = concat!(
        "SecRule REQUEST_METHOD \"@rx ^(GET)$\" \"id:1,phase:2,pass,capture,chain\"\n",
        "SecRule TX:1 \"@streq GET\" \"setvar:tx.seed=7\"\n",
        "SecRule ARGS \"@pm hit\" \"id:2,phase:2,pass\"\n",
        "SecRule TX:1 \"@streq GET\" \"id:3,phase:2,pass,chain,setvar:tx.seed=+1\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"setvar:tx.extra=kept\"\n",
        "SecRule REQUEST_METHOD \"@streq POST\" \"id:4,phase:2,pass,chain,setvar:tx.seed=99\"\n",
        "SecRule ARGS_GET:q \"@streq hit\" \"setvar:tx.extra=wrong\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:6,phase:2,pass,chain\"\n",
        "SecRule MATCHED_VARS \"@streq GET\" \"setvar:tx.own_match=yes\"\n",
        "SecRule ARGS_NAMES \"!@pm absent\" \"id:5,phase:2,pass\"\n",
    );
    let body = br#"{"a":"hit","a":"miss","b":["hit","miss"]}"#;
    for split in 0..=body.len() {
        compare_json_transaction(source, body, split, RuntimeLimits::default(), RuleEngineMode::On);
    }
}

#[test]
fn json_mixed_skips_and_denial_hide_only_unreached_deferred_errors() {
    let body = format!(r#"{{"a":"hit","z":"{}"}}"#, "x".repeat(128));
    for jump in ["skip:1", "skipAfter:AFTER", "deny,status:418"] {
        for method in ["GET", "POST"] {
            let source = format!(
                concat!(
                    "SecRule REQUEST_METHOD \"@streq {}\" \"id:1,phase:2,pass,{}\"\n",
                    "SecRule ARGS:json.z \"@pm hit\" \"id:2,phase:2,pass\"\n",
                    "SecMarker AFTER\n",
                    "SecRule ARGS:json.a \"@pm hit\" \"id:3,phase:2,pass\"\n",
                    "SecAction \"id:4,phase:2,pass,setvar:tx.seed=9\"\n",
                ),
                method, jump
            );
            let source = source.replace("pass,deny", "deny");
            for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                compare_json_transaction(
                    &source,
                    body.as_bytes(),
                    19,
                    RuntimeLimits {
                        max_transformation_output_bytes: 16,
                        ..RuntimeLimits::default()
                    },
                    mode,
                );
            }
        }
    }
}

#[test]
fn json_mixed_runtime_limits_and_retry_state_match_complete_publication() {
    let source = concat!(
        "SecAction \"id:1,phase:2,pass,setvar:tx.seed=1\"\n",
        "SecRule ARGS \"@pm hit\" \"id:2,phase:2,pass\"\n",
        "SecRule REQUEST_METHOD \"@rx ^(GET)$\" \"id:3,phase:2,pass,capture,setvar:tx.seed=+1\"\n",
        "SecRule ARGS_NAMES \"@pm json\" \"id:4,phase:2,pass\"\n",
        "SecAction \"id:5,phase:2,pass,setvar:tx.extra=more\"\n",
    );
    let body = br#"{"a":"hit","b":["hit","miss"]}"#;
    let default = RuntimeLimits::default();
    for value in [0, 1, 2, 4, 8, 16, 32, 128, 512] {
        for limits in [
            RuntimeLimits {
                max_tx_variables: value,
                ..default
            },
            RuntimeLimits {
                max_tx_bytes: value,
                ..default
            },
            RuntimeLimits {
                max_selected_values: value,
                ..default
            },
            RuntimeLimits {
                max_operator_evaluations: value,
                ..default
            },
            RuntimeLimits {
                max_operator_work_bytes: value,
                ..default
            },
            RuntimeLimits {
                max_findings: value,
                ..default
            },
        ] {
            compare_json_transaction(source, body, 12, limits, RuleEngineMode::On);
        }
    }
}

#[test]
fn json_mixed_zero_subscribers_still_uses_ordinary_schedule_and_canonical_body_charge() {
    let source = concat!(
        "SecAction \"id:1,phase:2,pass,setvar:tx.seed=5\"\n",
        "SecRule ARGS_GET:q \"@streq hit\" \"id:2,phase:2,pass,setvar:tx.seed=+1\"\n",
        "SecRule TX:seed \"@eq 6\" \"id:3,phase:2,deny,status:418\"\n",
    );
    for body in [br#"{"a":"hit"}"#.as_slice(), b"{}", b"[", b""] {
        compare_json_transaction(source, body, 0, RuntimeLimits::default(), RuleEngineMode::On);
    }
}

#[test]
fn json_mixed_retry_reexecutes_preceding_committed_ordinary_actions() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let generation = generation(concat!(
        "SecAction \"id:1,phase:1,pass,setvar:tx.seed=5\"\n",
        "SecAction \"id:2,phase:2,pass,setvar:tx.seed=+1\"\n",
        "SecRule ARGS \"@pm hit\" \"id:3,phase:2,pass\"\n",
    ));
    let limits = RuntimeLimits {
        max_transformation_output_bytes: 16,
        ..RuntimeLimits::default()
    };
    let body = format!(r#"{{"a":"{}"}}"#, "x".repeat(64));
    let mut reference = Transaction::new(&generation, request(&[], b"/"), limits).unwrap();
    let mut streamed = Transaction::new(&generation, request(&[], b"/"), limits).unwrap();
    reference.execute_request_headers().unwrap();
    streamed.execute_request_headers().unwrap();
    reference
        .publish_request_body(RequestBody::new(body.as_bytes(), BodyProcessorSelection::Json))
        .unwrap();
    streamed
        .start_json_request_body(BodyProcessorSelection::Json, json_execution_limits())
        .unwrap();
    streamed
        .feed_json_request_body(JsonStreamInput::Data(body.as_bytes()))
        .unwrap();
    streamed.feed_json_request_body(JsonStreamInput::Complete).unwrap();
    let error = streamed.finish_json_request_body().map_err(json_runtime_error);
    assert!(error.is_err());
    assert_eq!(error, reference.execute_request_body());
    for expected in [6, 7, 8] {
        let expected = expected.to_string();
        assert_eq!(streamed.tx_value(b"seed"), Some(expected.as_bytes()));
        assert_eq!(streamed.tx_value(b"seed"), reference.tx_value(b"seed"));
        assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
        assert_eq!(streamed.json_body_state.len(), 1);
        assert_eq!(streamed.execute_request_body(), reference.execute_request_body());
    }
}

#[test]
fn json_mixed_missing_descriptor_never_falls_back_to_unavailable_body_values() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let mut generation = generation("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    // Simulate a broken private compiled capability: omission is not permission
    // to evaluate a body-dependent rule against metadata-only publication.
    generation.json_capability = super::super::json_plan::JsonCapability::Eligible(Vec::new());
    let mut tx = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    assert_eq!(
        tx.execute_json_body_inputs(
            BodyProcessorSelection::Json,
            [JsonStreamInput::Data(br#"{"a":"hit"}"#), JsonStreamInput::Complete],
            json_execution_limits()
        ),
        Err(json_rules::JsonExecutionError::Runtime(
            RuntimeError::GenerationInvariant
        ))
    );
    assert_eq!(tx.next_phase(), Some(Phase::RequestBody));
    assert_eq!(tx.execute_request_body(), Err(RuntimeError::GenerationInvariant));
}

#[test]
fn json_mixed_retained_boundary_includes_ordinary_tx_and_capture_base() {
    use crate::seclang::BodyProcessorSelection;
    let source = format!(
        concat!(
            "SecAction \"id:1,phase:2,pass,setvar:tx.seed={}\"\n",
            "SecRule REQUEST_METHOD \"@rx ^(GET)$\" \"id:2,phase:2,pass,capture\"\n",
            "SecRule ARGS \"@pm hit\" \"id:3,phase:2,pass\"\n",
        ),
        "x".repeat(128)
    );
    let body = format!(r#"{{"a":"hit{}","b":"hit{}"}}"#, "x".repeat(64), "y".repeat(64));
    let generation = generation(&source);
    let mut reference =
        Transaction::new(&generation, request(&[], b"/?q=hit&q=miss"), RuntimeLimits::default()).unwrap();
    reference.execute_request_headers().unwrap();
    reference
        .publish_request_body(RequestBody::new(body.as_bytes(), BodyProcessorSelection::Json))
        .unwrap();
    reference.execute_request_body().unwrap();
    assert_eq!(reference.tx_value(b"1"), Some(b"GET".as_slice()));
    assert_eq!(reference.tx_value(b"seed").unwrap().len(), 128);
    let retained = reference.retained_bytes();
    for max_retained_bytes in [retained - 1, retained, retained + 1] {
        compare_json_transaction(
            &source,
            body.as_bytes(),
            31,
            RuntimeLimits {
                max_retained_bytes,
                ..RuntimeLimits::default()
            },
            RuleEngineMode::On,
        );
    }
}

#[test]
fn json_macros_query_to_tx_dynamic_operators_keys_and_chain_matches_match_reference() {
    let source = concat!(
        "SecAction \"id:1,phase:1,pass,setvar:tx.seed=%{ARGS.q},setvar:tx.method=GET\"\n",
        "SecRule ARGS_GET:q \"@streq %{TX.seed}\" \"id:2,phase:2,pass,setvar:tx.key_%{MATCHED_VAR}=%{MATCHED_VAR_NAME},msg:'%{REQUEST_LINE}',logdata:'%{REMOTE_ADDR}'\"\n",
        "SecRule ARGS \"@pm hit\" \"id:3,phase:2,pass\"\n",
        "SecAction \"id:4,phase:2,pass,setvar:tx.inherited=%{MATCHED_VAR}\"\n",
        "SecRule REQUEST_METHOD \"@rx ^(GET)$\" \"id:5,phase:2,pass,capture,chain,setvar:tx.capture=%{TX.1}\"\n",
        "SecRule TX:1 \"@streq %{MATCHED_VAR}\" \"setvar:tx.chain=%{MATCHED_VAR_NAME}\"\n",
        "SecRule REQUEST_METHOD \"@streq %{TX.method}\" \"id:6,phase:2,pass,msg:'%{MATCHED_VAR}',logdata:'%{TX.capture}'\"\n",
        "SecRule ARGS_NAMES \"@pm json\" \"id:7,phase:2,pass\"\n",
    );
    for body in [br#"{"a":"hit"}"#.as_slice(), br#"{"a":"miss"}"#, b"{}", b"["] {
        for split in 0..=body.len() {
            compare_json_transaction(source, body, split, RuntimeLimits::default(), RuleEngineMode::On);
        }
    }
}

#[test]
fn json_macros_later_phases_clear_virtual_matches_and_keep_materialized_tx() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    let source = concat!(
        "SecAction \"id:1,phase:1,pass,setvar:tx.seed=%{REQUEST_HEADERS.X-Seed}\"\n",
        "SecRule ARGS \"@pm hit\" \"id:2,phase:2,pass\"\n",
        "SecRule REQUEST_METHOD \"@streq POST\" \"id:3,phase:2,pass,msg:'%{MATCHED_VAR}'\"\n",
        "SecAction \"id:4,phase:3,pass,setvar:tx.inherited=%{MATCHED_VAR},setvar:tx.inherited_name=%{MATCHED_VAR_NAME}\"\n",
        "SecRule REQUEST_METHOD \"@streq %{TX.seed}\" \"id:5,phase:3,pass,setvar:tx.own=%{MATCHED_VAR},msg:'%{MATCHED_VAR_NAME}'\"\n",
        "SecRule TX:own \"@streq %{TX.seed}\" \"id:6,phase:4,pass,logdata:'%{TX.own}'\"\n",
        "SecAction \"id:7,phase:5,pass,setvar:tx.logged=%{TX.own},msg:'%{REQUEST_LINE}'\"\n",
    );
    let generation = generation(source);
    let headers = [RequestHeader::new(b"X-Seed", b"GET")];
    let make = || Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
    let mut reference = make();
    let mut streamed = make();
    assert_eq!(reference.execute_request_headers(), streamed.execute_request_headers());
    let body = br#"{"a":"hit"}"#;
    reference
        .publish_request_body(RequestBody::new(body, BodyProcessorSelection::Json))
        .unwrap();
    assert_eq!(
        streamed
            .execute_json_body_inputs(
                BodyProcessorSelection::Json,
                [JsonStreamInput::Data(body), JsonStreamInput::Complete],
                json_execution_limits()
            )
            .unwrap(),
        reference.execute_request_body().unwrap()
    );
    assert!(streamed.tx.virtual_matches.is_some());
    reference.publish_response_head(ResponseHead::new(200, &[])).unwrap();
    streamed.publish_response_head(ResponseHead::new(200, &[])).unwrap();
    assert_eq!(
        streamed.execute_response_headers(),
        reference.execute_response_headers()
    );
    for key in [b"inherited".as_slice(), b"inherited_name"] {
        assert_eq!(streamed.tx_value(key), Some(b"".as_slice()));
        assert_eq!(streamed.tx_value(key), reference.tx_value(key));
    }
    assert_eq!(streamed.tx_value(b"own"), Some(b"GET".as_slice()));
    assert_eq!(
        streamed.execute_response_window(ResponseWindow::new(b"", true)),
        reference.execute_response_window(ResponseWindow::new(b"", true))
    );
    assert_eq!(streamed.execute_logging(), reference.execute_logging());
    assert_eq!(streamed.tx_value(b"logged"), Some(b"GET".as_slice()));
    assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
}

#[test]
fn json_macros_expansion_limits_skips_denial_and_retry_match_reference() {
    use crate::seclang::{BodyProcessorSelection, JsonStreamInput};
    for action in ["pass", "pass,skip:1", "deny,status:418"] {
        let source = format!(concat!(
            "SecAction \"id:1,phase:1,pass,setvar:tx.seed={}\"\n",
            "SecRule ARGS \"@pm hit\" \"id:2,phase:2,pass\"\n",
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:3,phase:2,{}\"\n",
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:4,phase:2,pass,setvar:tx.rollback=yes,msg:'%{{TX.seed}}%{{TX.seed}}'\"\n",
        ), "x".repeat(40), action);
        let mut frontend = FrontendLimits::default();
        frontend.compiler.max_template_runtime_bytes = 64;
        let generation = generation_with_frontend_limits(&source, frontend);
        for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
            let make = || {
                Transaction::new_with_modes(
                    &generation,
                    request(&[], b"/"),
                    RuntimeLimits::default(),
                    EvaluationMode::Accelerated,
                    mode,
                )
                .unwrap()
            };
            let mut reference = make();
            let mut streamed = make();
            reference.execute_request_headers().unwrap();
            streamed.execute_request_headers().unwrap();
            let body = br#"{"a":"hit"}"#;
            reference
                .publish_request_body(RequestBody::new(body, BodyProcessorSelection::Json))
                .unwrap();
            let actual = streamed
                .execute_json_body_inputs(
                    BodyProcessorSelection::Json,
                    [JsonStreamInput::Data(body), JsonStreamInput::Complete],
                    json_execution_limits(),
                )
                .map_err(json_runtime_error);
            assert_eq!(actual, reference.execute_request_body());
            let fails = action == "pass" || action.starts_with("deny") && mode == RuleEngineMode::DetectionOnly;
            assert_eq!(actual.is_err(), fails);
            assert_eq!(streamed.tx_value(b"rollback"), None);
            assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
            if fails {
                assert_eq!(actual, Err(RuntimeError::ValueByteLimit { actual: 80, limit: 64 }));
                assert_eq!(streamed.execute_request_body(), reference.execute_request_body());
                assert_eq!(streamed.retained_bytes(), reference.retained_bytes());
            }
        }
    }
}
