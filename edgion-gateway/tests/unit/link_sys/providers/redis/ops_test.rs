use super::*;

fn bulk(data: impl Into<bytes::Bytes>) -> Resp3Frame {
    Resp3Frame::BlobString {
        data: data.into(),
        attributes: None,
    }
}

fn raw_hash(entries: impl IntoIterator<Item = (String, Resp3Frame)>) -> Resp3Frame {
    Resp3Frame::Map {
        data: entries.into_iter().map(|(key, value)| (bulk(key), value)).collect(),
        attributes: None,
    }
}

fn raw_array(entries: impl IntoIterator<Item = (String, Resp3Frame)>) -> Resp3Frame {
    Resp3Frame::Array {
        data: entries
            .into_iter()
            .flat_map(|(key, value)| [bulk(key), value])
            .collect(),
        attributes: None,
    }
}

#[test]
fn bounded_hash_counts_original_key_and_value_bytes() {
    let make_value = || raw_hash([("é".into(), bulk("ñ€"))]);
    let map = bounded_hash_result(make_value(), 1, 7).unwrap().unwrap();
    assert_eq!(map.get("é").map(String::as_str), Some("ñ€"));
    assert!(bounded_hash_result(make_value(), 1, 6).unwrap().is_none());
}

#[test]
fn bounded_hash_accepts_one_mib_and_rejects_one_more_byte() {
    let cap = 1024 * 1024;
    let value = raw_hash([("x".into(), bulk("a".repeat(cap - 1)))]);
    let map = bounded_hash_result(value, 1, cap).unwrap().unwrap();
    assert_eq!(map.get("x").unwrap().len(), cap - 1);
    let value = raw_hash([("x".into(), bulk("a".repeat(cap)))]);
    assert!(bounded_hash_result(value, 1, cap).unwrap().is_none());
}

#[test]
fn bounded_hash_rejects_many_large_entries_before_string_conversion() {
    let make_value = || raw_hash((0..32).map(|i| (format!("field-{i}"), bulk(vec![0xFF; 32 * 1024]))));
    // Invalid UTF-8 would fail conversion. The byte guard must reject first.
    assert!(bounded_hash_result(make_value(), 32, 1024 * 1024).unwrap().is_none());
    assert!(bounded_hash_result(make_value(), 32, usize::MAX).is_err());
}

#[test]
fn bounded_hash_entry_limit_precedes_conversion_and_allows_exact_count() {
    let make_value = || raw_hash([("a".into(), bulk("one")), ("b".into(), bulk("two"))]);
    assert_eq!(bounded_hash_result(make_value(), 2, 8).unwrap().unwrap().len(), 2);
    assert!(bounded_hash_result(make_value(), 1, 8).unwrap().is_none());
    let invalid = raw_hash([("a".into(), bulk(bytes::Bytes::from_static(&[0xFF])))]);
    assert!(bounded_hash_result(invalid, 0, usize::MAX).unwrap().is_none());
    assert!(bounded_hash_result(raw_hash([]), 0, 0).unwrap().unwrap().is_empty());
}

#[test]
fn bounded_hash_conversion_errors_do_not_include_payload() {
    let value = raw_hash([("secret-field".into(), raw_hash([]))]);
    let error = bounded_hash_result(value, 1, 100).unwrap_err();
    assert_eq!(error.to_string(), "Redis HGETALL returned a non-string value");
    assert!(!error.to_string().contains("secret-field"));
    let value = raw_hash([("a".into(), bulk(bytes::Bytes::from_static(&[0xFF])))]);
    assert!(bounded_hash_result(value, 1, 2).is_err());
}

#[test]
fn bounded_hash_resp2_array_preserves_bulk_text_and_byte_boundaries() {
    let make_frame = || raw_array([("é".into(), bulk("123"))]);
    assert_eq!(
        bounded_hash_result(make_frame(), 1, 5)
            .unwrap()
            .unwrap()
            .get("é")
            .unwrap(),
        "123"
    );
    assert!(bounded_hash_result(make_frame(), 1, 4).unwrap().is_none());
    let cap = 1024 * 1024;
    assert!(
        bounded_hash_result(raw_array([("x".into(), bulk("a".repeat(cap - 1)))]), 1, cap)
            .unwrap()
            .is_some()
    );
    assert!(
        bounded_hash_result(raw_array([("x".into(), bulk("a".repeat(cap)))]), 1, cap)
            .unwrap()
            .is_none()
    );
    assert!(bounded_hash_result(raw_array([]), 0, 0).unwrap().unwrap().is_empty());
}

#[test]
fn bounded_hash_resp2_count_preflight_precedes_nested_frame_conversion() {
    // Nested errors fail Fred's frame-to-Value conversion itself. Returning None
    // proves that count rejection precedes even that conversion, not just Strings.
    let make_frame = || {
        raw_array((0..1024).map(|i| {
            (
                format!("field-{i}"),
                Resp3Frame::SimpleError {
                    data: "WRONGTYPE nested test error".into(),
                    attributes: None,
                },
            )
        }))
    };
    assert!(RedisValue::try_from(make_frame()).is_err());
    assert!(bounded_hash_result(make_frame(), 32, usize::MAX).unwrap().is_none());
    assert!(bounded_hash_result(make_frame(), 1024, usize::MAX).is_err());
    let many_large = raw_array((0..32).map(|i| (format!("field-{i}"), bulk(vec![0xFF; 32 * 1024]))));
    assert!(bounded_hash_result(many_large, 32, 1024 * 1024).unwrap().is_none());
}

#[test]
fn bounded_hash_rejects_malformed_response_frames() {
    let odd = Resp3Frame::Array {
        data: vec![bulk("secret-field")],
        attributes: None,
    };
    let error = bounded_hash_result(odd, 0, 0).unwrap_err();
    assert_eq!(error.to_string(), "Redis HGETALL returned an odd number of elements");
    for frame in [
        Resp3Frame::Null,
        bulk("QUEUED"),
        Resp3Frame::Number {
            data: 123,
            attributes: None,
        },
    ] {
        assert_eq!(
            bounded_hash_result(frame, 32, 1024).unwrap_err().to_string(),
            "Redis HGETALL returned an invalid response"
        );
    }
    let bad_key = Resp3Frame::Array {
        data: vec![raw_array([]), bulk("value")],
        attributes: None,
    };
    assert_eq!(
        bounded_hash_result(bad_key, 1, 1024).unwrap_err().to_string(),
        "Redis HGETALL returned a non-string value"
    );
}

#[test]
fn bounded_hash_preserves_fred_command_error_classification() {
    let simple = Resp3Frame::SimpleError {
        data: "WRONGTYPE test error".into(),
        attributes: None,
    };
    let blob = Resp3Frame::BlobError {
        data: bytes::Bytes::from_static(b"WRONGTYPE test error"),
        attributes: None,
    };
    for frame in [simple, blob] {
        let expected = RedisValue::try_from(frame.clone()).unwrap_err().to_string();
        assert_eq!(bounded_hash_result(frame, 0, 0).unwrap_err().to_string(), expected);
    }
}

#[test]
fn hash_byte_accounting_rejects_overflow_and_accepts_exact_cap() {
    assert_eq!(checked_hash_bytes(3, 2, 5, 10), Some(10));
    assert_eq!(checked_hash_bytes(3, 2, 6, 10), None);
    assert_eq!(checked_hash_bytes(usize::MAX, 1, 0, usize::MAX), None);
    assert_eq!(checked_hash_bytes(0, usize::MAX, 1, usize::MAX), None);
}

#[test]
fn test_generate_lock_value_is_unique() {
    let v1 = generate_lock_value();
    std::thread::sleep(std::time::Duration::from_nanos(1));
    let v2 = generate_lock_value();
    assert_ne!(v1, v2);
}

#[test]
fn test_lock_options_default() {
    let opts = LockOptions::default();
    assert_eq!(opts.ttl, Duration::from_secs(10));
    assert_eq!(opts.max_wait, Duration::from_secs(5));
    assert_eq!(opts.retry_interval, Duration::from_millis(50));
}
