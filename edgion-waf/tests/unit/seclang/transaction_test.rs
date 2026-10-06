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
            bytes::Bytes::copy_from_slice(b"item=from-body"),
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
        bytes::Bytes::copy_from_slice(&payload),
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
                bytes::Bytes::copy_from_slice(body),
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
        bytes::Bytes::copy_from_slice(br#"{"user":{"name":"alice"},"roles":["a","b"]}"#),
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
        bytes::Bytes::copy_from_slice(br#"<root mode="dangerous"><item>attack</item></root>"#),
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
fn failed_json_publication_is_atomic_and_sticky() {
    let generation = generation("SecRule ARGS:a \"@streq ok\" \"id:69,phase:2,pass\"\n");
    let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    assert!(matches!(
        transaction.publish_request_body(RequestBody::new(
            bytes::Bytes::copy_from_slice(b"{private"),
            super::super::super::BodyProcessorSelection::Json
        )),
        Err(RuntimeError::Body {
            kind: BodyError::Malformed {
                processor: super::super::super::BodyProcessorSelection::Json
            }
        })
    ));
    assert!(transaction.request.body.is_none());
    assert_eq!(transaction.next_phase(), Some(Phase::RequestBody));
    assert!(transaction
        .publish_request_body(RequestBody::new(
            bytes::Bytes::from_static(b"a=ok"),
            super::super::super::BodyProcessorSelection::UrlEncoded
        ))
        .is_err());
    assert!(transaction.execute_request_body().is_err());
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
            bytes::Bytes::copy_from_slice(b"{private"),
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
        .publish_request_body(RequestBody::new(
            bytes::Bytes::copy_from_slice(b""),
            super::super::super::BodyProcessorSelection::Raw,
        ))
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
        .publish_request_body(RequestBody::new(
            bytes::Bytes::copy_from_slice(b""),
            super::super::super::BodyProcessorSelection::Raw,
        ))
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
        .publish_request_body(RequestBody::new(
            bytes::Bytes::copy_from_slice(b""),
            super::super::super::BodyProcessorSelection::Raw,
        ))
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
        tx.publish_request_body(RequestBody::new(
            bytes::Bytes::copy_from_slice(b""),
            super::super::super::BodyProcessorSelection::Auto,
        ))
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
        bytes::Bytes::copy_from_slice(b"{\"name\":\"blocked\"}"),
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
    assert_eq!(
        tx.request.first_value(CollectionKind::RequestBody),
        Some(b"{\"name\":\"blocked\"}".as_slice())
    );
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
        bytes::Bytes::copy_from_slice(b"blocked"),
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
        tx.publish_request_body(RequestBody::new(
            bytes::Bytes::copy_from_slice(body),
            super::super::super::BodyProcessorSelection::Xml,
        ))
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
    tx.publish_request_body(RequestBody::new(
        bytes::Bytes::copy_from_slice(b""),
        super::super::super::BodyProcessorSelection::Auto,
    ))
    .unwrap();
    assert!(matches!(
        tx.execute_request_body(),
        Err(RuntimeError::OperatorWorkLimit { limit: 2 })
    ));
}

impl RequestState<'_> {
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
            bytes::Bytes::copy_from_slice(br#"{"key":"/blocked"}"#),
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
                bytes::Bytes::copy_from_slice(body),
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
            bytes::Bytes::copy_from_slice(br#"{"z":"attack-last","m":"safe","a":"attack-first"}"#),
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
                bytes::Bytes::copy_from_slice(body),
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
            bytes::Bytes::copy_from_slice(br#"{"a":"alpha-1"}"#),
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
            bytes::Bytes::copy_from_slice(br#"{"z":"beta-2","a":"alpha-1"}"#),
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
                bytes::Bytes::copy_from_slice(body),
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
                bytes::Bytes::copy_from_slice(body.as_bytes()),
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
            bytes::Bytes::copy_from_slice(br#"{"a":"hit","z":"hit-again"}"#),
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
                bytes::Bytes::copy_from_slice(body.as_bytes()),
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
fn single_input_validates_later_transform_before_an_early_multimatch_hit() {
    let generation =
        generation("SecRule REQUEST_HEADERS:X-Test \"@streq a\" \"id:870001,phase:1,pass,multiMatch,t:hexEncode\"\n");
    let headers = [RequestHeader::new(b"X-Test", b"a")];
    let limits = RuntimeLimits {
        max_transformation_output_bytes: 1,
        ..RuntimeLimits::default()
    };
    let mut transaction = Transaction::new(&generation, request(&headers, b"/"), limits).unwrap();
    assert_eq!(
        transaction.execute_request_headers(),
        Err(RuntimeError::Transformation {
            kind: RuntimeTransformError::OutputLimit
        })
    );
    assert!(transaction.tx.matched_var.is_none());
}

#[test]
fn single_input_capture_then_match_count_then_payload_work_error_precedence() {
    let generation = generation("SecRule REQUEST_HEADERS:X-Test \"@rx (a)\" \"id:870002,phase:1,pass,capture\"\n");
    let headers = [RequestHeader::new(b"X-Test", b"a")];
    for (capture_bytes, evaluations, expected) in [
        (0, 0, RuntimeError::CaptureByteLimit { limit: 0 }),
        (64, 0, RuntimeError::OperatorEvaluationLimit { limit: 0 }),
        (64, 1, RuntimeError::OperatorWorkLimit { limit: 2 }),
    ] {
        let limits = RuntimeLimits {
            max_capture_bytes: capture_bytes,
            max_operator_evaluations: evaluations,
            max_operator_work_bytes: 2,
            ..RuntimeLimits::default()
        };
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), limits).unwrap();
        // Exercise candidate limits directly: the phase scheduler independently
        // rejects a zero unit budget before reaching any candidate.
        assert_eq!(
            transaction
                .evaluate_member_matches(0, 0, &TxState::default(), false)
                .err(),
            Some(expected)
        );
        assert_eq!(transaction.tx_value(b"0"), None);
        assert!(transaction.tx.matched_var.is_none());
    }
}

#[test]
fn single_input_operator_work_resets_for_each_member_invocation() {
    let generation = generation(concat!(
        "SecRule REQUEST_HEADERS:X-Test \"@streq absent\" \"id:870003,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Test \"@streq absent\" \"id:870004,phase:1,pass\"\n",
    ));
    let headers = [RequestHeader::new(b"X-Test", b"a")];
    let mut transaction = Transaction::new(
        &generation,
        request(&headers, b"/"),
        RuntimeLimits {
            max_operator_work_bytes: 2,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    assert!(transaction.execute_request_headers().unwrap().findings().is_empty());
    assert_eq!(transaction.detector_work_bytes, 0);
}

struct CaptureProbe {
    reject_admission: bool,
    admissions: usize,
    actual: usize,
}
impl PredicateStageObserver for CaptureProbe {
    type Error = ();
    fn charge_stage(&mut self, _: usize, _: usize) -> Result<(), ()> {
        Ok(())
    }
    fn before_captures(&mut self, _: usize) -> Result<(), ()> {
        Ok(())
    }
    fn captures(&mut self, _: &[CaptureEntry]) -> Result<(), ()> {
        Ok(())
    }
    fn admit_capture_allocation(&mut self, _: usize, _: usize) -> Result<(), ()> {
        self.admissions += 1;
        if self.reject_admission {
            Err(())
        } else {
            Ok(())
        }
    }
    fn capture_capacity(&mut self, actual: usize) -> Result<(), ()> {
        self.actual = actual;
        Ok(())
    }
}

#[test]
fn predicate_capture_admission_precedes_allocation_and_preserves_semantic_limits() {
    let mut captures = Vec::new();
    let mut observer = CaptureProbe {
        reject_admission: true,
        admissions: 0,
        actual: 0,
    };
    assert!(matches!(
        push_capture(&mut captures, b"0", b"secret", RuntimeLimits::default(), &mut observer),
        Err(PredicateStageFailure::Observer(()))
    ));
    assert_eq!(captures.capacity(), 0);
    assert_eq!(observer.actual, 0);
    observer.admissions = 0;
    let limits = RuntimeLimits {
        max_capture_bytes: 0,
        ..RuntimeLimits::default()
    };
    assert!(matches!(
        push_capture(&mut captures, b"0", b"secret", limits, &mut observer),
        Err(PredicateStageFailure::Semantic {
            checkpoint: PredicateCheckpoint::Captures,
            error: RuntimeError::CaptureByteLimit { limit: 0 }
        })
    ));
    assert_eq!(observer.admissions, 0);
    observer.reject_admission = false;
    assert!(push_capture(&mut captures, b"0", b"value", RuntimeLimits::default(), &mut observer).is_ok());
    assert_eq!(observer.admissions, 3);
    assert_eq!(observer.actual, capture_retained_bytes(&captures).unwrap());
    assert!(matches!(
        PredicateStageFailure::<()>::from(RuntimeError::Allocation),
        PredicateStageFailure::Physical(RuntimeError::Allocation)
    ));
    assert!(matches!(
        PredicateStageFailure::<()>::at(PredicateCheckpoint::Operator, RuntimeError::Arithmetic),
        PredicateStageFailure::Physical(RuntimeError::Arithmetic)
    ));
}

#[test]
fn new_unit_clone_omits_matches_but_preserves_entries_and_controls() {
    let limits = RuntimeLimits::default();
    let mut state = TxState::default();
    state.set(b"capture".to_vec(), b"persistent".to_vec(), limits).unwrap();
    state.add_removed_unit(7, limits).unwrap();
    state.request_body_processor = Some(crate::seclang::BodyProcessorSelection::Json);
    state.matched_var = Some(vec![b'x'; 4096].into());
    state.matched_var_name = Some(b"ARGS:json.name".to_vec().into());
    state.matched_vars.push(TxEntry {
        key: b"ARGS:json.name".to_vec().into(),
        value: vec![b'x'; 4096].into(),
    });
    state.recount();
    let full = state.try_clone().unwrap();
    assert_eq!(full.retained_bytes, state.retained_bytes);
    assert_eq!(full.matched_vars.len(), 1);
    let unit = state.try_clone_for_unit().unwrap();
    assert_eq!(unit.get(b"capture"), Some(&MatchValue::Owned(b"persistent".to_vec())));
    assert_eq!(unit.removed_units, [7]);
    assert_eq!(unit.request_body_processor, state.request_body_processor);
    assert!(unit.matched_var.is_none());
    assert!(unit.matched_var_name.is_none());
    assert_eq!(unit.matched_vars.capacity(), 0);
    assert!(unit.retained_bytes < state.retained_bytes);
}

#[test]
fn selection_scratch_reuses_capacity_and_no_hit_matches_allocate_nothing() {
    let generation = generation("SecRule ARGS \"@streq absent\" \"id:1,phase:2,pass\"\n");
    let mut transaction = Transaction::new(
        &generation,
        request(&[], b"/?a=one&b=two&c=three"),
        RuntimeLimits::default(),
    )
    .unwrap();
    let before = transaction.retained_bytes();
    let staged = TxState::default();
    let first = transaction.evaluate_member_matches(0, 0, &staged, false).unwrap();
    assert_eq!(first.capacity(), 0);
    assert!(transaction.selected_scratch.is_empty());
    let capacity = transaction.selected_scratch.capacity();
    let pointer = transaction.selected_scratch.as_ptr();
    assert!(capacity >= 3);
    assert_eq!(
        transaction.retained_bytes() - before,
        capacity * size_of::<SelectedValue>()
    );
    let second = transaction.evaluate_member_matches(0, 0, &staged, false).unwrap();
    assert_eq!(second.capacity(), 0);
    assert_eq!(transaction.selected_scratch.as_ptr(), pointer);
    assert_eq!(transaction.selected_scratch.capacity(), capacity);
}

#[test]
fn selection_scratch_restores_after_limit_error_and_stop_first() {
    let generation = generation("SecRule ARGS \"@contains o\" \"id:1,phase:2,pass\"\n");
    let mut transaction = Transaction::new(
        &generation,
        request(&[], b"/?a=one&b=two&c=three"),
        RuntimeLimits {
            max_selected_values: 2,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        transaction.evaluate_member_matches(0, 0, &TxState::default(), false),
        Err(RuntimeError::SelectedValueLimit { limit: 2 })
    ));
    assert!(transaction.selected_scratch.is_empty());
    assert_eq!(transaction.selected_scratch.capacity(), 2);
    transaction.limits.max_selected_values = 3;
    let matches = transaction
        .evaluate_member_matches(0, 0, &TxState::default(), true)
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches.capacity(), 1);
    assert!(transaction.selected_scratch.is_empty());
}

#[test]
fn selection_scratch_capacity_obeys_exact_retained_budget() {
    let generation = generation("SecRule ARGS \"@streq absent\" \"id:1,phase:2,pass\"\n");
    for slots in [0, 1, 2] {
        let mut transaction =
            Transaction::new(&generation, request(&[], b"/?a=one&b=two"), RuntimeLimits::default()).unwrap();
        let before = transaction.retained_bytes();
        transaction.limits.max_retained_bytes = before + slots * size_of::<SelectedValue>();
        let result = transaction.evaluate_member_matches(0, 0, &TxState::default(), false);
        assert_eq!(result.is_ok(), slots == 2);
        if slots != 2 {
            assert!(matches!(result, Err(RuntimeError::RetainedByteLimit { .. })));
        }
        assert!(transaction.selected_scratch.is_empty());
        assert!(transaction.retained_bytes() <= transaction.limits.max_retained_bytes);
    }
}

#[test]
fn selection_cursor_preserves_target_multiplicity_exclusions_and_count() {
    let generation = generation(concat!(
        "SecRule ARGS|ARGS:a|!ARGS:b \"@rx .+\" \"id:1,phase:2,pass\"\n",
        "SecRule &ARGS \"@eq 3\" \"id:2,phase:2,pass\"\n",
    ));
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        let mut transaction = Transaction::new_with_evaluation(
            &generation,
            request(&[], b"/?a=one&b=two&c=three"),
            RuntimeLimits::default(),
            mode,
        )
        .unwrap();
        let matches = transaction
            .evaluate_member_matches(0, 0, &TxState::default(), false)
            .unwrap();
        assert_eq!(
            matches
                .iter()
                .map(|matched| matched
                    .matched_var
                    .as_ref()
                    .unwrap()
                    .bytes(&transaction.request, None)
                    .unwrap())
                .collect::<Vec<_>>(),
            [b"one".as_slice(), b"three".as_slice(), b"one".as_slice()]
        );
        let count = transaction
            .evaluate_member_matches(1, 1, &TxState::default(), false)
            .unwrap();
        assert_eq!(count.len(), 1);
        assert_eq!(
            count[0]
                .matched_var
                .as_ref()
                .unwrap()
                .bytes(&transaction.request, None)
                .unwrap(),
            b"3"
        );
    }
}

#[test]
fn retained_selection_capacity_is_checked_before_staged_branch_selection() {
    let generation = generation("SecRule ARGS \"@streq absent\" \"id:1,phase:2,pass\"\n");
    let mut transaction =
        Transaction::new(&generation, request(&[], b"/?a=one&b=two"), RuntimeLimits::default()).unwrap();
    transaction
        .evaluate_member_matches(0, 0, &TxState::default(), false)
        .unwrap();
    transaction.limits.max_retained_bytes = transaction.retained_bytes();
    let mut staged = TxState::default();
    staged
        .set(b"x".to_vec(), b"y".to_vec(), RuntimeLimits::default())
        .unwrap();
    let before = transaction.detector_work_bytes;
    assert!(matches!(
        transaction.evaluate_member_matches(0, 0, &staged, false),
        Err(RuntimeError::RetainedByteLimit { .. })
    ));
    assert_eq!(transaction.detector_work_bytes, before);
    assert!(transaction.selected_scratch.is_empty());
}

#[test]
fn unit_match_buffer_reuses_capacity_and_keeps_loaned_storage_charged() {
    let generation = generation(concat!(
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@contains hit\" \"id:2,phase:2,pass\"\n",
    ));
    let mut transaction = Transaction::new(
        &generation,
        request(&[], b"/?a=hit&b=hit&c=hit"),
        RuntimeLimits::default(),
    )
    .unwrap();
    let original_scratch = transaction.scratch_bytes;
    assert!(transaction.evaluate_unit(0, None).unwrap());
    let pointer = transaction.match_scratch.as_ptr();
    let capacity = transaction.match_scratch.capacity();
    assert!(capacity >= 3);
    assert!(transaction.match_scratch.is_empty());
    assert_eq!(transaction.match_scratch_bytes, capacity * size_of::<MemberMatch>());
    assert_eq!(
        transaction.scratch_bytes,
        original_scratch
            + transaction.selected_scratch.capacity() * size_of::<SelectedValue>()
            + transaction.match_scratch_bytes
    );
    assert!(transaction.evaluate_unit(1, None).unwrap());
    assert_eq!(transaction.match_scratch.as_ptr(), pointer);
    assert_eq!(transaction.match_scratch.capacity(), capacity);
    assert!(transaction.match_scratch.is_empty());
    assert_eq!(transaction.tx.matched_vars.len(), 3);
}

#[test]
fn unit_match_buffer_returns_after_action_error_and_early_chain_commit() {
    let generation = generation(concat!(
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,setvar:'tx.saved=done'\"\n",
        "SecRule ARGS \"@rx (hit)\" \"id:2,phase:2,pass,capture,chain\"\n",
        "SecRule TX:1 \"@streq hit\" \"\"\n",
    ));
    let mut transaction = Transaction::new(
        &generation,
        request(&[], b"/?a=hit&b=hit&c=hit"),
        RuntimeLimits {
            max_setvar_actions: 0,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        transaction.evaluate_unit(0, None),
        Err(RuntimeError::SetVarActionLimit { limit: 0 })
    ));
    assert!(transaction.tx_value(b"saved").is_none());
    assert!(transaction.match_scratch.is_empty());
    assert!(transaction.selected_scratch.is_empty());
    let pointer = transaction.match_scratch.as_ptr();
    let capacity = transaction.match_scratch.capacity();
    transaction.limits.max_setvar_actions = 10;
    assert!(transaction.evaluate_unit(0, None).unwrap());
    assert_eq!(transaction.tx_value(b"saved"), Some(b"done".as_slice()));
    assert!(transaction.evaluate_unit(1, None).unwrap());
    assert_eq!(transaction.tx_value(b"1"), Some(b"hit".as_slice()));
    assert_eq!(transaction.match_scratch.as_ptr(), pointer);
    assert_eq!(transaction.match_scratch.capacity(), capacity);
    assert!(transaction.match_scratch.is_empty());
    assert_eq!(transaction.match_scratch_bytes, capacity * size_of::<MemberMatch>());
}

#[test]
fn match_buffer_growth_admits_overlap_and_reuses_at_exact_capacity_limit() {
    let size = size_of::<MemberMatch>();
    let mut matches = Vec::new();
    let mut scratch = 100;
    let mut charged = 0;
    let storage = MatchStorage {
        fixed_bytes: 100,
        retained_limit: 100 + size,
        limit: 2,
    };
    storage
        .push(&mut matches, MemberMatch::default(), &mut scratch, &mut charged)
        .unwrap();
    let pointer = matches.as_ptr();
    assert_eq!(matches.capacity(), 1);
    assert_eq!(scratch, 100 + size);
    assert_eq!(charged, size);
    let limit = 100 + 3 * size - 1;
    assert_eq!(
        MatchStorage {
            retained_limit: limit,
            ..storage
        }
        .push(&mut matches, MemberMatch::default(), &mut scratch, &mut charged),
        Err(RuntimeError::RetainedByteLimit { limit })
    );
    assert_eq!(matches.len(), 1);
    assert_eq!(matches.as_ptr(), pointer);
    assert_eq!(scratch, 100 + size);
    matches.clear();
    storage
        .push(&mut matches, MemberMatch::default(), &mut scratch, &mut charged)
        .unwrap();
    assert_eq!(matches.as_ptr(), pointer);
    assert_eq!(charged, size);
    MatchStorage {
        retained_limit: 100 + 3 * size,
        ..storage
    }
    .push(&mut matches, MemberMatch::default(), &mut scratch, &mut charged)
    .unwrap();
    assert_eq!(matches.capacity(), 2);
    assert_eq!(scratch, 100 + 2 * size);
    assert_eq!(charged, 2 * size);
}

#[test]
fn no_hit_units_never_allocate_the_match_buffer() {
    let generation = generation("SecRule ARGS \"@streq absent\" \"id:1,phase:2,pass\"\n");
    let mut transaction =
        Transaction::new(&generation, request(&[], b"/?a=one&b=two"), RuntimeLimits::default()).unwrap();
    assert!(!transaction.evaluate_unit(0, None).unwrap());
    assert_eq!(transaction.match_scratch.capacity(), 0);
    assert_eq!(transaction.match_scratch_bytes, 0);
    assert!(transaction.selected_scratch.capacity() >= 2);
}

#[test]
fn capture_descriptor_capacity_tracks_operator_bound_and_optional_groups() {
    for (pattern, input, slots, keys) in [
        ("^(a)$", b"a".as_slice(), 4, vec![b"0".as_slice(), b"1"]),
        ("^(a)(b)?$", b"a".as_slice(), 6, vec![b"0".as_slice(), b"1"]),
        (
            "^(?P<word>a)$",
            b"a".as_slice(),
            4,
            vec![b"0".as_slice(), b"1", b"word"],
        ),
    ] {
        let generation = generation(&format!(
            "SecRule REQUEST_HEADERS:X-Probe \"@rx {pattern}\" \"id:1,phase:1,pass,capture\"\n"
        ));
        let headers = [RequestHeader::new(b"X-Probe", input)];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        let matches = transaction
            .evaluate_member_matches(0, 0, &TxState::default(), false)
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].captures.capacity(), slots);
        assert_eq!(
            matches[0]
                .captures
                .iter()
                .map(|capture| capture.key.as_slice())
                .collect::<Vec<_>>(),
            keys
        );
    }
}

#[test]
fn smaller_capture_reservation_preserves_named_entry_limits_and_large_growth() {
    let compiled = generation("SecRule REQUEST_HEADERS:X-Probe \"@rx ^(?P<word>a)$\" \"id:1,phase:1,pass,capture\"\n");
    let headers = [RequestHeader::new(b"X-Probe", b"a")];
    for limit in [0, 2, 3] {
        let mut transaction = Transaction::new(
            &compiled,
            request(&headers, b"/"),
            RuntimeLimits {
                max_capture_entries: limit,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        let result = transaction.evaluate_member_matches(0, 0, &TxState::default(), false);
        if limit == 3 {
            let matches = result.unwrap();
            assert_eq!(matches[0].captures.len(), 3);
            assert_eq!(matches[0].captures.capacity(), 3);
        } else {
            assert!(matches!(result, Err(RuntimeError::CaptureEntryLimit { limit: actual }) if actual == limit));
        }
    }
    let pattern = (0..34).map(|index| format!("(?P<g{index}>a)")).collect::<String>();
    let source = format!("SecRule REQUEST_HEADERS:X-Probe \"@rx ^{pattern}$\" \"id:1,phase:1,pass,capture\"\n");
    let mut frontend = FrontendLimits::default();
    frontend.compiler.regex.max_capture_slots = 128;
    frontend.compiler.regex.max_named_groups = 40;
    let compiled = generation_with_frontend_limits(&source, frontend);
    let input = "a".repeat(34);
    let headers = [RequestHeader::new(b"X-Probe", input.as_bytes())];
    for limit in [68, 69] {
        let mut transaction = Transaction::new(
            &compiled,
            request(&headers, b"/"),
            RuntimeLimits {
                max_capture_entries: limit,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        let result = transaction.evaluate_member_matches(0, 0, &TxState::default(), false);
        if limit == 69 {
            let matches = result.unwrap();
            assert_eq!(matches[0].captures.len(), 69);
            assert!(matches[0].captures.capacity() >= 69);
            assert_eq!(matches[0].captures.last().unwrap().key, b"g33");
        } else {
            assert!(matches!(result, Err(RuntimeError::CaptureEntryLimit { limit: 68 })));
        }
    }
}

#[test]
fn phrase_and_sqli_capture_reserve_one_descriptor() {
    for (operator, input, captured) in [
        ("@pm token", b"a token here".as_slice(), b"token".as_slice()),
        ("@detectSQLi", b"1 OR 1=1".as_slice(), b"1&1".as_slice()),
    ] {
        let generation = generation(&format!(
            "SecRule REQUEST_HEADERS:X-Probe \"{operator}\" \"id:1,phase:1,pass,capture\"\n"
        ));
        let headers = [RequestHeader::new(b"X-Probe", input)];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        let matches = transaction
            .evaluate_member_matches(0, 0, &TxState::default(), false)
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].captures.len(), 1);
        assert_eq!(matches[0].captures.capacity(), 1);
        assert_eq!(matches[0].captures[0].key, b"0");
        assert_eq!(
            matches[0].captures[0].value.bytes(&transaction.request, None).unwrap(),
            captured
        );
    }
}
