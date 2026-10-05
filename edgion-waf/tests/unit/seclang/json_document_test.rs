use super::*;

fn limits() -> JsonDocumentLimits {
    JsonDocumentLimits {
        max_retained_bytes: 1024 * 1024,
        max_key_bytes: 64 * 1024,
        max_members: 1024,
        max_events: 8192,
        max_path_bytes: 4096,
        max_depth: 64,
        max_work: 16 * 1024 * 1024,
    }
}

fn document(input: &[u8], limits: JsonDocumentLimits) -> Result<JsonDocument<usize>, JsonDocumentError> {
    let mut document = JsonDocument::new(JsonStreamLimits::default(), limits);
    document.feed(JsonStreamInput::Data(input), |_, value| Ok(value.len()))?;
    document.feed(JsonStreamInput::Complete, |_, value| Ok(value.len()))?;
    Ok(document)
}

#[test]
fn duplicate_subtrees_replace_events_and_keep_reference_order_at_every_split() {
    let input = br#"{"z":0,"a":{"old":["removed"]},"a":{"b":true,"a":null},"a.b":"literal","list":[3,2,1]}"#;
    let mut expected = None;
    for split in 0..=input.len() {
        let mut document = JsonDocument::new(JsonStreamLimits::default(), limits());
        let mut provisional = Vec::new();
        let mut sink = |path: &str, value: &[u8]| {
            let index = provisional.len();
            provisional.push((path.to_owned(), value.to_vec()));
            Ok(index)
        };
        document
            .feed(JsonStreamInput::Data(&input[..split]), &mut sink)
            .unwrap();
        assert!(document.events().is_none());
        document
            .feed(JsonStreamInput::Data(&input[split..]), &mut sink)
            .unwrap();
        assert!(document.events().is_none());
        document.feed(JsonStreamInput::Complete, &mut sink).unwrap();
        let events: Vec<_> = document.events().unwrap().collect();
        let published: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                JsonDocumentEvent::Scalar { result, .. } => Some(provisional[*result].clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            published.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
            [
                "json.a.a",
                "json.a.b",
                "json.a.b",
                "json.list.0",
                "json.list.1",
                "json.list.2",
                "json.list",
                "json.z"
            ]
        );
        assert_eq!(published[0].1, b"");
        assert_eq!(published[1].1, b"true");
        assert_eq!(published[2].1, b"literal");
        assert_eq!(published[6].1, b"3");
        if let Some((expected_events, expected_usage)) = &expected {
            assert_eq!(&events, expected_events);
            assert_eq!(&document.usage(), expected_usage);
        } else {
            expected = Some((events, document.usage()));
        }
        assert_eq!(document.usage().key_bytes, 0);
        assert_eq!(document.usage().members, 0);
        assert_eq!(document.lexical_usage().retained_bytes, 0);
    }
}

#[test]
fn numbers_use_the_reference_normalization_without_heap_scratch() {
    for spelling in [
        "-0",
        "0",
        "1e2",
        "1.00",
        "-0.0",
        "1e-300",
        "1.7976931348623157e308",
        "18446744073709551615",
        "18446744073709551616",
        "-9223372036854775809",
    ] {
        let expected: serde_json::Number = serde_json::from_str(spelling).unwrap();
        let mut document = JsonDocument::<()>::new(JsonStreamLimits::default(), limits());
        let mut values = Vec::new();
        document
            .feed(JsonStreamInput::Data(spelling.as_bytes()), |_, bytes| {
                values.push(bytes.to_vec());
                Ok(())
            })
            .unwrap();
        document
            .feed(JsonStreamInput::Complete, |_, bytes| {
                values.push(bytes.to_vec());
                Ok(())
            })
            .unwrap();
        assert_eq!(values, [expected.to_string().into_bytes()], "{spelling}");
    }
}

#[test]
fn final_events_include_empty_container_visits_and_child_path_checks() {
    use JsonDocumentEvent::*;
    let document = document(br#"{"b":[],"a":{}}"#, limits()).unwrap();
    assert_eq!(
        document.events().unwrap().collect::<Vec<_>>(),
        [
            Visit {
                path_bytes: 4,
                depth: 0
            },
            ExtendPath { path_bytes: 6 },
            Visit {
                path_bytes: 6,
                depth: 1
            },
            ExtendPath { path_bytes: 6 },
            Visit {
                path_bytes: 6,
                depth: 1
            },
            Scalar {
                path_bytes: 6,
                value_bytes: 1,
                result: 1
            },
        ]
    );
}

#[test]
fn all_independent_limits_accept_exact_charge_and_reject_first_excess() {
    let input = br#"{"z":{"child":["value",true]},"a":{},"z":null}"#;
    let usage = document(input, limits()).unwrap().usage();
    let cases = [
        (JsonDocumentLimit::KeyBytes, usage.peak_key_bytes),
        (JsonDocumentLimit::Members, usage.peak_members),
        (JsonDocumentLimit::Events, usage.peak_events),
        (JsonDocumentLimit::PathBytes, "json.z.child.0".len()),
        (JsonDocumentLimit::Depth, 3),
        (JsonDocumentLimit::Work, usage.work),
    ];
    for (kind, exact) in cases {
        for allowance in [exact, exact - 1] {
            let mut bounds = limits();
            match kind {
                JsonDocumentLimit::RetainedBytes => bounds.max_retained_bytes = allowance,
                JsonDocumentLimit::KeyBytes => bounds.max_key_bytes = allowance,
                JsonDocumentLimit::Members => bounds.max_members = allowance,
                JsonDocumentLimit::Events => bounds.max_events = allowance,
                JsonDocumentLimit::PathBytes => bounds.max_path_bytes = allowance,
                JsonDocumentLimit::Depth => bounds.max_depth = allowance,
                JsonDocumentLimit::Work => bounds.max_work = allowance,
            }
            let result = document(input, bounds);
            if allowance == exact {
                assert!(result.is_ok(), "{kind:?}: {result:?}");
            } else {
                assert_eq!(result.unwrap_err(), JsonDocumentError::Limit { kind, limit: allowance });
            }
        }
    }
    // Geometric growth may become smaller under a tighter cap. Find the first
    // admitted allocation budget rather than treating the generous-cap peak as
    // the minimum required allocation policy.
    let minimum = (0..=usage.peak_retained_bytes)
        .find(|&cap| {
            let mut bounds = limits();
            bounds.max_retained_bytes = cap;
            document(input, bounds).is_ok()
        })
        .unwrap();
    let mut bounds = limits();
    bounds.max_retained_bytes = minimum;
    assert!(document(input, bounds).unwrap().usage().peak_retained_bytes <= minimum);
    bounds.max_retained_bytes -= 1;
    assert_eq!(
        document(input, bounds).unwrap_err(),
        JsonDocumentError::Limit {
            kind: JsonDocumentLimit::RetainedBytes,
            limit: minimum - 1
        }
    );
}

#[test]
fn errors_and_abandonment_are_sticky_and_release_every_owned_capacity() {
    for input in [
        br#"{"private-key":["secret"],"unfinished":"#.as_slice(),
        br#"{"private-key":["secret"]} invalid"#,
    ] {
        let mut document = JsonDocument::<usize>::new(JsonStreamLimits::default(), limits());
        let result = document.feed(JsonStreamInput::Data(input), |_, _| Ok(99));
        let error = result
            .err()
            .unwrap_or_else(|| document.feed(JsonStreamInput::Complete, |_, _| Ok(99)).unwrap_err());
        assert!(document.events().is_none());
        assert_eq!(document.usage().retained_bytes, 0);
        assert_eq!(document.lexical_usage().retained_bytes, 0);
        assert_eq!(
            document.feed(JsonStreamInput::Complete, |_, _| panic!("terminal callback")),
            Err(error)
        );
        let debug = format!("{document:?} {error}");
        assert!(!debug.contains("private-key"));
        assert!(!debug.contains("secret"));
    }
    let mut document = JsonDocument::<()>::new(JsonStreamLimits::default(), limits());
    document
        .feed(JsonStreamInput::Data(br#"{"key":[true,"#), |_, _| Ok(()))
        .unwrap();
    assert!(document.usage().retained_bytes > 0);
    let error = document
        .feed(JsonStreamInput::Abandoned, |_, _| panic!("abandon callback"))
        .unwrap_err();
    assert_eq!(error, JsonDocumentError::Lexical(JsonStreamError::Abandoned));
    assert_eq!(document.usage().retained_bytes, 0);
    assert_eq!(document.usage().events, 0);
    assert_eq!(document.usage().members, 0);
    assert_eq!(document.usage().key_bytes, 0);
    assert_eq!(document.lexical_usage().retained_bytes, 0);
}

#[test]
fn sink_error_is_terminal_but_deferred_result_is_replaceable_and_never_debugged() {
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Secret(&'static str);
    impl fmt::Debug for Secret {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            panic!("must not format result")
        }
    }
    let mut document = JsonDocument::new(JsonStreamLimits::default(), limits());
    document
        .feed(JsonStreamInput::Data(br#"{"a":1,"a":2}"#), |_, value| {
            Ok(Secret(if value == b"1" { "deferred-error" } else { "success" }))
        })
        .unwrap();
    document
        .feed(JsonStreamInput::Complete, |_, _| panic!("no scalar at EOS"))
        .unwrap();
    let events: Vec<_> = document.events().unwrap().collect();
    assert!(events.iter().any(|event| matches!(
        event,
        JsonDocumentEvent::Scalar {
            result: Secret("success"),
            ..
        }
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        JsonDocumentEvent::Scalar {
            result: Secret("deferred-error"),
            ..
        }
    )));
    assert!(!format!("{events:?} {document:?}").contains("success"));
    assert_eq!(
        document.feed(JsonStreamInput::Abandoned, |_, _| panic!("complete callback")),
        Err(JsonDocumentError::AlreadyComplete)
    );
    assert_eq!(document.events().unwrap().count(), events.len());

    let mut document = JsonDocument::<()>::new(JsonStreamLimits::default(), limits());
    assert_eq!(
        document.feed(JsonStreamInput::Data(br#"{"a":1,"a":2}"#), |_, _| Err(
            JsonDocumentSinkError
        )),
        Err(JsonDocumentError::SinkRejected)
    );
    assert_eq!(document.usage().retained_bytes, 0);
    assert_eq!(document.lexical_usage().retained_bytes, 0);
    assert!(document.events().is_none());
}

#[test]
fn duplicate_replacement_reuses_arena_and_closed_keys_are_released() {
    let mut document = JsonDocument::<()>::new(JsonStreamLimits::default(), limits());
    document
        .feed(JsonStreamInput::Data(br#"{"same":{"inner":null}"#), |_, _| Ok(()))
        .unwrap();
    let retained = document.usage().retained_bytes;
    let events = document.usage().events;
    assert_eq!(document.usage().key_bytes, 4);
    assert_eq!(document.usage().members, 1);
    for _ in 0..500 {
        document
            .feed(JsonStreamInput::Data(br#", "same":{"inner":null}"#), |_, _| Ok(()))
            .unwrap();
        assert_eq!(document.usage().retained_bytes, retained);
        assert_eq!(document.usage().events, events);
        assert_eq!(document.usage().members, 1);
        assert_eq!(document.usage().key_bytes, 4);
    }
    document.feed(JsonStreamInput::Data(b"}"), |_, _| Ok(())).unwrap();
    document.feed(JsonStreamInput::Complete, |_, _| Ok(())).unwrap();
    assert_eq!(document.usage().members, 0);
    assert_eq!(document.usage().key_bytes, 0);
    assert_eq!(document.usage().events, events);
}

#[test]
fn reverse_sorted_objects_charge_shifts_and_failed_budget_cleans_up() {
    let input = br#"{"d":0,"c":0,"b":0,"a":0}"#;
    let forward = document(br#"{"a":0,"b":0,"c":0,"d":0}"#, limits()).unwrap().usage();
    let reverse = document(input, limits()).unwrap().usage();
    assert!(reverse.work > forward.work);
    let mut bounds = limits();
    bounds.max_work = forward.work;
    let mut document = JsonDocument::<usize>::new(JsonStreamLimits::default(), bounds);
    assert!(matches!(
        document.feed(JsonStreamInput::Data(input), |_, _| Ok(0)),
        Err(JsonDocumentError::Limit {
            kind: JsonDocumentLimit::Work,
            ..
        })
    ));
    assert_eq!(document.usage().retained_bytes, 0);
    assert_eq!(document.lexical_usage().retained_bytes, 0);
}

#[test]
fn zero_state_limits_and_overwritten_subtree_limits_are_explicit() {
    let mut bounds = limits();
    bounds.max_key_bytes = 0;
    bounds.max_members = 0;
    document(b"{}", bounds).unwrap();
    document(b"[]", bounds).unwrap();
    assert_eq!(
        document(br#"{"":null}"#, bounds).unwrap_err(),
        JsonDocumentError::Limit {
            kind: JsonDocumentLimit::Members,
            limit: 0
        }
    );
    bounds.max_members = 1;
    document(br#"{"":null}"#, bounds).unwrap();
    assert_eq!(
        document(br#"{"a":null}"#, bounds).unwrap_err(),
        JsonDocumentError::Limit {
            kind: JsonDocumentLimit::KeyBytes,
            limit: 0
        }
    );
    for kind in [
        JsonDocumentLimit::RetainedBytes,
        JsonDocumentLimit::Events,
        JsonDocumentLimit::PathBytes,
        JsonDocumentLimit::Work,
    ] {
        let mut bounds = limits();
        match kind {
            JsonDocumentLimit::RetainedBytes => bounds.max_retained_bytes = 0,
            JsonDocumentLimit::Events => bounds.max_events = 0,
            JsonDocumentLimit::PathBytes => bounds.max_path_bytes = 0,
            JsonDocumentLimit::Work => bounds.max_work = 0,
            _ => unreachable!(),
        }
        assert_eq!(
            document(b"{}", bounds).unwrap_err(),
            JsonDocumentError::Limit { kind, limit: 0 }
        );
    }
    let mut bounds = limits();
    bounds.max_depth = 0;
    document(b"null", bounds).unwrap();
    document(b"[]", bounds).unwrap();
    assert_eq!(
        document(b"[null]", bounds).unwrap_err(),
        JsonDocumentError::Limit {
            kind: JsonDocumentLimit::Depth,
            limit: 0
        }
    );
    let mut bounds = limits();
    bounds.max_path_bytes = 6;
    document(br#"{"a":0}"#, bounds).unwrap();
    assert_eq!(
        document(br#"{"a":{"x":0},"a":0}"#, bounds).unwrap_err(),
        JsonDocumentError::Limit {
            kind: JsonDocumentLimit::PathBytes,
            limit: 6
        }
    );
}

#[test]
fn generic_result_storage_is_charged_for_reused_slots_and_allocations() {
    type Fact = [u8; 4096];
    let mut bounds = limits();
    bounds.max_work = size_of::<Slot<Fact>>() - 1;
    let mut document = JsonDocument::<Fact>::new(JsonStreamLimits::default(), bounds);
    assert_eq!(
        document.feed(JsonStreamInput::Data(b"null "), |_, _| panic!(
            "visit budget fails before callback"
        )),
        Err(JsonDocumentError::Limit {
            kind: JsonDocumentLimit::Work,
            limit: bounds.max_work
        })
    );
    assert_eq!(document.usage().retained_bytes, 0);
    let mut bounds = limits();
    bounds.max_retained_bytes = size_of::<Slot<Fact>>() - 1;
    let mut document = JsonDocument::<Fact>::new(JsonStreamLimits::default(), bounds);
    assert_eq!(
        document.feed(JsonStreamInput::Data(b"null "), |_, _| panic!(
            "visit allocation fails before callback"
        )),
        Err(JsonDocumentError::Limit {
            kind: JsonDocumentLimit::RetainedBytes,
            limit: bounds.max_retained_bytes
        })
    );
    assert_eq!(document.usage().retained_bytes, 0);

    let mut document = JsonDocument::<Fact>::new(JsonStreamLimits::default(), limits());
    document
        .feed(JsonStreamInput::Data(br#"{"a":null"#), |_, _| Ok([7; 4096]))
        .unwrap();
    let first = document.usage();
    document
        .feed(JsonStreamInput::Data(br#", "a":null"#), |_, _| Ok([8; 4096]))
        .unwrap();
    let second = document.usage();
    assert_eq!(first.retained_bytes, second.retained_bytes);
    assert_eq!(first.events, second.events);
    assert!(second.work - first.work >= 3 * size_of::<Slot<Fact>>());
}

#[test]
fn retained_usage_equals_all_actual_owned_vector_capacities_after_each_byte() {
    let mut document = JsonDocument::<usize>::new(JsonStreamLimits::default(), limits());
    for &byte in br#"{"z":{"inner":[null,{"empty":{}}]},"a":true,"z":0}"# {
        document.feed(JsonStreamInput::Data(&[byte]), |_, _| Ok(0)).unwrap();
        let reducer = &document.reducer;
        let mut actual = reducer.path.capacity()
            + reducer.stack.capacity() * size_of::<Frame>()
            + reducer.arena.capacity() * size_of::<Slot<usize>>();
        let mut key_bytes = 0;
        let mut member_slots = 0;
        for frame in &reducer.stack {
            if let Container::Object { members, .. } = &frame.container {
                actual += members.capacity() * size_of::<Member>();
                member_slots += members.len();
                for member in members {
                    actual += member.key.capacity();
                    key_bytes += member.key.len();
                }
            }
        }
        assert_eq!(document.usage().retained_bytes, actual);
        assert_eq!(document.usage().key_bytes, key_bytes);
        assert_eq!(document.usage().members, member_slots);
        assert_eq!(
            document.usage().events,
            reducer.arena.iter().filter(|slot| slot.event.is_some()).count()
        );
    }
    document.feed(JsonStreamInput::Complete, |_, _| Ok(0)).unwrap();
    assert_eq!(
        document.usage().retained_bytes,
        document.reducer.arena.capacity() * size_of::<Slot<usize>>()
    );
}
