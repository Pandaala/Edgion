use super::*;
use crate::seclang::body::{build_fallback_publication_with_raw_policy, build_publication_with_raw_policy};
use crate::seclang::{BodyProcessorSelection, CollectionKind, RequestBody};

fn config() -> JsonBodyConfig {
    JsonBodyConfig {
        lexical: JsonStreamLimits {
            max_depth: 256,
            ..JsonStreamLimits::default()
        },
        document: JsonDocumentLimits {
            max_retained_bytes: 16 * 1024 * 1024,
            max_key_bytes: 1024 * 1024,
            max_members: 65_536,
            max_events: 131_072,
            max_path_bytes: 16 * 1024,
            max_depth: 256,
            max_work: 128 * 1024 * 1024,
        },
        body: BodyLimits {
            max_depth: 256,
            ..BodyLimits::default()
        },
        head_args_combined_size: 17,
        retain_raw_body: None,
        detection_only: false,
    }
}

fn compare(input: &[u8], split: usize, config: JsonBodyConfig) {
    let body = RequestBody::new(input, BodyProcessorSelection::Json);
    let original = build_publication_with_raw_policy(
        body,
        None,
        config.head_args_combined_size,
        config.body,
        config.retain_raw_body,
    );
    let (expected, body_error) = match original {
        Err(error @ BodyError::Malformed { .. }) if config.detection_only => (
            build_fallback_publication_with_raw_policy(
                body,
                None,
                config.head_args_combined_size,
                config.body,
                config.retain_raw_body,
            ),
            Some(error),
        ),
        result => (result, None),
    };
    let mut adapter = JsonBodyAdapter::<()>::new(config);
    adapter
        .feed(JsonStreamInput::Data(&input[..split]), |_, _| Ok(()))
        .unwrap();
    assert!(adapter.summary().is_none());
    assert!(adapter.events().is_none());
    adapter
        .feed(JsonStreamInput::Data(&input[split..]), |_, _| Ok(()))
        .unwrap();
    assert_eq!(adapter.source_bytes(), input.len());
    let result = adapter.feed(JsonStreamInput::Complete, |_, _| Ok(()));
    match expected {
        Ok(expected) => {
            result.unwrap();
            let summary = adapter.summary().unwrap();
            assert_eq!(summary.body_error, body_error);
            assert_eq!(summary.publication.entries, expected.entries.len());
            assert_eq!(summary.publication.retained_bytes, expected.retained_bytes);
            let decoded: usize = expected
                .entries
                .iter()
                .map(|entry| entry.name.as_ref().map_or(0, |name| name.len()) + entry.value.len())
                .sum();
            let args: usize = expected
                .entries
                .iter()
                .filter(|entry| entry.collection == CollectionKind::Args)
                .map(|entry| entry.name.as_ref().map_or(0, |name| name.len()) + entry.value.len())
                .sum();
            assert_eq!(summary.publication.decoded_bytes, decoded);
            assert_eq!(summary.publication.body_args_combined_size, args);
            assert_eq!(
                summary.publication.args_combined_size,
                config.head_args_combined_size + args
            );
            assert_eq!(adapter.events().is_some(), !input.is_empty() && body_error.is_none());
            assert_eq!(
                adapter.feed(JsonStreamInput::Data(b"x"), |_, _| panic!("terminal callback")),
                Err(JsonBodyError::AlreadyComplete)
            );
            assert!(adapter.summary().is_some());
        }
        Err(error) => {
            assert_eq!(result, Err(JsonBodyError::Body(error)));
            assert!(adapter.summary().is_none());
            assert!(adapter.events().is_none());
            assert!(adapter.usage().is_none());
            assert_eq!(
                adapter.feed(JsonStreamInput::Complete, |_, _| panic!("terminal callback")),
                Err(JsonBodyError::Body(error))
            );
        }
    }
}

#[test]
fn every_split_preserves_empty_valid_malformed_and_fallback_publication() {
    for input in [
        b"".as_slice(),
        b" ",
        b"\r\n\t",
        b"{}",
        b"null",
        b"0",
        b"-0",
        b"1e400",
        b"1e",
        b"[",
        b"[0,]",
        b"\"bad\\q\"",
        b"\"\xff\"",
        b"!1234567890123456789",
        br#"{"a":1,"a":[],"z":null}"#,
        br#"{"x":[true,false,{},[]],"a":"text"} trailing"#,
    ] {
        for split in 0..=input.len() {
            for raw in [None, Some(false), Some(true)] {
                for detection_only in [false, true] {
                    compare(
                        input,
                        split,
                        JsonBodyConfig {
                            retain_raw_body: raw,
                            detection_only,
                            ..config()
                        },
                    );
                }
            }
        }
    }
}

#[test]
fn malformed_and_empty_bodies_preserve_competing_prelude_limits() {
    for input in [
        b"".as_slice(),
        b" ",
        b"!12345678",
        b"!123456789",
        b"!123456789012345678901234567890",
        br#"{"a":1}x"#,
    ] {
        for raw in [None, Some(false), Some(true)] {
            for detection_only in [false, true] {
                for max_body_bytes in [0, input.len()] {
                    for max_work_bytes in [0, input.len()] {
                        for max_entries in [0, 1, 2, 3, 4] {
                            for max_decoded_bytes in [0, 1, 5, 6, 7, 40] {
                                let mut config = config();
                                config.retain_raw_body = raw;
                                config.detection_only = detection_only;
                                config.body = BodyLimits {
                                    max_body_bytes,
                                    max_work_bytes,
                                    max_entries,
                                    max_decoded_bytes,
                                    ..config.body
                                };
                                compare(input, input.len() / 2, config);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn malformed_drain_counts_whole_chunks_and_stops_callbacks() {
    let mut adapter = JsonBodyAdapter::<()>::new(config());
    adapter.feed(JsonStreamInput::Data(b"[1,!rest"), |_, _| Ok(())).unwrap();
    assert!(adapter.usage().is_none());
    adapter
        .feed(JsonStreamInput::Data(b"123456789"), |_, _| {
            panic!("callback after syntax failure")
        })
        .unwrap();
    assert_eq!(adapter.source_bytes(), 17);
    assert_eq!(
        adapter.feed(JsonStreamInput::Complete, |_, _| panic!(
            "callback after syntax failure"
        )),
        Err(JsonBodyError::Body(BodyError::Malformed {
            processor: BodyProcessorSelection::Json
        }))
    );

    let mut config = config();
    config.lexical.max_source_bytes = 4;
    config.detection_only = true;
    let mut adapter = JsonBodyAdapter::<()>::new(config);
    adapter.feed(JsonStreamInput::Data(b"!"), |_, _| Ok(())).unwrap();
    let error = JsonBodyError::SourceByteLimit { limit: 4 };
    assert_eq!(
        adapter.feed(JsonStreamInput::Data(b"1234"), |_, _| panic!("drain callback")),
        Err(error)
    );
    assert_eq!(adapter.feed(JsonStreamInput::Complete, |_, _| Ok(())), Err(error));
    assert!(adapter.summary().is_none());
}

#[test]
fn independent_limits_sink_failure_and_abandonment_never_fallback() {
    let mut config = config();
    config.detection_only = true;
    config.document.max_events = 0;
    let mut adapter = JsonBodyAdapter::<()>::new(config);
    assert!(matches!(
        adapter.feed(JsonStreamInput::Data(b"null"), |_, _| Ok(())),
        Err(JsonBodyError::Document(JsonDocumentError::Limit { .. }))
    ));
    assert!(adapter.usage().is_none());
    assert!(adapter.summary().is_none());

    config.document.max_events = 1024;
    let mut adapter = JsonBodyAdapter::<()>::new(config);
    assert_eq!(
        adapter.feed(JsonStreamInput::Data(b"null"), |_, _| Err(JsonDocumentSinkError)),
        Err(JsonBodyError::Document(JsonDocumentError::SinkRejected))
    );
    assert!(adapter.usage().is_none());

    for input in [b"".as_slice(), b"[", b"!", b"{}"] {
        let mut adapter = JsonBodyAdapter::<()>::new(config);
        adapter.feed(JsonStreamInput::Data(input), |_, _| Ok(())).unwrap();
        assert_eq!(
            adapter.feed(JsonStreamInput::Abandoned, |_, _| panic!("abandoned callback")),
            Err(JsonBodyError::Abandoned)
        );
        assert_eq!(
            adapter.feed(JsonStreamInput::Complete, |_, _| Ok(())),
            Err(JsonBodyError::Abandoned)
        );
        assert!(adapter.usage().is_none());
        assert!(adapter.summary().is_none());
    }
}

#[test]
fn reference_nesting_syntax_is_distinct_from_configured_resource_depth() {
    for depth in [127, 128] {
        let input = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        for detection_only in [false, true] {
            for split in [0, depth, input.len()] {
                compare(
                    input.as_bytes(),
                    split,
                    JsonBodyConfig {
                        detection_only,
                        ..config()
                    },
                );
                let replaced = format!("{{\"a\":{input},\"a\":0}}");
                compare(
                    replaced.as_bytes(),
                    split,
                    JsonBodyConfig {
                        detection_only,
                        ..config()
                    },
                );
            }
        }
    }
    let mut config = config();
    config.lexical.max_depth = 1;
    config.detection_only = true;
    let mut adapter = JsonBodyAdapter::<()>::new(config);
    assert!(matches!(
        adapter.feed(JsonStreamInput::Data(b"[[[0]]]"), |_, _| Ok(())),
        Err(JsonBodyError::Document(JsonDocumentError::Lexical(
            JsonStreamError::Limit { .. }
        )))
    ));
    assert!(adapter.summary().is_none());
}

#[test]
fn canonical_errors_use_final_length_and_original_prelude_before_fallback() {
    let mut config = config();
    config.body.max_body_bytes = 3;
    config.detection_only = true;
    let mut adapter = JsonBodyAdapter::<()>::new(config);
    adapter.feed(JsonStreamInput::Data(b"!"), |_, _| Ok(())).unwrap();
    adapter
        .feed(JsonStreamInput::Data(b"123456789"), |_, _| panic!("drain callback"))
        .unwrap();
    assert_eq!(
        adapter.feed(JsonStreamInput::Complete, |_, _| Ok(())),
        Err(JsonBodyError::Body(BodyError::ByteLimit { actual: 10, limit: 3 }))
    );

    config.body = BodyLimits {
        max_entries: 2,
        ..BodyLimits::default()
    };
    config.detection_only = false;
    let mut adapter = JsonBodyAdapter::<()>::new(config);
    adapter.feed(JsonStreamInput::Data(b"!"), |_, _| Ok(())).unwrap();
    assert_eq!(
        adapter.feed(JsonStreamInput::Complete, |_, _| Ok(())),
        Err(JsonBodyError::Body(BodyError::Malformed {
            processor: BodyProcessorSelection::Json
        }))
    );

    config.detection_only = true;
    let mut adapter = JsonBodyAdapter::<()>::new(config);
    adapter.feed(JsonStreamInput::Data(b"!"), |_, _| Ok(())).unwrap();
    assert_eq!(
        adapter.feed(JsonStreamInput::Complete, |_, _| Ok(())),
        Err(JsonBodyError::Body(BodyError::EntryLimit { limit: 2 }))
    );

    config.body.max_entries = 0;
    let mut adapter = JsonBodyAdapter::<()>::new(config);
    adapter.feed(JsonStreamInput::Data(b"!"), |_, _| Ok(())).unwrap();
    assert_eq!(
        adapter.feed(JsonStreamInput::Complete, |_, _| Ok(())),
        Err(JsonBodyError::Body(BodyError::EntryLimit { limit: 0 }))
    );
}

#[test]
fn adapter_debug_hides_callback_facts_and_chunk_admission_precedes_callbacks() {
    #[derive(Clone, Copy)]
    struct Fact;
    impl std::fmt::Debug for Fact {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("subscriber facts must never be formatted")
        }
    }
    let mut adapter = JsonBodyAdapter::<Fact>::new(config());
    adapter
        .feed(
            JsonStreamInput::Data(br#"{"sensitive-key":"sensitive-value"}"#),
            |_, _| Ok(Fact),
        )
        .unwrap();
    adapter.feed(JsonStreamInput::Complete, |_, _| Ok(Fact)).unwrap();
    let debug = format!("{adapter:?}");
    assert!(!debug.contains("sensitive"));
    for event in adapter.events().unwrap() {
        assert!(!format!("{event:?}").contains("sensitive"));
    }
    let mut config = config();
    config.lexical.max_source_bytes = 1;
    let mut adapter = JsonBodyAdapter::<()>::new(config);
    assert_eq!(
        adapter.feed(JsonStreamInput::Data(b"null"), |_, _| panic!("unadmitted callback")),
        Err(JsonBodyError::SourceByteLimit { limit: 1 })
    );
    assert_eq!(adapter.source_bytes(), 0);
    assert!(adapter.usage().is_none());
}
