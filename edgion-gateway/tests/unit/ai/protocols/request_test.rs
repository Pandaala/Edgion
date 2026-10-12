use super::*;

#[test]
fn native_utf8_admission_borrows_original_body_without_allocation() {
    let body = r#" {"model":"m","extension":"é€"} "#.as_bytes();
    let (text, stats) = crate::test_allocation_counter::count_allocation_stats(|| native_json_utf8(body).unwrap());
    assert_eq!(text.as_ptr(), body.as_ptr());
    assert_eq!(text.len(), body.len());
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
}

#[test]
fn native_utf8_admission_rejects_invalid_bytes_anywhere() {
    for body in [
        b"{\"model\":\"m\",\"extension\":\"\xff\"}".as_slice(),
        b"{\"model\":\"m\",\"extension\":{\"items\":[\"\xff\"]}}",
    ] {
        assert!(matches!(native_json_utf8(body), Err(AiProxyError::MalformedBody)));
    }
}
