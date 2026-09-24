use super::ApiResponse;

#[test]
fn empty_warnings_are_absent_from_serialized_payload() {
    let body = ApiResponse::ok_body("HTTPRoute created".to_string());
    let serialized = serde_json::to_value(&body).expect("ApiResponse must serialize");
    assert!(
        serialized.get("warnings").is_none(),
        "an empty warnings list must not appear in the JSON at all, matching every pre-existing ok_body call site"
    );
}

#[test]
fn non_empty_warnings_are_present_and_ordered() {
    let body = ApiResponse::ok_body_with_warnings(
        "EdgionPlugins created".to_string(),
        vec!["requestPlugins[0].config.denys".to_string()],
    );
    let serialized = serde_json::to_value(&body).expect("ApiResponse must serialize");
    assert_eq!(serialized["warnings"][0], "requestPlugins[0].config.denys");
}

#[test]
fn err_body_never_carries_warnings() {
    let body: ApiResponse<()> = ApiResponse::err_body("bad request".to_string());
    let serialized = serde_json::to_value(&body).expect("ApiResponse must serialize");
    assert!(serialized.get("warnings").is_none());
}
