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
