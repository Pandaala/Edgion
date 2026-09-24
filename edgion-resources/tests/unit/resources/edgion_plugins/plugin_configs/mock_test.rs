use super::*;
const PROTECTED_HEADER_ERROR: &str = "response header mutation targets a protected protocol field";

fn headers(entries: &[(&str, &str)]) -> Option<HashMap<String, String>> {
    Some(
        entries
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect(),
    )
}

#[test]
fn test_default_config() {
    let config = MockConfig::default();
    assert_eq!(config.status_code, 200);
    assert_eq!(config.content_type, "application/json");
    assert!(config.body.is_none());
    assert!(config.headers.is_none());
    assert!(config.delay.is_none());
}

#[test]
fn test_new_config() {
    let config = MockConfig::new(404, r#"{"error":"Not Found"}"#.to_string());
    assert_eq!(config.status_code, 404);
    assert_eq!(config.body, Some(r#"{"error":"Not Found"}"#.to_string()));
}

#[test]
fn test_validate_success() {
    for status_code in [200, 599] {
        let config = MockConfig {
            headers: headers(&[("X-Mock", "yes"), ("Cache-Control", "no-cache")]),
            content_type: "application/json; charset=utf-8".to_string(),
            ..MockConfig::new(status_code, "OK".to_string())
        };
        assert!(config.validate().is_ok(), "{status_code}");
    }
}

#[test]
fn test_validate_invalid_status() {
    for status_code in [100, 101, 199, 600] {
        let config = MockConfig {
            status_code,
            ..Default::default()
        };
        assert!(config.validate().is_err(), "{status_code}");
    }
}

#[test]
fn status_code_schema_matches_final_response_range() {
    let schema = serde_json::to_value(schemars::schema_for!(MockConfig)).unwrap();
    let status = &schema["properties"]["statusCode"];
    assert_eq!(status["minimum"].as_u64(), Some(200));
    assert_eq!(status["maximum"].as_u64(), Some(599));
}

#[test]
fn test_validate_delay_limit() {
    let config = MockConfig {
        delay: Some(MAX_DELAY_MS),
        ..Default::default()
    };
    assert!(config.validate().is_ok());

    let config = MockConfig {
        delay: Some(MAX_DELAY_MS + 1),
        ..Default::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_validate_header_names() {
    for invalid_name in ["bad header".to_string(), "x".repeat(HEADER_NAME_MAX_LEN as usize + 1)] {
        let config = MockConfig {
            headers: Some(HashMap::from([(invalid_name, "value".to_string())])),
            ..Default::default()
        };
        assert_eq!(config.validate().unwrap_err(), INVALID_HEADER_NAME);
    }
}

#[test]
fn test_validate_header_values_without_echoing_input() {
    let sentinel = "private-value";
    let config = MockConfig {
        headers: headers(&[("X-Mock", &format!("{sentinel}\r\ninjected: true"))]),
        ..Default::default()
    };

    let error = config.validate().unwrap_err();
    assert_eq!(error, INVALID_HEADER_VALUE);
    assert!(!error.contains(sentinel));
}

#[test]
fn test_validate_content_type_without_echoing_input() {
    let sentinel = "private-content-type";
    let config = MockConfig {
        content_type: format!("text/plain\r\n{sentinel}"),
        ..Default::default()
    };

    let error = config.validate().unwrap_err();
    assert_eq!(error, INVALID_CONTENT_TYPE);
    assert!(!error.contains(sentinel));
}

#[test]
fn test_validate_reserved_headers_case_insensitively() {
    for (name, expected) in [
        ("cOnTeNt-TyPe", CONTENT_TYPE_IN_HEADERS),
        ("CONTENT-length", PROTECTED_HEADER_ERROR),
        ("Transfer-Encoding", PROTECTED_HEADER_ERROR),
    ] {
        let config = MockConfig {
            headers: headers(&[(name, "value")]),
            ..Default::default()
        };
        assert_eq!(config.validate().unwrap_err(), expected);
    }
}

#[test]
fn test_validate_duplicate_headers_case_insensitively() {
    let config = MockConfig {
        headers: headers(&[("X-Mock", "one"), ("x-mock", "two")]),
        ..Default::default()
    };
    assert_eq!(config.validate().unwrap_err(), DUPLICATE_HEADER_NAME);
}

#[test]
fn test_validation_priority_is_deterministic() {
    let mut config = MockConfig {
        status_code: 99,
        delay: Some(MAX_DELAY_MS + 1),
        content_type: "bad\r\ncontent-type".to_string(),
        headers: headers(&[("bad header", "bad\r\nvalue")]),
        ..Default::default()
    };
    assert_eq!(
        config.validate().unwrap_err(),
        "Invalid status code: 99. Must be between 200 and 599"
    );

    config.status_code = 200;
    assert_eq!(
        config.validate().unwrap_err(),
        "delay must not exceed 30000ms, got 30001ms"
    );

    config.delay = None;
    assert_eq!(config.validate().unwrap_err(), INVALID_CONTENT_TYPE);

    config.content_type = default_content_type();
    assert_eq!(config.validate().unwrap_err(), INVALID_HEADER_NAME);

    config.headers = headers(&[("X-Mock", "bad\r\nvalue")]);
    assert_eq!(config.validate().unwrap_err(), INVALID_HEADER_VALUE);

    config.headers = headers(&[("Content-Type", "text/plain")]);
    assert_eq!(config.validate().unwrap_err(), CONTENT_TYPE_IN_HEADERS);

    config.headers = headers(&[("Content-Length", "1")]);
    assert_eq!(config.validate().unwrap_err(), PROTECTED_HEADER_ERROR);

    config.headers = headers(&[("Transfer-Encoding", "chunked")]);
    assert_eq!(config.validate().unwrap_err(), PROTECTED_HEADER_ERROR);

    config.headers = headers(&[("X-Mock", "one"), ("x-mock", "two")]);
    assert_eq!(config.validate().unwrap_err(), DUPLICATE_HEADER_NAME);
}

#[test]
fn test_serde_serialization() {
    let mut headers = HashMap::new();
    headers.insert("X-Custom".to_string(), "value".to_string());

    let config = MockConfig {
        status_code: 201,
        body: Some(r#"{"created":true}"#.to_string()),
        headers: Some(headers),
        content_type: "application/json".to_string(),
        delay: Some(100),
        terminate: true,
    };

    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("201"));
    assert!(json.contains("created"));
    assert!(json.contains("X-Custom"));
    assert!(json.contains("100"));
}

#[test]
fn test_serde_deserialization() {
    let json = r#"{
            "statusCode": 500,
            "body": "Internal Server Error",
            "contentType": "text/plain",
            "delay": 50
        }"#;

    let config: MockConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.status_code, 500);
    assert_eq!(config.body, Some("Internal Server Error".to_string()));
    assert_eq!(config.content_type, "text/plain");
    assert_eq!(config.delay, Some(50));
}

#[test]
fn test_serde_with_defaults() {
    let json = r#"{"body": "test"}"#;
    let config: MockConfig = serde_json::from_str(json).unwrap();

    assert_eq!(config.status_code, 200); // default
    assert_eq!(config.content_type, "application/json"); // default
    assert_eq!(config.body, Some("test".to_string()));
}
