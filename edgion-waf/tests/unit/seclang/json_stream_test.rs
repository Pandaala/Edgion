use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
enum OwnedToken {
    ObjectStart,
    ObjectEnd,
    ArrayStart,
    ArrayEnd,
    Key(String),
    String(String),
    Number(String),
    Boolean(bool),
    Null,
}

impl From<JsonStreamToken<'_>> for OwnedToken {
    fn from(token: JsonStreamToken<'_>) -> Self {
        match token {
            JsonStreamToken::ObjectStart => Self::ObjectStart,
            JsonStreamToken::ObjectEnd => Self::ObjectEnd,
            JsonStreamToken::ArrayStart => Self::ArrayStart,
            JsonStreamToken::ArrayEnd => Self::ArrayEnd,
            JsonStreamToken::Key(value) => Self::Key(value.to_owned()),
            JsonStreamToken::String(value) => Self::String(value.to_owned()),
            JsonStreamToken::Number { raw, .. } => Self::Number(raw.to_owned()),
            JsonStreamToken::Boolean(value) => Self::Boolean(value),
            JsonStreamToken::Null => Self::Null,
        }
    }
}

fn tokenize(
    chunks: &[&[u8]],
    limits: JsonStreamLimits,
) -> (Result<(), JsonStreamError>, Vec<OwnedToken>, JsonStreamUsage) {
    let mut stream = JsonStream::new(limits);
    let mut tokens = Vec::new();
    for chunk in chunks {
        let result = stream.feed(JsonStreamInput::Data(chunk), |token| {
            tokens.push(token.into());
            Ok(())
        });
        if result.is_err() {
            return (result, tokens, stream.usage());
        }
    }
    let result = stream.feed(JsonStreamInput::Complete, |token| {
        tokens.push(token.into());
        Ok(())
    });
    (result, tokens, stream.usage())
}

// Borrowing removes retained copies only for contiguous unescaped tokens.
// Logical input/events stay chunk-independent; physical work/capacity do not.
fn assert_same_semantics(
    actual: &(Result<(), JsonStreamError>, Vec<OwnedToken>, JsonStreamUsage),
    expected: &(Result<(), JsonStreamError>, Vec<OwnedToken>, JsonStreamUsage),
) {
    assert_eq!(actual.0, expected.0);
    assert_eq!(actual.1, expected.1);
    assert_eq!(actual.2.source_bytes, expected.2.source_bytes);
    assert_eq!(actual.2.decoded_bytes, expected.2.decoded_bytes);
    assert_eq!(actual.2.events, expected.2.events);
    assert_eq!(actual.2.retained_bytes, 0);
}

#[test]
fn tokens_preserve_source_order_duplicates_and_decoded_spelling() {
    let source = br#"{"z":[],"key":"old","key":"new","escaped":"\u20ac\ud834\udd1e\b\f\n\r\t\/\\\"","n":-1.25e+2,"b":false,"nil":null}"#;
    let (result, tokens, usage) = tokenize(&[source], JsonStreamLimits::default());
    result.unwrap();
    assert_eq!(
        tokens,
        [
            OwnedToken::ObjectStart,
            OwnedToken::Key("z".into()),
            OwnedToken::ArrayStart,
            OwnedToken::ArrayEnd,
            OwnedToken::Key("key".into()),
            OwnedToken::String("old".into()),
            OwnedToken::Key("key".into()),
            OwnedToken::String("new".into()),
            OwnedToken::Key("escaped".into()),
            OwnedToken::String("€𝄞\u{8}\u{c}\n\r\t/\\\"".into()),
            OwnedToken::Key("n".into()),
            OwnedToken::Number("-1.25e+2".into()),
            OwnedToken::Key("b".into()),
            OwnedToken::Boolean(false),
            OwnedToken::Key("nil".into()),
            OwnedToken::Null,
            OwnedToken::ObjectEnd,
        ]
    );
    assert_eq!(usage.events, tokens.len());
    assert_eq!(usage.source_bytes, source.len());
    assert_eq!(usage.retained_bytes, 0);
}

fn fixtures() -> Vec<Vec<u8>> {
    let mut inputs: Vec<Vec<u8>> = [
        "null",
        "true",
        "false",
        "0",
        "-0",
        "1",
        "-1.125e+12",
        "1e-9999",
        "0e9999",
        "18446744073709551616",
        "-9223372036854775809",
        "1.7976931348623157e308",
        "1e309",
        "[]",
        "{}",
        "[0,true,false,null,{},[]]",
        "{\"a\":1,\"a\":[2]}",
        "\"éñ€©π𝄞\"",
        " \r\n\t[ \"\", -42 ] \n",
        "{\"\":\"\"}",
        r#""\u0000\u007f\u00e9\u03c0\u20ac""#,
        r#""\uD834\uDD1E""#,
        r#""\"\\\/\b\f\n\r\t""#,
        "",
        " ",
        "nul",
        "tru",
        "falsee",
        "true false",
        "true[]",
        "01",
        "-01",
        "+1",
        ".1",
        "1.",
        "1e",
        "1e+",
        "--1",
        "1a",
        "NaN",
        "Infinity",
        "[1,]",
        "{\"a\":0,}",
        "[",
        "{",
        "}",
        "]",
        "[}",
        "{]",
        "[,1]",
        "[1 2]",
        "{a:1}",
        "{\"a\" 1}",
        "{\"a\":}",
        "{\"a\":1 \"b\":2}",
        "{\"a\"}",
        "\"",
        "\"abc",
        "\"a\n\"",
        r#""\x""#,
        r#""\u123""#,
        r#""\u12xz""#,
        r#""\ud800""#,
        r#""\udc00""#,
        r#""\ud800\u0041""#,
        r#""\ud800a""#,
        r#""\ud800\x1234""#,
    ]
    .iter()
    .map(|source| source.as_bytes().to_vec())
    .collect();
    inputs.extend([
        vec![b'"', 0xc0, 0xaf, b'"'],
        vec![b'"', 0xe0, 0x80, 0x80, b'"'],
        vec![b'"', 0xed, 0xa0, 0x80, b'"'],
        vec![b'"', 0xf4, 0x90, 0x80, 0x80, b'"'],
        vec![b'"', 0xf5, 0x80, 0x80, 0x80, b'"'],
        vec![b'"', 0xc2, b'"'],
        vec![b'"', 0x80, b'"'],
        vec![b'"', 0xc2],
        vec![0xef, 0xbb, 0xbf, b'0'],
        vec![b'0', 0x0b],
    ]);
    inputs
}

#[test]
fn every_byte_split_matches_serde_json_acceptance_and_whole_input_events() {
    for source in fixtures() {
        let expected = serde_json::from_slice::<serde_json::Value>(&source).is_ok();
        let baseline = tokenize(&[&source], JsonStreamLimits::default());
        assert_eq!(baseline.0.is_ok(), expected, "input {source:?}: {:?}", baseline.0);
        for split in 0..=source.len() {
            let actual = tokenize(&[&source[..split], &[], &source[split..]], JsonStreamLimits::default());
            assert_same_semantics(&actual, &baseline);
        }
        let bytes: Vec<&[u8]> = source.chunks(1).collect();
        assert_same_semantics(&tokenize(&bytes, JsonStreamLimits::default()), &baseline);
    }
}

fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn random_value(state: &mut u64, depth: usize) -> serde_json::Value {
    let choice = random(state) % if depth < 3 { 6 } else { 4 };
    match choice {
        0 => serde_json::Value::Null,
        1 => serde_json::Value::Bool(random(state) & 1 == 0),
        2 => serde_json::Value::Number((random(state) as i64).into()),
        3 => serde_json::Value::String(format!("key-{}-é-€-\n-\"", random(state) % 17)),
        4 => {
            let length = random(state) % 5;
            serde_json::Value::Array((0..length).map(|_| random_value(state, depth + 1)).collect())
        }
        _ => {
            let length = random(state) % 5;
            serde_json::Value::Object(
                (0..length)
                    .map(|index| (format!("field.{index}"), random_value(state, depth + 1)))
                    .collect(),
            )
        }
    }
}

fn value_tokens(value: &serde_json::Value, output: &mut Vec<OwnedToken>) {
    match value {
        serde_json::Value::Null => output.push(OwnedToken::Null),
        serde_json::Value::Bool(value) => output.push(OwnedToken::Boolean(*value)),
        serde_json::Value::Number(value) => output.push(OwnedToken::Number(value.to_string())),
        serde_json::Value::String(value) => output.push(OwnedToken::String(value.clone())),
        serde_json::Value::Array(values) => {
            output.push(OwnedToken::ArrayStart);
            for value in values {
                value_tokens(value, output);
            }
            output.push(OwnedToken::ArrayEnd);
        }
        serde_json::Value::Object(values) => {
            output.push(OwnedToken::ObjectStart);
            for (key, value) in values {
                output.push(OwnedToken::Key(key.clone()));
                value_tokens(value, output);
            }
            output.push(OwnedToken::ObjectEnd);
        }
    }
}

#[test]
fn deterministic_random_documents_mutations_and_partitions_match_reference() {
    let mut state = 0x6ca4_836e_d817_2395;
    for _ in 0..400 {
        let value = random_value(&mut state, 0);
        let source = serde_json::to_vec(&value).unwrap();
        let mut expected_tokens = Vec::new();
        value_tokens(&value, &mut expected_tokens);
        let baseline = tokenize(&[&source], JsonStreamLimits::default());
        baseline.0.unwrap();
        assert_eq!(baseline.1, expected_tokens);
        let mut mutated = source.clone();
        let position = random(&mut state) as usize % mutated.len();
        mutated[position] = (random(&mut state) & 0x7f) as u8;
        for input in [&source, &mutated] {
            let mut chunks = Vec::new();
            let mut start = 0;
            while start < input.len() {
                let end = (start + 1 + random(&mut state) as usize % 11).min(input.len());
                chunks.push(&input[start..end]);
                start = end;
            }
            let whole = tokenize(&[input], JsonStreamLimits::default());
            assert_eq!(
                whole.0.is_ok(),
                serde_json::from_slice::<serde_json::Value>(input).is_ok(),
                "{input:?}"
            );
            assert_same_semantics(&tokenize(&chunks, JsonStreamLimits::default()), &whole);
        }
    }
}

fn assert_boundary(source: &[u8], limit: usize, kind: JsonStreamLimit, set: impl Fn(&mut JsonStreamLimits, usize)) {
    let mut limits = JsonStreamLimits::default();
    set(&mut limits, limit);
    tokenize(&[source], limits).0.unwrap();
    set(&mut limits, limit - 1);
    let failed = tokenize(&[source], limits);
    assert_eq!(failed.0, Err(JsonStreamError::Limit { kind, limit: limit - 1 }));
    assert_eq!(failed.2.retained_bytes, 0);
    for split in 0..=source.len() {
        let chunks = [&source[..split], &source[split..]];
        if kind == JsonStreamLimit::Work {
            let partition_work = tokenize(&chunks, JsonStreamLimits::default()).2.work;
            set(&mut limits, partition_work);
            tokenize(&chunks, limits).0.unwrap();
            set(&mut limits, partition_work - 1);
            assert_eq!(tokenize(&chunks, limits).0, Err(limit_error(kind, partition_work - 1)));
        } else {
            assert_same_semantics(&tokenize(&chunks, limits), &failed);
        }
    }
}

#[test]
fn each_independent_limit_accepts_exact_boundary_and_rejects_first_excess() {
    assert_boundary(b"null", 4, JsonStreamLimit::SourceBytes, |limits, value| {
        limits.max_source_bytes = value
    });
    assert_boundary(br#""\u20ac""#, 8, JsonStreamLimit::RawTokenBytes, |limits, value| {
        limits.max_raw_token_bytes = value
    });
    assert_boundary(br#"["a","bc"]"#, 3, JsonStreamLimit::DecodedBytes, |limits, value| {
        limits.max_decoded_bytes = value
    });
    assert_boundary(br#"{"a":[true,null]}"#, 7, JsonStreamLimit::Events, |limits, value| {
        limits.max_events = value
    });
    assert_boundary(br#"{"a":[0]}"#, 2, JsonStreamLimit::Depth, |limits, value| {
        limits.max_depth = value
    });
    assert_boundary(
        b"[123456789]",
        size_of::<Frame>(),
        JsonStreamLimit::RetainedBytes,
        |limits, value| limits.max_retained_bytes = value,
    );
    let source = br#"{"key":["\u20ac",12e-3,true]}"#;
    let work = tokenize(&[source], JsonStreamLimits::default()).2.work;
    assert_boundary(source, work, JsonStreamLimit::Work, |limits, value| {
        limits.max_work = value
    });
}

#[test]
fn zero_depth_allows_root_containers_but_not_nested_values() {
    let limits = JsonStreamLimits {
        max_depth: 0,
        ..JsonStreamLimits::default()
    };
    for source in [b"{}".as_slice(), b"[]", b"null", b"0", b"\"a\""] {
        tokenize(&[source], limits).0.unwrap();
    }
    for source in [b"[0]".as_slice(), b"[[]]", b"{\"a\":{}}"] {
        assert_eq!(
            tokenize(&[source], limits).0,
            Err(limit_error(JsonStreamLimit::Depth, 0))
        );
    }
}

#[test]
fn duplicate_keys_are_charged_even_though_future_publication_may_replace_them() {
    let source = br#"{"key":"old","key":"new"}"#;
    let usage = tokenize(&[source], JsonStreamLimits::default()).2;
    assert_eq!(usage.decoded_bytes, 12);
    assert_eq!(usage.events, 6);
    assert_boundary(source, 12, JsonStreamLimit::DecodedBytes, |limits, value| {
        limits.max_decoded_bytes = value
    });
}

#[test]
fn syntax_escape_utf8_number_and_incomplete_errors_are_typed_and_sticky() {
    for (source, expected) in [
        (b"[1,]".as_slice(), JsonStreamError::Syntax),
        (br#""\x""#.as_slice(), JsonStreamError::Escape),
        (&[b'"', 0xff, b'"'], JsonStreamError::Utf8),
        (b"1e400", JsonStreamError::Number),
        (b"[", JsonStreamError::Incomplete),
    ] {
        let mut stream = JsonStream::new(JsonStreamLimits::default());
        let result = stream
            .feed(JsonStreamInput::Data(source), |_| Ok(()))
            .and_then(|()| stream.feed(JsonStreamInput::Complete, |_| Ok(())));
        assert_eq!(result, Err(expected));
        assert_eq!(stream.usage().retained_bytes, 0);
        assert_eq!(heap_capacity(&stream.token), 0);
        assert_eq!(stream.stack.capacity(), 0);
        for input in [
            JsonStreamInput::Data(b"null"),
            JsonStreamInput::Complete,
            JsonStreamInput::Abandoned,
        ] {
            assert_eq!(
                stream.feed(input, |_| panic!("terminal stream must not emit")),
                Err(expected)
            );
        }
    }
}

#[test]
fn callback_failure_is_terminal_and_releases_all_state() {
    let mut stream = JsonStream::new(JsonStreamLimits::default());
    let mut events = 0;
    assert_eq!(
        stream.feed(JsonStreamInput::Data(br#"["private",42]"#), |_| {
            events += 1;
            if events == 2 {
                Err(JsonStreamSinkError)
            } else {
                Ok(())
            }
        }),
        Err(JsonStreamError::SinkRejected)
    );
    assert_eq!(events, 2);
    assert_eq!(stream.usage().retained_bytes, 0);
    assert_eq!(
        stream.feed(JsonStreamInput::Complete, |_| panic!("terminal stream must not emit")),
        Err(JsonStreamError::SinkRejected)
    );
}

#[test]
fn abandonment_and_completion_are_explicit_terminal_boundaries() {
    let mut abandoned = JsonStream::new(JsonStreamLimits::default());
    abandoned
        .feed(JsonStreamInput::Data(br#"["unfinished\u2"#), |_| Ok(()))
        .unwrap();
    assert!(abandoned.usage().retained_bytes > 0);
    assert_eq!(
        abandoned.feed(JsonStreamInput::Abandoned, |_| panic!("abandon must not emit")),
        Err(JsonStreamError::Abandoned)
    );
    assert_eq!(abandoned.usage().retained_bytes, 0);
    assert_eq!(
        abandoned.feed(JsonStreamInput::Data(b"null"), |_| panic!("abandon must be sticky")),
        Err(JsonStreamError::Abandoned)
    );

    let mut complete = JsonStream::new(JsonStreamLimits::default());
    complete
        .feed(JsonStreamInput::Data(b"123"), |_| {
            panic!("number awaits delimiter or EOS")
        })
        .unwrap();
    let mut events = Vec::new();
    complete
        .feed(JsonStreamInput::Complete, |token| {
            events.push(OwnedToken::from(token));
            Ok(())
        })
        .unwrap();
    assert_eq!(events, [OwnedToken::Number("123".into())]);
    assert_eq!(complete.usage().retained_bytes, 0);
    for input in [
        JsonStreamInput::Data(b""),
        JsonStreamInput::Data(b" "),
        JsonStreamInput::Complete,
        JsonStreamInput::Abandoned,
    ] {
        assert_eq!(
            complete.feed(input, |_| panic!("completed stream must not emit")),
            Err(JsonStreamError::AlreadyComplete)
        );
    }
}

#[test]
fn debug_and_errors_do_not_expose_payloads() {
    let secret = "private-sensitive-json-value";
    let mut stream = JsonStream::new(JsonStreamLimits::default());
    let input = format!("[\"{secret}");
    stream
        .feed(JsonStreamInput::Data(input.as_bytes()), |_| Ok(()))
        .unwrap();
    for rendered in [
        format!("{stream:?}"),
        format!("{:?}", JsonStreamInput::Data(input.as_bytes())),
        format!("{:?}", JsonStreamToken::Key(secret)),
        format!("{:?}", JsonStreamToken::String(secret)),
        format!(
            "{:?}",
            JsonStreamToken::Number {
                raw: secret,
                value: JsonNumber::from_validated_lexeme("0").unwrap()
            }
        ),
    ] {
        assert!(!rendered.contains(secret));
    }
    let error = stream.feed(JsonStreamInput::Complete, |_| Ok(())).unwrap_err();
    assert!(!format!("{error:?} {error}").contains(secret));
}

#[test]
fn retained_memory_stays_constant_across_long_documents_and_counts_stack_capacity() {
    let limits = JsonStreamLimits {
        max_retained_bytes: 32,
        ..JsonStreamLimits::default()
    };
    let mut stream = JsonStream::new(limits);
    stream
        .feed(JsonStreamInput::Data(b"[\"abcdefgh\""), |_| Ok(()))
        .unwrap();
    let expected = stream.usage().retained_bytes;
    assert_eq!(
        expected,
        heap_capacity(&stream.token) + stream.stack.capacity() * size_of::<Frame>()
    );
    for _ in 0..4096 {
        stream
            .feed(JsonStreamInput::Data(b",\"abcdefgh\""), |_| Ok(()))
            .unwrap();
        assert_eq!(stream.usage().retained_bytes, expected);
    }
    assert!(stream.usage().source_bytes > 40000);
    assert!(stream.usage().peak_retained_bytes <= 32);
    stream.feed(JsonStreamInput::Data(b"]"), |_| Ok(())).unwrap();
    stream.feed(JsonStreamInput::Complete, |_| Ok(())).unwrap();
    assert_eq!(stream.usage().retained_bytes, 0);
    assert_eq!(stream.usage().peak_retained_bytes, expected);
}

#[test]
fn empty_http_body_exception_is_not_part_of_lexical_json() {
    for source in [b"".as_slice(), b" \r\n\t"] {
        assert_eq!(
            tokenize(&[source], JsonStreamLimits::default()).0,
            Err(JsonStreamError::Incomplete)
        );
    }
}

#[test]
fn long_fraction_and_exponent_tokens_match_reference_across_partitions_and_raw_limits() {
    let sources = [
        format!("0.{}1", "0".repeat(1024)),
        format!("1.{}5", "0".repeat(4096)),
        format!("1e-{}3", "0".repeat(4096)),
        format!("1e+{}3", "0".repeat(1024)),
        format!("1e{}", "9".repeat(4096)),
    ];
    for source in sources {
        let bytes = source.as_bytes();
        let expected = serde_json::from_slice::<serde_json::Value>(bytes).is_ok();
        let limits = JsonStreamLimits {
            max_raw_token_bytes: bytes.len(),
            ..JsonStreamLimits::default()
        };
        let baseline = tokenize(&[bytes], limits);
        assert_eq!(baseline.0.is_ok(), expected);
        for split in [0, 1, 2, bytes.len() / 2, bytes.len() - 1, bytes.len()] {
            assert_eq!(tokenize(&[&bytes[..split], &bytes[split..]], limits), baseline);
        }
        for width in [1, 3, 257, 1024] {
            let chunks: Vec<&[u8]> = bytes.chunks(width).collect();
            assert_eq!(tokenize(&chunks, limits), baseline);
        }
        let shorter_limit = JsonStreamLimits {
            max_raw_token_bytes: bytes.len() - 1,
            ..limits
        };
        let failure = tokenize(&[bytes], shorter_limit);
        assert_eq!(
            failure.0,
            Err(limit_error(JsonStreamLimit::RawTokenBytes, bytes.len() - 1))
        );
        assert_eq!(failure.2.retained_bytes, 0);
        let chunks: Vec<&[u8]> = bytes.chunks(17).collect();
        assert_eq!(tokenize(&chunks, shorter_limit), failure);
        if expected {
            // Extend an otherwise accepted spelling by exactly one valid digit.
            let extended = format!("{source}0");
            assert!(serde_json::from_str::<serde_json::Value>(&extended).is_ok());
            assert_eq!(
                tokenize(&[extended.as_bytes()], limits).0,
                Err(limit_error(JsonStreamLimit::RawTokenBytes, bytes.len()))
            );
        }
    }
}

#[test]
fn nested_stack_has_its_own_exact_retained_capacity_boundary() {
    let retained = 8 * size_of::<Frame>();
    let limits = JsonStreamLimits {
        max_retained_bytes: retained,
        ..JsonStreamLimits::default()
    };
    let mut stream = JsonStream::new(limits);
    stream.feed(JsonStreamInput::Data(b"[[[[[[[["), |_| Ok(())).unwrap();
    assert_eq!(stream.stack.len(), 8);
    assert_eq!(heap_capacity(&stream.token), 0);
    assert_eq!(stream.usage().retained_bytes, retained);
    assert_eq!(stream.stack.capacity() * size_of::<Frame>(), retained);
    stream.feed(JsonStreamInput::Data(b"]]]]]]]]"), |_| Ok(())).unwrap();
    stream.feed(JsonStreamInput::Complete, |_| Ok(())).unwrap();
    assert_eq!(stream.usage().retained_bytes, 0);
    assert_eq!(stream.usage().peak_retained_bytes, retained);
    assert_boundary(
        b"[[[[[[[[]]]]]]]]",
        retained,
        JsonStreamLimit::RetainedBytes,
        |limits, value| limits.max_retained_bytes = value,
    );
}

#[test]
fn zero_limits_reject_only_the_resources_the_document_needs() {
    for (limits, source, kind) in [
        (
            JsonStreamLimits {
                max_source_bytes: 0,
                ..JsonStreamLimits::default()
            },
            b"null".as_slice(),
            JsonStreamLimit::SourceBytes,
        ),
        (
            JsonStreamLimits {
                max_raw_token_bytes: 0,
                ..JsonStreamLimits::default()
            },
            b"0",
            JsonStreamLimit::RawTokenBytes,
        ),
        (
            JsonStreamLimits {
                max_decoded_bytes: 0,
                ..JsonStreamLimits::default()
            },
            b"null",
            JsonStreamLimit::DecodedBytes,
        ),
        (
            JsonStreamLimits {
                max_events: 0,
                ..JsonStreamLimits::default()
            },
            b"[]",
            JsonStreamLimit::Events,
        ),
        (
            JsonStreamLimits {
                max_retained_bytes: 0,
                ..JsonStreamLimits::default()
            },
            b"[]",
            JsonStreamLimit::RetainedBytes,
        ),
        (
            JsonStreamLimits {
                max_work: 0,
                ..JsonStreamLimits::default()
            },
            b" ",
            JsonStreamLimit::Work,
        ),
    ] {
        let failure = tokenize(&[source], limits);
        assert_eq!(failure.0, Err(limit_error(kind, 0)));
        assert_eq!(failure.2.retained_bytes, 0);
    }
    let no_allocations = JsonStreamLimits {
        max_retained_bytes: 0,
        ..JsonStreamLimits::default()
    };
    for source in [b"null".as_slice(), b"true", b"false", b"\"\""] {
        let result = tokenize(&[source], no_allocations);
        result.0.unwrap();
        assert_eq!(result.2.peak_retained_bytes, 0);
    }
    let no_decoded = JsonStreamLimits {
        max_decoded_bytes: 0,
        ..JsonStreamLimits::default()
    };
    for source in [b"[]".as_slice(), b"{}", b"\"\""] {
        let result = tokenize(&[source], no_decoded);
        result.0.unwrap();
        assert_eq!(result.2.decoded_bytes, 0);
    }
}

#[test]
fn typed_numbers_preserve_raw_spelling_and_value_at_every_split() {
    for raw in [
        "-0",
        "18446744073709551615",
        "-9223372036854775808",
        "9007199254740993.0",
        "2.4703282292062328e-324",
    ] {
        let expected = JsonNumber::from_validated_lexeme(raw).unwrap();
        for split in 0..=raw.len() {
            let mut stream = JsonStream::new(JsonStreamLimits::default());
            let mut count = 0;
            for input in [
                JsonStreamInput::Data(&raw.as_bytes()[..split]),
                JsonStreamInput::Data(&raw.as_bytes()[split..]),
                JsonStreamInput::Complete,
            ] {
                stream
                    .feed(input, |token| {
                        let JsonStreamToken::Number { raw: spelling, value } = token else {
                            panic!("expected numeric token")
                        };
                        assert_eq!(spelling, raw);
                        assert_eq!(value, expected);
                        count += 1;
                        Ok(())
                    })
                    .unwrap();
            }
            assert_eq!(count, 1);
        }
    }
}

#[test]
fn contiguous_unescaped_strings_and_keys_borrow_the_original_chunk() {
    let source = "{\"key\":\"value-é\",\"\":\"\"}".as_bytes();
    let mut stream = JsonStream::new(JsonStreamLimits::default());
    let mut strings = 0;
    stream
        .feed(JsonStreamInput::Data(source), |token| {
            if let JsonStreamToken::Key(value) | JsonStreamToken::String(value) = token {
                let start = value.as_ptr() as usize;
                let source_start = source.as_ptr() as usize;
                assert!(start >= source_start && start + value.len() <= source_start + source.len());
                strings += 1;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(strings, 4);
    assert_eq!(heap_capacity(&stream.token), 0);
    stream.feed(JsonStreamInput::Complete, |_| Ok(())).unwrap();
    let mut scalar = JsonStream::new(JsonStreamLimits {
        max_retained_bytes: 0,
        ..JsonStreamLimits::default()
    });
    scalar
        .feed(JsonStreamInput::Data(br#""ordinary value""#), |_| Ok(()))
        .unwrap();
    scalar.feed(JsonStreamInput::Complete, |_| Ok(())).unwrap();
    assert_eq!(scalar.usage().peak_retained_bytes, 0);
}

#[test]
fn borrowed_prefix_falls_back_once_for_escape_and_partial_utf8() {
    for source in [br#""prefix\u20ac-suffix""#.as_slice(), "\"prefix-é-suffix\"".as_bytes()] {
        let baseline = tokenize(&[source], JsonStreamLimits::default());
        baseline.0.unwrap();
        for split in 0..=source.len() {
            let actual = tokenize(&[&source[..split], &source[split..]], JsonStreamLimits::default());
            assert_same_semantics(&actual, &baseline);
            assert!(actual.2.peak_retained_bytes <= JsonStreamLimits::default().max_retained_bytes);
        }
    }
    let mut stream = JsonStream::new(JsonStreamLimits::default());
    stream.feed(JsonStreamInput::Data(br#""prefix"#), |_| Ok(())).unwrap();
    assert_eq!(stream.token.as_slice(), b"prefix");
    assert_eq!(stream.usage().decoded_bytes, 6);
    stream
        .feed(JsonStreamInput::Data(br#"-suffix""#), |token| {
            assert!(matches!(token, JsonStreamToken::String("prefix-suffix")));
            Ok(())
        })
        .unwrap();
    assert_eq!(stream.usage().decoded_bytes, 13);
}

#[test]
fn borrowed_string_admission_errors_precede_later_invalid_bytes() {
    for (limit, expected) in [
        (
            JsonStreamLimits {
                max_source_bytes: 2,
                ..JsonStreamLimits::default()
            },
            limit_error(JsonStreamLimit::SourceBytes, 2),
        ),
        (
            JsonStreamLimits {
                max_raw_token_bytes: 2,
                ..JsonStreamLimits::default()
            },
            limit_error(JsonStreamLimit::RawTokenBytes, 2),
        ),
        (
            JsonStreamLimits {
                max_decoded_bytes: 0,
                ..JsonStreamLimits::default()
            },
            limit_error(JsonStreamLimit::DecodedBytes, 0),
        ),
        (
            JsonStreamLimits {
                max_work: 1,
                ..JsonStreamLimits::default()
            },
            limit_error(JsonStreamLimit::Work, 1),
        ),
    ] {
        let mut stream = JsonStream::new(limit);
        assert_eq!(
            stream.feed(JsonStreamInput::Data(&[b'"', b'a', 0xff, b'"']), |_| panic!(
                "invalid token emitted"
            )),
            Err(expected)
        );
        assert_eq!(
            stream.feed(JsonStreamInput::Complete, |_| panic!("sticky failure emitted")),
            Err(expected)
        );
        assert_eq!(stream.usage().retained_bytes, 0);
    }
    for source in [&[b'"', 0xff, b'"'][..], &[b'"', 0, b'"'][..]] {
        assert!(tokenize(&[source], JsonStreamLimits::default()).0.is_err());
    }
    for limit in 0..=2 {
        let outcome = tokenize(
            &[br#""a""#],
            JsonStreamLimits {
                max_raw_token_bytes: limit,
                ..JsonStreamLimits::default()
            },
        );
        assert_eq!(outcome.0, Err(limit_error(JsonStreamLimit::RawTokenBytes, limit)));
    }
    tokenize(
        &[br#""a""#],
        JsonStreamLimits {
            max_raw_token_bytes: 3,
            ..JsonStreamLimits::default()
        },
    )
    .0
    .unwrap();
}

#[test]
fn mechanism_counter_tracks_consumed_prefix_not_supplied_slice() {
    use crate::seclang::measurements::{self, Event};
    let scope = measurements::start();
    let mut stream = JsonStream::new(JsonStreamLimits {
        max_source_bytes: 2,
        ..JsonStreamLimits::default()
    });
    assert!(stream.feed(JsonStreamInput::Data(br#""abcdef""#), |_| Ok(())).is_err());
    assert_eq!(scope.counts().get(Event::SourceBytes), stream.usage().source_bytes);
    assert_eq!(scope.counts().get(Event::SourceBytes), 2);
    assert_eq!(scope.counts().get(Event::Completions), 0);
    assert!(stream.feed(JsonStreamInput::Complete, |_| Ok(())).is_err());
    assert_eq!(scope.counts().get(Event::SourceBytes), 2);
}

#[test]
fn inline_decoding_boundaries_and_exact_spill_budget_survive_chunk_splits() {
    for len in [1023, 1024, 1025] {
        let source = format!("\"{}\"", "\\u0061".repeat(len));
        let limit = if len <= 1024 { 0 } else { len };
        let limits = JsonStreamLimits {
            max_retained_bytes: limit,
            ..JsonStreamLimits::default()
        };
        let whole = tokenize(&[source.as_bytes()], limits);
        whole.0.unwrap();
        assert_eq!(whole.1, vec![OwnedToken::String("a".repeat(len))]);
        assert_eq!(whole.2.peak_retained_bytes, limit);
        let chunks: Vec<_> = source.as_bytes().chunks(7).collect();
        let split = tokenize(&chunks, limits);
        assert_same_semantics(&split, &whole);
        if len > 1024 {
            let failure = tokenize(
                &chunks,
                JsonStreamLimits {
                    max_retained_bytes: len - 1,
                    ..limits
                },
            );
            assert_eq!(failure.0, Err(limit_error(JsonStreamLimit::RetainedBytes, len - 1)));
            assert_eq!(failure.2.retained_bytes, 0);
        }
    }
}

#[test]
fn spilled_token_storage_is_reused_until_terminal_release() {
    let mut stream = JsonStream::new(JsonStreamLimits::default());
    let source = format!("[\"{}\"", "\\u0061".repeat(1025));
    stream
        .feed(JsonStreamInput::Data(source.as_bytes()), |_| Ok(()))
        .unwrap();
    let retained = stream.usage().retained_bytes;
    assert!(heap_capacity(&stream.token) >= 1025);
    for _ in 0..100 {
        stream
            .feed(JsonStreamInput::Data(br#", "\u0061""#), |_| Ok(()))
            .unwrap();
        assert_eq!(stream.usage().retained_bytes, retained);
    }
    let failure = stream.feed(JsonStreamInput::Data(b",]"), |_| Ok(()));
    assert_eq!(failure, Err(JsonStreamError::Syntax));
    assert_eq!(heap_capacity(&stream.token), 0);
    assert!(stream.token.is_empty());
    assert_eq!(stream.feed(JsonStreamInput::Complete, |_| panic!("terminal")), failure);
}

// Frozen pre-batching lexical path: identical chunk ownership and delimiter work.
impl JsonStream {
    pub(super) fn borrow_string_prefix_sequential<F>(
        &mut self,
        bytes: &[u8],
        key: bool,
        sink: &mut F,
    ) -> Result<usize, JsonStreamError>
    where
        F: FnMut(JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>,
    {
        for (offset, &byte) in bytes.iter().enumerate() {
            charge(
                &mut self.usage.source_bytes,
                1,
                self.limits.max_source_bytes,
                JsonStreamLimit::SourceBytes,
            )?;
            #[cfg(test)]
            measurements::record(Event::SourceBytes, 1);
            self.work(1)?;
            self.raw_byte()?;
            if self.utf8_remaining != 0 {
                self.utf8(byte)?;
                self.decoded(1)?;
                continue;
            }
            match byte {
                b'"' => {
                    self.work(offset)?;
                    self.event()?;
                    let value = std::str::from_utf8(&bytes[..offset]).map_err(|_| JsonStreamError::Utf8)?;
                    sink(if key {
                        JsonStreamToken::Key(value)
                    } else {
                        JsonStreamToken::String(value)
                    })
                    .map_err(|_| JsonStreamError::SinkRejected)?;
                    self.lexical = Lexical::Idle;
                    if key {
                        *self.stack.last_mut().ok_or(JsonStreamError::Syntax)? = Frame::ObjectColon;
                    }
                    return Ok(offset + 1);
                }
                b'\\' => {
                    self.work(offset)?;
                    self.retain_token_bytes(&bytes[..offset])?;
                    self.lexical = Lexical::String {
                        key,
                        escape: EscapeState::Slash,
                    };
                    return Ok(offset + 1);
                }
                0..=0x1f => return Err(JsonStreamError::Syntax),
                _ => {
                    self.utf8(byte)?;
                    self.decoded(1)?;
                }
            }
        }
        self.work(bytes.len())?;
        self.retain_token_bytes(bytes)?;
        Ok(bytes.len())
    }
}

fn tokenize_ascii_mode(
    chunks: &[&[u8]],
    limits: JsonStreamLimits,
    sequential: bool,
) -> (Result<(), JsonStreamError>, Vec<OwnedToken>, JsonStreamUsage, usize) {
    let mut stream = JsonStream::new(limits);
    stream.sequential_ascii = sequential;
    let mut tokens = Vec::new();
    let mut result = Ok(());
    for input in chunks
        .iter()
        .copied()
        .map(JsonStreamInput::Data)
        .chain([JsonStreamInput::Complete])
    {
        result = stream.feed(input, |token| {
            tokens.push(token.into());
            Ok(())
        });
        if result.is_err() {
            break;
        }
    }
    if let Err(error) = result {
        assert_eq!(
            stream.feed(JsonStreamInput::Complete, |_| panic!("sticky error emitted")),
            Err(error)
        );
    }
    (result, tokens, stream.usage(), stream.raw_token_bytes)
}

#[test]
fn ascii_batch_matches_sequential_errors_and_all_counters_at_every_boundary() {
    let mut sources = vec![
        br#"{"ordinary key":"ordinary value","ordinary key":"replacement"}"#.to_vec(),
        br#"["plain-prefix\nrest","plain-prefix\u20ac",true]"#.to_vec(),
        "[\"plain-prefix-é-suffix\",\"plain-prefix-€\"]".as_bytes().to_vec(),
        br#""ordinary incomplete"#.to_vec(),
        br#"["ordinary",]"#.to_vec(),
    ];
    // Every byte class can end the admitted ASCII prefix, including DEL.
    for byte in 0..=u8::MAX {
        sources.push(vec![b'"', b'a', b'b', byte, b'c', b'"']);
    }
    for source in sources {
        for split in 0..=source.len() {
            let chunks = [&source[..split], &source[split..]];
            assert_eq!(
                tokenize_ascii_mode(&chunks, JsonStreamLimits::default(), false),
                tokenize_ascii_mode(&chunks, JsonStreamLimits::default(), true),
                "default limits, split {split}"
            );
        }
        // All source/raw/decoded/work caps near any consumed prefix, plus
        // competing caps, must preserve partially admitted counters and priority.
        for cap in 0..=source.len() * 3 + 3 {
            for kind in 0..7 {
                let mut limits = JsonStreamLimits::default();
                match kind {
                    0 => limits.max_source_bytes = cap,
                    1 => limits.max_raw_token_bytes = cap,
                    2 => limits.max_decoded_bytes = cap,
                    3 => limits.max_work = cap,
                    4 => {
                        limits.max_source_bytes = cap;
                        limits.max_raw_token_bytes = cap;
                        limits.max_decoded_bytes = cap;
                        limits.max_work = cap;
                    }
                    5 => limits.max_events = cap,
                    _ => limits.max_retained_bytes = cap,
                }
                for split in 0..=source.len() {
                    let chunks = [&source[..split], &source[split..]];
                    assert_eq!(
                        tokenize_ascii_mode(&chunks, limits, false),
                        tokenize_ascii_mode(&chunks, limits, true),
                        "cap {cap}, kind {kind}, split {split}"
                    );
                }
            }
        }
    }
}

#[test]
fn ascii_batch_preserves_spill_and_delimiter_validation_work() {
    for len in [1023, 1024, 1025, 4095, 4096, 4097] {
        let source = format!("\"{}\"", "a".repeat(len));
        for split in [0, 1, 2, len / 2, len, len + 1, len + 2] {
            let chunks = [&source.as_bytes()[..split], &source.as_bytes()[split..]];
            let baseline = tokenize_ascii_mode(&chunks, JsonStreamLimits::default(), true);
            baseline.0.unwrap();
            for work in [baseline.2.work - 1, baseline.2.work, baseline.2.work + 1] {
                for retained in [
                    0,
                    baseline.2.peak_retained_bytes.saturating_sub(1),
                    baseline.2.peak_retained_bytes,
                ] {
                    let limits = JsonStreamLimits {
                        max_work: work,
                        max_retained_bytes: retained,
                        ..JsonStreamLimits::default()
                    };
                    assert_eq!(
                        tokenize_ascii_mode(&chunks, limits, false),
                        tokenize_ascii_mode(&chunks, limits, true)
                    );
                }
            }
        }
    }
}

fn assert_buffered_ascii_trace(chunks: &[&[u8]], limits: JsonStreamLimits, reject_string: bool) {
    use crate::seclang::measurements::{self, Event};
    let mut batched = JsonStream::new(limits);
    let mut sequential = JsonStream::new(limits);
    sequential.sequential_ascii = true;
    let mut tokens = [Vec::<OwnedToken>::new(), Vec::<OwnedToken>::new()];
    for input in chunks.iter().copied().map(JsonStreamInput::Data).chain([
        JsonStreamInput::Complete,
        JsonStreamInput::Abandoned,
        JsonStreamInput::Complete,
    ]) {
        let mut snapshots = Vec::new();
        for (index, stream) in [&mut batched, &mut sequential].into_iter().enumerate() {
            let scope = measurements::start();
            let result = stream.feed(input, |token| {
                tokens[index].push(token.into());
                if reject_string && matches!(token, JsonStreamToken::Key(_) | JsonStreamToken::String(_)) {
                    Err(JsonStreamSinkError)
                } else {
                    Ok(())
                }
            });
            snapshots.push((
                result,
                stream.usage(),
                stream.raw_token_bytes,
                stream.token.capacity(),
                stream.token.to_vec(),
                stream.stack.capacity(),
                (stream.utf8_remaining, stream.utf8_value, stream.utf8_minimum),
                scope.counts().get(Event::SourceBytes),
                scope.counts().get(Event::TokenCopyBytes),
            ));
        }
        assert_eq!(snapshots[0], snapshots[1]);
        assert_eq!(tokens[0], tokens[1]);
    }
}

#[test]
fn buffered_ascii_preserves_special_byte_errors_and_competing_admission() {
    let mut sources = vec![
        br#""\nordinary\u20ac-tail""#.to_vec(),
        br#"{"\u0061-key":"prefix\nordinary suffix"}"#.to_vec(),
        br#""\nordinary incomplete"#.to_vec(),
        br#""prefix\nordinary\q""#.to_vec(),
        "\"\\né-suffix\"".as_bytes().to_vec(),
        "\"\\n€-suffix\"".as_bytes().to_vec(),
    ];
    for byte in 0..=u8::MAX {
        for prefix in [br#""\n"#.as_slice(), br#""\nabc"#.as_slice()] {
            let mut source = prefix.to_vec();
            source.extend_from_slice(&[byte, b'd', b'"']);
            sources.push(source);
        }
    }
    for source in sources {
        for cap in 0..=source.len() * 3 + 3 {
            for kind in 0..7 {
                let mut limits = JsonStreamLimits::default();
                match kind {
                    0 => limits.max_source_bytes = cap,
                    1 => limits.max_raw_token_bytes = cap,
                    2 => limits.max_decoded_bytes = cap,
                    3 => limits.max_work = cap,
                    4 => {
                        limits.max_source_bytes = cap;
                        limits.max_raw_token_bytes = cap;
                        limits.max_decoded_bytes = cap;
                        limits.max_work = cap;
                    }
                    5 => limits.max_events = cap,
                    _ => limits.max_retained_bytes = cap,
                }
                for split in [0, 2, 3, source.len() / 2, source.len()] {
                    let chunks = [&source[..split], &source[split..]];
                    assert_eq!(
                        tokenize_ascii_mode(&chunks, limits, false),
                        tokenize_ascii_mode(&chunks, limits, true),
                        "buffered cap {cap}, kind {kind}, split {split}"
                    );
                }
            }
        }
        for split in 0..=source.len() {
            assert_buffered_ascii_trace(&[&source[..split], &source[split..]], JsonStreamLimits::default(), true);
        }
    }
}

#[test]
fn buffered_ascii_preserves_growth_work_and_exact_limits_at_every_spill() {
    for decoded in [1023, 1024, 1025, 2047, 2048, 2049, 4097] {
        for prefix in [0, decoded / 2, decoded - 1] {
            let source = format!("\"{}\\n{}\"", "a".repeat(prefix), "b".repeat(decoded - prefix - 1));
            for split in [0, 1, prefix + 2, prefix + 3, source.len() / 2, source.len()] {
                let chunks = [&source.as_bytes()[..split], &source.as_bytes()[split..]];
                let baseline = tokenize_ascii_mode(&chunks, JsonStreamLimits::default(), true);
                baseline.0.unwrap();
                for kind in 0..6 {
                    let required = match kind {
                        0 => baseline.2.source_bytes,
                        1 => source.len(),
                        2 => baseline.2.decoded_bytes,
                        3 => baseline.2.work,
                        4 => baseline.2.events,
                        _ => baseline.2.peak_retained_bytes,
                    };
                    for cap in [required.saturating_sub(1), required, required + 1] {
                        let mut limits = JsonStreamLimits::default();
                        match kind {
                            0 => limits.max_source_bytes = cap,
                            1 => limits.max_raw_token_bytes = cap,
                            2 => limits.max_decoded_bytes = cap,
                            3 => limits.max_work = cap,
                            4 => limits.max_events = cap,
                            _ => limits.max_retained_bytes = cap,
                        }
                        assert_buffered_ascii_trace(&chunks, limits, false);
                    }
                }
            }
        }
    }
}

#[test]
fn buffered_ascii_reuses_long_token_capacity_for_short_tokens_and_chunks() {
    let parts = [
        format!("[\"\\n{}\"", "a".repeat(4096)),
        ",\"\\nshort\"".into(),
        ",\"plain\"".into(),
        ",\"prefix".into(),
        "-suffix\"".into(),
        ",\"\\u20".into(),
        "ac-tail\"".into(),
        format!(",\"late{}\\n{}\"]", "b".repeat(2048), "c".repeat(4096)),
    ];
    let chunks: Vec<_> = parts.iter().map(|part| part.as_bytes()).collect();
    assert_buffered_ascii_trace(&chunks, JsonStreamLimits::default(), false);
    let joined = parts.concat();
    let baseline = tokenize_ascii_mode(&chunks, JsonStreamLimits::default(), true);
    for work in [baseline.2.work - 1, baseline.2.work] {
        assert_buffered_ascii_trace(
            &chunks,
            JsonStreamLimits {
                max_work: work,
                ..JsonStreamLimits::default()
            },
            false,
        );
    }
    for width in [1, 2, 7, 1024, 1025] {
        let chunks: Vec<_> = joined.as_bytes().chunks(width).collect();
        assert_buffered_ascii_trace(&chunks, JsonStreamLimits::default(), false);
    }
}

#[derive(Default)]
struct SelectiveEvents(Vec<String>);
impl JsonStreamSelectiveSink for SelectiveEvents {
    fn on_token(&mut self, token: JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError> {
        self.0.push(match token {
            JsonStreamToken::Key(key) => format!("key:{key}"),
            JsonStreamToken::ObjectStart => "object".into(),
            JsonStreamToken::ObjectEnd => "end-object".into(),
            JsonStreamToken::ArrayStart => "array".into(),
            JsonStreamToken::ArrayEnd => "end-array".into(),
            JsonStreamToken::Boolean(value) => format!("bool:{value}"),
            JsonStreamToken::Null => "null".into(),
            JsonStreamToken::String(_) | JsonStreamToken::Number { .. } => {
                panic!("capture-all scalar in selective callback")
            }
        });
        Ok(())
    }
    fn on_number(&mut self, raw: &str) -> Result<(), JsonStreamSinkError> {
        self.0.push(format!("number:{raw}"));
        Ok(())
    }
    fn on_discarded_string(&mut self, summary: JsonDiscardedStringSummary) -> Result<(), JsonStreamSinkError> {
        self.0
            .push(format!("discard:{}:{}", summary.decoded_bytes, summary.decoded_chars));
        Ok(())
    }
}
fn selective_limits() -> JsonStreamSelectiveLimits {
    JsonStreamSelectiveLimits {
        lexical: JsonStreamLimits {
            max_raw_token_bytes: 1538,
            ..JsonStreamLimits::default()
        },
        max_key_bytes: 256,
        max_number_bytes: 64,
    }
}
fn selective_run(
    body: &[u8],
    width: usize,
    limits: JsonStreamSelectiveLimits,
) -> (Result<(), JsonStreamError>, Vec<String>, JsonStreamUsage) {
    let mut stream = JsonStreamSelective::new(limits);
    let mut events = SelectiveEvents::default();
    let mut result = Ok(());
    for chunk in body.chunks(width) {
        result = stream.feed(JsonStreamInput::Data(chunk), &mut events);
        if result.is_err() {
            break;
        }
    }
    if result.is_ok() {
        result = stream.feed(JsonStreamInput::Complete, &mut events);
    }
    (result, events.0, stream.usage())
}

#[test]
fn selective_discard_validates_strings_without_retaining_them_or_coercing_numbers() {
    let body = br#"{"key":"a\u00e9\uD834\uDD1E","ignored":1e1000,"other":null}"#;
    let expected = selective_run(body, body.len(), selective_limits());
    assert_eq!(expected.0, Ok(()));
    assert!(expected.1.contains(&"discard:7:3".to_string()));
    assert!(expected.1.contains(&"number:1e1000".to_string()));
    for width in 1..=body.len() {
        assert_eq!(selective_run(body, width, selective_limits()), expected);
    }
    let large = format!("{{\"key\":\"{}\"}}", "x".repeat(100_000));
    let (result, events, usage) = selective_run(large.as_bytes(), 13, selective_limits());
    assert_eq!(result, Ok(()));
    assert!(events.contains(&"discard:100000:100000".to_string()));
    assert!(usage.peak_retained_bytes < 1024);
}

#[test]
fn selective_independent_budgets_and_error_counters_are_chunk_invariant() {
    let body = br#"{"abc":[12345,"xyz\u00e9",{"ab":"tail"}]}"#;
    let baseline = selective_run(body, body.len(), selective_limits());
    assert_eq!(baseline.0, Ok(()));
    let mut profiles = Vec::new();
    for budget in 0..=baseline.2.work + 1 {
        let mut limits = selective_limits();
        limits.lexical.max_work = budget;
        profiles.push(limits);
    }
    for budget in 0..=body.len() + 1 {
        let mut limits = selective_limits();
        limits.lexical.max_source_bytes = budget;
        profiles.push(limits);
    }
    for budget in 0..=baseline.2.decoded_bytes + 1 {
        let mut limits = selective_limits();
        limits.lexical.max_decoded_bytes = budget;
        profiles.push(limits);
    }
    for budget in 0..=baseline.2.events + 1 {
        let mut limits = selective_limits();
        limits.lexical.max_events = budget;
        profiles.push(limits);
    }
    for budget in 0..=baseline.2.peak_retained_bytes + 1 {
        let mut limits = selective_limits();
        limits.lexical.max_retained_bytes = budget;
        profiles.push(limits);
    }
    for budget in 0..=6 {
        let mut limits = selective_limits();
        limits.max_number_bytes = budget;
        profiles.push(limits);
        let mut limits = selective_limits();
        limits.max_key_bytes = budget;
        profiles.push(limits);
        let mut limits = selective_limits();
        limits.lexical.max_raw_token_bytes = budget;
        profiles.push(limits);
        let mut limits = selective_limits();
        limits.lexical.max_depth = budget;
        profiles.push(limits);
    }
    for limits in profiles {
        let whole = selective_run(body, body.len(), limits);
        for width in [1, 2, 7] {
            assert_eq!(
                selective_run(body, width, limits),
                whole,
                "profile {limits:?}, width {width}"
            );
        }
    }
}

#[test]
fn selective_syntax_failures_stay_sticky_and_release_state() {
    for body in [
        br#"{"key":"\uD800x"}"#.as_slice(),
        b"{\"key\":\"\xff\"}".as_slice(),
        br#"{"key":"tail"#.as_slice(),
        br#"{"key":"valid"}x"#.as_slice(),
    ] {
        let whole = selective_run(body, body.len(), selective_limits());
        assert!(whole.0.is_err());
        assert_eq!(whole.2.retained_bytes, 0);
        for width in 1..=body.len() {
            assert_eq!(selective_run(body, width, selective_limits()), whole);
        }
    }
    let mut stream = JsonStreamSelective::new(selective_limits());
    let mut events = SelectiveEvents::default();
    let first = stream.feed(JsonStreamInput::Data(b"?"), &mut events);
    assert_eq!(stream.feed(JsonStreamInput::Data(b"{}"), &mut events), first);
    assert_eq!(stream.feed(JsonStreamInput::Complete, &mut events), first);
    assert!(events.0.is_empty());
}

#[derive(Default)]
struct SelectedStrings {
    selected: bool,
    values: Vec<String>,
    discarded: usize,
    cap: usize,
}
impl JsonStreamSelectiveSink for SelectedStrings {
    fn on_token(&mut self, token: JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError> {
        if let JsonStreamToken::Key(key) = token {
            self.selected = key == "status";
        }
        Ok(())
    }
    fn on_number(&mut self, _: &str) -> Result<(), JsonStreamSinkError> {
        Ok(())
    }
    fn on_discarded_string(&mut self, _: JsonDiscardedStringSummary) -> Result<(), JsonStreamSinkError> {
        self.discarded += 1;
        Ok(())
    }
    fn string_capture_limit(&mut self) -> Option<usize> {
        self.selected.then_some(self.cap)
    }
    fn on_captured_string(&mut self, value: &str) -> Result<(), JsonStreamSinkError> {
        self.values.push(value.to_owned());
        self.selected = false;
        Ok(())
    }
}

#[test]
fn selected_strings_share_grammar_and_caps_while_large_unselected_values_are_discarded() {
    let document = format!(
        r#"{{"output":"{}","status":"compl\u0065ted","other":"é€","status":"cancelled"}}"#,
        "x".repeat(100_000)
    );
    for width in [1, 7, 65536, document.len()] {
        let mut parser = JsonStreamSelective::new(selective_limits());
        let mut sink = SelectedStrings {
            cap: 9,
            ..Default::default()
        };
        for part in document.as_bytes().chunks(width) {
            parser.feed(JsonStreamInput::Data(part), &mut sink).unwrap();
        }
        parser.feed(JsonStreamInput::Complete, &mut sink).unwrap();
        assert_eq!(sink.values, ["completed", "cancelled"]);
        assert_eq!(sink.discarded, 2);
        assert!(parser.usage().peak_retained_bytes < 4096);
    }
    for raw in [
        br#"{"status":"completed"}"#.as_slice(),
        br#"{"status":"\u0063ompleted"}"#,
    ] {
        for width in 1..=raw.len() {
            let mut parser = JsonStreamSelective::new(selective_limits());
            let mut sink = SelectedStrings {
                cap: 8,
                ..Default::default()
            };
            let mut error = None;
            for part in raw.chunks(width) {
                if let Err(value) = parser.feed(JsonStreamInput::Data(part), &mut sink) {
                    error = Some(value);
                    break;
                }
            }
            assert!(matches!(
                error,
                Some(JsonStreamError::Limit {
                    kind: JsonStreamLimit::DecodedBytes,
                    ..
                })
            ));
            assert!(sink.values.is_empty());
        }
    }
}
