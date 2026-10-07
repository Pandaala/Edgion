use super::*;
use edgion_waf::seclang::JsonStreamUsage;

impl JsonUsageObserver {
    fn parser_usage(&self) -> JsonStreamUsage {
        self.parser.usage()
    }
}

fn outcome(bytes: &[u8], width: usize) -> JsonUsageOutcome {
    let mut observer = JsonUsageObserver::new();
    for chunk in bytes.chunks(width) {
        observer.feed(chunk);
    }
    let result = observer.finish();
    assert_eq!(observer.finish(), result);
    result
}
fn oracle(bytes: &[u8]) -> JsonUsageOutcome {
    let root: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    let usage = root.get("usage");
    let get = |name: &str| {
        usage
            .and_then(|value| value.get(name))
            .and_then(serde_json::Value::as_u64)
    };
    match (get("prompt_tokens"), get("completion_tokens")) {
        (Some(input_tokens), Some(output_tokens)) => JsonUsageOutcome::Reported(ReportedAiUsage {
            input_tokens,
            output_tokens,
            cache_read_input_tokens: usage
                .and_then(|value| value.pointer("/prompt_tokens_details/cached_tokens"))
                .and_then(serde_json::Value::as_u64)
                .or_else(|| get("cache_read_input_tokens"))
                .unwrap_or(0),
            cache_write_input_tokens: get("cache_creation_input_tokens").unwrap_or(0),
        }),
        _ => JsonUsageOutcome::Missing,
    }
}

#[test]
fn every_byte_partition_preserves_last_wins_and_parent_replacement() {
    let cases: &[&[u8]] = &[
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":3}}"#,
        br#"{"usage":{"prompt_tokens":{},"completion_tokens":3}}"#,
        br#"{"usage":[{"prompt_tokens":2,"completion_tokens":3}]}"#,
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":3},"usage":{"prompt_tokens":4,"completion_tokens":5}}"#,
        br#"{"nested":{"usage":{"prompt_tokens":9,"completion_tokens":9}},"usage":{"prompt_tokens":2,"prompt_tokens":4,"completion_tokens":3}}"#,
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":3},"usage":{"completion_tokens":6}}"#,
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":3},"usage":null}"#,
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":1},"prompt_tokens_details":{},"cache_read_input_tokens":2}}"#,
        br#"{"us\u0061ge":{"prompt_tokens":0,"completion_tokens":0}}"#,
        br#"{"usage":{"prompt_tokens":18446744073709551615,"completion_tokens":0}}"#,
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":3,"completion_tokens":1e0}}"#,
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":1.0000000000000001}}"#,
        br#"{"usage":{"prompt_tokens":9007199254740991.1,"completion_tokens":3}}"#,
        br#"{"usage":{"prompt_tokens":18446744073709551616,"completion_tokens":3}}"#,
        br#"{"usage":{"prompt_tokens":-1,"completion_tokens":3}}"#,
        br#"{"usage":{"prompt_tokens":-0,"completion_tokens":3}}"#,
        br#"{"usage":{"prompt_tokens":"2","completion_tokens":3}}"#,
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":1,"cached_tokens":"invalid"},"cache_read_input_tokens":2}}"#,
    ];
    for &body in cases {
        let expected = oracle(body);
        for width in 1..=body.len() {
            assert_eq!(outcome(body, width), expected, "partition width {width}");
        }
        let mut observer = JsonUsageObserver::new();
        let mut offset = 0;
        for width in [1, 7, 2, 11].into_iter().cycle() {
            if offset == body.len() {
                break;
            }
            let end = (offset + width).min(body.len());
            observer.feed(&body[offset..end]);
            offset = end;
        }
        assert_eq!(observer.finish(), expected, "mixed partitions");
    }
}

#[test]
fn malformed_tail_and_partial_eos_never_publish_valid_prefix() {
    for bytes in [
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":3}}garbage"#.as_slice(),
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":3},"ignored":"unfinished"#.as_slice(),
        br#"{"ignored":"\uD800x","usage":{"prompt_tokens":2,"completion_tokens":3}}"#.as_slice(),
        br#"{"ignored":"\uDC00"}"#.as_slice(),
        b"{\"ignored\":\"\xff\"}".as_slice(),
    ] {
        for width in 1..=bytes.len() {
            assert!(matches!(outcome(bytes, width), JsonUsageOutcome::Unavailable(_)));
        }
    }
}

#[test]
fn large_ignored_values_do_not_allocate_content_or_delay_usage_selection() {
    let payload = [b'x'; 4096];
    for before in [true, false] {
        let (_, allocation) = crate::test_allocation_counter::count_allocation_stats(|| {
            let mut observer = Box::new(JsonUsageObserver::new());
            observer.feed(if before {
                br#"{"usage":{"prompt_tokens":2,"completion_tokens":3},"ignored":""#
            } else {
                br#"{"ignored":""#
            });
            for _ in 0..2048 {
                observer.feed(&payload);
            }
            observer.feed(if before {
                br#""}"#
            } else {
                br#"","usage":{"prompt_tokens":2,"completion_tokens":3}}"#
            });
            assert!(observer.parser_usage().peak_retained_bytes <= 8192);
            assert_eq!(
                observer.finish(),
                JsonUsageOutcome::Reported(ReportedAiUsage {
                    input_tokens: 2,
                    output_tokens: 3,
                    cache_read_input_tokens: 0,
                    cache_write_input_tokens: 0
                })
            );
        });
        let fixed = std::mem::size_of::<JsonUsageObserver>();
        assert!(
            allocation.requested_bytes < fixed + 8192,
            "scoped allocator requests: {allocation:?}"
        );
        eprintln!("usage observer: placement={before}, fixed_box_bytes={fixed}, allocation_calls={}, cumulative_requested_bytes={}; allocator metadata and hidden slack are unmeasured", allocation.calls, allocation.requested_bytes);
    }
}

#[test]
fn capture_limits_apply_to_contiguous_and_split_tokens() {
    let bodies = [
        format!("{{\"{}\":0}}", "k".repeat(257)),
        format!("{{\"ignored\":1.{}}}", "0".repeat(63)),
        format!("{}0{}", "[".repeat(66), "]".repeat(66)),
    ];
    for body in bodies {
        let expected = outcome(body.as_bytes(), body.len());
        assert!(matches!(expected, JsonUsageOutcome::Unavailable(_)));
        assert_eq!(outcome(body.as_bytes(), 1), expected);
        assert_eq!(outcome(body.as_bytes(), 7), expected);
    }
}

#[test]
fn responses_json_selects_exact_terminal_status_and_preserves_large_output_discard() {
    for status in ["completed", "incomplete", "failed", "cancelled"] {
        let body = format!(
            r#"{{"output":[{{"content":[{{"text":"{}"}}]}}],"status":"{status}","usage":{{"input_tokens":13,"output_tokens":7,"input_tokens_details":{{"cached_tokens":3}},"output_tokens_details":{{"reasoning_tokens":5}}}}}}"#,
            "x".repeat(131_072)
        );
        for width in [1, 17, body.len()] {
            let mut observer = JsonUsageObserver::responses(false);
            for chunk in body.as_bytes().chunks(width) {
                observer.feed(chunk);
            }
            assert_eq!(
                observer.finish(),
                JsonUsageOutcome::Reported(ReportedAiUsage {
                    input_tokens: 13,
                    output_tokens: 7,
                    cache_read_input_tokens: 3,
                    cache_write_input_tokens: 0,
                })
            );
            assert_eq!(observer.response_outcome(), ResponseOutcome::from_status(status));
            assert!(observer.parser_usage().peak_retained_bytes < 8192);
        }
    }
}

#[test]
fn responses_usage_parent_replacement_and_status_never_guess_strings_by_length() {
    let cases: &[(&[u8], ResponseOutcome, bool)] = &[
        (br#"{"status":"completed","status":"cancelled","usage":{"input_tokens":4,"output_tokens":5}}"#, ResponseOutcome::Cancelled, true),
        (br#"{"status":"failed","status":"queued","usage":null}"#, ResponseOutcome::Unknown, false),
        (br#"{"status":"completed","usage":{"input_tokens":4,"output_tokens":5},"usage":{}}"#, ResponseOutcome::Completed, false),
        (br#"{"status":"completed","usage":{"input_tokens":4,"output_tokens":5,"input_tokens_details":{"cached_tokens":9}}}"#, ResponseOutcome::Completed, false),
        (br#"{"status":"completed","usage":{"input_tokens":18446744073709551615,"output_tokens":1}}"#, ResponseOutcome::Completed, false),
    ];
    for (body, status, reported) in cases {
        for width in 1..=body.len() {
            let mut observer = JsonUsageObserver::responses(false);
            for chunk in body.chunks(width) {
                observer.feed(chunk);
            }
            assert_eq!(matches!(observer.finish(), JsonUsageOutcome::Reported(_)), *reported);
            assert_eq!(observer.response_outcome(), *status);
        }
    }
}
