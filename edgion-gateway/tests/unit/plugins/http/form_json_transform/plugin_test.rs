use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderValue};

fn request_headers(content_type: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers
}

fn filter(direction: FormJsonDirection, limit: u64) -> FormJsonTransform {
    FormJsonTransform {
        direction,
        body_limit: limit,
    }
}

fn session(body: &'static [u8], content_type: &'static str) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(request_headers(content_type));
    session.expect_declares_body().return_const(Some(!body.is_empty()));
    session
        .expect_request_body_available_len()
        .with(mockall::predicate::eq(body.is_empty()))
        .return_const(Ok(body.len() as u64));
    session
        .expect_read_request_body()
        .with(mockall::predicate::eq(None))
        .return_once(move |_| Ok(Bytes::from_static(body)));
    session
}

#[test]
fn decodes_form_strictly_and_groups_repeated_keys() {
    let output = form_to_json(b"b=2&a=1&a=3&empty&plus=%2b+%2B&&nul=%00&=blank", 1024).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&output).unwrap(),
        serde_json::json!({"": "blank", "a": ["1", "3"], "b": "2", "empty": "", "nul": "\u{0}", "plus": "+ +"})
    );

    for malformed in [b"%".as_slice(), b"%0", b"%GG", b"%ff", b"k=\xff"] {
        assert_eq!(form_to_json(malformed, 1024), Err(TransformFailure::BadRequest));
    }
    assert_eq!(form_to_json(b"key=%c3%a9", 1024).unwrap(), r#"{"key":"é"}"#.as_bytes());
}

#[test]
fn encodes_exact_whatwg_form_set_and_sorted_keys() {
    let output = json_to_form(br#"{"z":"*~+%&=/ ","a":["one",true,2],"empty":""}"#, 1024).unwrap();
    assert_eq!(output, b"a=one&a=true&a=2&empty=&z=*%7E%2B%25%26%3D%2F+");
    assert_eq!(json_to_form(b"{}", 1).unwrap(), b"");
}

#[test]
fn rejects_unsupported_json_shapes() {
    for input in [
        b"[]".as_slice(),
        br#"{"a":null}"#,
        br#"{"a":{}}"#,
        br#"{"a":[]}"#,
        br#"{"a":[["nested"]]}"#,
    ] {
        assert_eq!(json_to_form(input, 1024), Err(TransformFailure::BadRequest));
    }
}

#[test]
fn enforces_pair_component_and_output_limits() {
    let exact_pairs = std::iter::repeat_n("a=b", MAX_PAIRS).collect::<Vec<_>>().join("&");
    assert!(form_to_json(exact_pairs.as_bytes(), usize::MAX).is_ok());
    let pairs = std::iter::repeat_n("a=b", MAX_PAIRS + 1).collect::<Vec<_>>().join("&");
    assert_eq!(
        form_to_json(pairs.as_bytes(), usize::MAX),
        Err(TransformFailure::TooLarge)
    );
    assert_eq!(
        form_to_json(format!("{}=v", "k".repeat(MAX_KEY_BYTES + 1)).as_bytes(), usize::MAX),
        Err(TransformFailure::TooLarge)
    );
    assert_eq!(
        form_to_json(format!("k={}", "v".repeat(MAX_VALUE_BYTES + 1)).as_bytes(), usize::MAX),
        Err(TransformFailure::TooLarge)
    );
    assert_eq!(
        json_to_form(
            format!(r#"{{"{}":"v"}}"#, "k".repeat(MAX_KEY_BYTES + 1)).as_bytes(),
            usize::MAX
        ),
        Err(TransformFailure::TooLarge)
    );
    assert_eq!(
        json_to_form(
            format!(r#"{{"k":"{}"}}"#, "v".repeat(MAX_VALUE_BYTES + 1)).as_bytes(),
            usize::MAX
        ),
        Err(TransformFailure::TooLarge)
    );
    assert_eq!(
        json_to_form(br#"{"a":"first","a":"second"}"#, usize::MAX),
        Err(TransformFailure::BadRequest)
    );

    assert!(json_to_form(br#"{"a":"b"}"#, 3).is_ok());
    assert_eq!(json_to_form(br#"{"a":"b"}"#, 2), Err(TransformFailure::TooLarge));
    assert_eq!(form_to_json(b"", 2).unwrap(), b"{}");
    assert_eq!(form_to_json(b"", 1), Err(TransformFailure::TooLarge));
}

#[tokio::test]
async fn publishes_body_then_canonical_content_type() {
    let filter = filter(FormJsonDirection::FormToJson, 1024);
    let mut session = session(b"b=2&a=1", "application/x-www-form-urlencoded; charset=utf-8");
    let mut sequence = mockall::Sequence::new();
    session
        .expect_overwrite_request_body()
        .withf(|body| body.as_ref() == br#"{"a":"1","b":"2"}"#)
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(Ok(()));
    session
        .expect_set_request_header()
        .withf(|name, value| name == "content-type" && value == "application/json")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    let mut log = PluginLog::new(filter.name());

    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::Nothing
    );
}

#[tokio::test]
async fn header_failure_rolls_back_original_body() {
    let filter = filter(FormJsonDirection::FormToJson, 1024);
    let original = b"a=1";
    let mut session = session(original, "application/x-www-form-urlencoded");
    let mut sequence = mockall::Sequence::new();
    session
        .expect_overwrite_request_body()
        .withf(|body| body.as_ref() == br#"{"a":"1"}"#)
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(Ok(()));
    session
        .expect_set_request_header()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Err(std::io::Error::other("injected header failure").into()));
    session
        .expect_overwrite_request_body()
        .withf(move |body| body.as_ref() == original)
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(Ok(()));
    let mut log = PluginLog::new(filter.name());

    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        FormJsonTransform::reject(500, None)
    );
}

#[tokio::test]
async fn overwrite_failure_does_not_update_content_type() {
    let filter = filter(FormJsonDirection::JsonToForm, 1024);
    let mut session = session(br#"{"a":"1"}"#, "application/json");
    session
        .expect_overwrite_request_body()
        .times(1)
        .return_const(Err(BodyReadError::StorageFailure));
    session.expect_set_request_header().times(0);
    session
        .expect_record_error()
        .with(mockall::predicate::eq(EdgionErrorCode::BodyCaptureStorageFailed))
        .times(1)
        .return_const(());
    let mut log = PluginLog::new(filter.name());

    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        FormJsonTransform::reject(500, None)
    );
}

#[tokio::test]
async fn media_and_pre_materialization_limits_fail_before_mutation() {
    let filter = filter(FormJsonDirection::FormToJson, 3);
    let mut media = MockPluginSession::new();
    media
        .expect_request_header_map()
        .return_const(request_headers("application/json"));
    media.expect_read_request_body().times(0);
    media.expect_overwrite_request_body().times(0);
    let mut log = PluginLog::new(filter.name());
    assert_eq!(
        filter.run_request(&mut media, &mut log).await,
        FormJsonTransform::reject(415, Some(UNSUPPORTED_MEDIA_TYPE))
    );

    let mut too_large = MockPluginSession::new();
    too_large
        .expect_request_header_map()
        .return_const(request_headers("application/x-www-form-urlencoded"));
    too_large.expect_declares_body().return_const(Some(true));
    too_large.expect_request_body_available_len().return_const(Ok(4));
    too_large
        .expect_record_error()
        .with(mockall::predicate::eq(EdgionErrorCode::BodyReadTooLarge))
        .return_const(());
    too_large.expect_read_request_body().times(0);
    too_large.expect_overwrite_request_body().times(0);
    let mut log = PluginLog::new(filter.name());
    assert_eq!(
        filter.run_request(&mut too_large, &mut log).await,
        FormJsonTransform::reject(413, None)
    );
}

#[tokio::test]
async fn unavailable_length_records_diagnostic_once_before_read_or_mutation() {
    let filter = filter(FormJsonDirection::FormToJson, 1024);
    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(request_headers("application/x-www-form-urlencoded"));
    session.expect_declares_body().return_const(Some(true));
    session
        .expect_request_body_available_len()
        .with(mockall::predicate::eq(false))
        .times(1)
        .return_const(Err(BodyReadError::StorageFailure));
    session
        .expect_record_error()
        .with(mockall::predicate::eq(EdgionErrorCode::BodyCaptureStorageFailed))
        .times(1)
        .return_const(());
    session.expect_read_request_body().times(0);
    session.expect_overwrite_request_body().times(0);
    session.expect_set_request_header().times(0);
    let mut log = PluginLog::new(filter.name());

    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        FormJsonTransform::reject(500, None)
    );
    assert_eq!(log.render_for_test(), "err: body length unavailable");
}

async fn assert_read_failure(error: BodyReadError, expected: Option<EdgionErrorCode>, status: u16, message: &str) {
    let filter = filter(FormJsonDirection::FormToJson, 1024);
    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(request_headers("application/x-www-form-urlencoded"));
    session.expect_declares_body().return_const(Some(true));
    session.expect_request_body_available_len().return_const(Ok(3));
    session
        .expect_read_request_body()
        .with(mockall::predicate::eq(None))
        .times(1)
        .return_once(move |_| Err(error));
    if let Some(expected) = expected {
        session
            .expect_record_error()
            .with(mockall::predicate::eq(expected))
            .times(1)
            .return_const(());
    } else {
        session.expect_record_error().times(0);
    }
    session.expect_overwrite_request_body().times(0);
    session.expect_set_request_header().times(0);
    let mut log = PluginLog::new(filter.name());

    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        FormJsonTransform::reject(status, None)
    );
    assert_eq!(log.render_for_test(), message);
}

#[tokio::test]
async fn physical_cap_read_failure_records_once_and_returns_413() {
    assert_read_failure(
        BodyReadError::BufferLimitExceeded { limit: 1024 },
        Some(EdgionErrorCode::BodyReadTooLarge),
        413,
        "deny body too large",
    )
    .await;
}

#[tokio::test]
async fn storage_read_failure_records_once_and_returns_500() {
    assert_read_failure(
        BodyReadError::StorageFailure,
        Some(EdgionErrorCode::BodyCaptureStorageFailed),
        500,
        "err: body materialization failed",
    )
    .await;
}

#[tokio::test]
async fn refused_read_adds_no_diagnostic_and_returns_500() {
    assert_read_failure(
        BodyReadError::NotBuffered,
        None,
        500,
        "err: body materialization failed",
    )
    .await;
}

async fn assert_failed_rollback(error: BodyReadError, expected: EdgionErrorCode) {
    let filter = filter(FormJsonDirection::FormToJson, 1024);
    let original = b"a=1";
    let mut session = session(original, "application/x-www-form-urlencoded");
    let mut sequence = mockall::Sequence::new();
    session
        .expect_overwrite_request_body()
        .withf(|body| body.as_ref() == br#"{"a":"1"}"#)
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(Ok(()));
    session
        .expect_set_request_header()
        .withf(|name, value| name == "content-type" && value == "application/json")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Err(std::io::Error::other("injected header failure").into()));
    session
        .expect_overwrite_request_body()
        .withf(move |body| body.as_ref() == original)
        .times(1)
        .in_sequence(&mut sequence)
        .return_once(move |_| Err(error));
    session
        .expect_record_error()
        .with(mockall::predicate::eq(expected))
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(());
    let mut log = PluginLog::new(filter.name());

    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        FormJsonTransform::reject(500, None)
    );
    assert_eq!(log.render_for_test(), "err: body rollback failed");
}

#[tokio::test]
async fn storage_rollback_failure_records_once_and_retains_selected_500() {
    assert_failed_rollback(BodyReadError::StorageFailure, EdgionErrorCode::BodyCaptureStorageFailed).await;
}

#[tokio::test]
async fn oversized_rollback_failure_records_once_and_retains_selected_500() {
    assert_failed_rollback(
        BodyReadError::BufferLimitExceeded { limit: 1024 },
        EdgionErrorCode::BodyReadTooLarge,
    )
    .await;
}

#[test]
fn boolean_pairs_borrow_text_without_temporary_allocations() {
    let mut writer = CappedWriter::new(14);
    let mut count = 0;
    let (result, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        write_json_pair(&mut writer, "k", Value::Bool(true), &mut count)?;
        write_json_pair(&mut writer, "k", Value::Bool(false), &mut count)
    });
    assert_eq!(result, Ok(()));
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
    assert_eq!(count, 2);
    assert_eq!(writer.into_inner(), b"k=true&k=false");
    let input = br#"{"k":[true,false]}"#;
    assert_eq!(json_to_form(input, 14).unwrap(), b"k=true&k=false");
    assert_eq!(json_to_form(input, 13), Err(TransformFailure::TooLarge));
}

#[test]
fn boolean_pair_admission_preserves_counter_and_output_boundaries() {
    for initial in [MAX_PAIRS - 1, MAX_PAIRS, usize::MAX] {
        let mut writer = CappedWriter::new(16);
        let mut count = initial;
        let (result, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            write_json_pair(&mut writer, "k", Value::Bool(false), &mut count)
        });
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
        if initial == MAX_PAIRS - 1 {
            assert_eq!(result, Ok(()));
            assert_eq!(count, MAX_PAIRS);
            assert_eq!(writer.into_inner(), b"&k=false");
        } else {
            assert_eq!(result, Err(TransformFailure::TooLarge));
            assert_eq!(count, initial.saturating_add(1));
            assert!(writer.into_inner().is_empty());
        }
    }
}

fn original_form_to_json(input: &[u8], output_limit: usize) -> Result<Vec<u8>, TransformFailure> {
    let mut values = BTreeMap::<String, Vec<String>>::new();
    let mut pair_count = 0usize;
    for segment in input.split(|byte| *byte == b'&').filter(|segment| !segment.is_empty()) {
        pair_count = pair_count.checked_add(1).ok_or(TransformFailure::TooLarge)?;
        if pair_count > MAX_PAIRS {
            return Err(TransformFailure::TooLarge);
        }
        let split = segment.iter().position(|byte| *byte == b'=').unwrap_or(segment.len());
        let key = original_decode_component(&segment[..split], MAX_KEY_BYTES)?;
        let value_start = split.saturating_add(1).min(segment.len());
        let value = original_decode_component(&segment[value_start..], MAX_VALUE_BYTES)?;
        values.entry(key).or_default().push(value);
    }
    let object = values
        .into_iter()
        .map(|(key, mut values)| {
            let value = if values.len() == 1 {
                Value::String(values.pop().unwrap())
            } else {
                Value::Array(values.into_iter().map(Value::String).collect())
            };
            (key, value)
        })
        .collect::<serde_json::Map<String, Value>>();
    serialize_json(&Value::Object(object), output_limit)
}

#[test]
fn direct_form_group_serialization_matches_original_bytes_and_failures() {
    let cases = [
        Vec::new(),
        b"&&&".to_vec(),
        b"a=one".to_vec(),
        b"z=0&a=1&a=2&a=3&z=4".to_vec(),
        b"a=1&%61=2&=empty&bare&plus=%2b+%2B&quote=%22&slash=%5c&control=%00%0a&utf8=%c3%a9".to_vec(),
        b"a=ok&b=%".to_vec(),
        b"a=%ff".to_vec(),
        b"a=\xff".to_vec(),
        format!("{}=v", "k".repeat(MAX_KEY_BYTES + 1)).into_bytes(),
        format!("k={}", "v".repeat(MAX_VALUE_BYTES + 1)).into_bytes(),
        std::iter::repeat_n("k=v", MAX_PAIRS)
            .collect::<Vec<_>>()
            .join("&")
            .into_bytes(),
        std::iter::repeat_n("k=v", MAX_PAIRS + 1)
            .collect::<Vec<_>>()
            .join("&")
            .into_bytes(),
    ];
    for input in cases {
        let mut caps = vec![0, 1, 2, 15, 1024, 20000];
        if let Ok(output) = original_form_to_json(&input, 20000) {
            caps.extend([output.len(), output.len().saturating_sub(1)]);
        }
        for cap in caps {
            assert_eq!(
                form_to_json(&input, cap),
                original_form_to_json(&input, cap),
                "input={input:?}, cap={cap}"
            );
        }
    }
}

#[test]
fn distinct_form_keys_avoid_singleton_vectors_and_second_object() {
    let input = (0..64)
        .map(|index| format!("key{index}=value{index}"))
        .collect::<Vec<_>>()
        .join("&");
    let (old, old_stats) =
        crate::test_allocation_counter::count_allocation_stats(|| original_form_to_json(input.as_bytes(), 8192));
    let (new, new_stats) =
        crate::test_allocation_counter::count_allocation_stats(|| form_to_json(input.as_bytes(), 8192));
    assert!(old.is_ok());
    assert_eq!(new, old);
    assert!(
        new_stats.calls < old_stats.calls,
        "old={old_stats:?}, new={new_stats:?}"
    );
    assert!(
        new_stats.requested_bytes < old_stats.requested_bytes,
        "old={old_stats:?}, new={new_stats:?}"
    );
}

fn original_decode_component(input: &[u8], limit: usize) -> Result<String, TransformFailure> {
    let mut output = Vec::with_capacity(input.len().min(limit));
    let mut index = 0usize;
    while index < input.len() {
        let byte = match input[index] {
            b'+' => {
                index += 1;
                b' '
            }
            b'%' => {
                let high = input.get(index + 1).and_then(|byte| hex_value(*byte));
                let low = input.get(index + 2).and_then(|byte| hex_value(*byte));
                let (Some(high), Some(low)) = (high, low) else {
                    return Err(TransformFailure::BadRequest);
                };
                index += 3;
                (high << 4) | low
            }
            byte => {
                index += 1;
                byte
            }
        };
        if output.len() >= limit {
            return Err(TransformFailure::TooLarge);
        }
        output.push(byte);
    }
    String::from_utf8(output).map_err(|_| TransformFailure::BadRequest)
}

#[test]
fn borrowed_form_decoder_preserves_original_error_precedence() {
    let mut inputs: Vec<Vec<u8>> = (0_u8..=255).map(|byte| vec![byte]).collect();
    inputs.extend([
        b"".to_vec(),
        b"plain".to_vec(),
        "café".as_bytes().to_vec(),
        b"a+b".to_vec(),
        b"%61%62".to_vec(),
        b"%c3%a9".to_vec(),
        b"%".to_vec(),
        b"%0".to_vec(),
        b"%GG".to_vec(),
        b"%ff".to_vec(),
        b"a%".to_vec(),
        b"ab%".to_vec(),
        b"abc%".to_vec(),
        b"a%61".to_vec(),
        b"ab%61".to_vec(),
        b"abc%61".to_vec(),
        b"\xff%".to_vec(),
        b"\xff+".to_vec(),
        b"abc+".to_vec(),
    ]);
    for input in inputs {
        for limit in [0, 1, 2, 3, 4, 8, usize::MAX] {
            let expected = original_decode_component(&input, limit);
            let actual = decode_component(&input, limit);
            assert_eq!(actual.as_deref(), expected.as_deref(), "input={input:?}, limit={limit}");
        }
    }
}

#[test]
fn plain_form_components_borrow_input_without_allocation() {
    for input in [b"".as_slice(), b"plain", "café".as_bytes()] {
        let (value, stats) =
            crate::test_allocation_counter::count_allocation_stats(|| decode_component(input, input.len()).unwrap());
        assert!(matches!(value, Cow::Borrowed(_)));
        assert_eq!(value.as_bytes(), input);
        assert_eq!(value.as_ptr(), input.as_ptr());
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
    for (input, expected) in [(b"a+b".as_slice(), "a b"), (b"%61", "a")] {
        let value = decode_component(input, 3).unwrap();
        assert!(matches!(value, Cow::Owned(_)));
        assert_eq!(value, expected);
    }
}
