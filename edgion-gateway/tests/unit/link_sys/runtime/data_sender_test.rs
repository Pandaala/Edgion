use super::*;

#[test]
fn test_severity_from_status() {
    assert_eq!(LogMeta::severity_from_status(Some(200)), 9);
    assert_eq!(LogMeta::severity_from_status(Some(301)), 9);
    assert_eq!(LogMeta::severity_from_status(Some(400)), 13);
    assert_eq!(LogMeta::severity_from_status(Some(404)), 13);
    assert_eq!(LogMeta::severity_from_status(Some(500)), 17);
    assert_eq!(LogMeta::severity_from_status(Some(503)), 17);
    assert_eq!(LogMeta::severity_from_status(None), 0);
}

#[test]
fn log_meta_borrows_call_scoped_method_and_path() {
    let method = String::from("PATCH");
    let path = String::from("/v1/items/42");
    let meta = LogMeta {
        method: Some(method.as_str()),
        path: Some(path.as_str()),
        ..Default::default()
    };

    assert_eq!(meta.method, Some("PATCH"));
    assert_eq!(meta.path, Some("/v1/items/42"));
    assert_eq!(meta.method.expect("method is set").as_ptr(), method.as_ptr());
    assert_eq!(meta.path.expect("path is set").as_ptr(), path.as_ptr());
}
