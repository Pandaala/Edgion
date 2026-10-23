use super::*;

fn dfa(pattern: &str) -> Arc<dense::DFA<Vec<u32>>> {
    Arc::new(dense::Builder::new().build(pattern).unwrap())
}

#[test]
fn regex_stops_after_unanchored_match() {
    let mut scanner = StreamingRegexScanner::new(dfa("needle")).unwrap();
    assert_eq!(scanner.push("prefix nee").unwrap(), ScanControl::Continue);
    assert_eq!(scanner.push("dle suffix").unwrap(), ScanControl::Stop);
    assert!(scanner.finish().unwrap());
}

#[test]
fn regex_eoi_resolves_end_anchor() {
    let mut scanner = StreamingRegexScanner::new(dfa("needle$")).unwrap();
    assert_eq!(scanner.push("needle").unwrap(), ScanControl::Continue);
    assert!(scanner.finish().unwrap());

    let mut scanner = StreamingRegexScanner::new(dfa("needle$")).unwrap();
    assert_eq!(scanner.push("needle!").unwrap(), ScanControl::Continue);
    assert!(!scanner.finish().unwrap());
}

#[test]
fn regex_empty_pattern_matches() {
    let scanner = StreamingRegexScanner::new(dfa("")).unwrap();
    assert!(scanner.finish().unwrap());
}

async fn scan_json(input: &[&[u8]], path: &str, limit: usize) -> JsonScanValue {
    let path = DefiniteJsonPath::parse(path).unwrap();
    let mut scanner = JsonPathScanner::new(path, limit).await.unwrap();
    for chunk in input {
        if scanner.push(chunk).unwrap() == ScanControl::Stop {
            break;
        }
    }
    scanner.finish().unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn owned_json_path_scan_preserves_last_wins_limits_and_invalid_input() {
    let path = DefiniteJsonPath::parse("$.value").unwrap();
    let result = JsonPathScanner::scan_owned(Arc::from(r#"{"value":"old","value":"new"}"#), path.clone(), 8)
        .await
        .unwrap();
    assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == "new"));

    let result = JsonPathScanner::scan_owned(Arc::from(r#"{"value":"too long for limit"}"#), path.clone(), 8)
        .await
        .unwrap();
    assert!(matches!(result, JsonScanValue::TooLong));

    let result = JsonPathScanner::scan_owned(Arc::from(r#"{"value":1} trailing"#), path, 8)
        .await
        .unwrap();
    assert!(matches!(result, JsonScanValue::Missing));
}

async fn scan_json_select(
    input: &[&[u8]],
    entries: &[(&str, &str)],
    limit: usize,
) -> (Arc<JsonSelectPlan>, Vec<JsonScanValue>) {
    let plan = Arc::new(JsonSelectPlan::new(entries.iter().map(|(name, path)| {
        (
            (*name).to_string(),
            DefiniteJsonPath::parse(path).expect("test path should be valid"),
        )
    })));
    let mut scanner = JsonSelectScanner::new(Arc::clone(&plan), limit).await.unwrap();
    for chunk in input {
        if scanner.push(chunk).unwrap() == ScanControl::Stop {
            break;
        }
    }
    let values = scanner.finish().unwrap();
    (plan, values)
}

fn selected<'a>(plan: &JsonSelectPlan, values: &'a [JsonScanValue], name: &str) -> &'a JsonScanValue {
    let (_, capture) = plan
        .outputs()
        .iter()
        .find(|(output, _)| output.as_ref() == name)
        .expect("output should exist");
    &values[*capture]
}

#[tokio::test]
async fn json_selects_across_chunks_and_last_duplicate_wins() {
    let result = scan_json(
        &[br#"{"a":{"b":"old"},"a":{"b":"ne"# as &[u8], br#"w"},"tail":true}"#],
        "$.a.b",
        32,
    )
    .await;
    assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == "new"));
}

#[tokio::test]
async fn oversized_earlier_duplicate_does_not_mask_later_value() {
    let result = scan_json(&[br#"{"a":"far too large","a":"ok"}"#], "$.a", 2).await;
    assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == "ok"));
}

#[tokio::test]
async fn json_float_scalar_preserves_json_number_format() {
    for (input, expected) in [(br#"{"n":1.0}"# as &[u8], "1.0"), (br#"{"n":-0.0}"# as &[u8], "-0.0")] {
        let result = scan_json(&[input], "$.n", 16).await;
        assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == expected));
    }
}

#[tokio::test]
async fn selected_json_preserves_float_number_format() {
    let result = scan_json(&[br#"{"selected":{"n":1.0,"z":-0.0}}"#], "$.selected", 32).await;
    assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == r#"{"n":1.0,"z":-0.0}"#));
}

#[tokio::test]
async fn nested_duplicate_path_uses_last_value() {
    let result = scan_json(&[br#"{"a":{"b":"far too large","b":"ok"}}"#], "$.a.b", 2).await;
    assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == "ok"));
}

#[tokio::test]
async fn selected_object_collapses_duplicates_last_wins() {
    let result = scan_json(
        &[br#"{"selected":{"k":"far too large","k":"ok","a":1}}"#],
        "$.selected",
        20,
    )
    .await;
    assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == r#"{"a":1,"k":"ok"}"#));
}

#[tokio::test]
async fn selected_object_does_not_replay_two_distinct_overflowed_keys() {
    let result = scan_json(
        &[br#"{"selected":{"a":"far too large for limit","b":"also far too large","a":"x","b":"y"}}"#],
        "$.selected",
        20,
    )
    .await;
    assert!(matches!(result, JsonScanValue::TooLong));
}

#[tokio::test]
async fn invalid_json_returns_missing() {
    assert!(matches!(
        scan_json(&[br#"{"a":1} trailing"#], "$.a", 16).await,
        JsonScanValue::Missing
    ));
}

#[tokio::test]
async fn json_select_shares_prefixes_and_duplicate_paths_across_chunks() {
    let (plan, values) = scan_json_select(
        &[
            br#"{"user":{"id":7,"na"# as &[u8],
            br#"me":"alice"},"items":[true,false]}"#,
        ],
        &[
            ("user", "$.user"),
            ("id", "$.user.id"),
            ("id_alias", "$.user.id"),
            ("second", "$.items[1]"),
        ],
        128,
    )
    .await;

    assert!(matches!(
        selected(&plan, &values, "user"),
        JsonScanValue::Value(Value::Str(value)) if &**value == r#"{"id":7,"name":"alice"}"#
    ));
    assert!(matches!(
        selected(&plan, &values, "id"),
        JsonScanValue::Value(Value::Int(7))
    ));
    assert!(matches!(
        selected(&plan, &values, "id_alias"),
        JsonScanValue::Value(Value::Int(7))
    ));
    assert!(matches!(
        selected(&plan, &values, "second"),
        JsonScanValue::Value(Value::Bool(false))
    ));
}

#[tokio::test]
async fn json_select_root_and_descendant_share_one_parse() {
    let (plan, values) = scan_json_select(&[br#"{"id":7,"name":"alice"}"#], &[("root", "$"), ("id", "$.id")], 64).await;

    assert!(matches!(
        selected(&plan, &values, "root"),
        JsonScanValue::Value(Value::Str(value)) if &**value == r#"{"id":7,"name":"alice"}"#
    ));
    assert!(matches!(
        selected(&plan, &values, "id"),
        JsonScanValue::Value(Value::Int(7))
    ));
}

#[tokio::test]
async fn json_select_integer_captures_preserve_parent_encoding_limits() {
    for value in [i64::MIN, -1, 0, 1, i64::MAX] {
        let input = format!(r#"{{"n":{value}}}"#);
        let (plan, values) = scan_json_select(&[input.as_bytes()], &[("number", "$.n")], 0).await;
        assert!(matches!(
            selected(&plan, &values, "number"),
            JsonScanValue::Value(Value::Int(actual)) if *actual == value
        ));

        for limit in [input.len() - 1, input.len()] {
            let (plan, values) =
                scan_json_select(&[input.as_bytes()], &[("root", "$"), ("number", "$.n")], limit).await;
            assert!(matches!(
                selected(&plan, &values, "number"),
                JsonScanValue::Value(Value::Int(actual)) if *actual == value
            ));
            if limit == input.len() {
                assert!(matches!(
                    selected(&plan, &values, "root"),
                    JsonScanValue::Value(Value::Str(actual)) if actual.as_ref() == input
                ));
            } else {
                assert!(matches!(selected(&plan, &values, "root"), JsonScanValue::TooLong));
            }
        }
    }
}

#[tokio::test]
async fn json_select_preserves_existing_non_i64_number_conversion() {
    let (plan, values) = scan_json_select(
        &[br#"{"float":1.5,"big":18446744073709551615}"#],
        &[("float", "$.float"), ("big", "$.big")],
        64,
    )
    .await;

    assert!(matches!(
        selected(&plan, &values, "float"),
        JsonScanValue::Value(Value::Str(value)) if &**value == "1.5"
    ));
    assert!(matches!(
        selected(&plan, &values, "big"),
        JsonScanValue::Value(Value::Str(value)) if &**value == "18446744073709551615"
    ));
}

#[tokio::test]
async fn oversized_parent_still_scans_selected_descendants() {
    let (plan, values) = scan_json_select(
        &[br#"{"large":{"padding":"too long","target":7}}"#],
        &[("large", "$.large"), ("target", "$.large.target")],
        8,
    )
    .await;

    assert!(matches!(selected(&plan, &values, "large"), JsonScanValue::TooLong));
    assert!(matches!(
        selected(&plan, &values, "target"),
        JsonScanValue::Value(Value::Int(7))
    ));
}

#[test]
fn bounded_map_caps_retained_entries_when_the_aggregate_overflows() {
    let mut output = BoundedMapAccumulator::new(24);
    output
        .insert("first".to_string(), BoundedJson::from_bytes(24, br#""12345678""#))
        .unwrap();
    assert!(!output.entries.is_empty());

    output
        .insert("second".to_string(), BoundedJson::from_bytes(24, br#""12345678""#))
        .unwrap();
    assert!(output.is_oversized());
    assert!(output.rendered_len <= output.limit);
    assert!(output.entries.values().map(|value| value.bytes.len()).sum::<usize>() <= output.limit);
    assert!(output.finish().unwrap().oversized);
}

#[test]
fn bounded_map_replacement_updates_rendered_length_and_commas() {
    let mut output = BoundedMapAccumulator::new(64);
    output
        .insert("a".to_string(), BoundedJson::from_bytes(64, b"1"))
        .unwrap();
    output
        .insert("b".to_string(), BoundedJson::from_bytes(64, b"2"))
        .unwrap();
    output
        .insert("a".to_string(), BoundedJson::from_bytes(64, b"333"))
        .unwrap();

    let output = output.finish().unwrap();
    assert_eq!(String::from_utf8(output.bytes).unwrap(), r#"{"a":333,"b":2}"#);
}

#[test]
fn bounded_map_overflow_marker_recovers_only_for_the_same_key() {
    let mut recovered = BoundedMapAccumulator::new(20);
    recovered
        .insert("key".to_string(), BoundedJson::from_bytes(20, &[b'x'; 21]))
        .unwrap();
    assert!(recovered.is_oversized());
    recovered
        .insert("key".to_string(), BoundedJson::from_bytes(20, br#""ok""#))
        .unwrap();
    assert!(!recovered.is_oversized());
    assert_eq!(
        String::from_utf8(recovered.finish().unwrap().bytes).unwrap(),
        r#"{"key":"ok"}"#
    );

    let mut irreversible = BoundedMapAccumulator::new(20);
    irreversible
        .insert("first".to_string(), BoundedJson::from_bytes(20, &[b'x'; 21]))
        .unwrap();
    irreversible
        .insert("second".to_string(), BoundedJson::from_bytes(20, &[b'y'; 21]))
        .unwrap();
    assert!(irreversible.irreversible_oversize);
    irreversible
        .insert("first".to_string(), BoundedJson::from_bytes(20, b"1"))
        .unwrap();
    assert!(irreversible.entries.is_empty());
}

#[test]
fn bounded_map_overlong_key_is_irreversible() {
    let mut output = BoundedMapAccumulator::new(8);
    output
        .insert("key-too-long".to_string(), BoundedJson::from_bytes(8, b"1"))
        .unwrap();
    assert!(output.irreversible_oversize);
    assert!(output.entries.is_empty());
}

#[test]
fn json_encoding_is_counted_and_written_without_expanded_temporaries() {
    let key = "quote\" slash\\ control\u{0001} café";
    let expected = serde_json::to_vec(key).unwrap();
    let mut count = CountingWriter::default();
    serde_json::to_writer(&mut count, &key).unwrap();
    assert_eq!(count.len, expected.len());

    let raw = "\0".repeat(32);
    let mut bounded = BoundedJson::new(16);
    serde_json::to_writer(&mut bounded, &raw.as_str()).unwrap();
    assert!(bounded.oversized);
    assert!(bounded.bytes.is_empty());
    assert!(bounded.bytes.capacity() <= bounded.limit);

    let mut map = BoundedMapAccumulator::new(16);
    map.insert(raw, BoundedJson::from_bytes(16, b"1")).unwrap();
    assert!(map.irreversible_oversize);
    assert!(map.entries.is_empty());
}

#[test]
fn borrowed_json_capture_matches_consuming_conversion_without_mutating_source() {
    for bytes in [b"".as_slice(), b"{}", "\"é\"".as_bytes(), &[0xff], &[b'a', 0xe2, 0x82]] {
        for limit in [0, bytes.len().saturating_sub(1), bytes.len(), 64] {
            let bounded = BoundedJson::from_bytes(limit, bytes);
            let original = bounded.bytes.clone();
            let oversized = bounded.oversized;
            let expected = BoundedJson::from_bytes(limit, bytes).into_scan_value();
            let first = bounded.to_scan_value();
            let second = bounded.to_scan_value();
            assert_eq!(bounded.bytes, original);
            assert_eq!(bounded.oversized, oversized);
            drop(bounded);
            for actual in [first, second] {
                match (&actual, &expected) {
                    (JsonScanValue::Value(actual), JsonScanValue::Value(expected)) => assert_eq!(actual, expected),
                    (JsonScanValue::Missing, JsonScanValue::Missing)
                    | (JsonScanValue::TooLong, JsonScanValue::TooLong) => {}
                    _ => panic!("capture conversion differs for {bytes:?} at limit {limit}"),
                }
            }
        }
    }
}

#[tokio::test]
async fn json_select_retains_parent_encoding_after_capturing_nested_container() {
    let (plan, values) = scan_json_select(&[br#"{"a":{"x":1}}"#], &[("root", "$"), ("nested", "$.a")], 64).await;
    assert!(matches!(
        selected(&plan, &values, "root"),
        JsonScanValue::Value(Value::Str(value)) if &**value == r#"{"a":{"x":1}}"#
    ));
    assert!(matches!(
        selected(&plan, &values, "nested"),
        JsonScanValue::Value(Value::Str(value)) if &**value == r#"{"x":1}"#
    ));
}

#[test]
fn selected_map_keys_reuse_the_bounded_plan_key() {
    let plan = JsonSelectPlan::new([("value".to_string(), DefiniteJsonPath::parse("$.selected").unwrap())]);
    let (canonical, _) = plan.root.members.get_key_value("selected").unwrap();
    let mut deserializer = serde_json::Deserializer::from_str(r#""selected""#);
    let key = SelectedMapKeySeed {
        node: &plan.root,
        retain_for_parent: true,
        limit: 32,
    }
    .deserialize(&mut deserializer)
    .unwrap();

    assert!(key.selected.is_some());
    let retained = key.retained.unwrap();
    assert!(Arc::ptr_eq(canonical, &retained));

    let mut deserializer = serde_json::Deserializer::from_str(r#""body-sized-irrelevant-key""#);
    let key = SelectedMapKeySeed {
        node: &plan.root,
        retain_for_parent: true,
        limit: 8,
    }
    .deserialize(&mut deserializer)
    .unwrap();
    assert!(key.selected.is_none());
    assert!(key.retained.is_none());
    assert!(key.overlong_for_parent);
}

#[tokio::test]
async fn oversized_selected_array_still_scans_later_selected_indices() {
    let (plan, values) = scan_json_select(
        &[br#"["far too large",{"nested":"also too large"},7]"#],
        &[("root", "$"), ("third", "$[2]")],
        8,
    )
    .await;

    assert!(matches!(selected(&plan, &values, "root"), JsonScanValue::TooLong));
    assert!(matches!(
        selected(&plan, &values, "third"),
        JsonScanValue::Value(Value::Int(7))
    ));
}

#[tokio::test]
async fn json_select_duplicate_parent_replaces_the_complete_subtree() {
    let (plan, values) = scan_json_select(
        &[br#"{"a":{"b":"old"},"a":{}}"#],
        &[("parent", "$.a"), ("child", "$.a.b")],
        64,
    )
    .await;

    assert!(matches!(
        selected(&plan, &values, "parent"),
        JsonScanValue::Value(Value::Str(value)) if &**value == "{}"
    ));
    assert!(matches!(selected(&plan, &values, "child"), JsonScanValue::Missing));
}

#[tokio::test]
async fn json_select_later_duplicate_can_replace_oversized_same_capture() {
    let (plan, values) = scan_json_select(&[br#"{"a":"far too large","a":"ok"}"#], &[("value", "$.a")], 2).await;
    assert!(matches!(
        selected(&plan, &values, "value"),
        JsonScanValue::Value(Value::Str(value)) if &**value == "ok"
    ));
}

#[tokio::test]
async fn json_select_online_aggregate_overflow_is_not_replayed() {
    let (plan, values) = scan_json_select(
        &[br#"{"a":"1234","b":"1234","a":""}"#],
        &[("a", "$.a"), ("b", "$.b")],
        5,
    )
    .await;

    assert!(matches!(
        selected(&plan, &values, "a"),
        JsonScanValue::Value(Value::Str(value)) if value.is_empty()
    ));
    assert!(matches!(selected(&plan, &values, "b"), JsonScanValue::TooLong));
}

#[tokio::test]
async fn invalid_json_discards_every_json_select_capture() {
    let (plan, values) = scan_json_select(&[br#"{"a":1,"b":2} trailing"#], &[("a", "$.a"), ("b", "$.b")], 64).await;
    assert!(matches!(selected(&plan, &values, "a"), JsonScanValue::Missing));
    assert!(matches!(selected(&plan, &values, "b"), JsonScanValue::Missing));
}
