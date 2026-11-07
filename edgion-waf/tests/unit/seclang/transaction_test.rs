use super::*;

#[test]
fn ctl_tag_lookup_preserves_duplicate_units_and_missing_ranges() {
    let controls = concat!(
        "SecAction \"id:1,phase:1,pass,ctl:ruleRemoveByTag=aardvark,",
        "ctl:ruleRemoveByTag=alpha,ctl:ruleRemoveByTag=bet,ctl:ruleRemoveByTag=beta,",
        "ctl:ruleRemoveByTag=between,ctl:ruleRemoveByTag=omega,",
        "ctl:ruleRemoveByTag=zeta,ctl:ruleRemoveByTag=Beta\"\n",
    );
    let populated = generation(&format!(
        "{controls}{}",
        concat!(
            "SecRule REQUEST_URI \"@streq /\" \"id:2,phase:1,pass,tag:beta,tag:beta\"\n",
            "SecRule REQUEST_URI \"@streq /\" \"id:3,phase:1,pass,tag:alpha\"\n",
            "SecRule REQUEST_URI \"@streq /\" \"id:4,phase:1,pass,tag:omega\"\n",
            "SecRule REQUEST_URI \"@streq /\" \"id:5,phase:1,pass,tag:beta\"\n",
        )
    ));
    let empty = generation(controls);
    for (query, expected) in [
        ("aardvark", &[][..]),
        ("alpha", &[2][..]),
        ("bet", &[][..]),
        ("beta", &[1, 1, 4][..]),
        ("between", &[][..]),
        ("omega", &[3][..]),
        ("zeta", &[][..]),
        ("Beta", &[][..]),
    ] {
        for (compiled, expected) in [(&populated, expected), (&empty, &[][..])] {
            let tag = compiled
                .actions
                .iter()
                .find_map(|action| match action.operand {
                    super::super::CompiledActionOperand::Ctl(super::super::CompiledCtl::RuleRemoveByTag(tag))
                        if generation_bytes(&compiled.bytes, tag).unwrap() == query.as_bytes() =>
                    {
                        Some(tag)
                    }
                    _ => None,
                })
                .unwrap();
            let actual = tagged_units(compiled, tag).unwrap();
            assert_eq!(
                actual.iter().map(|entry| entry.unit).collect::<Vec<_>>(),
                expected,
                "{query}"
            );
            assert!(actual
                .iter()
                .all(|entry| generation_bytes(&compiled.bytes, entry.tag).unwrap() == query.as_bytes()));
        }
    }
}

#[test]
fn unsigned_integer_grammar_preserves_boundaries_and_rejects_signs() {
    let maximum = usize::MAX.to_string();
    let padded = format!("{}1", "0".repeat(128));
    for (input, expected) in [
        ("0", 0),
        ("00042", 42),
        (maximum.as_str(), usize::MAX),
        (padded.as_str(), 1),
    ] {
        assert_eq!(parse_usize(input.as_bytes()), Ok(expected), "{input:?}");
    }
    let overflow = format!("{maximum}0");
    for input in [
        "",
        "+",
        "-",
        "+0",
        "+42",
        "+00042",
        "-0",
        "-1",
        "++1",
        "--1",
        "1+",
        "1-",
        " 1",
        "1 ",
        "1\n",
        "\t1",
        "1_0",
        "1.0",
        "1e2",
        "0x10",
        "é",
        overflow.as_str(),
    ] {
        assert_eq!(
            parse_usize(input.as_bytes()),
            Err(RuntimeError::GenerationInvariant),
            "{input:?}"
        );
    }
    for byte in u8::MIN..=u8::MAX {
        let input = [b'1', byte, b'2'];
        let expected = if byte.is_ascii_digit() {
            Ok(102 + usize::from(byte - b'0') * 10)
        } else {
            Err(RuntimeError::GenerationInvariant)
        };
        assert_eq!(parse_usize(&input), expected, "middle byte {byte}");
    }
}

#[test]
fn signed_integer_grammar_preserves_boundaries_and_rejects_invalid_bytes() {
    for (input, expected) in [
        ("0", 0),
        ("+0", 0),
        ("-0", 0),
        ("00042", 42),
        ("+00042", 42),
        ("-00042", -42),
        ("9223372036854775807", i64::MAX),
        ("+9223372036854775807", i64::MAX),
        ("-9223372036854775808", i64::MIN),
    ] {
        assert_eq!(parse_i64(input.as_bytes()), Ok(expected), "{input:?}");
    }
    let padded = format!("{}1", "0".repeat(128));
    assert_eq!(parse_i64(padded.as_bytes()), Ok(1));
    for input in [
        "",
        "+",
        "-",
        "++1",
        "--1",
        "+-1",
        "-+1",
        "1+",
        "1-",
        " 1",
        "1 ",
        "1\n",
        "\t1",
        "1_0",
        "1.0",
        "1e2",
        "0x10",
        "é",
        "9223372036854775808",
        "+9223372036854775808",
        "-9223372036854775809",
    ] {
        assert_eq!(
            parse_i64(input.as_bytes()),
            Err(RuntimeError::IntegerConversion),
            "{input:?}"
        );
    }
    for byte in u8::MIN..=u8::MAX {
        let input = [b'1', byte, b'2'];
        let expected = if byte.is_ascii_digit() {
            Ok(102 + i64::from(byte - b'0') * 10)
        } else {
            Err(RuntimeError::IntegerConversion)
        };
        assert_eq!(parse_i64(&input), expected, "middle byte {byte}");
    }
}

pub(super) fn matcher_scratch_bytes(generation: &CompiledGeneration) -> Result<usize, RuntimeError> {
    matcher_scratch_capacity(
        matcher_program_scratch_bytes(generation)?,
        generation.matcher_count(),
        generation.boolean_regexes.len(),
        generation.capture_regexes.len(),
    )
}

#[test]
#[cfg(target_pointer_width = "64")]
fn xml_kind_variants_preserve_selected_value_storage_size() {
    // Frozen pre-optimization sizes: the count operand remains the largest payload.
    assert_eq!(size_of::<SelectedBytes>(), 48);
    assert_eq!(size_of::<SelectedValue>(), 80);
}

fn owned_key_request() -> RequestState<'static> {
    RequestState {
        bytes: Vec::new(),
        collections: std::array::from_fn(|_| Vec::new()),
        body: None,
        entry_count: 0,
        retained_bytes: 0,
    }
}

// Independent retained-capacity oracle for the incremental capture builder.
fn capture_retained_bytes<V: StageCapturePayload>(captures: &Vec<CaptureEntry<V>>) -> Result<usize, RuntimeError> {
    captures.iter().try_fold(
        captures
            .capacity()
            .checked_mul(size_of::<CaptureEntry<V>>())
            .ok_or(RuntimeError::Arithmetic)?,
        |bytes, entry| {
            bytes
                .checked_add(entry.key.owned_capacity())
                .and_then(|bytes| bytes.checked_add(entry.value.heap_capacity()))
                .ok_or(RuntimeError::Arithmetic)
        },
    )
}

impl TxState {
    // Repair private fixtures that deliberately write observer fields directly.
    fn recount(&mut self) {
        self.entry_payload_bytes = self
            .entries
            .iter()
            .try_fold(0_u128, |total, entry| {
                tx_payload_delta(total, 0, tx_payload_bytes(entry.key.len(), entry.value.len())?)
            })
            .expect("real entry lengths fit the exact payload counter");
        self.matched_vars_payload_bytes = self
            .matched_vars
            .iter()
            .try_fold(0_u128, |total, entry| {
                tx_payload_delta(total, 0, tx_payload_bytes(entry.key.len(), entry.value.len())?)
            })
            .expect("real observer lengths fit the exact payload counter");
        self.refresh_retained_bytes();
    }
}
use crate::seclang::{
    compile_with_includes, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink, IncludeSource, PhaseDefaults,
    SemanticValidationContext,
};

struct NoIncludes;

#[test]
fn tx_cached_totals_cover_sealed_captures_controls_and_later_phases() {
    let g = generation(concat!(
        "SecAction \"id:1,phase:1,pass,ctl:requestBodyProcessor=JSON,ctl:ruleRemoveByTag=gone,ctl:ruleRemoveTargetByTag=kept;ARGS:ignored\"\n",
        "SecRule ARGS:json.a \"@rx ^(?P<word>12)$\" \"id:2,phase:2,pass,capture,tag:kept,chain\"\n",
        "SecRule TX:1 \"@streq 12\" \"setvar:tx.1=+3,setvar:tx.saved=%{TX.word},setvar:!tx.0\"\n",
        "SecRule ARGS \"@contains 12\" \"id:3,phase:2,deny,tag:gone\"\n",
        "SecAction \"id:4,phase:3,pass,setvar:tx.later=%{TX.saved}\"\n",
    ));
    let mut tx = Transaction::new(&g, request(&[], b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    assert!(!tx.tx.removed_units.is_empty());
    assert!(!tx.tx.target_exclusions.is_empty());
    assert_tx_accounting(&tx.tx);
    tx.publish_request_body(RequestBody::new(
        bytes::Bytes::from_static(br#"{"a":"12"}"#),
        crate::seclang::BodyProcessorSelection::Auto,
    ))
    .unwrap();
    let outcome = tx.execute_request_body().unwrap();
    assert!(outcome.intervention().is_none());
    assert_eq!(tx.tx_value(b"1"), Some(b"15".as_slice()));
    assert_eq!(tx.tx_value(b"0"), None);
    assert!(matches!(
        tx.tx.get(b"word", &tx.request, tx.json_budget.as_mut()).unwrap(),
        Some(MatchValue::Json(_))
    ));
    assert_tx_accounting(&tx.tx);
    assert_tx_accounting(&tx.tx.try_clone().unwrap());
    assert_tx_accounting(&tx.tx.try_clone_for_unit().unwrap());
    tx.publish_response_head(ResponseHead::new(200, &[])).unwrap();
    tx.execute_response_headers().unwrap();
    assert_eq!(tx.tx_value(b"later"), Some(b"12".as_slice()));
    assert_tx_accounting(&tx.tx);
}

// Frozen pre-cache expression. Only call for representable payload sums.
fn old_tx_retained_bytes(state: &TxState) -> usize {
    state
        .entries
        .len()
        .saturating_mul(size_of::<TxEntry>())
        .saturating_add(
            state
                .entries
                .iter()
                .map(|entry| entry.key.len().saturating_add(entry.value.len()))
                .sum::<usize>(),
        )
        .saturating_add(state.matched_var.as_ref().map_or(0, MatchValue::len))
        .saturating_add(state.matched_var_name.as_ref().map_or(0, MatchValue::len))
        .saturating_add(
            state
                .matched_vars
                .len()
                .saturating_mul(size_of::<TxEntry<MatchValue, MatchValue>>()),
        )
        .saturating_add(state.removed_units.len().saturating_mul(size_of::<usize>()))
        .saturating_add(
            state
                .target_exclusions
                .len()
                .saturating_mul(size_of::<ActiveTargetExclusion>()),
        )
        .saturating_add(
            state
                .matched_vars
                .iter()
                .map(|entry| entry.key.len().saturating_add(entry.value.len()))
                .sum::<usize>(),
        )
}

fn assert_tx_accounting(state: &TxState) {
    assert_eq!(state.retained_bytes, old_tx_retained_bytes(state));
    assert_eq!(
        state.entry_payload_bytes,
        state
            .entries
            .iter()
            .map(|entry| entry.key.len() as u128 + entry.value.len() as u128)
            .sum::<u128>()
    );
    assert_eq!(
        state.matched_vars_payload_bytes,
        state
            .matched_vars
            .iter()
            .map(|entry| entry.key.len() as u128 + entry.value.len() as u128)
            .sum::<u128>()
    );
}

#[test]
fn tx_cached_totals_match_frozen_recount_after_mutations_and_failures() {
    let limits = RuntimeLimits::default();
    let mut state = TxState::default();
    let mut capture_keys = Vec::new();
    for index in 0..64 {
        let key = format!("key{index}").into_bytes();
        state
            .set(key.clone(), vec![b'a'; index], limits, &owned_key_request(), None)
            .unwrap();
        assert_tx_accounting(&state);
        state
            .set(
                format!("KEY{index}").into_bytes(),
                vec![b'b'; 64 - index],
                limits,
                &owned_key_request(),
                None,
            )
            .unwrap();
        assert_eq!(
            state.entries[index].key.observed_bytes(&owned_key_request()).unwrap(),
            key
        );
        assert_tx_accounting(&state);
        apply_member_match(
            &mut state,
            &mut capture_keys,
            MemberMatch {
                matched_var: Some(vec![b'x'; index].into()),
                matched_var_name: Some(key.into()),
                captures: Vec::new(),
            },
            limits,
            &owned_key_request(),
            None,
        )
        .unwrap();
        assert_tx_accounting(&state);
    }
    for index in (0..64).rev() {
        state
            .delete(format!("KEY{index}").as_bytes(), &owned_key_request(), None)
            .unwrap();
        assert_tx_accounting(&state);
        state.delete(b"absent", &owned_key_request(), None).unwrap();
        assert_tx_accounting(&state);
    }
    apply_member_match(
        &mut state,
        &mut capture_keys,
        MemberMatch::default(),
        limits,
        &owned_key_request(),
        None,
    )
    .unwrap();
    assert_tx_accounting(&state);
    state.add_removed_unit(7, limits).unwrap();
    state.add_removed_unit(7, limits).unwrap();
    assert_tx_accounting(&state);
    assert_tx_accounting(&state.try_clone().unwrap());
    let unit = state.try_clone_for_unit().unwrap();
    assert_tx_accounting(&unit);
    assert_eq!(unit.matched_vars_payload_bytes, 0);
    assert!(unit.matched_vars.is_empty());
    let limited = RuntimeLimits {
        max_tx_bytes: state.retained_bytes,
        ..limits
    };
    assert!(matches!(
        state.set(b"failed".to_vec(), vec![b'a'; 128], limited, &owned_key_request(), None),
        Err(RuntimeError::TxByteLimit { .. })
    ));
    assert_eq!(
        state.get(b"failed", &owned_key_request(), None).unwrap().unwrap().len(),
        128
    );
    assert_tx_accounting(&state);
    let before = state.retained_bytes;
    assert!(matches!(
        state.set(
            b"over-count".to_vec(),
            Vec::new(),
            RuntimeLimits {
                max_tx_variables: state.entries.len(),
                ..limits
            },
            &owned_key_request(),
            None
        ),
        Err(RuntimeError::TxVariableLimit { .. })
    ));
    assert_eq!(state.retained_bytes, before);
    assert_tx_accounting(&state);
}

#[test]
fn tx_wide_payload_totals_saturate_only_the_final_charge() {
    let mut state = TxState::default();
    let wide = usize::MAX as u128 + 17;
    state.entry_payload_bytes = wide;
    state.refresh_retained_bytes();
    assert_eq!(state.retained_bytes, usize::MAX);
    state.entry_payload_bytes = tx_payload_delta(state.entry_payload_bytes, 18, 0).unwrap();
    state.refresh_retained_bytes();
    assert_eq!(state.retained_bytes, usize::MAX - 1);
    assert_eq!(tx_payload_delta(0, 1, 0), Err(RuntimeError::Arithmetic));
    assert_eq!(tx_payload_delta(u128::MAX, 0, 1), Err(RuntimeError::Arithmetic));
    state = TxState::default();
    state
        .set(
            b"key".to_vec(),
            b"value".to_vec(),
            RuntimeLimits::default(),
            &owned_key_request(),
            None,
        )
        .unwrap();
    state.entry_payload_bytes = 0;
    assert_eq!(
        state.delete(b"KEY", &owned_key_request(), None),
        Err(RuntimeError::Arithmetic)
    );
    assert_eq!(state.get(b"key", &owned_key_request(), None).unwrap().unwrap().len(), 5);
    state.recount();
    assert_tx_accounting(&state);
}

#[test]
fn tx_accounting_layout_reports_actual_branch_sizes() {
    println!(
        "TX_ACCOUNTING_LAYOUT,tx_state={},chain_branch={}",
        size_of::<TxState>(),
        size_of::<ChainBranch>()
    );
    let state = TxState::default();
    assert_eq!(state.retained_bytes, 0);
    let branches = vec![ChainBranch {
        tx: state,
        capture_keys: Vec::new(),
        setvar_actions: 0,
    }];
    assert_eq!(chain_branches_bytes(&branches).unwrap(), size_of::<ChainBranch>());
}

#[test]
fn shared_capture_key_layout_and_owned_builder_use_actual_storage() {
    println!("CAPTURE_KEY_LAYOUT,key_owned={},key_shared={},entry_owned={},entry_shared={},capture_owned={},capture_shared={},tx_state={},chain_branch={}",
        size_of::<Vec<u8>>(), size_of::<MatchValue>(),
        size_of::<TxEntry<MatchValue, Vec<u8>>>(), size_of::<TxEntry>(),
        size_of::<CaptureEntry<MatchValue, Vec<u8>>>(), size_of::<CaptureEntry>(),
        size_of::<TxState>(), size_of::<ChainBranch>());
    let mut captures = CaptureBuilder::default();
    let mut observer = CaptureProbe {
        reject_admission: false,
        admissions: 0,
        actual: 0,
    };
    push_capture(
        &mut captures,
        CaptureKeySource::Numeric { index: 1, bytes: b"1" },
        CaptureSource::InputRange {
            input: b"value",
            start: 0,
            len: 5,
        },
        RuntimeLimits::default(),
        &mut observer,
    )
    .unwrap_or_else(|_| panic!("owned capture"));
    assert!(matches!(captures.entries[0].key, MatchValue::Owned(_)));
    let pointer = captures.entries.as_ptr();
    let capacity = captures.entries.capacity();
    assert_eq!(captures.retained_bytes().unwrap(), observer.actual);
    let matched = finish_member_match(
        captures.entries,
        Vec::new(),
        Vec::new(),
        0,
        &mut 0,
        RuntimeLimits::default(),
    )
    .unwrap();
    assert_eq!(matched.captures.as_ptr(), pointer);
    assert_eq!(matched.captures.capacity(), capacity);
}

#[test]
fn borrowed_single_match_query_dynamic_json_and_tx_keep_owned_observers() {
    let g = generation(concat!(
        "SecAction \"id:1,phase:1,pass,setvar:tx.expected=hit\"\n",
        "SecRule ARGS_GET:q \"@streq hit\" \"id:2,phase:1,pass,t:lowercase,setvar:tx.query=%{MATCHED_VAR}\"\n",
        "SecRule ARGS:json.a \"@streq %{TX.expected}\" \"id:3,phase:2,pass,t:lowercase,t:removeNulls,t:htmlEntityDecode,t:urlDecodeUni,setvar:tx.json=%{MATCHED_VAR}\"\n",
        "SecRule TX:json \"@streq hit\" \"id:4,phase:2,pass,t:lowercase,setvar:tx.from_tx=%{MATCHED_VAR}\"\n",
    ));
    for value in ["hit".to_string(), "HIT".to_string(), "x".repeat(8192)] {
        let mut tx = Transaction::new(&g, request(&[], b"/?q=hit"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        assert_eq!(tx.tx_value(b"query"), Some(b"hit".as_slice()));
        assert_tx_accounting(&tx.tx);
        let body = format!("{{\"a\":\"{value}\"}}");
        tx.publish_request_body(RequestBody::new(
            body.into_bytes().into(),
            crate::seclang::BodyProcessorSelection::Json,
        ))
        .unwrap();
        let result = tx.execute_request_body().unwrap();
        let expected = if value.len() == 3 {
            Some(b"hit".as_slice())
        } else {
            None
        };
        assert_eq!(tx.tx_value(b"json"), expected);
        assert_eq!(tx.tx_value(b"from_tx"), expected);
        assert_eq!(result.findings().len(), if expected.is_some() { 2 } else { 0 });
        assert_tx_accounting(&tx.tx);
        if expected.is_some() {
            assert!(matches!(tx.tx.matched_var, Some(MatchValue::Owned(_))));
        }
    }
}

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
fn no_hit_first_members_borrow_tx_and_restore_prior_observers() {
    for source in [
        "SecRule REQUEST_HEADERS:X-Probe \"@streq hit\" \"id:1,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Probe \"@streq hit\" \"id:1,phase:1,pass,chain\"\nSecRule TX:saved \"@contains a\" \"\"\n",
        "SecRule MATCHED_VAR|MATCHED_VARS \"@streq previous\" \"id:1,phase:1,pass\"\n",
    ] {
        let g = generation(source);
        let headers = [RequestHeader::new(b"X-Probe", b"miss")];
        let mut tx = Transaction::new(&g, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        tx.tx.set(b"saved".to_vec(), vec![b'a'; 4096], tx.limits, &tx.request, tx.json_budget.as_mut()).unwrap();
        tx.tx.matched_var = Some(b"previous".to_vec().into());
        tx.tx.matched_var_name = Some(b"ARGS:previous".to_vec().into());
        tx.tx.matched_vars.push(TxEntry {
            key: b"ARGS:previous".to_vec().into(),
            value: b"previous".to_vec().into(),
        });
        tx.tx.request_body_processor = Some(super::super::super::BodyProcessorSelection::Json);
        tx.tx.recount();
        let retained = tx.tx.retained_bytes;
        let pointer = tx.tx_value(b"saved").unwrap().as_ptr();
        let scope = measurements::start();
        assert!(!tx.evaluate_unit(0, None).unwrap());
        assert_eq!(scope.counts().get(Event::UnitTxClones), 0);
        assert_eq!(tx.tx_value(b"saved").unwrap().as_ptr(), pointer);
        assert_eq!(tx.tx.retained_bytes, retained);
        assert_tx_accounting(&tx.tx);
        assert_eq!(tx.tx.matched_var, Some(b"previous".to_vec().into()));
        assert_eq!(tx.tx.matched_vars.len(), 1);
        assert_eq!(tx.tx.request_body_processor, Some(super::super::super::BodyProcessorSelection::Json));
    }
}

#[test]
fn first_member_errors_restore_tx_before_propagation() {
    let g = generation("SecRule REQUEST_HEADERS:X-Probe \"@eq 1\" \"id:1,phase:1,pass\"\n");
    let headers = [RequestHeader::new(b"X-Probe", b"not-a-number")];
    for limited in [false, true] {
        let mut tx = Transaction::new(&g, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        tx.tx
            .set(
                b"saved".to_vec(),
                b"persistent".to_vec(),
                tx.limits,
                &tx.request,
                tx.json_budget.as_mut(),
            )
            .unwrap();
        tx.tx.matched_var = Some(b"previous".to_vec().into());
        tx.tx.recount();
        let retained = tx.tx.retained_bytes;
        let pointer = tx.tx_value(b"saved").unwrap().as_ptr();
        if limited {
            tx.limits.max_retained_bytes = tx.retained_bytes();
        }
        let scope = measurements::start();
        let result = tx.evaluate_unit(0, None);
        assert!(matches!(
            result,
            Err(RuntimeError::IntegerConversion | RuntimeError::RetainedByteLimit { .. })
        ));
        assert_eq!(scope.counts().get(Event::UnitTxClones), 0);
        assert_eq!(tx.tx_value(b"saved").unwrap().as_ptr(), pointer);
        assert_eq!(tx.tx.retained_bytes, retained);
        assert_tx_accounting(&tx.tx);
        assert_eq!(tx.tx.matched_var, Some(b"previous".to_vec().into()));
    }
}

#[test]
fn matched_units_clone_after_first_member_and_failed_chains_roll_back() {
    for succeeds in [false, true] {
        let g = generation(&format!(
            "SecRule REQUEST_HEADERS:X-Probe \"@rx (hit)\" \"id:1,phase:1,pass,capture,chain,setvar:tx.saved=changed\"\nSecRule TX:saved \"@streq {}\" \"\"\n",
            if succeeds { "changed" } else { "never" }
        ));
        let headers = [RequestHeader::new(b"X-Probe", b"hit")];
        let mut tx = Transaction::new(&g, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        tx.tx
            .set(
                b"saved".to_vec(),
                b"persistent".to_vec(),
                tx.limits,
                &tx.request,
                tx.json_budget.as_mut(),
            )
            .unwrap();
        tx.tx.matched_var = Some(b"previous".to_vec().into());
        tx.tx.recount();
        let scope = measurements::start();
        assert_eq!(tx.evaluate_unit(0, None).unwrap(), succeeds);
        assert_eq!(scope.counts().get(Event::UnitTxClones), 1);
        assert_eq!(
            tx.tx_value(b"saved"),
            Some(if succeeds {
                b"changed".as_slice()
            } else {
                b"persistent".as_slice()
            })
        );
        assert_eq!(tx.tx_value(b"1"), if succeeds { Some(b"hit".as_slice()) } else { None });
        assert_tx_accounting(&tx.tx);
        if !succeeds {
            assert_eq!(tx.tx.matched_var, Some(b"previous".to_vec().into()));
        }
    }
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
fn request_decoders_preserve_scalar_bytes_ranges_and_reserved_capacity() {
    let oracle = |input: &[u8], query: bool| {
        let mut output = Vec::new();
        let mut index = 0;
        while index < input.len() {
            if query && input[index] == b'+' {
                output.push(b' ');
                index += 1;
                continue;
            }
            if input[index] == b'%' && index + 2 < input.len() {
                if let (Some(high), Some(low)) = (hex(input[index + 1]), hex(input[index + 2])) {
                    output.push((high << 4) | low);
                    index += 3;
                    continue;
                }
            }
            output.push(input[index]);
            index += 1;
        }
        output
    };
    let mut inputs = vec![Vec::new(), vec![b'a'; 1024], vec![255; 65]];
    let mut level = vec![Vec::new()];
    for _ in 0..4 {
        let mut next = Vec::new();
        for prefix in &level {
            for byte in [b'a', b'+', b'%', b'0', b'2', b'f', b'G', 255] {
                let mut value = prefix.clone();
                value.push(byte);
                next.push(value);
            }
        }
        inputs.extend(next.iter().cloned());
        level = next;
    }
    for literal in [b"%2520+tail".as_slice(), b"%00%ff%2F", b"%GG%", b"/a+b/plain"] {
        inputs.push(literal.to_vec());
    }
    for input in inputs {
        for query in [false, true] {
            let expected = oracle(&input, query);
            assert_eq!(percent_decoded_len(&input), expected.len());
            let mut state = owned_key_request();
            state.bytes.try_reserve_exact(4 + expected.len()).unwrap();
            state.bytes.extend_from_slice(b"seed");
            let capacity = state.bytes.capacity();
            let range = if query {
                state.copy_percent_decoded(&input)
            } else {
                state.copy_percent_decoded_path(&input)
            }
            .unwrap();
            assert_eq!(range.start, 4);
            assert_eq!(range.len, expected.len());
            assert_eq!(state.bytes_at(range), expected);
            assert_eq!(&state.bytes[..4], b"seed");
            assert_eq!(state.bytes.len(), 4 + expected.len());
            assert_eq!(state.bytes.capacity(), capacity);
        }
    }
}

#[test]
fn request_delimiter_search_preserves_scalar_slices() {
    fn same_slice(actual: &[u8], expected: &[u8]) {
        assert_eq!(actual, expected);
        if !expected.is_empty() {
            assert_eq!(actual.as_ptr(), expected.as_ptr());
        }
    }

    fn check(input: &[u8], delimiter: u8) {
        let position = input.iter().position(|byte| *byte == delimiter);
        let expected = position.map_or((input, &[][..]), |index| (&input[..index], &input[index + 1..]));
        let actual = split_once(input, delimiter);
        same_slice(actual.0, expected.0);
        same_slice(actual.1, expected.1);

        let query = input.iter().position(|byte| *byte == b'?');
        same_slice(query_part(input), query.map_or(&[][..], |index| &input[index + 1..]));
        same_slice(request_filename(input), &input[..query.unwrap_or(input.len())]);
    }

    let binary: Vec<_> = (0..=255_u8).collect();
    for delimiter in 0..=255_u8 {
        check(&binary, delimiter);
        for len in [0, 1, 2, 15, 16, 17, 31, 32, 33, 63, 64, 65, 255, 1024] {
            let plain = vec![delimiter.wrapping_add(1); len];
            check(&plain, delimiter);
            if len != 0 {
                for position in [0, len / 2, len - 1] {
                    let mut input = plain.clone();
                    input[position] = delimiter;
                    check(&input, delimiter);
                    input[len - 1] = delimiter;
                    check(&input, delimiter);
                }
            }
        }
    }
    for input in [
        b"/path?name=value=tail??".as_slice(),
        b"?",
        b"??",
        b"name==value",
        b"=value",
        b"name=",
        b"/encoded%3Fpath?x=%3D",
    ] {
        check(input, b'=');
    }
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
fn request_head_aliases_share_only_source_fields_and_preserve_decoding() {
    let headers = [
        RequestHeader::new(b"X-A", b"same"),
        RequestHeader::new(b"x-a", b"same"),
        RequestHeader::new(b"Cookie", b" a = one ; =skip; empty=; a=one "),
        RequestHeader::new(b"cookie", b"a=one"),
    ];
    let head = request(
        &headers,
        b"/a+b%2Fc?x=one&x=one&&=v&empty=&solo&bad=%G1%2%&bin=%00%FF&plus=a+b&esc%26=%3D",
    );
    let state = RequestState::build(head, RuntimeLimits::default()).unwrap();
    let query: &[(&[u8], &[u8])] = &[
        (b"x", b"one"),
        (b"x", b"one"),
        (b"", b"v"),
        (b"empty", b""),
        (b"solo", b""),
        (b"bad", b"%G1%2%"),
        (b"bin", b"\0\xff"),
        (b"plus", b"a b"),
        (b"esc&", b"="),
    ];
    for (source, aliases, expected) in [
        (
            CollectionKind::Args,
            vec![
                (CollectionKind::ArgsGet, false),
                (CollectionKind::ArgsNames, true),
                (CollectionKind::ArgsGetNames, true),
            ],
            query.to_vec(),
        ),
        (
            CollectionKind::RequestCookies,
            vec![(CollectionKind::RequestCookiesNames, true)],
            vec![
                (b"a".as_slice(), b"one".as_slice()),
                (b"empty", b""),
                (b"a", b"one"),
                (b"a", b"one"),
            ],
        ),
        (
            CollectionKind::RequestHeaders,
            vec![(CollectionKind::RequestHeadersNames, true)],
            headers.iter().map(|h| (h.name, h.value)).collect(),
        ),
    ] {
        let records = state.collection_records(source).unwrap();
        assert_eq!(records.len(), expected.len());
        for (index, (record, (name, value))) in records.iter().zip(&expected).enumerate() {
            assert_eq!(state.bytes_at(record.name.unwrap()), *name);
            assert_eq!(state.bytes_at(record.value), *value);
            if !name.is_empty() && !value.is_empty() {
                assert_ne!(record.name.unwrap(), record.value);
            }
            for prior in &records[..index] {
                if !name.is_empty() {
                    assert_ne!(record.name, prior.name);
                }
                if !value.is_empty() && prior.value.len != 0 {
                    assert_ne!(record.value, prior.value);
                }
            }
        }
        for (alias, names) in aliases {
            let aliases = state.collection_records(alias).unwrap();
            assert_eq!(aliases.len(), records.len());
            for (record, alias) in records.iter().zip(aliases) {
                assert_eq!(alias.name, record.name);
                assert_eq!(alias.value, if names { record.name.unwrap() } else { record.value });
            }
        }
    }
    assert_eq!(
        state.first_value(CollectionKind::QueryString),
        Some(query_part(head.uri))
    );
    assert_eq!(state.first_value(CollectionKind::RequestUriRaw), Some(head.uri_raw));
    assert_eq!(state.first_value(CollectionKind::RequestLine), Some(head.request_line));
    assert_eq!(
        state.first_value(CollectionKind::RequestFilename),
        Some(b"/a+b/c".as_slice())
    );
    assert_eq!(
        state.first_value(CollectionKind::RequestBasename),
        Some(b"c".as_slice())
    );
    assert_eq!(
        state.bytes.len(),
        projected_derived_bytes(head, query_part(head.uri)).unwrap().arena
    );

    // Adjacent empty source fields naturally share empty ranges, without padding.
    let empty = RequestState::build(request(&[], b"/?=&="), RuntimeLimits::default()).unwrap();
    let records = empty.collection_records(CollectionKind::Args).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0], records[1]);
}

#[test]
fn request_head_projected_metadata_preserves_query_sizes_and_first_content_type() {
    for (uri, expected_size) in [
        (b"/".as_slice(), b"0".as_slice()),
        (b"/?a=one", b"4"),
        (b"/?a=one&a=%26&=x&flag&bad=%Q1&&", b"17"),
    ] {
        for (content_type, expected_processor) in [
            (None, b"RAW".as_slice()),
            (Some(b"application/x-www-form-urlencoded".as_slice()), b"URLENCODED"),
            (Some(b" Application/Problem+JSON ; charset=utf-8"), b"JSON"),
            (Some(b"multipart/form-data; boundary=example"), b"MULTIPART"),
            (Some(b"text/xml"), b"XML"),
            (Some(b"application/octet-stream"), b"RAW"),
        ] {
            let mut headers = vec![RequestHeader::new(b"X-Probe", b"unchanged")];
            if let Some(value) = content_type {
                headers.push(RequestHeader::new(b"CoNtEnT-TyPe", value));
                // The first Content-Type wins, even when a later one disagrees.
                headers.push(RequestHeader::new(b"content-type", b"application/json"));
            }
            let state = RequestState::build(request(&headers, uri), RuntimeLimits::default()).unwrap();
            assert_eq!(state.first_value(CollectionKind::ArgsCombinedSize), Some(expected_size));
            assert_eq!(
                state.first_value(CollectionKind::ReqbodyProcessor),
                Some(expected_processor)
            );
        }
    }
}

#[test]
fn request_head_aliases_keep_literal_nominal_and_aggregate_retained_charges() {
    let headers = [RequestHeader::new(b"X", b"z"), RequestHeader::new(b"Cookie", b"a=one")];
    let head = request(&headers, b"/?q=hello%20world");
    // Scalars: combined-size digit, raw query, address, processor, path, request
    // line, method, protocol, both URIs and ID. Alias payload: 26 query bytes,
    // 3 + 17 header bytes and 5 cookie bytes (the pre-sharing representation).
    let scalar_payload = 2
        + b"q=hello%20world".len()
        + 9
        + 3
        + 1
        + b"GET /search?a=one&a=two%20words HTTP/1.1".len()
        + 3
        + 8
        + 2 * b"/?q=hello%20world".len()
        + 9;
    let nominal_payload = scalar_payload + 26 + 3 + 17 + 5;
    let entries = 12 + 4 + 4 + 2;
    let nominal = nominal_payload
        + entries * size_of::<CollectionValue>()
        + size_of::<[Vec<CollectionValue>; COLLECTION_COUNT]>();
    let state = RequestState::build(head, RuntimeLimits::default()).unwrap();
    assert_eq!(state.entry_count, entries);
    assert_eq!(state.retained_bytes, nominal);
    // Query saves 14 bytes; header names save 7; cookie name saves 1.
    assert_eq!(state.bytes.len(), nominal_payload - 22);
    assert_eq!(
        projected_derived_bytes(head, query_part(head.uri)).unwrap().arena,
        nominal_payload - 22
    );
    for limit in [nominal - 1, nominal] {
        let result = RequestState::build(
            head,
            RuntimeLimits {
                max_retained_bytes: limit,
                ..RuntimeLimits::default()
            },
        );
        if limit == nominal {
            assert_eq!(result.unwrap().retained_bytes, nominal);
        } else {
            assert!(matches!(result, Err(RuntimeError::RetainedByteLimit { .. })));
        }
    }
    let g = generation("SecRule ARGS_GET:q \"@rx hello\" \"id:950,phase:1,pass\"\n");
    let scratch = matcher_scratch_bytes(&g).unwrap();
    assert!(scratch > 0);
    for limit in [nominal + scratch - 1, nominal + scratch] {
        let result = Transaction::new(
            &g,
            head,
            RuntimeLimits {
                max_retained_bytes: limit,
                ..RuntimeLimits::default()
            },
        );
        if limit == nominal + scratch {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(RuntimeError::RetainedByteLimit { .. })));
        }
    }
    let body = || {
        RequestBody::new(
            bytes::Bytes::from_static(b"body"),
            crate::seclang::BodyProcessorSelection::Raw,
        )
    };
    let mut baseline = Transaction::new(&g, head, RuntimeLimits::default()).unwrap();
    baseline.execute_request_headers().unwrap();
    let other = baseline.retained_bytes() - nominal;
    baseline.publish_request_body(body()).unwrap();
    let body_charge = baseline.request.retained_bytes - nominal;
    assert!(body_charge > 0);
    for excess in [0, 1] {
        let mut tx = Transaction::new(&g, head, RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        tx.limits.max_retained_bytes = nominal + other + body_charge - excess;
        let result = tx.publish_request_body(body());
        if excess == 0 {
            assert_eq!(result, Ok(()));
        } else {
            assert!(matches!(result, Err(RuntimeError::RetainedByteLimit { .. })));
            assert!(tx.request.body.is_none());
        }
    }
}

#[test]
fn request_head_alias_admission_boundaries_and_error_precedence_stay_ordered() {
    let headers = [RequestHeader::new(b"Cookie", b"a=1;b=2")];
    let head = request(&headers, b"/?q=1&r=2");
    let entries = 12 + 2 + 8 + 4;
    let header_bytes = 13;
    let input_bytes = head.request_line.len()
        + head.method.len()
        + head.uri.len()
        + head.uri_raw.len()
        + head.protocol.len()
        + head.remote_addr.len()
        + head.unique_id.len()
        + header_bytes;
    let max_value = head.request_line.len();
    for excess in [0, 1] {
        let limits = RuntimeLimits {
            max_headers: 1 - excess,
            max_query_arguments: 2,
            max_cookies: 2,
            max_collection_entries: entries,
            max_header_bytes: header_bytes,
            max_input_bytes: input_bytes,
            max_value_bytes: max_value,
            ..RuntimeLimits::default()
        };
        let result = RequestState::build(head, limits);
        if excess == 0 {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(RuntimeError::HeaderLimit { .. })));
        }
    }
    let cases = [
        (
            RuntimeLimits {
                max_value_bytes: max_value - 1,
                max_input_bytes: 0,
                ..RuntimeLimits::default()
            },
            RuntimeError::ValueByteLimit {
                actual: max_value,
                limit: max_value - 1,
            },
        ),
        (
            RuntimeLimits {
                max_header_bytes: header_bytes - 1,
                max_input_bytes: 0,
                ..RuntimeLimits::default()
            },
            RuntimeError::HeaderByteLimit {
                actual: header_bytes,
                limit: header_bytes - 1,
            },
        ),
        (
            RuntimeLimits {
                max_input_bytes: input_bytes - 1,
                max_query_arguments: 0,
                ..RuntimeLimits::default()
            },
            RuntimeError::InputByteLimit {
                actual: input_bytes,
                limit: input_bytes - 1,
            },
        ),
        (
            RuntimeLimits {
                max_query_arguments: 1,
                max_cookies: 0,
                ..RuntimeLimits::default()
            },
            RuntimeError::QueryArgumentLimit { limit: 1 },
        ),
        (
            RuntimeLimits {
                max_cookies: 1,
                max_collection_entries: 0,
                ..RuntimeLimits::default()
            },
            RuntimeError::CookieLimit { limit: 1 },
        ),
        (
            RuntimeLimits {
                max_collection_entries: entries - 1,
                max_retained_bytes: 0,
                ..RuntimeLimits::default()
            },
            RuntimeError::CollectionEntryLimit { limit: entries - 1 },
        ),
    ];
    for (limits, expected) in cases {
        assert_eq!(RequestState::build(head, limits).err(), Some(expected));
    }
    let invalid = [RequestHeader::new(b"Bad Name", b"\n")];
    assert_eq!(
        RequestState::build(
            request(&invalid, b"/?q=1"),
            RuntimeLimits {
                max_query_arguments: 0,
                ..RuntimeLimits::default()
            }
        )
        .err(),
        Some(RuntimeError::InvalidHeaderName { ordinal: 0 })
    );
    let invalid = [RequestHeader::new(b"X", b"\n")];
    assert_eq!(
        RequestState::build(
            request(&invalid, b"/?q=1"),
            RuntimeLimits {
                max_query_arguments: 0,
                ..RuntimeLimits::default()
            }
        )
        .err(),
        Some(RuntimeError::InvalidHeaderValue { ordinal: 0 })
    );
}

#[test]
fn head_value_validation_preserves_all_bytes_ordinals_and_error_order() {
    for byte in 0..=255_u8 {
        for position in [0, 32, 64] {
            let mut value = [b'a'; 65];
            value[position] = byte;
            let request_headers = [RequestHeader::new(b"X-First", b"ok"), RequestHeader::new(b"X", &value)];
            let response_headers = [
                ResponseHeader::new(b"X-First", b"ok"),
                ResponseHeader::new(b"X", &value),
            ];
            let request_result = RequestState::build(request(&request_headers, b"/"), RuntimeLimits::default());
            let response_result =
                ResponseState::build(ResponseHead::new(200, &response_headers), RuntimeLimits::default());
            if matches!(byte, 0 | b'\r' | b'\n') {
                assert_eq!(
                    request_result.err(),
                    Some(RuntimeError::InvalidHeaderValue { ordinal: 1 })
                );
                assert_eq!(
                    response_result.err(),
                    Some(RuntimeError::InvalidResponseHeaderValue { ordinal: 1 })
                );
            } else {
                assert_eq!(
                    request_result
                        .unwrap()
                        .first_named(CollectionKind::RequestHeaders, b"X", false),
                    Some(value.as_slice())
                );
                assert_eq!(response_result.unwrap().entries[2].value, value);
            }
        }
    }
    for name in [b"X".as_slice(), b"Bad Name"] {
        let mut value = [b'a'; 65];
        value[64] = b'\r';
        let request_headers = [RequestHeader::new(name, &value)];
        let response_headers = [ResponseHeader::new(name, &value)];
        let limits = RuntimeLimits {
            max_value_bytes: 64,
            ..RuntimeLimits::default()
        };
        let (request_error, response_error) = if name == b"X" {
            (
                RuntimeError::InvalidHeaderValue { ordinal: 0 },
                RuntimeError::InvalidResponseHeaderValue { ordinal: 0 },
            )
        } else {
            (
                RuntimeError::InvalidHeaderName { ordinal: 0 },
                RuntimeError::InvalidResponseHeaderName { ordinal: 0 },
            )
        };
        assert_eq!(
            RequestState::build(request(&request_headers, b"/"), limits).err(),
            Some(request_error)
        );
        assert_eq!(
            ResponseState::build(ResponseHead::new(200, &response_headers), limits).err(),
            Some(response_error)
        );
    }
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
    let mut transaction = Transaction::new_with_policy(
        &generation,
        request(&[], b"/private"),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
        RuleCheckPolicy::default(),
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
fn compiled_ip_network_bases_are_normalized_for_every_prefix() {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    for (v4, v6) in [
        ("255.255.255.255", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"),
        ("203.0.113.179", "2001:db8:1234:5678:9abc:def0:1234:5678"),
    ] {
        let networks = (0..=32)
            .map(|prefix| format!("{v4}/{prefix}"))
            .chain((0..=128).map(|prefix| format!("{v6}/{prefix}")))
            .collect::<Vec<_>>()
            .join(",");
        let g = generation(&format!(
            "SecRule REMOTE_ADDR \"@ipMatch {networks}\" \"id:1,phase:1,pass\"\n"
        ));
        assert_eq!(g.ip_networks.len(), 162);
        for network in &g.ip_networks {
            let (width, maximum, original, stored) = match network.address {
                IpAddr::V4(base) => (
                    32,
                    u128::from(u32::MAX),
                    u128::from(u32::from(v4.parse::<Ipv4Addr>().unwrap())),
                    u128::from(u32::from(base)),
                ),
                IpAddr::V6(base) => (
                    128,
                    u128::MAX,
                    u128::from(v6.parse::<Ipv6Addr>().unwrap()),
                    u128::from(base),
                ),
            };
            let mask = if network.prefix == 0 {
                0
            } else {
                (maximum << (width - u32::from(network.prefix))) & maximum
            };
            assert_eq!(stored, original & mask);
            let last = stored | (maximum ^ mask);
            let candidates = [
                Some(0),
                Some(maximum),
                Some(original),
                Some(stored),
                Some(last),
                stored.checked_sub(1),
                last.checked_add(1).filter(|value| *value <= maximum),
            ];
            for value in candidates.into_iter().flatten() {
                let address = if width == 32 {
                    IpAddr::V4(Ipv4Addr::from(u32::try_from(value).unwrap()))
                } else {
                    IpAddr::V6(Ipv6Addr::from(value))
                };
                assert_eq!(ip_matches(address, *network), value & mask == original & mask);
            }
            let other_family = if width == 32 {
                IpAddr::V6(Ipv6Addr::UNSPECIFIED)
            } else {
                IpAddr::V4(Ipv4Addr::UNSPECIFIED)
            };
            assert!(!ip_matches(other_family, *network));
        }
    }
}

#[test]
fn ip_network_operator_preserves_host_prefixes_families_and_negation() {
    let g = generation(concat!(
        "SecRule REQUEST_HEADERS:X-Address \"@ipMatch 192.0.2.1,2001:db8:abcd::1/32\" \"id:1,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Address \"!@ipMatch 192.0.2.1,2001:db8:abcd::1/32\" \"id:2,phase:1,pass\"\n",
    ));
    for (input, expected) in [
        ("192.0.2.1", 1),
        ("192.0.2.2", 2),
        ("2001:db8:ffff::1", 1),
        ("2001:db9::1", 2),
        ("::ffff:192.0.2.1", 2),
        ("invalid", 2),
    ] {
        let headers = [RequestHeader::new(b"X-Address", input.as_bytes())];
        let mut tx = Transaction::new(&g, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        let outcome = tx.execute_request_headers().unwrap();
        assert_eq!(outcome.findings().len(), 1);
        assert_eq!(outcome.findings()[0].rule_id(), expected);
    }
}

#[test]
fn url_encoding_scan_matches_scalar_oracle() {
    let oracle = |input: &[u8]| {
        let mut index = 0;
        while index < input.len() {
            if input[index] != b'%' {
                index += 1;
                continue;
            }
            if index + 2 >= input.len() || hex(input[index + 1]).is_none() || hex(input[index + 2]).is_none() {
                return true;
            }
            index += 3;
        }
        false
    };
    let mut level = vec![Vec::new()];
    assert!(!invalid_url_encoding(&[]));
    for _ in 0..5 {
        let mut next = Vec::new();
        for prefix in &level {
            for byte in [b'%', b'0', b'A', b'f', b'G', b'+', 255] {
                let mut value = prefix.clone();
                value.push(byte);
                assert_eq!(invalid_url_encoding(&value), oracle(&value), "{value:?}");
                next.push(value);
            }
        }
        level = next;
    }
    for offset in [0, 63, 64, 65, 1024] {
        for suffix in [
            b"plain".as_slice(),
            b"%00",
            b"%2f",
            b"%G0",
            b"%0",
            b"%",
            b"%%",
            b"%00%ff",
            b"%00%",
        ] {
            let mut value = vec![255; offset];
            value.extend_from_slice(suffix);
            assert_eq!(invalid_url_encoding(&value), oracle(&value));
        }
    }
}

#[test]
fn url_encoding_operator_preserves_binary_input_and_negation() {
    let g = generation(concat!(
        "SecRule REQUEST_HEADERS:X-Probe \"@validateUrlEncoding\" \"id:1,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Probe \"!@validateUrlEncoding\" \"id:2,phase:1,pass\"\n",
    ));
    for (input, expected) in [
        (b"\xff+plain".as_slice(), 2),
        (b"\xff%00%2F".as_slice(), 2),
        (b"\xff%0".as_slice(), 1),
        (b"\xff%00%GG".as_slice(), 1),
    ] {
        let headers = [RequestHeader::new(b"X-Probe", input)];
        let mut tx = Transaction::new(&g, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        let outcome = tx.execute_request_headers().unwrap();
        assert_eq!(outcome.findings().len(), 1);
        assert_eq!(outcome.findings()[0].rule_id(), expected);
    }
}

#[test]
fn substring_search_matches_window_oracle_for_binary_and_repetitive_inputs() {
    let oracle = |haystack: &[u8], needle: &[u8]| {
        needle.is_empty() || haystack.windows(needle.len()).any(|window| window == needle)
    };
    let mut inputs = vec![Vec::new()];
    let mut level = vec![Vec::new()];
    for _ in 0..4 {
        let mut next = Vec::new();
        for prefix in &level {
            for byte in [0, b'a', 255] {
                let mut value = prefix.clone();
                value.push(byte);
                next.push(value);
            }
        }
        inputs.extend(next.iter().cloned());
        level = next;
    }
    for haystack in &inputs {
        for needle in &inputs {
            assert_eq!(contains(haystack, needle), oracle(haystack, needle));
        }
    }
    for size in [63, 64, 65, 255, 1024] {
        let mut haystack = vec![b'a'; size];
        haystack[size - 1] = b'b';
        for needle_size in [0, 1, 2, 31, 64, 65, 256, 1025] {
            let mut needle = vec![b'a'; needle_size];
            for last in [b'a', b'b', b'c'] {
                if let Some(byte) = needle.last_mut() {
                    *byte = last;
                }
                assert_eq!(contains(&haystack, &needle), oracle(&haystack, &needle));
            }
        }
        let binary: Vec<u8> = (0..size).map(|i| (i.wrapping_mul(37) % 256) as u8).collect();
        for start in [0, size / 2, size - 1] {
            for end in [start, size] {
                assert_eq!(
                    contains(&binary, &binary[start..end]),
                    oracle(&binary, &binary[start..end])
                );
            }
        }
    }
}

#[test]
fn substring_operators_preserve_static_dynamic_direction_and_negation() {
    let g = generation(concat!(
        "SecRule REQUEST_HEADERS:X-Probe \"@contains hit\" \"id:1,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Probe \"@contains %{ARGS.needle}\" \"id:2,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Token \"@within prefix-hit-suffix\" \"id:3,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Token \"@within %{ARGS.pool}\" \"id:4,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Probe \"!@contains absent\" \"id:5,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Probe \"@within hit\" \"id:6,phase:1,pass\"\n",
    ));
    let headers = [
        RequestHeader::new(b"X-Probe", b"prefix-hit-suffix"),
        RequestHeader::new(b"X-Token", b"hit"),
    ];
    let mut tx = Transaction::new(
        &g,
        request(&headers, b"/?needle=hit&pool=prefix-hit-suffix"),
        RuntimeLimits::default(),
    )
    .unwrap();
    let outcome = tx.execute_request_headers().unwrap();
    assert_eq!(
        outcome
            .findings()
            .iter()
            .map(|finding| finding.rule_id())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
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
fn delete_key_templates_borrow_static_bytes_and_preserve_expansion_limits() {
    for (text, expected, borrowed) in [
        ("Score", b"Score".as_slice(), true),
        ("%{ARGS.key}", b"Score".as_slice(), false),
        ("pre%{ARGS.key}", b"preScore".as_slice(), false),
        ("%{TX.missing}", b"".as_slice(), false),
    ] {
        let g = generation(&format!("SecAction \"id:1,phase:1,pass,setvar:'!tx.{text}'\"\n"));
        let template = g.setvars[0].key;
        let state = RequestState::build(request(&[], b"/?key=Score"), RuntimeLimits::default()).unwrap();
        let tx = TxState::default();
        for limit in [0, 1, expected.len().saturating_sub(1), expected.len(), usize::MAX] {
            let owned = expand_template::<true>(&g, &state, &tx, template, limit, None);
            let operand = template_operand(&g, &state, &tx, template, limit, None);
            assert_eq!(owned.as_deref(), operand.as_deref());
            if let Ok(value) = operand {
                assert_eq!(value.as_ref(), expected);
                assert_eq!(matches!(value, Cow::Borrowed(_)), borrowed);
                if borrowed {
                    assert!(g.bytes.as_ptr_range().contains(&value.as_ptr()));
                }
            }
        }
    }
}

#[test]
fn static_and_dynamic_tx_deletes_preserve_lookup_and_rollback() {
    for (key, removed) in [
        ("sCoRe", true),
        ("%{ARGS.key}", true),
        ("absent", false),
        ("%{TX.missing}", false),
    ] {
        for fail_after_delete in [false, true] {
            let failure = if fail_after_delete {
                ",setvar:'tx.invalid=+%{TX.missing}'"
            } else {
                ""
            };
            let g = generation(&format!(
                "SecAction \"id:1,phase:1,pass,setvar:tx.score=kept\"\n\
                 SecAction \"id:2,phase:1,pass,setvar:'!tx.{key}',setvar:tx.marker=hit{failure}\"\n"
            ));
            let mut tx = Transaction::new(&g, request(&[], b"/?key=SCORE"), RuntimeLimits::default()).unwrap();
            let outcome = tx.execute_request_headers();
            if fail_after_delete {
                assert!(matches!(outcome, Err(RuntimeError::IntegerConversion)));
                assert_eq!(tx.tx_value(b"score"), Some(b"kept".as_slice()));
                assert_eq!(tx.tx_value(b"marker"), None);
                assert_eq!(tx.tx_value(b"invalid"), None);
                assert_eq!(tx.next_phase(), Some(Phase::RequestHeaders));
            } else {
                assert_eq!(outcome.unwrap().findings().len(), 2);
                assert_eq!(
                    tx.tx_value(b"score"),
                    if removed { None } else { Some(b"kept".as_slice()) }
                );
                assert_eq!(tx.tx_value(b"marker"), Some(b"hit".as_slice()));
            }
        }
    }
}

#[test]
fn setvar_constant_and_dynamic_arithmetic_preserve_results_and_rollback() {
    let cases = [
        (None, "+", "0", Some("0")),
        (None, "-", "2", Some("-2")),
        (Some("7"), "+", "2", Some("9")),
        (Some("7"), "-", "2", Some("5")),
        (Some("7"), "+", "-2", Some("5")),
        (Some("7"), "-", "-2", Some("9")),
        (None, "+", "9223372036854775807", Some("9223372036854775807")),
        (None, "+", "-9223372036854775808", Some("-9223372036854775808")),
        (Some("-1"), "-", "9223372036854775807", Some("-9223372036854775808")),
        (Some("-1"), "-", "-9223372036854775808", Some("9223372036854775807")),
        (Some("1"), "+", "9223372036854775807", None),
        (Some("-1"), "+", "-9223372036854775808", None),
        (Some("-9223372036854775808"), "-", "1", None),
        (None, "-", "-9223372036854775808", None),
        (Some("invalid"), "+", "1", None),
        (Some(""), "-", "1", None),
    ];
    for (initial, operation, delta, expected) in cases {
        for dynamic in [false, true] {
            let initialize = if initial.is_some() {
                ",setvar:'tx.score=%{ARGS.initial}'"
            } else {
                ""
            };
            let operand = if dynamic { "%{ARGS.delta}" } else { delta };
            let source = format!(
                "SecAction \"id:1,phase:1,pass,setvar:tx.marker=hit{initialize},setvar:'tx.score={operation}{operand}'\"\n"
            );
            let generation = generation(&source);
            let arithmetic = generation.setvars.last().unwrap();
            assert_eq!(
                arithmetic.constant,
                if dynamic { None } else { Some(delta.parse().unwrap()) }
            );
            let uri = format!("/?initial={}&delta={delta}", initial.unwrap_or_default());
            let mut transaction =
                Transaction::new(&generation, request(&[], uri.as_bytes()), RuntimeLimits::default()).unwrap();
            let result = transaction.execute_request_headers();
            if let Some(expected) = expected {
                assert_eq!(result.unwrap().findings().len(), 1, "{source}");
                assert_eq!(transaction.tx_value(b"score"), Some(expected.as_bytes()), "{source}");
                assert_eq!(transaction.tx_value(b"marker"), Some(b"hit".as_slice()));
            } else {
                assert!(matches!(result, Err(RuntimeError::IntegerConversion)), "{source}");
                assert_eq!(transaction.tx_value(b"score"), None);
                assert_eq!(transaction.tx_value(b"marker"), None);
                assert_eq!(transaction.next_phase(), Some(Phase::RequestHeaders));
            }
        }
    }
}

#[test]
fn setvar_dynamic_arithmetic_rejects_invalid_delta_without_committing() {
    let generation =
        generation("SecAction \"id:1,phase:1,pass,setvar:tx.marker=hit,setvar:'tx.score=+%{ARGS.delta}'\"\n");
    for delta in ["", "invalid", "9223372036854775808", "-9223372036854775809"] {
        let uri = format!("/?delta={delta}");
        let mut transaction =
            Transaction::new(&generation, request(&[], uri.as_bytes()), RuntimeLimits::default()).unwrap();
        assert!(matches!(
            transaction.execute_request_headers(),
            Err(RuntimeError::IntegerConversion)
        ));
        assert_eq!(transaction.tx_value(b"score"), None);
        assert_eq!(transaction.tx_value(b"marker"), None);
        assert_eq!(transaction.next_phase(), Some(Phase::RequestHeaders));
    }
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
fn phrase_capture_spans_preserve_selection_negation_and_limits() {
    for (patterns, input, expected) in [
        ("ab abc", "xxABCab", Some("ABC")),
        ("bc a", "Abc", Some("A")),
        ("token", "Token token", Some("Token")),
        ("needle", "missing", None),
    ] {
        for negated in [false, true] {
            let operator_prefix = if negated { "!" } else { "" };
            let g = generation(&format!(
                "SecRule REQUEST_HEADERS:X-Probe \"{operator_prefix}@pm {patterns}\" \"id:1,phase:1,pass,capture\"\n"
            ));
            let required = expected.map_or(0, |value| value.len() + 1);
            for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
                for limit in [required.saturating_sub(1), required] {
                    let headers = [RequestHeader::new(b"X-Probe", input.as_bytes())];
                    let mut tx = Transaction::new_with_policy(
                        &g,
                        request(&headers, b"/"),
                        RuntimeLimits {
                            max_capture_bytes: limit,
                            ..RuntimeLimits::default()
                        },
                        mode,
                        RuleEngineMode::On,
                        RuleCheckPolicy::default(),
                    )
                    .unwrap();
                    let outcome = tx.execute_request_headers();
                    if !negated && expected.is_some() && limit < required {
                        assert_eq!(outcome, Err(RuntimeError::CaptureByteLimit { limit }));
                        assert_eq!(tx.tx_value(b"0"), None);
                        assert_eq!(tx.next_phase(), Some(Phase::RequestHeaders));
                    } else {
                        assert_eq!(
                            outcome.unwrap().findings().len(),
                            usize::from(expected.is_some() != negated)
                        );
                        assert_eq!(
                            tx.tx_value(b"0"),
                            if negated { None } else { expected.map(str::as_bytes) }
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn reused_phrase_capture_is_discarded_with_failed_chain() {
    let g = generation(concat!(
        "SecRule REQUEST_HEADERS:X-Probe \"@pm ab abc\" \"id:1,phase:1,pass,capture,chain,setvar:tx.leak=%{TX.0}\"\n",
        "SecRule REQUEST_METHOD \"@streq POST\"\n",
    ));
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        let headers = [RequestHeader::new(b"X-Probe", b"ABC")];
        let mut tx = Transaction::new_with_policy(
            &g,
            request(&headers, b"/"),
            RuntimeLimits::default(),
            mode,
            RuleEngineMode::On,
            RuleCheckPolicy::default(),
        )
        .unwrap();
        assert!(tx.execute_request_headers().unwrap().findings().is_empty());
        assert_eq!(tx.tx_value(b"0"), None);
        assert_eq!(tx.tx_value(b"leak"), None);
    }
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
fn ordinary_work_accumulates_across_failed_chain_branches() {
    let generation = generation(concat!(
        "SecRule REQUEST_HEADERS \"@rx .\" \"id:204,phase:1,pass,chain\"\n",
        "SecRule REQUEST_HEADERS:X-Payload \"@rx never-present\" \"setvar:tx.hit=1\"\n",
    ));
    let payload = vec![b'a'; 1_000];
    // Each member invocation over the 1000-byte payload charges a few KiB:
    // one branch fits the limit, while five failed branches together exceed it
    // even though every single invocation stays below it.
    let limit = 6_000;
    let limits = RuntimeLimits {
        max_operator_work_bytes: limit,
        ..RuntimeLimits::default()
    };

    let single_headers = [RequestHeader::new(b"X-Payload", &payload)];
    let mut single = Transaction::new(&generation, request(&single_headers, b"/"), limits).unwrap();
    let outcome = single.execute_request_headers().unwrap();
    assert!(outcome.findings.is_empty());

    let fan_out_headers = [
        RequestHeader::new(b"X-Payload", &payload),
        RequestHeader::new(b"X-A", b"1"),
        RequestHeader::new(b"X-B", b"1"),
        RequestHeader::new(b"X-C", b"1"),
        RequestHeader::new(b"X-D", b"1"),
    ];
    let mut fan_out = Transaction::new(&generation, request(&fan_out_headers, b"/"), limits).unwrap();
    assert_eq!(
        fan_out.execute_request_headers(),
        Err(RuntimeError::OperatorWorkLimit { limit })
    );
    assert_eq!(fan_out.tx_value(b"hit"), None);
}

#[test]
fn ordinary_work_budget_resets_for_each_rule_unit() {
    let generation = generation(concat!(
        "SecRule REQUEST_HEADERS:X-Payload \"@rx never-present\" \"id:205,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Payload \"@rx never-present\" \"id:206,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Payload \"@rx never-present\" \"id:207,phase:1,pass\"\n",
    ));
    let payload = vec![b'a'; 1_000];
    let headers = [RequestHeader::new(b"X-Payload", &payload)];
    let mut transaction = Transaction::new(
        &generation,
        request(&headers, b"/"),
        RuntimeLimits {
            max_operator_work_bytes: 2_500,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    assert!(transaction.execute_request_headers().unwrap().findings.is_empty());
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
fn shared_selector_programs_preserve_include_exclude_count_and_tx_case() {
    let generation = generation(concat!(
        "SecAction \"id:1,phase:2,pass,nolog,setvar:tx.name=value\"\n",
        "SecRule ARGS:/^NAME$/ \"@streq value\" \"id:2,phase:2,pass\"\n",
        "SecRule ARGS|!ARGS:/^NAME$/ \"@streq value\" \"id:3,phase:2,pass\"\n",
        "SecRule &ARGS:/^NAME$/ \"@eq 1\" \"id:4,phase:2,pass\"\n",
        "SecRule TX:/^NAME$/ \"@streq value\" \"id:5,phase:2,pass\"\n",
    ));
    for (uri, expected) in [
        (b"/?NAME=value".as_slice(), &[1, 2, 4, 5][..]),
        (b"/?name=value".as_slice(), &[1, 3, 5][..]),
    ] {
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let mut tx = Transaction::new_with_policy(
                &generation,
                request(&[], uri),
                RuntimeLimits::default(),
                mode,
                RuleEngineMode::On,
                RuleCheckPolicy::default(),
            )
            .unwrap();
            tx.execute_request_headers().unwrap();
            tx.publish_request_body(RequestBody::new(
                bytes::Bytes::new(),
                crate::seclang::BodyProcessorSelection::Auto,
            ))
            .unwrap();
            assert_eq!(
                tx.execute_request_body()
                    .unwrap()
                    .findings()
                    .iter()
                    .map(|finding| finding.rule_id())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
}

#[test]
fn duplicate_boolean_regexes_keep_ordered_effects_negation_and_chain_results() {
    let generation = generation(concat!(
        "SecRule ARGS:x \"@rx ^a$\" \"id:1,phase:2,pass,setvar:tx.first=%{MATCHED_VAR}\"\n",
        "SecRule ARGS:x \"@rx ^a$\" \"id:2,phase:2,pass,setvar:tx.second=%{tx.first},chain\"\n",
        "SecRule TX:second \"@streq a\" \"setvar:tx.chain=ok\"\n",
        "SecRule ARGS:x \"!@rx ^a$\" \"id:3,phase:2,pass,setvar:tx.negated=ok\"\n",
        "SecRule ARGS:x \"@rx ^b$\" \"id:4,phase:2,pass,setvar:tx.fourth=ok\"\n",
    ));
    for (uri, expected, hit) in [
        (b"/?x=a".as_slice(), &[1, 2][..], true),
        (b"/?x=b".as_slice(), &[3, 4][..], false),
    ] {
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let mut tx = Transaction::new_with_policy(
                &generation,
                request(&[], uri),
                RuntimeLimits::default(),
                mode,
                RuleEngineMode::On,
                RuleCheckPolicy::default(),
            )
            .unwrap();
            tx.execute_request_headers().unwrap();
            tx.publish_request_body(RequestBody::new(
                bytes::Bytes::new(),
                crate::seclang::BodyProcessorSelection::Auto,
            ))
            .unwrap();
            let outcome = tx.execute_request_body().unwrap();
            assert_eq!(
                outcome
                    .findings()
                    .iter()
                    .map(|finding| finding.rule_id())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(tx.tx_value(b"first"), hit.then_some(b"a".as_slice()));
            assert_eq!(tx.tx_value(b"second"), hit.then_some(b"a".as_slice()));
            assert_eq!(tx.tx_value(b"chain"), hit.then_some(b"ok".as_slice()));
            assert_eq!(tx.tx_value(b"negated"), (!hit).then_some(b"ok".as_slice()));
        }
    }
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
fn discarded_action_templates_preserve_bytes_and_length_errors() {
    for (text, expected) in [
        ("plain", b"plain".as_slice()),
        ("%{TX.missing}", b""),
        ("pre:%{ARGS.a}:%{TX.missing}:post", b"pre:one::post"),
    ] {
        let g = generation(&format!("SecAction \"id:1,phase:1,pass,msg:'{text}'\"\n"));
        let template = g
            .actions
            .iter()
            .find_map(|action| match (action.kind, action.operand) {
                (
                    super::super::ActionEnvelopeKind::Action(crate::seclang::ActionKind::Msg),
                    super::super::CompiledActionOperand::Template(id),
                ) => Some(id),
                _ => None,
            })
            .unwrap();
        let state = RequestState::build(request(&[], b"/?a=one"), RuntimeLimits::default()).unwrap();
        let tx = TxState::default();
        for limit in [0, 1, expected.len().saturating_sub(1), expected.len(), usize::MAX] {
            let owned = expand_template::<true>(&g, &state, &tx, template, limit, None);
            let discarded = expand_template::<false>(&g, &state, &tx, template, limit, None);
            if limit >= expected.len() {
                assert_eq!(owned.as_deref(), Ok(expected));
            }
            if let Ok(value) = &discarded {
                assert!(value.is_empty());
                assert_eq!(value.capacity(), 0);
            }
            assert_eq!(owned.map(|_| ()), discarded.map(|_| ()));
        }
    }
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
            let mut transaction = Transaction::new_with_policy(
                &generation,
                request(&headers, b"/"),
                RuntimeLimits::default(),
                EvaluationMode::Accelerated,
                mode,
                RuleCheckPolicy::default(),
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
        let mut transaction = Transaction::new_with_policy(
            &generation,
            request(&[], b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            mode,
            RuleCheckPolicy::default(),
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
    let mut transaction = Transaction::new_with_policy(
        &generation,
        request(&[], b"/"),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
        RuleCheckPolicy::default(),
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
fn prefix_member_accepts_msg_action() {
    // The request-prefix path accepts `Msg` on top of the streaming-safe action
    // list; state actions such as `setvar` remain rejected.
    let with_msg =
        generation("SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,status:403,msg:'m'\"\n");
    assert_eq!(with_msg.validate_request_body_prefix(), Ok(()));
    let with_setvar =
        generation("SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,status:403,setvar:tx.a=1\"\n");
    assert_eq!(
        with_setvar.validate_request_body_prefix(),
        Err("requestBody.inspection prefix does not support control or state actions; select full")
    );
}

#[test]
fn prefix_detection_only_uses_pm_transform_and_short_body_completion() {
    let generation = generation("SecRule REQUEST_BODY \"@pm forbidden blocked\" \"id:1,phase:2,deny,t:lowercase\"\n");
    let mut transaction = Transaction::new_with_policy(
        &generation,
        request(&[], b"/"),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
        RuleCheckPolicy::default(),
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
    let mut transaction = Transaction::new_with_policy(
        &generation,
        request(&[], b"/"),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
        RuleCheckPolicy::default(),
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
        let mut transaction = Transaction::new_with_policy(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            mode,
            RuleCheckPolicy::default(),
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
    let mut transaction = Transaction::new_with_policy(
        &generation,
        request(&[], b"/"),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
        RuleCheckPolicy::default(),
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
fn response_status_bytes_and_accounting_cover_the_accepted_range() {
    for status in 100..=999 {
        let response = ResponseState::build(ResponseHead::new(status, &[]), RuntimeLimits::default()).unwrap();
        assert_eq!(response.entries.len(), 1);
        let entry = &response.entries[0];
        assert_eq!(entry.collection, CollectionKind::ResponseStatus);
        assert!(entry.name.is_none());
        assert_eq!(entry.value, status.to_string().as_bytes());
        assert_eq!(response.total_retained_bytes(), size_of::<ResponseEntry>() + 3);
        assert!(response.window.is_none());
    }

    let headers = [
        ResponseHeader::new(b"X", b""),
        ResponseHeader::new(b"X", b"value"),
        ResponseHeader::new(b"Other", b"\xff"),
    ];
    let response = ResponseState::build(ResponseHead::new(200, &headers), RuntimeLimits::default()).unwrap();
    assert_eq!(response.total_retained_bytes(), 16 + 4 * size_of::<ResponseEntry>());
    for (entry, header) in response.entries[1..].iter().zip(&headers) {
        assert_eq!(entry.name.as_deref(), Some(header.name));
        assert_eq!(entry.value, header.value);
    }

    let invalid_headers = [ResponseHeader::new(b"", b"\n")];
    for status in [0, 99, 1000, u16::MAX] {
        assert!(matches!(
            ResponseState::build(ResponseHead::new(status, &invalid_headers), RuntimeLimits::default()),
            Err(RuntimeError::InvalidResponseStatus { status: actual }) if actual == status
        ));
    }
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
        let mut accelerated = Transaction::new_with_policy(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            RuleEngineMode::On,
            RuleCheckPolicy::default(),
        )
        .unwrap();
        let mut reference = Transaction::new_with_policy(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Reference,
            RuleEngineMode::On,
            RuleCheckPolicy::default(),
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
        let mut transaction = Transaction::new_with_policy(
            &generation,
            request(&[], b"/"),
            RuntimeLimits {
                max_operator_work_bytes: 256,
                ..RuntimeLimits::default()
            },
            EvaluationMode::Accelerated,
            mode,
            RuleCheckPolicy::default(),
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
    type CaptureValue = MatchValue;
    type CaptureKey = MatchValue;
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
    let mut captures = CaptureBuilder::default();
    let mut observer = CaptureProbe {
        reject_admission: true,
        admissions: 0,
        actual: 0,
    };
    assert!(matches!(
        push_capture(
            &mut captures,
            CaptureKeySource::Numeric { index: 0, bytes: b"0" },
            CaptureSource::InputRange {
                input: b"secret",
                start: 0,
                len: 6
            },
            RuntimeLimits::default(),
            &mut observer
        ),
        Err(PredicateStageFailure::Observer(()))
    ));
    assert_eq!(captures.entries.capacity(), 0);
    assert_eq!(observer.actual, 0);
    observer.admissions = 0;
    let limits = RuntimeLimits {
        max_capture_bytes: 0,
        ..RuntimeLimits::default()
    };
    assert!(matches!(
        push_capture(
            &mut captures,
            CaptureKeySource::Numeric { index: 0, bytes: b"0" },
            CaptureSource::InputRange {
                input: b"secret",
                start: 0,
                len: 6
            },
            limits,
            &mut observer
        ),
        Err(PredicateStageFailure::Semantic {
            checkpoint: PredicateCheckpoint::Captures,
            error: RuntimeError::CaptureByteLimit { limit: 0 }
        })
    ));
    assert_eq!(observer.admissions, 0);
    observer.reject_admission = false;
    assert!(push_capture(
        &mut captures,
        CaptureKeySource::Numeric { index: 0, bytes: b"0" },
        CaptureSource::InputRange {
            input: b"value",
            start: 0,
            len: 5
        },
        RuntimeLimits::default(),
        &mut observer
    )
    .is_ok());
    assert_eq!(observer.admissions, 3);
    assert_eq!(observer.actual, captures.retained_bytes().unwrap());
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
    state
        .set(
            b"capture".to_vec(),
            b"persistent".to_vec(),
            limits,
            &owned_key_request(),
            None,
        )
        .unwrap();
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
    assert_eq!(
        unit.get(b"capture", &owned_key_request(), None).unwrap(),
        Some(&MatchValue::Owned(b"persistent".to_vec()))
    );
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
        let mut transaction = Transaction::new_with_policy(
            &generation,
            request(&[], b"/?a=one&b=two&c=three"),
            RuntimeLimits::default(),
            mode,
            RuleEngineMode::On,
            RuleCheckPolicy::default(),
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
        .set(
            b"x".to_vec(),
            b"y".to_vec(),
            RuntimeLimits::default(),
            &owned_key_request(),
            None,
        )
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
                .map(|capture| capture.key.observed_bytes(&transaction.request).unwrap())
                .collect::<Vec<_>>(),
            keys
        );
    }
}

struct RangeCaptureProbe;
impl PredicateStageObserver for RangeCaptureProbe {
    type Error = ();
    type CaptureValue = StageCaptureValue;
    type CaptureKey = MatchValue;
    fn charge_stage(&mut self, _: usize, _: usize) -> Result<(), ()> {
        Ok(())
    }
    fn before_captures(&mut self, _: usize) -> Result<(), ()> {
        Ok(())
    }
    fn captures(&mut self, _: &[CaptureEntry<StageCaptureValue>]) -> Result<(), ()> {
        Ok(())
    }
    fn admit_capture_allocation(&mut self, _: usize, _: usize) -> Result<(), ()> {
        Ok(())
    }
    fn capture_capacity(&mut self, _: usize) -> Result<(), ()> {
        Ok(())
    }
}

#[test]
fn capture_builder_totals_match_owned_entries_and_survive_failed_appends() {
    let mut builder = CaptureBuilder::default();
    let mut observer = CaptureProbe {
        reject_admission: false,
        admissions: 0,
        actual: 0,
    };
    for index in 0..40 {
        let (digits, start) = decimal_bytes(index);
        push_capture(
            &mut builder,
            CaptureKeySource::Numeric {
                index,
                bytes: &digits[start..],
            },
            CaptureSource::InputRange {
                input: b"prefix-value",
                start: 7,
                len: index % 6,
            },
            RuntimeLimits::default(),
            &mut observer,
        )
        .unwrap_or_else(|_| panic!("capture append failed"));
        assert_eq!(
            builder.semantic_bytes,
            builder
                .entries
                .iter()
                .map(|entry| entry.key.len() + entry.value.len())
                .sum::<usize>()
        );
        assert_eq!(
            builder.retained_bytes().unwrap(),
            capture_retained_bytes(&builder.entries).unwrap()
        );
        assert_eq!(observer.actual, builder.retained_bytes().unwrap());
    }
    let previous = (
        builder.entries.len(),
        builder.semantic_bytes,
        builder.heap_bytes,
        builder.entries.capacity(),
    );
    observer.reject_admission = true;
    assert!(matches!(
        push_capture(
            &mut builder,
            CaptureKeySource::Numeric { index: 0, bytes: b"0" },
            CaptureSource::InputRange {
                input: b"hit",
                start: 0,
                len: 3
            },
            RuntimeLimits::default(),
            &mut observer,
        ),
        Err(PredicateStageFailure::Observer(()))
    ));
    assert_eq!(
        (
            builder.entries.len(),
            builder.semantic_bytes,
            builder.heap_bytes,
            builder.entries.capacity()
        ),
        previous
    );
    builder.clear();
    assert_eq!(builder.semantic_bytes, 0);
    assert_eq!(builder.heap_bytes, 0);
    assert_eq!(
        builder.retained_bytes().unwrap(),
        previous.3 * size_of::<CaptureEntry>()
    );
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
            assert_eq!(
                matches[0]
                    .captures
                    .last()
                    .unwrap()
                    .key
                    .observed_bytes(&transaction.request)
                    .unwrap(),
                b"g33"
            );
            let mut ranged_builder = CaptureBuilder::default();
            extract_captures_into(
                &compiled,
                &mut transaction.capture_scratch,
                compiled.operator_plans[0].unwrap(),
                input.as_bytes(),
                transaction.limits,
                None,
                &mut RangeCaptureProbe,
                &mut ranged_builder,
            )
            .unwrap_or_else(|_| panic!("range extraction failed"));
            let ranged = ranged_builder.entries;
            assert_eq!(ranged.len(), 69);
            assert!(ranged.capacity() >= 69);
            assert_eq!(
                ranged.last().unwrap().key.observed_bytes(&transaction.request).unwrap(),
                b"g33"
            );
            assert!(matches!(
                ranged.last().unwrap().value,
                StageCaptureValue::InputRange { start: 33, len: 1 }
            ));
            assert_eq!(
                capture_retained_bytes(&ranged).unwrap(),
                ranged.capacity() * size_of::<CaptureEntry<StageCaptureValue>>()
                    + ranged.iter().map(|entry| entry.key.owned_capacity()).sum::<usize>()
            );
            assert_eq!(
                member_match_bytes(&matches[0]).unwrap(),
                size_of::<MemberMatch>()
                    + 69 * size_of::<CaptureEntry<MatchValue>>()
                    + matches[0]
                        .captures
                        .iter()
                        .map(|entry| entry.key.len() + entry.value.len())
                        .sum::<usize>()
                    + matches[0].matched_var.as_ref().unwrap().len()
                    + matches[0].matched_var_name.as_ref().unwrap().len()
            );
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
        assert_eq!(
            matches[0].captures[0].key.observed_bytes(&transaction.request).unwrap(),
            b"0"
        );
        assert_eq!(
            matches[0].captures[0].value.bytes(&transaction.request, None).unwrap(),
            captured
        );
    }
}

// Frozen baseline frontier algorithm; only persistent key APIs are adapted.
#[test]
fn final_chain_move_matches_frozen_frontier_order_rollback_and_limits() {
    for final_recipe in ["@streq %{tx.saved_mid}", "@streq absent"] {
        for action in ["setvar:tx.winner=%{tx.saved_root}", "setvar:tx.counter=+%{tx.bad}"] {
            let source = format!(concat!(
                "SecRule REQUEST_HEADERS:X-Root \"@rx ^(?P<root>.*)$\" \"id:1,phase:1,pass,capture,chain,setvar:tx.saved_root=%{{TX.root}},ctl:ruleRemoveTargetByTag=kept;TX:skip\"\n",
                "SecRule REQUEST_HEADERS:X-Mid \"@rx ^(?P<mid>.*)$\" \"capture,chain,setvar:tx.saved_mid=%{{TX.mid}}\"\n",
                "SecRule TX:saved_root \"{}\" \"{}\"\n",
                "SecRule TX:skip \"@streq ignored\" \"id:2,phase:1,pass,tag:kept\"\n",
            ), final_recipe, action);
            let generation = generation(&source);
            let headers = [
                RequestHeader::new(b"X-Root", b"a"),
                RequestHeader::new(b"X-Root", b"b"),
                RequestHeader::new(b"X-Mid", b"x"),
                RequestHeader::new(b"X-Mid", b"b"),
            ];
            let check = |limits: RuntimeLimits| {
                let make = || {
                    let mut tx =
                        Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
                    for (key, value) in [
                        (b"saved_root".as_slice(), b"seed".as_slice()),
                        (b"saved_mid", b"seed"),
                        (b"bad", b"invalid"),
                    ] {
                        tx.tx
                            .set(key.to_vec(), value.to_vec(), tx.limits, &tx.request, None)
                            .unwrap();
                    }
                    tx.limits = limits;
                    tx
                };
                let mut actual = make();
                let mut oracle = make();
                let expected = frozen_clone_unit_into(&mut oracle, 0, None, &mut Vec::new());
                let result = actual.evaluate_unit(0, None);
                assert_eq!(result, expected, "{final_recipe}/{action}: {limits:?}");
                assert_eq!(actual.tx.retained_bytes, oracle.tx.retained_bytes);
                assert_eq!(actual.detector_work_bytes, oracle.detector_work_bytes);
                assert_eq!(actual.ctl_work_bytes, oracle.ctl_work_bytes);
                assert_eq!(actual.tx.target_exclusions, oracle.tx.target_exclusions);
                assert_eq!(actual.tx.removed_units, oracle.tx.removed_units);
                assert_eq!(
                    actual
                        .tx
                        .entries
                        .iter()
                        .map(|entry| (
                            entry.key.observed_bytes(&actual.request).unwrap(),
                            entry.value.observed_bytes(&actual.request).unwrap()
                        ))
                        .collect::<Vec<_>>(),
                    oracle
                        .tx
                        .entries
                        .iter()
                        .map(|entry| (
                            entry.key.observed_bytes(&oracle.request).unwrap(),
                            entry.value.observed_bytes(&oracle.request).unwrap()
                        ))
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    actual
                        .capture_keys
                        .iter()
                        .map(|key| key.observed_bytes(&actual.request).unwrap())
                        .collect::<Vec<_>>(),
                    oracle
                        .capture_keys
                        .iter()
                        .map(|key| key.observed_bytes(&oracle.request).unwrap())
                        .collect::<Vec<_>>()
                );
                if result == Ok(true) {
                    assert_eq!(actual.tx_value(b"winner"), Some(b"b".as_slice()));
                } else {
                    assert_eq!(actual.tx_value(b"saved_root"), Some(b"seed".as_slice()));
                }
                result
            };
            let defaults = RuntimeLimits::default();
            let expected = check(defaults);
            assert_eq!(
                expected.is_err(),
                action.contains("counter") && final_recipe.contains("mid")
            );
            assert_eq!(
                expected == Ok(true),
                final_recipe.contains("mid") && !action.contains("counter")
            );
            for bound in 0..=8 {
                let _ = check(RuntimeLimits {
                    max_operator_evaluations: bound,
                    ..defaults
                });
            }
            for bound in [0, 1, 2, 3, 4] {
                let _ = check(RuntimeLimits {
                    max_setvar_actions: bound,
                    ..defaults
                });
            }
            for bound in [0, 128, 256, 512, 1024, 2048, 4096, 8192] {
                let _ = check(RuntimeLimits {
                    max_operator_work_bytes: bound,
                    ..defaults
                });
                let _ = check(RuntimeLimits {
                    max_tx_bytes: bound,
                    ..defaults
                });
                let _ = check(RuntimeLimits {
                    max_retained_bytes: bound,
                    ..defaults
                });
            }
            if expected == Ok(true) {
                for dimension in 0..3 {
                    let mut low = 0;
                    let mut high = if dimension == 0 {
                        defaults.max_operator_work_bytes
                    } else if dimension == 1 {
                        defaults.max_retained_bytes
                    } else {
                        defaults.max_tx_bytes
                    };
                    while low < high {
                        let mid = low + (high - low) / 2;
                        let mut limits = defaults;
                        match dimension {
                            0 => limits.max_operator_work_bytes = mid,
                            1 => limits.max_retained_bytes = mid,
                            _ => limits.max_tx_bytes = mid,
                        }
                        if check(limits) == Ok(true) {
                            high = mid;
                        } else {
                            low = mid + 1;
                        }
                    }
                    for (bound, succeeds) in [(low, true), (low - 1, false)] {
                        let mut limits = defaults;
                        match dimension {
                            0 => limits.max_operator_work_bytes = bound,
                            1 => limits.max_retained_bytes = bound,
                            _ => limits.max_tx_bytes = bound,
                        }
                        assert_eq!(check(limits) == Ok(true), succeeds);
                    }
                }
            }
        }
    }
}

fn frozen_clone_unit_into(
    tx: &mut Transaction<'_>,
    unit_id: usize,
    prefix_match: Option<bool>,
    matches: &mut Vec<MemberMatch>,
) -> Result<bool, RuntimeError> {
    let unit = *tx
        .generation
        .units
        .get(unit_id)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let end = unit
        .member_start
        .checked_add(unit.member_count)
        .ok_or(RuntimeError::Arithmetic)?;
    tx.evaluate_unit_first_member(
        unit_id,
        unit.member_start,
        if unit.member_count == 1 { prefix_match } else { None },
        matches,
    )?;
    if matches.is_empty() {
        return Ok(false);
    }
    let mut initial = tx.tx.try_clone_for_unit()?;
    if unit.member_count == 1 {
        let mut capture_keys = try_clone_keys(&tx.capture_keys)?;
        let mut setvar_actions = 0_usize;
        for matched in matches.drain(..) {
            apply_member_match(
                &mut initial,
                &mut capture_keys,
                matched,
                tx.limits,
                &tx.request,
                tx.json_budget.as_mut(),
            )?;
            tx.apply_member_actions(unit.member_start, &mut initial, &mut setvar_actions)?;
        }
        return tx.commit_unit_match(unit, initial, capture_keys);
    }
    let mut frontier = Vec::new();
    frontier.try_reserve_exact(1).map_err(|_| RuntimeError::Allocation)?;
    frontier.push(ChainBranch {
        tx: initial,
        capture_keys: try_clone_keys(&tx.capture_keys)?,
        setvar_actions: 0,
    });
    for member_id in unit.member_start..end {
        let final_member = member_id + 1 == end;
        let current_bytes = chain_branches_bytes(&frontier)?;
        let mut next = Vec::new();
        let mut next_bytes = 0_usize;
        for branch in &frontier {
            if member_id != unit.member_start {
                tx.evaluate_member_matches_into(unit_id, member_id, &branch.tx, final_member, matches)?;
            }
            for matched in matches.drain(..) {
                let mut candidate = ChainBranch {
                    tx: branch.tx.try_clone()?,
                    capture_keys: try_clone_keys(&branch.capture_keys)?,
                    setvar_actions: branch.setvar_actions,
                };
                apply_member_match(
                    &mut candidate.tx,
                    &mut candidate.capture_keys,
                    matched,
                    tx.limits,
                    &tx.request,
                    tx.json_budget.as_mut(),
                )?;
                tx.apply_member_actions(member_id, &mut candidate.tx, &mut candidate.setvar_actions)?;
                if final_member {
                    return tx.commit_unit_match(unit, candidate.tx, candidate.capture_keys);
                }
                if frontier.len().saturating_add(next.len()) >= tx.limits.max_operator_evaluations {
                    return Err(RuntimeError::OperatorEvaluationLimit {
                        limit: tx.limits.max_operator_evaluations,
                    });
                }
                next_bytes = next_bytes
                    .checked_add(chain_branch_bytes(&candidate)?)
                    .ok_or(RuntimeError::Arithmetic)?;
                if current_bytes.checked_add(next_bytes).ok_or(RuntimeError::Arithmetic)?
                    > tx.limits.max_operator_work_bytes
                {
                    return Err(RuntimeError::OperatorWorkLimit {
                        limit: tx.limits.max_operator_work_bytes,
                    });
                }
                next.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
                next.push(candidate);
            }
        }
        if next.is_empty() {
            return Ok(false);
        }
        frontier = next;
    }
    Ok(false)
}

#[test]
fn matcher_capacity_accounting_checks_slot_multiplication_and_total_overflow() {
    assert_eq!(matcher_scratch_capacity(0, 0, 0, 0), Ok(0));
    assert_eq!(matcher_scratch_capacity(usize::MAX, 0, 0, 0), Ok(usize::MAX));
    let expected = 11
        + 2 * size_of::<Option<MatchScratch>>()
        + 3 * size_of::<Option<RegexBooleanScratch>>()
        + 4 * size_of::<Option<RegexCaptureScratch>>();
    assert_eq!(matcher_scratch_capacity(11, 2, 3, 4), Ok(expected));
    for (groups, booleans, captures) in [(usize::MAX, 0, 0), (0, usize::MAX, 0), (0, 0, usize::MAX)] {
        assert_eq!(
            matcher_scratch_capacity(0, groups, booleans, captures),
            Err(RuntimeError::Arithmetic)
        );
    }
    for (groups, booleans, captures) in [(1, 0, 0), (0, 1, 0), (0, 0, 1)] {
        assert_eq!(
            matcher_scratch_capacity(usize::MAX, groups, booleans, captures),
            Err(RuntimeError::Arithmetic)
        );
    }
}

#[test]
fn lazy_program_slots_initialize_once_and_preserve_generation_wide_admission() {
    let g = generation(concat!(
        "SecRule REQUEST_URI \"@pm hit one\" \"id:901,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:/^x-/ \"@rx hit\" \"id:902,phase:1,pass\"\n",
        "SecRule REQUEST_URI \"@rx (?P<word>hit)\" \"id:903,phase:1,pass,capture\"\n",
        "SecRule ARGS \"@rx unused\" \"id:904,phase:2,pass\"\n",
    ));
    let scope = measurements::start();
    let headers = [
        RequestHeader::new(b"x-one", b"hit"),
        RequestHeader::new(b"x-two", b"hit"),
    ];
    let mut tx = Transaction::new(&g, request(&headers, b"/hit"), RuntimeLimits::default()).unwrap();
    assert!(tx.group_scratch.iter().all(Option::is_none));
    assert!(tx.boolean_scratch.iter().all(Option::is_none));
    assert!(tx.capture_scratch.iter().all(Option::is_none));
    assert_eq!(scope.counts().get(Event::GroupScratchInitializations), 0);
    assert_eq!(scope.counts().get(Event::BooleanScratchInitializations), 0);
    assert_eq!(scope.counts().get(Event::CaptureScratchInitializations), 0);
    let program_reservation = tx.scratch_bytes;
    tx.execute_request_headers().unwrap();
    assert_eq!(scope.counts().get(Event::GroupScratchInitializations), 2);
    assert_eq!(scope.counts().get(Event::BooleanScratchInitializations), 1);
    assert_eq!(scope.counts().get(Event::CaptureScratchInitializations), 1);
    assert_eq!(tx.tx_value(b"word"), Some(b"hit".as_slice()));
    assert!(tx.group_scratch.iter().any(Option::is_none));
    let expected = matcher_scratch_capacity(
        matcher_program_scratch_bytes(&g).unwrap(),
        tx.group_scratch.capacity(),
        tx.boolean_scratch.capacity(),
        tx.capture_scratch.capacity(),
    )
    .unwrap();
    assert_eq!(program_reservation, expected);
    let exact = RuntimeLimits {
        max_matcher_scratch_bytes: expected,
        ..RuntimeLimits::default()
    };
    assert!(Transaction::new(&g, request(&headers, b"/hit"), exact).is_ok());
    assert!(matches!(
        Transaction::new(
            &g,
            request(&headers, b"/hit"),
            RuntimeLimits {
                max_matcher_scratch_bytes: expected - 1,
                ..exact
            }
        ),
        Err(RuntimeError::MatcherScratchByteLimit { .. })
    ));
}
#[test]
fn cached_group_scan_requires_an_initialized_slot_and_reuses_first_use() {
    let g = generation("SecRule REQUEST_URI \"@pm hit one\" \"id:905,phase:1,pass\"\n");
    let mut slots = vec![None];
    let mut cache = StageScanCache {
        storage: ScanCacheStorage::Overflow(vec![0]),
        ..StageScanCache::default()
    };
    assert_eq!(
        evaluate_matcher_operand_cached(
            &g,
            &mut slots,
            0,
            0,
            1,
            b"hit",
            EvaluationMode::Accelerated,
            Some(&mut cache)
        ),
        Err(RuntimeError::GenerationInvariant)
    );
    assert!(slots[0].is_none());
    cache.clear();
    let scope = measurements::start();
    assert!(evaluate_matcher_operand_cached(
        &g,
        &mut slots,
        0,
        0,
        1,
        b"hit",
        EvaluationMode::Accelerated,
        Some(&mut cache)
    )
    .unwrap());
    assert!(evaluate_matcher_operand_cached(
        &g,
        &mut slots,
        0,
        0,
        1,
        b"hit",
        EvaluationMode::Accelerated,
        Some(&mut cache)
    )
    .unwrap());
    assert_eq!(scope.counts().get(Event::GroupScratchInitializations), 1);
    assert_eq!(scope.counts().get(Event::FullScans), 1);
}

#[test]
fn stage_scan_cache_storage_has_checked_ids_capacity_and_reuse() {
    assert_eq!(std::mem::size_of::<ScanCacheStorage>(), 24);
    assert_eq!(std::mem::size_of::<StageScanCache>(), 32); // Includes the test-only counter.
    let mut cache = StageScanCache {
        storage: ScanCacheStorage::Inline { bits: 0, capacity: 2 },
        ..StageScanCache::default()
    };
    assert_eq!(cache.heap_capacity(), 0);
    for bad in [64, usize::MAX] {
        assert!(!cache.contains(bad));
        assert_eq!(cache.insert(bad), Err(RuntimeError::GenerationInvariant));
        assert_eq!(cache.len(), 0);
    }
    cache.insert(0).unwrap();
    cache.insert(63).unwrap();
    cache.insert(63).unwrap();
    assert_eq!(cache.len(), 2);
    assert!(cache.contains(0) && cache.contains(63) && cache.full());
    assert_eq!(cache.insert(1), Err(RuntimeError::GenerationInvariant));
    assert!(!cache.contains(1));
    cache.clear();
    assert_eq!(cache.len(), 0);
    assert!(!cache.contains(0) && !cache.contains(63));
    cache.insert(1).unwrap();
    assert!(cache.contains(1));

    let mut empty = StageScanCache::default();
    assert!(empty.full());
    assert_eq!(empty.insert(0), Err(RuntimeError::GenerationInvariant));
    let mut single = StageScanCache {
        storage: ScanCacheStorage::Single(None),
        ..StageScanCache::default()
    };
    for group in [0, 63, 64, usize::MAX] {
        single.insert(group).unwrap();
        assert!(single.contains(group));
        assert_eq!(single.len(), 1);
        assert_eq!(single.heap_capacity(), 0);
        assert!(!single.full());
    }
    assert!(!single.contains(64));
    single.clear();
    assert_eq!(single.len(), 0);

    let mut overflow = StageScanCache {
        storage: ScanCacheStorage::Overflow(Vec::with_capacity(2)),
        ..StageScanCache::default()
    };
    let capacity = overflow.heap_capacity();
    for id in 0..capacity {
        overflow.insert(64 + id).unwrap();
    }
    assert!(overflow.full());
    overflow.insert(64).unwrap();
    assert_eq!(overflow.insert(usize::MAX), Err(RuntimeError::GenerationInvariant));
    assert_eq!(overflow.heap_capacity(), capacity);
    overflow.clear();
    overflow.insert(usize::MAX).unwrap();
    assert!(overflow.contains(usize::MAX) && !overflow.contains(0));
    assert_eq!(overflow.heap_capacity(), capacity);
}

#[test]
fn ordinary_current_input_reuses_group_bits_and_preserves_ordered_effects() {
    let g = generation(concat!(
        "SecRule REQUEST_URI \"@rx alpha[0-9]\" \"id:910,phase:1,pass,setvar:tx.first=hit\"\n",
        "SecRule REQUEST_URI \"!@rx beta[0-9]\" \"id:911,phase:1,pass,setvar:tx.second=%{tx.first}\"\n",
        "SecRule REQUEST_URI \"@pm alpha gamma\" \"id:912,phase:1,pass,setvar:tx.last=hit\"\n",
    ));
    for uri in [b"/alpha1".as_slice(), b"/beta2", b"/none"] {
        let mut outcomes = Vec::new();
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let scope = measurements::start();
            let mut tx = Transaction::new_with_policy(
                &g,
                request(&[], uri),
                RuntimeLimits::default(),
                mode,
                RuleEngineMode::On,
                RuleCheckPolicy::default(),
            )
            .unwrap();
            let outcome = tx.execute_request_headers().unwrap();
            assert_eq!(
                scope.counts().get(Event::FullScans),
                if mode == EvaluationMode::Accelerated { 1 } else { 3 }
            );
            assert_eq!(tx.ordinary_scan_cache.cache.heap_capacity(), 0);
            outcomes.push((
                outcome,
                tx.tx_value(b"first").map(Vec::from),
                tx.tx_value(b"second").map(Vec::from),
                tx.tx_value(b"last").map(Vec::from),
            ));
        }
        assert_eq!(outcomes[0], outcomes[1]);
    }
}

fn query_alias_transaction<'g>(
    generation: &'g CompiledGeneration,
    head: RequestHead<'_>,
    limits: RuntimeLimits,
    mode: EvaluationMode,
) -> Result<Transaction<'g>, RuntimeError> {
    let mut tx = Transaction::new_with_policy(
        generation,
        head,
        limits,
        mode,
        RuleEngineMode::On,
        RuleCheckPolicy::default(),
    )?;
    tx.execute_request_headers()?;
    tx.publish_request_body(RequestBody::new(
        bytes::Bytes::new(),
        crate::seclang::BodyProcessorSelection::Raw,
    ))?;
    Ok(tx)
}

#[test]
fn ordinary_query_aliases_reuse_scans_but_charge_each_selected_value() {
    let g = generation("SecRule ARGS:q|ARGS_GET:q \"@rx absent[0-9]\" \"id:951,phase:2,pass,t:lowercase\"\n");
    for uri in [b"/?q=safe".as_slice(), b"/?q=safe&q=safe"] {
        let fields = if uri == b"/?q=safe" { 1 } else { 2 };
        let mut results = Vec::new();
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let scope = measurements::start();
            let mut tx = query_alias_transaction(&g, request(&[], uri), RuntimeLimits::default(), mode).unwrap();
            results.push(tx.execute_request_body().unwrap());
            // One-slot reuse applies to the adjacent aliases of one field only.
            assert_eq!(
                scope.counts().get(Event::FullScans),
                if fields == 1 && mode == EvaluationMode::Accelerated {
                    1
                } else {
                    2 * fields
                }
            );
            assert_eq!(
                tx.borrowed_transform_memo.key.is_some(),
                mode == EvaluationMode::Accelerated
            );
        }
        assert_eq!(results[0], results[1]);
    }
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        for work in [15, 16] {
            let mut tx = query_alias_transaction(
                &g,
                request(&[], b"/?q=safe"),
                RuntimeLimits {
                    max_operator_work_bytes: work,
                    ..RuntimeLimits::default()
                },
                mode,
            )
            .unwrap();
            let result = tx.execute_request_body();
            if work == 16 {
                assert!(result.is_ok());
            } else {
                assert_eq!(result, Err(RuntimeError::OperatorWorkLimit { limit: work }));
            }
        }
        for selected in [1, 2] {
            let mut tx = query_alias_transaction(
                &g,
                request(&[], b"/?q=safe"),
                RuntimeLimits {
                    max_selected_values: selected,
                    ..RuntimeLimits::default()
                },
                mode,
            )
            .unwrap();
            let result = tx.execute_request_body();
            if selected == 2 {
                assert!(result.is_ok());
            } else {
                assert_eq!(result, Err(RuntimeError::SelectedValueLimit { limit: selected }));
            }
        }
    }
}

#[test]
fn ordinary_alias_identity_memo_preserves_captures_tx_controls_and_collection_names() {
    let g = generation(concat!(
        "SecRule ARGS:q \"@rx ^(?P<saved>safe)$\" \"id:952,phase:2,pass,capture,t:lowercase,setvar:tx.first=%{MATCHED_VAR_NAME}\"\n",
        "SecRule ARGS_GET:q \"@rx ^(?P<saved>safe)$\" \"id:953,phase:2,pass,capture,t:lowercase,setvar:tx.second=%{MATCHED_VAR_NAME}\"\n",
        "SecAction \"id:954,phase:1,pass,ctl:ruleRemoveTargetByTag=next;ARGS:q\"\n",
        "SecRule ARGS:q|ARGS_GET:q \"@streq safe\" \"id:955,phase:2,pass,tag:next,setvar:tx.last=%{MATCHED_VAR_NAME}\"\n",
    ));
    let mut outcomes = Vec::new();
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        let mut tx = query_alias_transaction(&g, request(&[], b"/?q=safe"), RuntimeLimits::default(), mode).unwrap();
        let result = tx.execute_request_body().unwrap();
        assert_eq!(tx.tx_value(b"first"), Some(b"ARGS:q".as_slice()));
        assert_eq!(tx.tx_value(b"second"), Some(b"ARGS_GET:q".as_slice()));
        assert_eq!(tx.tx_value(b"last"), Some(b"ARGS_GET:q".as_slice()));
        assert_eq!(tx.tx_value(b"saved"), Some(b"safe".as_slice()));
        outcomes.push(result);
    }
    assert_eq!(outcomes[0], outcomes[1]);
}

#[test]
fn borrowed_transform_memo_alias_keys_bind_ranges_profiles_limits_and_eligibility() {
    let state = RequestState::build(request(&[], b"/?q=safe&q=safe"), RuntimeLimits::default()).unwrap();
    let first = state.collection_records(CollectionKind::Args).unwrap()[0].value;
    let alias = state.collection_records(CollectionKind::ArgsGet).unwrap()[0].value;
    let second = state.collection_records(CollectionKind::Args).unwrap()[1].value;
    let limits = RuntimeLimits::default();
    let mut memo = BorrowedTransformMemo::default();
    let key = memo
        .prepare(SelectedBytes::Request(first), Some(1), &state, limits)
        .unwrap()
        .1;
    memo.key = Some(key);
    let alias_key = memo
        .prepare(SelectedBytes::Request(alias), Some(1), &state, limits)
        .unwrap()
        .1;
    assert!(key == alias_key);
    for (range, plan, changed_limits) in [
        (second, Some(1), limits),
        (first, Some(2), limits),
        (
            first,
            Some(1),
            RuntimeLimits {
                max_transformation_output_bytes: 1,
                ..limits
            },
        ),
        (
            first,
            Some(1),
            RuntimeLimits {
                max_transformation_work_bytes: 1,
                ..limits
            },
        ),
    ] {
        let changed = memo
            .prepare(SelectedBytes::Request(range), plan, &state, changed_limits)
            .unwrap()
            .1;
        assert!(key != changed);
    }
    for value in [
        SelectedBytes::Body(0),
        SelectedBytes::Tx(0),
        SelectedBytes::Response(0),
        SelectedBytes::ResponseWindow,
        SelectedBytes::MatchedVar,
        SelectedBytes::Request(ByteRange {
            start: usize::MAX,
            len: 1,
        }),
    ] {
        memo.key = Some(key);
        assert!(memo.prepare(value, Some(1), &state, limits).is_none());
        assert!(memo.key.is_none());
    }
    memo.key = Some(key);
    assert!(memo
        .prepare(SelectedBytes::Request(first), None, &state, limits)
        .is_none());
    assert!(memo.key.is_none());
}

#[test]
fn borrowed_transform_memo_publishes_only_successful_ordinary_identity_walks() {
    for (actions, value, published) in [
        ("t:lowercase", "safe", true),
        ("t:lowercase", "SAFE", false),
        ("t:lowercase,multiMatch", "safe", false),
        ("t:none", "safe", false),
    ] {
        let g = generation(&format!(
            "SecRule ARGS:q|ARGS_GET:q \"@rx absent\" \"id:956,phase:2,pass,{actions}\"\n"
        ));
        let uri = format!("/?q={value}");
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let mut tx =
                query_alias_transaction(&g, request(&[], uri.as_bytes()), RuntimeLimits::default(), mode).unwrap();
            tx.execute_request_body().unwrap();
            assert_eq!(
                tx.borrowed_transform_memo.key.is_some(),
                published && mode == EvaluationMode::Accelerated
            );
        }
    }
    let g = generation(concat!(
        "SecRule ARGS_GET:q \"@rx absent\" \"id:957,phase:1,pass,t:lowercase\"\n",
        "SecRule ARGS_GET:q \"@rx absent\" \"id:958,phase:2,pass,t:lowercase\"\n",
    ));
    let mut tx = Transaction::new(&g, request(&[], b"/?q=safe"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    assert!(tx.borrowed_transform_memo.key.is_some());
    tx.limits.max_transformation_work_bytes = 0;
    assert!(matches!(
        tx.evaluate_prefix_member(1, b"safe", &mut 0),
        Err(RuntimeError::Transformation { .. })
    ));
    assert!(tx.borrowed_transform_memo.key.is_none());
    assert!(tx.ordinary_scan_cache.key.is_none());
    // A lower limit must not reuse the alias's earlier successful transform proof.
    tx.publish_request_body(RequestBody::new(
        bytes::Bytes::new(),
        crate::seclang::BodyProcessorSelection::Raw,
    ))
    .unwrap();
    assert!(matches!(
        tx.execute_request_body(),
        Err(RuntimeError::Transformation {
            kind: RuntimeTransformError::WorkLimit
        })
    ));
    assert!(tx.borrowed_transform_memo.key.is_none());
}

#[test]
fn borrowed_transform_memo_reuses_marker_free_crs_xss_decoder_chain() {
    let g = generation(concat!(
        "SecRule ARGS:q|ARGS_GET:q \"@rx absent[0-9]\" ",
        "\"id:960,phase:2,pass,t:urlDecodeUni,t:htmlEntityDecode,t:jsDecode,t:cssDecode\"\n",
    ));
    for (uri, published) in [(b"/?q=safe".as_slice(), true), (b"/?q=a%5Cb".as_slice(), false)] {
        let mut results = Vec::new();
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let scope = measurements::start();
            let mut tx = query_alias_transaction(&g, request(&[], uri), RuntimeLimits::default(), mode).unwrap();
            results.push(tx.execute_request_body().unwrap());
            // Only a chain that stays borrowed end to end publishes the one-slot
            // memo key that the alias evaluation of the same value then hits; the
            // marker-bearing value decodes through owned scratch and never does.
            assert_eq!(
                tx.borrowed_transform_memo.key.is_some(),
                published && mode == EvaluationMode::Accelerated
            );
            // Scan reuse across the alias pair is unchanged by the borrowed chain.
            assert_eq!(
                scope.counts().get(Event::FullScans),
                if mode == EvaluationMode::Accelerated { 1 } else { 2 }
            );
        }
        assert_eq!(results[0], results[1]);
    }
}

#[test]
fn borrowed_transform_memo_phase_entry_clears_identity_without_another_operand() {
    let g = generation("SecRule ARGS_GET:q \"@rx absent\" \"id:959,phase:1,pass,t:lowercase\"\n");
    let mut tx = Transaction::new(&g, request(&[], b"/?q=safe"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    assert!(tx.borrowed_transform_memo.key.is_some());
    assert!(tx.ordinary_scan_cache.key.is_some());
    tx.publish_request_body(RequestBody::new(
        bytes::Bytes::new(),
        crate::seclang::BodyProcessorSelection::Raw,
    ))
    .unwrap();
    tx.execute_request_body().unwrap();
    assert!(tx.borrowed_transform_memo.key.is_none());
    assert!(tx.ordinary_scan_cache.key.is_none());
}

#[test]
fn ordinary_current_input_switches_fields_profiles_and_uncacheable_operands() {
    let g = generation(concat!(
        "SecRule REQUEST_HEADERS:X-Probe \"@rx alpha[0-9]\" \"id:920,phase:1,pass,setvar:tx.value=alpha1\"\n",
        "SecRule REQUEST_HEADERS:X-Probe \"!@rx beta[0-9]\" \"id:921,phase:1,pass\"\n",
        "SecRule TX:value \"@rx alpha[0-9]\" \"id:922,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Probe \"@rx alpha[0-9]\" \"id:923,phase:1,pass\"\n",
        "SecRule REQUEST_HEADERS:X-Probe \"@rx alpha[0-9]\" \"id:924,phase:1,pass,t:lowercase\"\n",
        "SecRule REQUEST_HEADERS:X-Probe \"@rx alpha[0-9]\" \"id:925,phase:1,pass,multiMatch,t:lowercase\"\n",
        "SecRule REQUEST_HEADERS:X-Probe \"@rx (?P<saved>alpha[0-9])\" \"id:926,phase:1,pass,capture\"\n",
    ));
    let headers = [
        RequestHeader::new(b"X-Probe", b"beta2"),
        RequestHeader::new(b"X-Probe", b"alpha1"),
    ];
    let mut outcomes = Vec::new();
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        let mut tx = Transaction::new_with_policy(
            &g,
            request(&headers, b"/"),
            RuntimeLimits::default(),
            mode,
            RuleEngineMode::On,
            RuleCheckPolicy::default(),
        )
        .unwrap();
        outcomes.push((
            tx.execute_request_headers().unwrap(),
            tx.tx_value(b"saved").map(Vec::from),
        ));
        assert_eq!(tx.ordinary_scan_cache.cache.heap_capacity(), 0);
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(outcomes[0].1.as_deref(), Some(b"alpha1".as_slice()));
}

#[test]
fn ordinary_current_input_signature_is_inline_and_invalidates_all_mutable_sources() {
    let mut cache = OrdinaryScanCache::new();
    let first = SelectedBytes::Request(ByteRange { start: 1, len: 3 });
    let second = SelectedBytes::Request(ByteRange { start: 5, len: 3 });
    cache.prepare(first, None, true).unwrap().insert(2).unwrap();
    assert!(cache.prepare(first, None, true).unwrap().contains(2));
    assert!(!cache.prepare(second, None, true).unwrap().contains(2));
    cache.cache.insert(3).unwrap();
    assert!(!cache.prepare(second, Some(1), true).unwrap().contains(3));
    cache.cache.insert(4).unwrap();
    cache.cache.insert(5).unwrap();
    assert!(!cache.cache.contains(4));
    assert!(cache.cache.contains(5));
    assert!(cache.prepare(SelectedBytes::Body(0), None, true).is_none());
    cache.prepare(first, None, true).unwrap().insert(2).unwrap();
    assert!(cache.prepare(first, None, false).is_none());
    assert_eq!(cache.cache.heap_capacity(), 0);
    assert_eq!(cache.key, None);
}

#[test]
fn ordinary_current_input_phase_and_direct_prefix_writers_invalidate_saved_bits() {
    let g = generation(concat!(
        "SecRule REQUEST_URI \"@rx hit[0-9]\" \"id:930,phase:1,pass\"\n",
        "SecRule REQUEST_BODY \"@rx body[0-9]\" \"id:931,phase:2,pass\"\n",
    ));
    let mut tx = Transaction::new(&g, request(&[], b"/hit1"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    assert!(tx.ordinary_scan_cache.key.is_some());
    assert!(tx.evaluate_prefix_member(1, b"body1", &mut 0).unwrap());
    assert_eq!(tx.ordinary_scan_cache.key, None);
    tx.ordinary_scan_cache
        .prepare(SelectedBytes::Request(ByteRange { start: 0, len: 1 }), None, true)
        .unwrap()
        .insert(0)
        .unwrap();
    tx.publish_request_body(RequestBody::new(
        bytes::Bytes::from_static(b"body1"),
        crate::seclang::BodyProcessorSelection::Auto,
    ))
    .unwrap();
    tx.execute_request_body().unwrap();
    assert_eq!(tx.ordinary_scan_cache.key, None);
}

#[test]
fn ordinary_current_group_marker_publishes_only_valid_scans_and_ranges() {
    let g = generation("SecRule REQUEST_URI \"@rx hit[0-9]\" \"id:932,phase:1,pass\"\n");
    let mut slots = vec![None];
    let mut cache = OrdinaryScanCache::new();
    assert_eq!(
        evaluate_matcher_operand_cached(
            &g,
            &mut slots,
            0,
            1,
            1,
            b"hit1",
            EvaluationMode::Accelerated,
            Some(&mut cache.cache)
        ),
        Err(RuntimeError::GenerationInvariant)
    );
    assert!(!cache.cache.contains(0));
    assert!(evaluate_matcher_operand_cached(
        &g,
        &mut slots,
        0,
        0,
        1,
        b"hit1",
        EvaluationMode::Accelerated,
        Some(&mut cache.cache)
    )
    .unwrap());
    assert!(cache.cache.contains(0));
    assert_eq!(
        evaluate_matcher_operand_cached(
            &g,
            &mut slots,
            0,
            0,
            2,
            b"hit1",
            EvaluationMode::Accelerated,
            Some(&mut cache.cache)
        ),
        Err(RuntimeError::GenerationInvariant)
    );
    assert!(cache.cache.contains(0));
    assert_eq!(cache.cache.heap_capacity(), 0);
}
#[test]
fn unpruned_oracle_survives_both_explicit_clone_policies() {
    let tx = TxState {
        force_unpruned_inspection: true,
        ..TxState::default()
    };
    assert!(tx.try_clone().unwrap().force_unpruned_inspection);
    assert!(tx.try_clone_for_unit().unwrap().force_unpruned_inspection);
}

#[test]
fn optional_check_policy_survives_unpruned_inspection_and_chain_clones() {
    let original = include_str!("../../fixtures/expensive-checks-crs-4.25.1.conf");
    let g = generation(&format!("{original}SecRule ARGS_NAMES \"@rx .\" \"id:20,phase:2,pass,chain\"\nSecRule ARGS \"@contains attack\" \"setvar:tx.chain=matched\"\n"));
    assert!(g.has_expensive_checks);
    let headers = [RequestHeader::new(b"content-type", b"application/json")];
    for unpruned in [false, true] {
        let mut tx = Transaction::new_with_policy(
            &g,
            request(&headers, b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            RuleEngineMode::On,
            RuleCheckPolicy {
                expensive_checks: false,
            },
        )
        .unwrap();
        tx.tx.force_unpruned_inspection = unpruned;
        assert!(!tx.tx.try_clone().unwrap().check_policy.expensive_checks);
        assert!(!tx.tx.try_clone_for_unit().unwrap().check_policy.expensive_checks);
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(
            bytes::Bytes::from_static(br#"{"probe":"attack"}"#),
            crate::seclang::BodyProcessorSelection::Auto,
        ))
        .unwrap();
        tx.execute_request_body().unwrap();
        assert_eq!(tx.tx_value(b"chain"), Some(b"matched".as_slice()));
        assert_eq!(tx.tx_value(b"paramcounter_ARGS_NAMES:json.probe"), None);
    }
}

#[test]
fn target_exclusion_insertion_keeps_unit_groups_and_duplicate_budget() {
    let mut state = TxState::default();
    let limits = RuntimeLimits {
        max_ctl_entries: 6,
        ..RuntimeLimits::default()
    };
    let entry = |unit, collection| ActiveTargetExclusion {
        unit,
        collection,
        selector: super::super::CompiledTargetSelector::None,
    };
    for (unit, collection) in [
        (7, CollectionKind::Args),
        (1, CollectionKind::Args),
        (4, CollectionKind::Args),
        (7, CollectionKind::RequestHeaders),
        (4, CollectionKind::Args),
        (0, CollectionKind::Args),
        (9, CollectionKind::Args),
    ] {
        state.add_target_exclusion(entry(unit, collection), limits).unwrap();
    }
    assert_eq!(
        state.target_exclusions,
        [
            entry(0, CollectionKind::Args),
            entry(1, CollectionKind::Args),
            entry(4, CollectionKind::Args),
            entry(7, CollectionKind::Args),
            entry(7, CollectionKind::RequestHeaders),
            entry(9, CollectionKind::Args),
        ]
    );
    let retained = state.retained_bytes;
    state
        .add_target_exclusion(entry(7, CollectionKind::Args), limits)
        .unwrap();
    assert_eq!(state.target_exclusions.len(), 6);
    assert_eq!(state.retained_bytes, retained);
    assert_eq!(
        state.add_target_exclusion(entry(6, CollectionKind::Args), limits),
        Err(RuntimeError::CtlEntryLimit { limit: 6 })
    );
}

#[test]
fn request_head_projection_matches_decimal_boundaries() {
    for size in [0, 1, 9, 10, 11, 99, 100, 101, 999, 1000, 1001] {
        let mut uri = b"/?".to_vec();
        uri.resize(uri.len() + size, b'a');
        let head = request(&[], &uri);
        let projection = projected_derived_bytes(head, query_part(head.uri)).unwrap();
        let state = RequestState::build(head, RuntimeLimits::default()).unwrap();
        let (digits, start) = decimal_bytes(size);
        assert_eq!(projection.query_combined_size, size);
        assert_eq!(state.bytes.len(), projection.arena);
        assert_eq!(
            state.first_value(CollectionKind::ArgsCombinedSize),
            Some(&digits[start..])
        );
    }
}
