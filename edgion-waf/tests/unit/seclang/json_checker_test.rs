use super::*;

#[derive(Clone, Copy)]
struct Receipt {
    head: usize,
    tail: usize,
}
struct Fact {
    name: Vec<u8>,
    value: Vec<u8>,
    kind: JsonScalarKind,
    next: Option<usize>,
}
#[derive(Default)]
struct Sink {
    facts: Vec<Option<Fact>>,
    sealed: Vec<(Vec<u8>, Vec<u8>, JsonScalarKind)>,
    summary: Option<JsonSemanticSummary>,
    discarded: usize,
    abandoned: usize,
    callbacks: usize,
    retain: bool,
    failure: Option<&'static str>,
}
impl JsonCheckerSink for Sink {
    type Receipt = Receipt;
    fn scalar(
        &mut self,
        name: &[u8],
        value: &[u8],
        kind: JsonScalarKind,
    ) -> Result<Option<Receipt>, JsonCheckerSinkError> {
        self.callbacks += 1;
        if self.failure == Some("scalar") {
            return Err(JsonCheckerSinkError);
        }
        if !self.retain {
            return Ok(None);
        }
        let id = self.facts.len();
        self.facts.push(Some(Fact {
            name: name.to_vec(),
            value: value.to_vec(),
            kind,
            next: None,
        }));
        // IDs are never reused, satisfying the opaque receipt generation contract.
        Ok(Some(Receipt { head: id, tail: id }))
    }
    fn concat(
        &mut self,
        left: Option<Receipt>,
        right: Option<Receipt>,
    ) -> Result<Option<Receipt>, JsonCheckerSinkError> {
        if self.failure == Some("concat") {
            return Err(JsonCheckerSinkError);
        }
        match (left, right) {
            (Some(left), Some(right)) => {
                self.facts[left.tail].as_mut().unwrap().next = Some(right.head);
                Ok(Some(Receipt {
                    head: left.head,
                    tail: right.tail,
                }))
            }
            (None, value) | (value, None) => Ok(value),
        }
    }
    fn discard(&mut self, receipt: Receipt) -> Result<(), JsonCheckerSinkError> {
        let mut id = Some(receipt.head);
        while let Some(current) = id {
            let fact = self.facts[current].take().expect("receipt must own live facts");
            id = fact.next;
            self.discarded += 1;
            if self.failure == Some("discard") {
                return Err(JsonCheckerSinkError);
            }
        }
        Ok(())
    }
    fn seal(&mut self, receipt: Option<Receipt>, summary: JsonSemanticSummary) -> Result<(), JsonCheckerSinkError> {
        let mut id = receipt.map(|receipt| receipt.head);
        while let Some(current) = id {
            let fact = self.facts[current].take().expect("receipt must own live facts");
            id = fact.next;
            self.sealed.push((fact.name, fact.value, fact.kind));
            if self.failure == Some("seal") {
                return Err(JsonCheckerSinkError);
            }
        }
        if self.failure == Some("seal") {
            return Err(JsonCheckerSinkError);
        }
        self.summary = Some(summary);
        Ok(())
    }
    fn abandon(&mut self) {
        self.abandoned += 1;
        self.facts.clear();
        self.sealed.clear();
        self.summary = None;
    }
}

fn checker(root: &str, limits: JsonStructureLimits) -> JsonChecker<Receipt> {
    JsonChecker::new(root, JsonStreamLimits::default(), limits).unwrap()
}
fn run(input: &[u8], root: &str, retain: bool) -> (JsonChecker<Receipt>, Sink) {
    let mut checker = checker(root, JsonStructureLimits::default());
    let mut sink = Sink {
        retain,
        ..Sink::default()
    };
    checker.feed(JsonStreamInput::Data(input), &mut sink).unwrap();
    checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
    (checker, sink)
}

#[test]
fn duplicates_collisions_and_canonical_order_at_every_split() {
    let input = br#"{"z":0,"a":{"old":["removed"]},"\u0061":{"b":true,"a":null},"a.b":"literal","list":[3,2,1]}"#;
    let (_, baseline) = run(input, "json", true);
    let names: Vec<_> = baseline
        .sealed
        .iter()
        .map(|fact| std::str::from_utf8(&fact.0).unwrap())
        .collect();
    assert_eq!(
        names,
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
    assert_eq!(baseline.discarded, 2);
    assert_eq!(baseline.sealed[1].1, b"true");
    assert_eq!(baseline.sealed[2].1, b"literal");
    for split in 0..=input.len() {
        let mut checker = checker("json", JsonStructureLimits::default());
        let mut sink = Sink {
            retain: true,
            ..Sink::default()
        };
        checker.feed(JsonStreamInput::Data(&input[..split]), &mut sink).unwrap();
        checker.feed(JsonStreamInput::Data(&input[split..]), &mut sink).unwrap();
        assert!(checker.summary().is_none());
        checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
        assert_eq!(sink.sealed, baseline.sealed);
        assert_eq!(sink.summary, baseline.summary);
        assert!(sink.facts.iter().all(Option::is_none));
        assert_eq!(checker.usage().retained_bytes, 0);
    }
}

fn assert_avl(checker: &JsonChecker<Receipt>, root: Option<NodeId>) -> u8 {
    let Some(id) = root else {
        return 0;
    };
    let node = checker.reducer.nodes[id.index()];
    let left = assert_avl(checker, node.left);
    let right = assert_avl(checker, node.right);
    assert!((i16::from(left) - i16::from(right)).abs() <= 1);
    assert_eq!(node.height, left.max(right) + 1);
    node.height
}

#[test]
fn all_rotations_and_closed_child_slot_reuse() {
    for keys in [["a", "b", "c"], ["c", "b", "a"], ["c", "a", "b"], ["a", "c", "b"]] {
        let input = format!("{{\"{}\":0,\"{}\":0,\"{}\":0,", keys[0], keys[1], keys[2]);
        let mut checker = checker("json", JsonStructureLimits::default());
        let mut sink = Sink::default();
        checker
            .feed(JsonStreamInput::Data(input.as_bytes()), &mut sink)
            .unwrap();
        let Container::Object { root, .. } = checker.reducer.frames[0].container else {
            panic!("expected object");
        };
        assert_eq!(assert_avl(&checker, root), 2);
        let node = checker.reducer.nodes[root.unwrap().index()];
        assert_eq!(
            &checker.reducer.keys[node.key.start..node.key.start + node.key.len],
            b"b"
        );
    }
    let mut checker = checker("json", JsonStructureLimits::default());
    let mut sink = Sink::default();
    checker
        .feed(JsonStreamInput::Data(br#"[{"long-first-key":1},"#), &mut sink)
        .unwrap();
    assert_eq!(checker.reducer.keys.len(), 0);
    assert_eq!(checker.reducer.nodes.len(), 0);
    checker
        .feed(JsonStreamInput::Data(br#"{"second":2},{"third":3}]"#), &mut sink)
        .unwrap();
    assert_eq!(checker.reducer.nodes.len(), 0);
    assert_eq!(checker.usage().peak_members, 1);
    checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
}

#[test]
fn summaries_include_empty_visits_and_array_pseudo_values() {
    let (_, empty) = run(b"{}", "json", false);
    assert_eq!(
        empty.summary.unwrap(),
        JsonSemanticSummary {
            traversal_work: 4,
            ..JsonSemanticSummary::default()
        }
    );
    let (_, array) = run(b"[]", "json", false);
    assert_eq!(
        array.summary.unwrap(),
        JsonSemanticSummary {
            argument_count: 1,
            argument_name_bytes: 4,
            argument_value_bytes: 1,
            body_entry_count: 2,
            decoded_bytes: 13,
            combined_size: 5,
            traversal_work: 4,
            max_checked_name_bytes: 4,
            max_value_depth: 0,
        }
    );
    let (_, object) = run(br#"{"empty":{},"list":[]}"#, "json", false);
    let summary = object.summary.unwrap();
    assert_eq!(summary.traversal_work, 4 + 10 + 9);
    assert_eq!(summary.max_checked_name_bytes, 10);
    assert_eq!(summary.max_value_depth, 1);
    assert_eq!(summary.argument_count, 1);
    assert_eq!(summary.argument_name_bytes, 9);
    assert_eq!(summary.argument_value_bytes, 1);
    let (_, replaced) = run(br#"{"long":{"deeper":[1,2]},"long":null}"#, "json", false);
    let (_, direct) = run(br#"{"long":null}"#, "json", false);
    assert_eq!(replaced.summary, direct.summary);
}

#[test]
fn custom_and_empty_roots_preserve_empty_key_segments() {
    let (_, sink) = run(br#"{"":{"x":1},"a":2}"#, "", true);
    assert_eq!(sink.sealed[0].0, b".x");
    assert_eq!(sink.sealed[1].0, b"a");
    let (_, sink) = run(b"[null,true,1e0,-0]", "header", true);
    let values: Vec<_> = sink.sealed.iter().map(|fact| fact.1.as_slice()).collect();
    assert_eq!(values, [b"".as_slice(), b"true", b"1.0", b"-0.0", b"4"]);
    assert_eq!(sink.sealed[4].2, JsonScalarKind::ArrayLength);
    let (_, sink) = run(b"7", "", true);
    assert!(sink.sealed[0].0.is_empty());
}

#[test]
fn summary_only_arrays_do_not_retain_per_element_state() {
    let input = format!("[{}]", vec!["null"; 10_000].join(","));
    let (large, sink) = run(input.as_bytes(), "json", false);
    let (small, _) = run(b"[null]", "json", false);
    assert_eq!(sink.callbacks, 10_001);
    assert!(sink.facts.is_empty());
    assert_eq!(large.usage().peak_members, 0);
    // Only the decimal index scratch path can grow with element count.
    assert!(large.usage().peak_retained_bytes <= small.usage().peak_retained_bytes + 64);
}

#[test]
fn reverse_sorted_and_common_prefix_keys_have_bounded_comparison_work() {
    fn object(count: usize) -> String {
        format!(
            "{{{}}}",
            (0..count)
                .rev()
                .map(|index| format!("\"common-prefix-{index:05}\":0"))
                .collect::<Vec<_>>()
                .join(",")
        )
    }
    let (small, _) = run(object(256).as_bytes(), "json", false);
    let (large, _) = run(object(512).as_bytes(), "json", false);
    assert!(large.usage().work < small.usage().work * 3);
    assert_eq!(large.usage().peak_members, 512);
}

#[test]
fn sticky_errors_abandon_once_even_after_partial_callback_mutation() {
    for (failure, input) in [
        ("scalar", br#"{"a":1}"#.as_slice()),
        ("concat", br#"{"a":1,"b":2}"#.as_slice()),
        ("discard", br#"{"a":[1,2],"a":3}"#.as_slice()),
        ("seal", br#"{"a":1}"#.as_slice()),
    ] {
        let mut checker = checker("json", JsonStructureLimits::default());
        let mut sink = Sink {
            retain: true,
            failure: Some(failure),
            ..Sink::default()
        };
        let result = checker
            .feed(JsonStreamInput::Data(input), &mut sink)
            .and_then(|()| checker.feed(JsonStreamInput::Complete, &mut sink));
        assert_eq!(result, Err(JsonCheckerError::Sink));
        assert_eq!(sink.abandoned, 1);
        assert!(sink.facts.is_empty());
        assert!(sink.sealed.is_empty());
        assert_eq!(checker.usage().retained_bytes, 0);
        assert_eq!(checker.lexical_usage().retained_bytes, 0);
        assert_eq!(checker.feed(JsonStreamInput::Abandoned, &mut sink), result);
        assert_eq!(sink.abandoned, 1);
    }
    for input in [b"".as_slice(), br#"{"a":1} trailing"#, br#"{"a":1,"a":"\q"}"#] {
        let mut checker = checker("json", JsonStructureLimits::default());
        let mut sink = Sink {
            retain: true,
            ..Sink::default()
        };
        assert!(checker
            .feed(JsonStreamInput::Data(input), &mut sink)
            .and_then(|()| checker.feed(JsonStreamInput::Complete, &mut sink))
            .is_err());
        assert_eq!(sink.abandoned, 1);
        assert!(checker.summary().is_none());
    }
}

#[test]
fn physical_limits_are_independent_and_cleanup_ignores_exhausted_work() {
    for (kind, limits, input) in [
        (
            JsonStructureLimit::Members,
            JsonStructureLimits {
                max_members: 1,
                ..JsonStructureLimits::default()
            },
            br#"{"a":1,"b":2}"#.as_slice(),
        ),
        (
            JsonStructureLimit::KeyBytes,
            JsonStructureLimits {
                max_key_bytes: 1,
                ..JsonStructureLimits::default()
            },
            br#"{"ab":1}"#.as_slice(),
        ),
        (
            JsonStructureLimit::Depth,
            JsonStructureLimits {
                max_depth: 0,
                ..JsonStructureLimits::default()
            },
            b"[0]".as_slice(),
        ),
        (
            JsonStructureLimit::PathBytes,
            JsonStructureLimits {
                max_path_bytes: 4,
                ..JsonStructureLimits::default()
            },
            br#"{"a":1}"#.as_slice(),
        ),
        (
            JsonStructureLimit::RetainedBytes,
            JsonStructureLimits {
                max_retained_bytes: 4,
                ..JsonStructureLimits::default()
            },
            b"[]".as_slice(),
        ),
        (
            JsonStructureLimit::Work,
            JsonStructureLimits {
                max_work: 4,
                ..JsonStructureLimits::default()
            },
            b"null".as_slice(),
        ),
    ] {
        let mut checker = checker("json", limits);
        let mut sink = Sink::default();
        assert!(
            matches!(checker.feed(JsonStreamInput::Data(input), &mut sink), Err(JsonCheckerError::Limit { kind: actual, .. }) if actual == kind)
        );
        assert_eq!(sink.abandoned, 1);
        assert_eq!(checker.usage().retained_bytes, 0);
    }
    let mut checker = checker("json", JsonStructureLimits::default());
    let mut sink = Sink::default();
    checker.feed(JsonStreamInput::Data(br#"{"a":1,"#), &mut sink).unwrap();
    assert_eq!(
        checker.feed(JsonStreamInput::Abandoned, &mut sink),
        Err(JsonCheckerError::Lexical(JsonStreamError::Abandoned))
    );
    assert_eq!(sink.abandoned, 1);
}

#[test]
fn successful_completion_is_terminal_without_abandoning_published_receipts() {
    let (mut checker, mut sink) = run(b"1", "json", true);
    assert_eq!(
        checker.feed(JsonStreamInput::Complete, &mut sink),
        Err(JsonCheckerError::AlreadyComplete)
    );
    assert_eq!(sink.abandoned, 0);
    assert_eq!(sink.sealed.len(), 1);
}

#[test]
fn physical_limit_boundaries_are_inclusive() {
    for (kind, required, input) in [
        (JsonStructureLimit::Members, 2, br#"{"a":1,"b":2}"#.as_slice()),
        (JsonStructureLimit::KeyBytes, 2, br#"{"ab":1}"#.as_slice()),
        (JsonStructureLimit::Depth, 2, br#"{"a":{"b":0}}"#.as_slice()),
        (JsonStructureLimit::PathBytes, 6, br#"{"a":1}"#.as_slice()),
        (
            JsonStructureLimit::RetainedBytes,
            size_of::<Frame<Receipt>>(),
            b"[]".as_slice(),
        ),
    ] {
        for limit in [required - 1, required, required + 1] {
            let mut limits = JsonStructureLimits::default();
            match kind {
                JsonStructureLimit::Members => limits.max_members = limit,
                JsonStructureLimit::KeyBytes => limits.max_key_bytes = limit,
                JsonStructureLimit::Depth => limits.max_depth = limit,
                JsonStructureLimit::PathBytes => limits.max_path_bytes = limit,
                JsonStructureLimit::RetainedBytes => limits.max_retained_bytes = limit,
                _ => unreachable!(),
            }
            let mut sink = Sink::default();
            let result =
                JsonChecker::<Receipt>::new("json", JsonStreamLimits::default(), limits).and_then(|mut checker| {
                    checker.feed(JsonStreamInput::Data(input), &mut sink)?;
                    checker.feed(JsonStreamInput::Complete, &mut sink)
                });
            if limit < required {
                assert!(matches!(result, Err(JsonCheckerError::Limit { kind: actual, .. }) if actual == kind));
            } else {
                assert_eq!(result, Ok(()));
            }
        }
    }
    let (baseline, _) = run(b"12345", "json", false);
    let required = baseline.usage().work;
    for limit in [required - 1, required, required + 1] {
        let mut checker = checker(
            "json",
            JsonStructureLimits {
                max_work: limit,
                ..JsonStructureLimits::default()
            },
        );
        let mut sink = Sink::default();
        let result = checker
            .feed(JsonStreamInput::Data(b"12345"), &mut sink)
            .and_then(|()| checker.feed(JsonStreamInput::Complete, &mut sink));
        assert_eq!(result.is_ok(), limit >= required);
    }
}

#[test]
fn nested_long_keys_restore_all_arena_marks() {
    let key = "long-key-".repeat(10);
    let mut input = String::new();
    for _ in 0..48 {
        input.push_str(&format!("{{\"{key}\":"));
    }
    input.push_str("null");
    for _ in 0..48 {
        input.push('}');
    }
    let mut checker = checker("json", JsonStructureLimits::default());
    let mut sink = Sink::default();
    checker
        .feed(JsonStreamInput::Data(input.as_bytes()), &mut sink)
        .unwrap();
    assert_eq!(checker.usage().peak_members, 48);
    assert_eq!(checker.usage().peak_key_bytes, 48 * key.len());
    assert!(checker.reducer.keys.is_empty());
    assert_eq!(checker.reducer.path.as_slice(), b"json");
    assert_eq!(checker.usage().members, 0);
    checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
    assert_eq!(sink.summary.unwrap().max_value_depth, 48);
}

#[test]
fn custom_root_constructor_enforces_exact_path_and_retained_boundaries() {
    let root_text = "r".repeat(257);
    let root = root_text.as_str();
    let retained = root.len();
    for (capacity, ok) in [(retained, true), (retained - 1, false)] {
        let result = JsonChecker::<Receipt>::new(
            root,
            JsonStreamLimits::default(),
            JsonStructureLimits {
                max_retained_bytes: capacity,
                max_path_bytes: root.len(),
                ..JsonStructureLimits::default()
            },
        );
        assert_eq!(result.is_ok(), ok);
    }
    assert!(JsonChecker::<Receipt>::new(
        root,
        JsonStreamLimits::default(),
        JsonStructureLimits {
            max_path_bytes: root.len() - 1,
            ..JsonStructureLimits::default()
        }
    )
    .is_err());
    let (_, sink) = run(br#"{"":{"name":"alice"}}"#, "header.payload", true);
    assert_eq!(sink.sealed[0].0, b"header.payload..name");
}

#[test]
fn generated_nested_duplicates_match_test_only_dom_order() {
    fn flatten(value: &serde_json::Value, path: &str, out: &mut Vec<(Vec<u8>, Vec<u8>)>) {
        match value {
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    flatten(value, &format!("{path}.{key}"), out);
                }
            }
            serde_json::Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    flatten(value, &format!("{path}.{index}"), out);
                }
                out.push((path.as_bytes().to_vec(), values.len().to_string().into_bytes()));
            }
            value => out.push((
                path.as_bytes().to_vec(),
                match value {
                    serde_json::Value::Null => Vec::new(),
                    serde_json::Value::String(value) => value.as_bytes().to_vec(),
                    value => value.to_string().into_bytes(),
                },
            )),
        }
    }
    for seed in 0..48 {
        let input = format!(
            r#"{{"z":{{"old":[1,2,3]}},"a.b":null,"a":{{"b":[true,"é",{seed}]}},"z":{{"b":false,"a":[{{"x":1,"x":2}},[]]}}}}"#
        );
        let value: serde_json::Value = serde_json::from_str(&input).unwrap();
        let mut expected = Vec::new();
        flatten(&value, "json", &mut expected);
        for split in [0, 1, input.len() / 2, input.len() - 1, input.len()] {
            let mut checker = checker("json", JsonStructureLimits::default());
            let mut sink = Sink {
                retain: true,
                ..Sink::default()
            };
            checker
                .feed(JsonStreamInput::Data(&input.as_bytes()[..split]), &mut sink)
                .unwrap();
            checker
                .feed(JsonStreamInput::Data(&input.as_bytes()[split..]), &mut sink)
                .unwrap();
            checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
            let actual: Vec<_> = sink.sealed.into_iter().map(|(key, value, _)| (key, value)).collect();
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn child_scope_rewinds_nodes_without_invalidating_parent_pending_member() {
    let mut checker = checker("json", JsonStructureLimits::default());
    let mut sink = Sink {
        retain: true,
        ..Sink::default()
    };
    checker
        .feed(JsonStreamInput::Data(br#"{"parent":{"a":1,"b":2},"#), &mut sink)
        .unwrap();
    assert_eq!(checker.reducer.nodes.len(), 1);
    assert_eq!(checker.usage().members, 1);
    assert_eq!(checker.reducer.keys.as_slice(), b"parent");
    checker
        .feed(
            JsonStreamInput::Data(br#""other":[{"x":3},{"y":4}],"parent":5}"#),
            &mut sink,
        )
        .unwrap();
    assert!(checker.reducer.nodes.is_empty());
    assert!(checker.reducer.keys.is_empty());
    assert_eq!(sink.discarded, 2);
    checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
    assert_eq!(sink.sealed.last().unwrap().0, b"json.parent");
    assert_eq!(sink.sealed.last().unwrap().1, b"5");
}

#[test]
fn sibling_scopes_reuse_identical_capacity_and_peak_live_state() {
    let mut checker = checker("json", JsonStructureLimits::default());
    let mut sink = Sink::default();
    checker
        .feed(JsonStreamInput::Data(br#"[{"field":1}"#), &mut sink)
        .unwrap();
    let capacity = (checker.reducer.nodes.capacity(), heap_capacity(&checker.reducer.keys));
    let peak = (checker.usage().peak_members, checker.usage().peak_key_bytes);
    for _ in 0..1000 {
        checker
            .feed(JsonStreamInput::Data(br#",{"field":2}"#), &mut sink)
            .unwrap();
        assert_eq!(checker.reducer.nodes.len(), 0);
        assert_eq!(checker.reducer.keys.len(), 0);
        assert_eq!(
            (checker.reducer.nodes.capacity(), heap_capacity(&checker.reducer.keys)),
            capacity
        );
        assert_eq!((checker.usage().peak_members, checker.usage().peak_key_bytes), peak);
    }
    checker.feed(JsonStreamInput::Data(b"]"), &mut sink).unwrap();
    checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
}

#[test]
fn source_hints_are_lazy_bounded_and_disabled_when_bounds_are_unproven() {
    let limits = JsonStructureLimits::default();
    let mut checker =
        JsonChecker::<Receipt>::with_source_len("json", JsonStreamLimits::default(), limits, 1024).unwrap();
    assert_eq!(checker.reducer.hints.nodes, 32);
    assert_eq!(checker.reducer.nodes.capacity(), 0);
    assert_eq!(heap_capacity(&checker.reducer.keys), 0);
    let mut sink = Sink::default();
    checker
        .feed(JsonStreamInput::Data(br#"{"abcdef":0,"#), &mut sink)
        .unwrap();
    assert_eq!(checker.reducer.nodes.capacity(), 32);
    assert_eq!(heap_capacity(&checker.reducer.keys), 0);
    for len in [0, usize::MAX] {
        let hint = CapacityHints::for_source::<Receipt>(len, 4, limits);
        assert_eq!(hint.nodes, 0);
    }
    let hint = CapacityHints::for_source::<Receipt>(
        1024,
        4,
        JsonStructureLimits {
            max_retained_bytes: 4096,
            ..limits
        },
    );
    assert_eq!(hint.nodes, 0);
    for source in [b"null".as_slice(), b"{}", br#"{"":0}"#] {
        let mut checker =
            JsonChecker::<Receipt>::with_source_len("json", JsonStreamLimits::default(), limits, source.len()).unwrap();
        checker
            .feed(JsonStreamInput::Data(source), &mut Sink::default())
            .unwrap();
        assert_eq!(heap_capacity(&checker.reducer.keys), 0);
        if source != br#"{"":0}"# {
            assert_eq!(checker.reducer.nodes.capacity(), 0);
        }
    }
}

#[test]
fn hinted_scanning_preserves_results_for_uniform_keys() {
    let input = format!(
        "{{{}}}",
        (0..100)
            .map(|i| format!("\"key{i:03}\":{i}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let (plain, plain_sink) = run(input.as_bytes(), "json", true);
    let mut hinted = JsonChecker::<Receipt>::with_source_len(
        "json",
        JsonStreamLimits::default(),
        JsonStructureLimits {
            max_work: plain.usage().work,
            ..JsonStructureLimits::default()
        },
        input.len(),
    )
    .unwrap();
    let mut sink = Sink {
        retain: true,
        ..Sink::default()
    };
    hinted.feed(JsonStreamInput::Data(input.as_bytes()), &mut sink).unwrap();
    hinted.feed(JsonStreamInput::Complete, &mut sink).unwrap();
    assert_eq!(hinted.summary(), plain.summary());
    assert_eq!(sink.sealed, plain_sink.sealed);
    assert!(hinted.usage().work <= plain.usage().work);
}

#[test]
fn tight_limits_and_malformed_prefixes_do_not_gain_speculative_allocations() {
    for source in [b"null".as_slice(), br#"{"a":0}"#, b"[[[[[[", br#"{"a":{"b":0}"#] {
        for retained in [0, 4, 1024, 4096] {
            let limits = JsonStructureLimits {
                max_retained_bytes: retained,
                ..JsonStructureLimits::default()
            };
            let inspect = |hint: bool| {
                let mut sink = Sink::default();
                let checker = if hint {
                    JsonChecker::<Receipt>::with_source_len("json", JsonStreamLimits::default(), limits, source.len())
                } else {
                    JsonChecker::new("json", JsonStreamLimits::default(), limits)
                };
                let result = checker.and_then(|mut checker| {
                    checker.feed(JsonStreamInput::Data(source), &mut sink)?;
                    checker.feed(JsonStreamInput::Complete, &mut sink)
                });
                (result, sink.abandoned)
            };
            assert_eq!(inspect(true), inspect(false));
        }
    }
}

#[test]
fn variable_key_lengths_keep_actual_copy_work_bounded() {
    let input = format!(
        "{{{}}}",
        [6, 300, 100, 300, 400]
            .into_iter()
            .enumerate()
            .map(|(i, len)| format!("\"{}{}\":0", i, "x".repeat(len - 1)))
            .collect::<Vec<_>>()
            .join(",")
    );
    let limits = JsonStructureLimits {
        max_members: 5,
        ..JsonStructureLimits::default()
    };
    let inspect = |hint, limits| {
        let mut checker = if hint {
            JsonChecker::<Receipt>::with_source_len("json", JsonStreamLimits::default(), limits, input.len()).unwrap()
        } else {
            checker("json", limits)
        };
        let mut sink = Sink::default();
        let result = checker
            .feed(JsonStreamInput::Data(input.as_bytes()), &mut sink)
            .and_then(|()| checker.feed(JsonStreamInput::Complete, &mut sink));
        (result, checker.usage(), checker.summary(), sink.abandoned)
    };
    let plain = inspect(false, limits);
    let hinted = inspect(true, limits);
    assert!(plain.0.is_ok() && hinted.0.is_ok());
    assert_eq!(plain.2, hinted.2);
    assert_eq!(hinted.1.work, plain.1.work);
    let bounded = inspect(
        true,
        JsonStructureLimits {
            max_work: plain.1.work - 1,
            ..limits
        },
    );
    assert!(matches!(
        bounded.0,
        Err(JsonCheckerError::Limit {
            kind: JsonStructureLimit::Work,
            ..
        })
    ));
    assert_eq!(bounded.3, 1);
}

#[test]
fn small_wide_object_hints_reduce_buffer_growth_events() {
    let parts: Vec<String> = (0..32)
        .map(|i| format!("{}\"key{i:03}\":\"{}\"", if i == 0 { "{" } else { "," }, "x".repeat(16)))
        .collect();
    let source_len = parts.iter().map(String::len).sum::<usize>() + 1;
    let inspect = |hint| {
        let limits = JsonStructureLimits::default();
        let mut checker = if hint {
            JsonChecker::<Receipt>::with_source_len("json", JsonStreamLimits::default(), limits, source_len).unwrap()
        } else {
            checker("json", limits)
        };
        let mut sink = Sink::default();
        let mut previous = (0, 0);
        let mut growth = (0, 0);
        for part in &parts {
            checker.feed(JsonStreamInput::Data(part.as_bytes()), &mut sink).unwrap();
            let capacity = (checker.reducer.nodes.capacity(), heap_capacity(&checker.reducer.keys));
            growth.0 += usize::from(capacity.0 != previous.0);
            growth.1 += usize::from(capacity.1 != previous.1);
            previous = capacity;
        }
        checker.feed(JsonStreamInput::Data(b"}"), &mut sink).unwrap();
        checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
        (growth, previous, checker.summary())
    };
    let plain = inspect(false);
    let hinted = inspect(true);
    assert_eq!(plain.0, (4, 0));
    assert_eq!(hinted.0, (1, 0));
    assert_eq!(plain.1, (32, 0));
    assert_eq!(hinted.1, (32, 0));
    assert_eq!(plain.2, hinted.2);
}

#[test]
fn inline_key_and_path_boundaries_preserve_parent_scopes_after_spill() {
    for len in [255, 256, 257] {
        let key = "k".repeat(len);
        let input = format!("{{\"{key}\":1}}");
        let mut checker = checker("", JsonStructureLimits::default());
        checker
            .feed(JsonStreamInput::Data(input.as_bytes()), &mut Sink::default())
            .unwrap();
        assert_eq!(heap_capacity(&checker.reducer.keys) > 0, len > 256);
        assert_eq!(heap_capacity(&checker.reducer.path) > 0, len > 256);
        assert!(checker.reducer.keys.is_empty());
        assert!(checker.reducer.path.is_empty());
    }
    let long = "k".repeat(257);
    let mut checker = checker("json", JsonStructureLimits::default());
    let mut sink = Sink {
        retain: true,
        ..Sink::default()
    };
    checker
        .feed(
            JsonStreamInput::Data(format!("{{\"parent\":{{\"{long}\":1}},").as_bytes()),
            &mut sink,
        )
        .unwrap();
    let capacities = (
        heap_capacity(&checker.reducer.keys),
        heap_capacity(&checker.reducer.path),
    );
    assert!(capacities.0 > 256 && capacities.1 > 256);
    for _ in 0..100 {
        checker
            .feed(JsonStreamInput::Data(br#""child":{"x":0},"#), &mut sink)
            .unwrap();
        assert_eq!(
            (
                heap_capacity(&checker.reducer.keys),
                heap_capacity(&checker.reducer.path)
            ),
            capacities
        );
    }
    checker
        .feed(JsonStreamInput::Data(br#""parent":2}"#), &mut sink)
        .unwrap();
    checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
    assert!(sink
        .sealed
        .iter()
        .any(|(name, value, _)| name == b"json.parent" && value == b"2"));
    assert!(!sink.sealed.iter().any(|(name, _, _)| name.starts_with(b"json.parent.")));
    assert_eq!(heap_capacity(&checker.reducer.keys), 0);
    assert_eq!(heap_capacity(&checker.reducer.path), 0);
}

#[test]
fn inline_root_is_free_but_spill_charges_entire_heap_and_existing_storage() {
    for len in [255, 256, 257] {
        let root = "r".repeat(len);
        let result = JsonChecker::<Receipt>::new(
            &root,
            JsonStreamLimits::default(),
            JsonStructureLimits {
                max_retained_bytes: 0,
                ..JsonStructureLimits::default()
            },
        );
        assert_eq!(result.is_ok(), len <= 256);
    }
    let mut budget = Budget {
        limits: JsonStructureLimits {
            max_retained_bytes: 300,
            ..JsonStructureLimits::default()
        },
        usage: JsonStructureUsage::default(),
    };
    let mut nodes = Vec::<u8>::new();
    budget.reserve(&mut nodes, 44).unwrap();
    let mut bytes = SmallVec::<[u8; 256]>::new();
    bytes.extend_from_slice(&[0; 256]);
    assert!(matches!(
        budget.reserve_bytes(&mut bytes, 1),
        Err(JsonCheckerError::Limit {
            kind: JsonStructureLimit::RetainedBytes,
            ..
        })
    ));
    assert_eq!(heap_capacity(&bytes), 0);
    budget.limits.max_retained_bytes = 301;
    budget.reserve_bytes(&mut bytes, 1).unwrap();
    assert_eq!(heap_capacity(&bytes), 257);
    assert_eq!(budget.usage.retained_bytes, 301);
}

#[test]
fn fixed_inline_parser_layout_is_reported_separately_from_heap_usage() {
    let checker = checker("json", JsonStructureLimits::default());
    assert_eq!(checker.usage().retained_bytes, 0);
    assert_eq!(checker.lexical_usage().retained_bytes, 0);
    println!(
        "json_inline_layout,checker={},stream={},key_buffer={},token_buffer={},member={},frame={}",
        size_of::<JsonChecker<Receipt>>(),
        size_of::<JsonStream>(),
        size_of::<SmallVec<[u8; 256]>>(),
        size_of::<SmallVec<[u8; 1024]>>(),
        size_of::<Member<Receipt>>(),
        size_of::<Frame<Receipt>>()
    );
}

#[test]
fn node_links_use_one_word_and_reject_index_overflow() {
    assert_eq!(size_of::<Option<NodeId>>(), size_of::<usize>());
    for index in [0, 1, 65_535, usize::MAX - 1] {
        assert_eq!(NodeId::new(index).unwrap().index(), index);
    }
    assert_eq!(NodeId::new(usize::MAX), Err(JsonCheckerError::Arithmetic));
}

#[test]
fn wide_nonroot_duplicates_preserve_links_heights_scopes_and_sorted_results() {
    let prefix = format!(
        "{{{},",
        (0..256)
            .map(|i| format!("\"k{i:03}\":\"old\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut checker = checker("json", JsonStructureLimits::default());
    let mut sink = Sink {
        retain: true,
        ..Sink::default()
    };
    checker
        .feed(JsonStreamInput::Data(prefix.as_bytes()), &mut sink)
        .unwrap();
    let Container::Object { root: Some(root), .. } = checker.reducer.frames[0].container else {
        panic!("expected open object");
    };
    let topology = |checker: &JsonChecker<Receipt>| {
        checker
            .reducer
            .nodes
            .iter()
            .map(|node| (node.key.start, node.key.len, node.left, node.right, node.height))
            .collect::<Vec<_>>()
    };
    let original = topology(&checker);
    let original_keys = checker.reducer.keys.to_vec();
    let original_retained = checker.usage().retained_bytes;
    let root_key = checker.reducer.nodes[root.index()].key;
    let root_name = original_keys[root_key.start..root_key.start + root_key.len].to_vec();
    let mut source = prefix;
    for i in 0..2048 {
        let mut key = format!("k{:03}", (i * 17) % 256);
        if key.as_bytes() == root_name {
            key = "k000".into();
            if key.as_bytes() == root_name {
                key = "k255".into();
            }
        }
        let replacement = format!("\"{key}\":\"final\"{}", if i == 2047 { "}" } else { "," });
        checker
            .feed(JsonStreamInput::Data(replacement.as_bytes()), &mut sink)
            .unwrap();
        source.push_str(&replacement);
        if i != 2047 {
            assert_eq!(topology(&checker), original);
            assert_eq!(checker.reducer.keys.as_slice(), original_keys);
            assert_eq!(checker.usage().members, 256);
            assert_eq!(checker.usage().retained_bytes, original_retained);
            assert_avl(&checker, Some(root));
        }
    }
    assert_eq!(sink.discarded, 2048);
    checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
    let canonical = serde_json::from_str::<serde_json::Value>(&source).unwrap().to_string();
    let (direct, expected) = run(canonical.as_bytes(), "json", true);
    assert_eq!(sink.sealed, expected.sealed);
    assert_eq!(checker.summary(), direct.summary());
    assert_eq!(checker.usage().peak_members, 256);
    assert_eq!(checker.usage().retained_bytes, 0);
    let work = checker.usage().work;
    for cap in [work - 1, work, work + 1] {
        let mut checker = self::checker(
            "json",
            JsonStructureLimits {
                max_work: cap,
                ..JsonStructureLimits::default()
            },
        );
        let mut sink = Sink {
            retain: true,
            ..Sink::default()
        };
        let result = checker
            .feed(JsonStreamInput::Data(source.as_bytes()), &mut sink)
            .and_then(|()| checker.feed(JsonStreamInput::Complete, &mut sink));
        assert_eq!(result.is_ok(), cap >= work);
        if cap < work {
            assert_eq!(
                result,
                Err(JsonCheckerError::Limit {
                    kind: JsonStructureLimit::Work,
                    limit: cap,
                })
            );
            assert_eq!(sink.abandoned, 1);
            assert_eq!(checker.feed(JsonStreamInput::Complete, &mut sink), result);
            assert_eq!(sink.abandoned, 1);
            assert_eq!(checker.usage().retained_bytes, 0);
        } else {
            assert_eq!(sink.sealed, expected.sealed);
            assert_eq!(checker.usage().work, work);
        }
    }
}

#[test]
fn nonroot_decoded_duplicates_replace_nested_receipts_and_fail_stickily() {
    let prefix = format!(
        "{{{},\"k000\":{{\"old\":[1,2]}},",
        (0..32).map(|i| format!("\"k{i:03}\":0")).collect::<Vec<_>>().join(",")
    );
    let replacement = br#""\u006b000":null,"k000":{"new":true}}"#;
    let source = [prefix.as_bytes(), replacement].concat();
    for split in [0, prefix.len(), prefix.len() + 4, source.len() - 1, source.len()] {
        let mut checker = checker("json", JsonStructureLimits::default());
        let mut sink = Sink {
            retain: true,
            ..Sink::default()
        };
        checker
            .feed(JsonStreamInput::Data(&source[..split]), &mut sink)
            .unwrap();
        checker
            .feed(JsonStreamInput::Data(&source[split..]), &mut sink)
            .unwrap();
        checker.feed(JsonStreamInput::Complete, &mut sink).unwrap();
        assert_eq!(sink.sealed[0].0, b"json.k000.new");
        assert_eq!(sink.sealed[0].1, b"true");
        assert_eq!(sink.discarded, 5);
    }
    for (tail, failure, expected) in [
        (
            br#""\u006b000":"\q"}"#.as_slice(),
            None,
            JsonCheckerError::Lexical(JsonStreamError::Escape),
        ),
        (replacement.as_slice(), Some("discard"), JsonCheckerError::Sink),
    ] {
        let mut checker = checker("json", JsonStructureLimits::default());
        let mut sink = Sink {
            retain: true,
            ..Sink::default()
        };
        checker
            .feed(JsonStreamInput::Data(prefix.as_bytes()), &mut sink)
            .unwrap();
        sink.failure = failure;
        assert_eq!(checker.feed(JsonStreamInput::Data(tail), &mut sink), Err(expected));
        assert_eq!(checker.feed(JsonStreamInput::Complete, &mut sink), Err(expected));
        assert_eq!(checker.feed(JsonStreamInput::Abandoned, &mut sink), Err(expected));
        assert_eq!(sink.abandoned, 1);
        assert!(sink.facts.is_empty());
        assert_eq!(checker.usage().retained_bytes, 0);
        assert_eq!(checker.lexical_usage().retained_bytes, 0);
    }
}

#[test]
fn array_decimal_boundaries_preserve_paths_lengths_and_work_limits() {
    for length in [0, 1, 9, 10, 11, 99, 100, 101] {
        let source = format!("[{}]", vec!["null"; length].join(","));
        let (baseline, expected) = run(source.as_bytes(), "json", true);
        assert_eq!(expected.sealed.len(), length + 1);
        for (index, fact) in expected.sealed[..length].iter().enumerate() {
            assert_eq!(fact.0, format!("json.{index}").as_bytes());
            assert!(fact.1.is_empty());
            assert_eq!(fact.2, JsonScalarKind::Null);
        }
        assert_eq!(expected.sealed[length].0, b"json");
        assert_eq!(expected.sealed[length].1, length.to_string().as_bytes());
        assert_eq!(expected.sealed[length].2, JsonScalarKind::ArrayLength);
        let work = baseline.usage().work;
        for chunk in [1, 7, source.len()] {
            for limit in [work - 1, work] {
                let mut checker = checker(
                    "json",
                    JsonStructureLimits {
                        max_work: limit,
                        ..JsonStructureLimits::default()
                    },
                );
                let mut sink = Sink {
                    retain: true,
                    ..Sink::default()
                };
                let result = source
                    .as_bytes()
                    .chunks(chunk)
                    .try_for_each(|bytes| checker.feed(JsonStreamInput::Data(bytes), &mut sink))
                    .and_then(|()| checker.feed(JsonStreamInput::Complete, &mut sink));
                if limit == work {
                    result.unwrap();
                    assert_eq!(sink.sealed, expected.sealed);
                    assert_eq!(sink.summary, expected.summary);
                    assert_eq!(checker.usage().work, work);
                } else {
                    assert_eq!(
                        result,
                        Err(JsonCheckerError::Limit {
                            kind: JsonStructureLimit::Work,
                            limit,
                        })
                    );
                    assert_eq!(sink.abandoned, 1);
                }
            }
        }
    }
}

#[test]
fn maximum_array_decimal_index_and_length_use_complete_digits() {
    for length in [0, 9, 10, 99, 100, usize::MAX] {
        let digits = length.to_string();
        let mut index = checker("json", JsonStructureLimits::default());
        let mut sink = Sink {
            retain: true,
            ..Sink::default()
        };
        index.feed(JsonStreamInput::Data(b"["), &mut sink).unwrap();
        index.reducer.frames.last_mut().unwrap().container = Container::Array { length };
        let before = index.usage().work;
        let (restore, _) = index.reducer.begin().unwrap();
        assert_eq!(restore, 4);
        assert_eq!(index.reducer.path.as_slice(), format!("json.{length}").as_bytes());
        assert_eq!(index.usage().work - before, 6 + 3 * digits.len());

        let mut count = checker("json", JsonStructureLimits::default());
        let mut sink = Sink {
            retain: true,
            ..Sink::default()
        };
        count.feed(JsonStreamInput::Data(b"["), &mut sink).unwrap();
        count.reducer.frames.last_mut().unwrap().container = Container::Array { length };
        count.feed(JsonStreamInput::Data(b"]"), &mut sink).unwrap();
        count.feed(JsonStreamInput::Complete, &mut sink).unwrap();
        assert_eq!(
            sink.sealed,
            vec![(b"json".to_vec(), digits.into_bytes(), JsonScalarKind::ArrayLength)]
        );
    }
}
