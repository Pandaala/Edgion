use super::*;
use edgion_resources::sensitive::SENSITIVE_MODE;
use std::sync::atomic::Ordering;

#[test]
fn sensitive_unsigned_arguments_preserve_masking_limits_and_allocations() {
    assert_sensitive_test_in_subprocess(
        "sensitive_unsigned_arguments_preserve_masking_limits_and_allocations",
        || {
            for mode in [0, 1, 2] {
                SENSITIVE_MODE.store(mode, Ordering::SeqCst);
                for value in [0, 9, 10, 99, 100, u64::from(u16::MAX), u64::MAX] {
                    let decimal = value.to_string();
                    let rendered = match mode {
                        0 => decimal.clone(),
                        1 => "***".to_owned(),
                        _ => format!("<E>{decimal}</E>"),
                    };
                    // Warm the canonical masking state and Wrap buffer before counting.
                    let mut expected: SmallVec<[u8; 128]> = SmallVec::new();
                    assert!(format_into(&mut expected, "value={}", la![sensitive(&decimal)]));
                    let mut actual: SmallVec<[u8; 128]> = SmallVec::new();
                    let (accepted, allocations) = crate::test_allocation_counter::count_allocations(|| {
                        format_into(&mut actual, "value={}", la![sensitive_u64(value)])
                    });
                    assert!(accepted);
                    assert_eq!(actual, expected);
                    assert_eq!(allocations, 0);
                    assert_eq!(actual.as_slice(), format!("value={rendered}").as_bytes());

                    for size in [127, 128, 129, 511, 512, 513] {
                        let template = format!("{}{{}}", "x".repeat(size - rendered.len()));
                        let mut actual = PluginLog::new("sensitive-number");
                        let mut expected = PluginLog::new("sensitive-number");
                        assert_eq!(actual.push_fmt(&template, la![sensitive_u64(value)]), size <= 512);
                        assert_eq!(expected.push_fmt(&template, la![sensitive(&decimal)]), size <= 512);
                        assert_eq!(
                            serde_json::to_value(&actual).unwrap(),
                            serde_json::to_value(&expected).unwrap()
                        );
                        assert_eq!(actual.log_full, if size > 512 { Some(true) } else { None });
                        assert_eq!(
                            actual.push_fmt("later {}", la![sensitive_u64(0)]),
                            expected.push_fmt("later {}", la![sensitive("0")])
                        );
                        assert_eq!(
                            serde_json::to_value(&actual).unwrap(),
                            serde_json::to_value(&expected).unwrap()
                        );
                    }
                }
            }
        },
    );
}

#[test]
fn unsigned_arguments_match_decimal_text_without_scratch_allocation() {
    for value in [0, 200, u64::from(u16::MAX), u64::MAX] {
        let expected = format!("status={value}");
        let mut scratch: SmallVec<[u8; 128]> = SmallVec::new();
        let (accepted, allocations) =
            crate::test_allocation_counter::count_allocations(|| format_into(&mut scratch, "status={}", la![value]));
        assert!(accepted);
        assert_eq!(scratch.as_slice(), expected.as_bytes());
        assert_eq!(allocations, 0);
    }
    let mut scratch: SmallVec<[u8; 128]> = SmallVec::new();
    assert!(format_into(&mut scratch, "{}", la![u16::MAX]));
    assert_eq!(scratch.as_slice(), b"65535");
}

#[test]
fn unsigned_arguments_preserve_masking_and_entry_limits() {
    assert_sensitive_test_in_subprocess("unsigned_arguments_preserve_masking_and_entry_limits", || {
        for mode in [0, 1, 2] {
            SENSITIVE_MODE.store(mode, Ordering::SeqCst);
            let mut actual = PluginLog::new("unsigned");
            let mut expected = PluginLog::new("unsigned");
            assert!(actual.push_fmt("{} {}", la![200u16, sensitive("private")]));
            assert!(expected.push_fmt("{} {}", la!["200", sensitive("private")]));
            assert_eq!(
                serde_json::to_value(&actual).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );

            let decimal = u64::MAX.to_string();
            for size in [511, 512, 513] {
                let template = format!("{}{{}}", "x".repeat(size - decimal.len()));
                let mut actual = PluginLog::new("unsigned");
                let mut expected = PluginLog::new("unsigned");
                assert_eq!(actual.push_fmt(&template, la![u64::MAX]), size <= 512);
                assert_eq!(expected.push_fmt(&template, la![decimal.as_str()]), size <= 512);
                assert_eq!(
                    serde_json::to_value(&actual).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
                assert_eq!(
                    actual.push_fmt("later {}", la![0u64]),
                    expected.push_fmt("later {}", la!["0"])
                );
                assert_eq!(
                    serde_json::to_value(&actual).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
            }
        }
    });
}

#[test]
fn signed_and_pointer_arguments_match_decimal_text_without_scratch_allocation() {
    for (value, expected) in [
        (LogArg::from(i64::MIN), i64::MIN.to_string()),
        (LogArg::from(i64::MAX), i64::MAX.to_string()),
        (LogArg::from(-1i64), "-1".to_owned()),
        (LogArg::from(0i64), "0".to_owned()),
        (LogArg::from(isize::MIN), isize::MIN.to_string()),
        (LogArg::from(isize::MAX), isize::MAX.to_string()),
        (LogArg::from(usize::MAX), usize::MAX.to_string()),
    ] {
        let mut scratch: SmallVec<[u8; 128]> = SmallVec::new();
        let (accepted, allocations) = crate::test_allocation_counter::count_allocations(|| {
            format_into(&mut scratch, "{}", std::slice::from_ref(&value))
        });
        assert!(accepted);
        assert_eq!(scratch.as_slice(), expected.as_bytes());
        assert_eq!(allocations, 0);
    }
}

#[test]
fn signed_arguments_preserve_masking_and_entry_limits() {
    assert_sensitive_test_in_subprocess("signed_arguments_preserve_masking_and_entry_limits", || {
        for mode in [0, 1, 2] {
            SENSITIVE_MODE.store(mode, Ordering::SeqCst);
            let mut actual = PluginLog::new("signed");
            let mut expected = PluginLog::new("signed");
            assert!(actual.push_fmt("{} {} {}", la![200u16, -1i64, sensitive("private")]));
            assert!(expected.push_fmt("{} {} {}", la!["200", "-1", sensitive("private")]));
            assert_eq!(
                serde_json::to_value(&actual).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
            let decimal = i64::MIN.to_string();
            for size in [511, 512, 513] {
                let template = format!("{}{{}}", "x".repeat(size - decimal.len()));
                let mut actual = PluginLog::new("signed");
                let mut expected = PluginLog::new("signed");
                assert_eq!(actual.push_fmt(&template, la![i64::MIN]), size <= 512);
                assert_eq!(expected.push_fmt(&template, la![decimal.as_str()]), size <= 512);
                assert_eq!(
                    serde_json::to_value(&actual).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
                assert_eq!(
                    actual.push_fmt("later {}", la![-1isize]),
                    expected.push_fmt("later {}", la!["-1"])
                );
                assert_eq!(
                    serde_json::to_value(&actual).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
            }
        }
    });
}

fn assert_sensitive_test_in_subprocess(test_name: &str, assertion: impl FnOnce()) {
    const CHILD_ENV: &str = "EDGION_PLUGIN_LOG_SENSITIVE_CHILD";
    const TEST_MODULE: &str = "plugins::runtime::log::tests";
    crate::test_subprocess::assert_in_subprocess(TEST_MODULE, test_name, CHILD_ENV, test_name, assertion);
}

fn assert_sensitive_mode_test(test_name: &str, mode: u8, assertion: impl FnOnce()) {
    assert_sensitive_test_in_subprocess(test_name, || {
        SENSITIVE_MODE.store(mode, Ordering::SeqCst);
        assertion();
    });
}

#[test]
fn formatted_entries_respect_exact_rendered_limit_and_atomic_rejection() {
    assert_sensitive_test_in_subprocess(
        "formatted_entries_respect_exact_rendered_limit_and_atomic_rejection",
        || {
            for mode in [0, 1, 2] {
                SENSITIVE_MODE.store(mode, Ordering::SeqCst);
                let value = if mode == 1 {
                    "x".repeat(1024 * 1024)
                } else {
                    "é".to_string()
                };
                let rendered = match mode {
                    0 => "é",
                    1 => "***",
                    _ => "<E>é</E>",
                };
                for size in [512, 513] {
                    let template = format!("{}{{}}", "a".repeat(size - rendered.len()));
                    let mut log = PluginLog::new("bounded");
                    assert_eq!(log.push_fmt(&template, &[sensitive(&value)]), size == 512);
                    let json = serde_json::to_value(&log).unwrap();
                    if size == 512 {
                        assert_eq!(
                            json["log"],
                            serde_json::json!([format!("{}{rendered}", "a".repeat(size - rendered.len()))])
                        );
                        assert!(json.get("log_full").is_none());
                    } else {
                        assert_eq!(json["log"], serde_json::json!([]));
                        assert_eq!(json["log_full"], true);
                        assert!(log.log.is_some() && log.ulog.is_some());
                        assert!(log.push_fmt("later {}", &[LogArg::Plain("entry")]));
                        assert_eq!(log.log.as_ref().unwrap().entries().count(), 0);
                        assert_eq!(
                            log.ulog.as_ref().unwrap().entries().collect::<Vec<_>>(),
                            ["later entry"]
                        );
                    }
                    assert!(json.get("ulog").is_none());
                }
            }
        },
    );
}

#[test]
fn formatted_scratch_stops_before_large_appends_and_preserves_template_semantics() {
    assert_sensitive_test_in_subprocess(
        "formatted_scratch_stops_before_large_appends_and_preserves_template_semantics",
        || {
            let huge = "x".repeat(1024 * 1024);
            for mode in [0, 1, 2] {
                SENSITIVE_MODE.store(mode, Ordering::SeqCst);
                for (template, args) in [
                    (huge.as_str(), vec![]),
                    ("prefix{}", vec![LogArg::Plain(&huge)]),
                    ("prefix{}", vec![sensitive(&huge)]),
                ] {
                    let mut scratch: SmallVec<[u8; 128]> = SmallVec::new();
                    let accepted = format_into(&mut scratch, template, &args);
                    assert_eq!(
                        accepted,
                        mode == 1 && matches!(args.first(), Some(LogArg::Sensitive(_)))
                    );
                    assert!(scratch.len() <= ULOG_MAX_BUFFER);
                    assert!(!scratch.spilled(), "oversized slices must be rejected before copying");
                }
                let mut log = PluginLog::new("syntax");
                assert!(log.push_fmt("a{}b{}c", &[LogArg::Plain("é")]));
                assert!(log.push_fmt("{}", &[LogArg::Plain("first"), sensitive(&huge)]));
                assert_eq!(log.log.unwrap().entries().collect::<Vec<_>>(), ["aébc", "first"]);
            }
        },
    );
}

#[test]
fn test_push_fmt_plain_args_never_masked() {
    assert_sensitive_mode_test("test_push_fmt_plain_args_never_masked", 1, || {
        let mut log = PluginLog::new("test");
        log.push_fmt("key {} set", la!["my-key"]);
        assert!(log.contains("key my-key set"));
    });
}

#[test]
fn test_push_fmt_sensitive_arg_replaced() {
    assert_sensitive_mode_test("test_push_fmt_sensitive_arg_replaced", 1, || {
        let mut log = PluginLog::new("test");
        log.push_fmt("user {} from {}", la!["admin", sensitive("1.2.3.4")]);
        assert!(log.contains("user admin from ***"));
        assert!(!log.contains("1.2.3.4"));
    });
}

#[test]
fn test_push_fmt_sensitive_arg_wrapped() {
    assert_sensitive_mode_test("test_push_fmt_sensitive_arg_wrapped", 2, || {
        let mut log = PluginLog::new("test");
        log.push_fmt("user {} from {}", la!["admin", sensitive("1.2.3.4")]);
        assert!(log.contains("user admin from <E>1.2.3.4</E>"));
    });
}

#[test]
fn test_push_fmt_off_mode_writes_plain() {
    assert_sensitive_mode_test("test_push_fmt_off_mode_writes_plain", 0, || {
        let mut log = PluginLog::new("test");
        log.push_fmt("user {} from {}", la!["admin", sensitive("1.2.3.4")]);
        assert!(log.contains("user admin from 1.2.3.4"));
    });
}

#[test]
fn test_push_fmt_no_placeholders() {
    assert_sensitive_mode_test("test_push_fmt_no_placeholders", 1, || {
        let mut log = PluginLog::new("test");
        log.push_fmt("static message; ", la![]);
        assert!(log.contains("static message; "));
    });
}

#[test]
fn test_push_sensitive_off_writes_plain() {
    assert_sensitive_mode_test("test_push_sensitive_off_writes_plain", 0, || {
        let mut log = PluginLog::new("test");
        log.push_sensitive("hello world");
        assert!(log.contains("hello world"));
    });
}

#[test]
fn test_push_sensitive_replace_writes_stars() {
    assert_sensitive_mode_test("test_push_sensitive_replace_writes_stars", 1, || {
        let mut log = PluginLog::new("test");
        log.push_sensitive("secret-token-value");
        assert!(log.contains("***"));
        assert!(!log.contains("secret-token-value"));
    });
}

#[test]
fn test_push_sensitive_wrap_wraps_value() {
    assert_sensitive_mode_test("test_push_sensitive_wrap_wraps_value", 2, || {
        let mut log = PluginLog::new("test");
        log.push_sensitive("user@example.com");
        assert!(log.contains("<E>user@example.com</E>"));
    });
}

#[test]
fn test_push_sensitive_truncates_at_utf8_boundary() {
    assert_sensitive_mode_test("test_push_sensitive_truncates_at_utf8_boundary", 0, || {
        // 63 ASCII bytes + 2-byte 'é' (bytes 63-64) + "trail"
        let mut s = "a".repeat(63);
        s.push('é');
        s.push_str("trail");
        assert!(s.len() > 64);
        let mut log = PluginLog::new("test");
        log.push_sensitive(&s);
        assert!(!log.contains("trail"));
    });
}

#[test]
fn test_fixed_buffer() {
    let mut log = PluginLog::new("test");
    assert!(log.push("aaa; "));
    assert!(log.push("bbb; "));

    let json = serde_json::to_string(&log).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["log"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["log"][0], "aaa; ");
    assert_eq!(parsed["log"][1], "bbb; ");
    assert!(parsed.get("ulog").is_none());
    assert!(parsed.get("log_full").is_none()); // false doesn't serialize
}

#[test]
fn internal_attachment_token_is_never_serialized() {
    let mut log = PluginLog::new("ExtProc");
    log.set_internal_token("secret/request-scoped-identity".to_string());
    log.push("safe summary");
    let json = serde_json::to_string(&log).unwrap();
    assert!(!json.contains("internal_token"));
    assert!(!json.contains("request-scoped-identity"));
    assert!(json.contains("safe summary"));
}

#[test]
fn test_push_err_prefix() {
    let mut log = PluginLog::new("openid_connect");
    assert!(log.push_err("token refresh fail"));
    assert!(log.push_err("jwt expired"));

    let json = serde_json::to_string(&log).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["name"], "openid_connect");
    let entries = parsed["log"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], "err: token refresh fail");
    assert_eq!(entries[1], "err: jwt expired");
}

#[test]
fn test_mixed_push_and_push_err_order_preserved() {
    let mut log = PluginLog::new("mixed");
    assert!(log.push("OK u=alice; "));
    assert!(log.push_err("token refresh fail"));
    assert!(log.push("retry scheduled; "));

    let json = serde_json::to_string(&log).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    let entries = parsed["log"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0], "OK u=alice; ");
    assert_eq!(entries[1], "err: token refresh fail");
    assert_eq!(entries[2], "retry scheduled; ");
}

#[test]
fn test_grouped_logs_serialization() {
    let mut logs: Vec<StageLogs> = Vec::new();

    // Add request filters stage
    let mut request_filters = Vec::with_capacity(2);
    let mut log1 = PluginLog::new("cors");
    log1.time_cost = Some(10);
    log1.push("CORS check passed; ");
    request_filters.push(log1);

    let mut log2 = PluginLog::new("csrf");
    log2.time_cost = Some(5);
    request_filters.push(log2);

    logs.push(StageLogs {
        stage: "request_filters",
        filters: request_filters,
        edgion_plugins: Vec::new(),
    });

    // Add upstream response filters stage
    let mut upstream_filters = Vec::with_capacity(1);
    let mut log3 = PluginLog::new("ResponseHeaderModifier");
    log3.time_cost = Some(2);
    upstream_filters.push(log3);

    logs.push(StageLogs {
        stage: "upstream_response_filters",
        filters: upstream_filters,
        edgion_plugins: Vec::new(),
    });

    // Serialize
    let json = serde_json::to_string(&logs).unwrap();

    // Verify structure
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_array());

    let array = parsed.as_array().unwrap();
    assert_eq!(array.len(), 2);

    // Check first stage
    assert_eq!(array[0]["stage"], "request_filters");
    assert_eq!(array[0]["filters"].as_array().unwrap().len(), 2);
    assert_eq!(array[0]["filters"][0]["name"], "cors");
    assert_eq!(array[0]["filters"][0]["time_cost"], 10);

    // Check second stage
    assert_eq!(array[1]["stage"], "upstream_response_filters");
    assert_eq!(array[1]["filters"].as_array().unwrap().len(), 1);
    // edgion_plugins should not appear when empty
    assert!(array[0].get("edgion_plugins").is_none());
}

#[test]
fn test_empty_logs_serialization() {
    let logs: Vec<StageLogs> = Vec::new();
    let json = serde_json::to_string(&logs).unwrap();
    assert_eq!(json, "[]");
}

#[test]
fn test_skip_empty_stage() {
    let mut logs: Vec<StageLogs> = Vec::new();

    // Manual check: don't push empty stage
    let empty_stage = StageLogs {
        stage: "request_filters",
        filters: Vec::new(),
        edgion_plugins: Vec::new(),
    };

    if !empty_stage.filters.is_empty() {
        logs.push(empty_stage);
    }

    assert!(logs.is_empty());

    let json = serde_json::to_string(&logs).unwrap();
    assert_eq!(json, "[]");
}

#[test]
fn test_overflow_appends_to_merged_log() {
    let mut log = PluginLog::new("ovf");
    // 16-byte entries: 8 fill tier-1 (128 bytes) exactly; the 9th overflows.
    for i in 0..8 {
        assert!(log.push("0123456789abcdef"), "entry {i} fits tier-1");
    }
    assert!(log.push("overflow-entry-1"), "9th entry overflows into tier-2");

    let json = serde_json::to_string(&log).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    // Tier-1 and tier-2 serialize as ONE merged array, in push order.
    let entries = parsed["log"].as_array().unwrap();
    assert_eq!(entries.len(), 9, "8 tier-1 + 1 overflow entry, merged");
    assert_eq!(entries[8], "overflow-entry-1");
    assert!(
        parsed.get("ulog").is_none(),
        "ulog is never serialized as its own field"
    );
}

#[test]
fn test_overflow_preserves_order_for_later_small_entries() {
    let mut log = PluginLog::new("ord");
    for _ in 0..8 {
        log.push("0123456789abcdef"); // fill tier-1
    }
    log.push("big-overflow-entry"); // opens tier-2
    log.push("x"); // small, but must still land after the overflow entry

    let json = serde_json::to_string(&log).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let entries = parsed["log"].as_array().unwrap();
    assert_eq!(entries.len(), 10, "8 tier-1 + 2 overflow entries, merged");
    assert_eq!(entries[8], "big-overflow-entry");
    assert_eq!(entries[9], "x");
    assert!(parsed.get("ulog").is_none());
}

#[test]
fn test_single_oversize_entry_overflows_directly() {
    let mut log = PluginLog::new("huge");
    let big = "z".repeat(200); // larger than the 128-byte tier-1 budget
    assert!(log.push(&big), "an oversize entry overflows straight into tier-2");

    let json = serde_json::to_string(&log).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let entries = parsed["log"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].as_str().unwrap().len(), 200);
    assert!(parsed.get("ulog").is_none());
}

#[test]
fn test_ulog_full_drops_entry_and_marks_log_full() {
    let mut log = PluginLog::new("full");
    for _ in 0..8 {
        log.push("0123456789abcdef"); // fill tier-1
    }
    // Tier-2 cap is 512B. 64-byte entries: 8 fit exactly (8 * 64 = 512),
    // the 9th (576B) is rejected.
    let chunk = "a".repeat(64);
    let mut accepted = 0;
    for _ in 0..20 {
        if log.push(&chunk) {
            accepted += 1;
        }
    }
    assert_eq!(
        accepted, 8,
        "8 * 64B = 512B fills tier-2 exactly; the 9th chunk (576B) overflows"
    );
    assert_eq!(log.log_full, Some(true), "log_full is set when even tier-2 is full");
}

#[test]
fn test_push_err_overflows_with_prefix_into_merged_log() {
    let mut log = PluginLog::new("e");
    for _ in 0..8 {
        log.push("0123456789abcdef"); // fill tier-1
    }
    assert!(log.push_err("token refresh fail"));

    let json = serde_json::to_string(&log).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let entries = parsed["log"].as_array().unwrap();
    assert_eq!(entries.len(), 9);
    assert_eq!(entries[8], "err: token refresh fail");
    assert!(parsed.get("ulog").is_none());
}

#[test]
fn plugin_log_alias_field_serializes_only_when_set() {
    // alias unset -> field omitted
    let log_no_alias = PluginLog::new("KeyAuth");
    let json = serde_json::to_string(&log_no_alias).unwrap();
    assert!(!json.contains("alias"), "alias must be omitted when None: {json}");

    // alias set via with_alias -> field present and verbatim
    let log_with_alias = PluginLog::with_alias("KeyAuth", Some("auth-internal"));
    let json = serde_json::to_string(&log_with_alias).unwrap();
    assert!(
        json.contains(r#""alias":"auth-internal""#),
        "alias must serialize verbatim: {json}"
    );
    assert_eq!(log_with_alias.name, "KeyAuth");

    // with_alias(name, None) is equivalent to new(name)
    let log_none = PluginLog::with_alias("KeyAuth", None);
    let json = serde_json::to_string(&log_none).unwrap();
    assert!(
        !json.contains("alias"),
        "alias must be omitted when with_alias passed None: {json}"
    );
}

#[test]
fn test_stage_logs_with_edgion_plugins() {
    let stage = StageLogs {
        stage: "request_filters",
        filters: vec![{
            let mut log = PluginLog::new("ExtensionRef");
            log.set_refer_to("auth-plugins".to_string());
            log
        }],
        edgion_plugins: vec![EdgionPluginsLog {
            name: "auth-plugins".to_string(),
            logs: vec![{
                let mut log = PluginLog::new("BasicAuth");
                log.time_cost = Some(45);
                log.push("User authenticated");
                log
            }],
        }],
    };

    let json = serde_json::to_string(&stage).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    // Check filters
    assert_eq!(parsed["filters"][0]["name"], "ExtensionRef");
    assert_eq!(parsed["filters"][0]["refer_to"], "auth-plugins");
    // ExtensionRef should not have time_cost
    assert!(parsed["filters"][0].get("time_cost").is_none());

    // Check edgion_plugins
    assert_eq!(parsed["edgion_plugins"][0]["name"], "auth-plugins");
    assert_eq!(parsed["edgion_plugins"][0]["logs"][0]["name"], "BasicAuth");
    assert_eq!(parsed["edgion_plugins"][0]["logs"][0]["time_cost"], 45);
}

#[test]
fn condition_skip_preserves_text_and_replaces_with_one_allocation() {
    let long_cause = format!("run.anyOf[0].allOf[0]:keyMatch:header:{}", "x".repeat(256));
    let cases = [
        ("skip", "skip.allOf"),
        ("!run", "run.allOf[0]:probability"),
        ("!run", "group:é"),
        ("", ""),
        ("!run", long_cause.as_str()),
    ];
    let mut log = PluginLog::new("condition-capacity");
    let mut reduced_calls = false;
    for (action, cause) in cases {
        let (expected, original) = crate::test_allocation_counter::count_allocation_stats(|| {
            format!("{}:{cause}", std::hint::black_box(action))
        });
        let (_, actual) = crate::test_allocation_counter::count_allocation_stats(|| {
            log.set_cond_skip(std::hint::black_box(action), std::hint::black_box(cause));
        });
        assert!(log.is_cond_skipped());
        assert_eq!(log.cond_skip.as_deref(), Some(expected.as_str()));
        assert_eq!(serde_json::to_value(&log).unwrap()["cond_skip"], expected);
        assert_eq!(actual.calls, 1);
        assert_eq!(actual.requested_bytes, expected.len());
        assert!(actual.requested_bytes <= original.requested_bytes);
        reduced_calls |= actual.calls < original.calls;
    }
    assert!(reduced_calls);
}

#[test]
fn plugin_name_ownership_preserves_text_without_fixed_reservation() {
    let names = [
        String::new(),
        "Wasm".to_owned(),
        "éπ".to_owned(),
        "x".repeat(36),
        "x".repeat(37),
        "x".repeat(256),
    ];
    for name in &names {
        let (original, before) = crate::test_allocation_counter::count_allocation_stats(|| {
            let mut owned = String::with_capacity(36);
            owned.push_str(std::hint::black_box(name));
            owned
        });
        let (log, after) =
            crate::test_allocation_counter::count_allocation_stats(|| PluginLog::new(std::hint::black_box(name)));
        assert_eq!(log.name, original);
        assert_eq!(serde_json::to_value(&log).unwrap()["name"], original);
        assert_eq!(after.calls, usize::from(!name.is_empty()));
        assert_eq!(after.requested_bytes, name.len());
        assert!(after.calls <= before.calls);
        assert!(after.requested_bytes <= before.requested_bytes);
        if name.len() != 36 {
            assert!(after.requested_bytes < before.requested_bytes);
        }
    }
}
