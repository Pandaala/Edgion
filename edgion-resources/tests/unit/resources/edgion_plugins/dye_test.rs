use super::*;

fn rule(name: &str, on: &[DyeOutcome], value: Option<&str>) -> DyeRule {
    DyeRule {
        name: name.to_string(),
        on: on.to_vec(),
        value: value.map(|v| v.to_string()),
        conditions: None,
    }
}

const ALL_CAPS: &[DyeOutcome] = &[
    DyeOutcome::Success,
    DyeOutcome::Fallback,
    DyeOutcome::Reject,
    DyeOutcome::FailOpen,
    DyeOutcome::FailClose,
];

fn suite(request: Vec<DyeRule>, response: Vec<DyeRule>) -> DyeSuite {
    DyeSuite { request, response }
}

#[test]
fn on_defaults_to_success_in_serde() {
    let r: DyeRule = serde_yaml::from_str("name: X-Track").unwrap();
    assert_eq!(r.on, vec![DyeOutcome::Success]);
    // kebab-case round trip
    let r2: DyeRule = serde_yaml::from_str("name: X-O\non: [fail-open, fail-close]").unwrap();
    assert_eq!(r2.on, vec![DyeOutcome::FailOpen, DyeOutcome::FailClose]);
}

#[test]
fn valid_minimal_suite_passes() {
    let s = suite(vec![rule("X-Track", &[DyeOutcome::Success], None)], vec![]);
    assert!(s.validation_error(true, ALL_CAPS, &[]).is_none());
}

#[test]
fn response_header_policy_does_not_widen_request_dye_restrictions() {
    for name in ["Keep-Alive", "Proxy-Connection", "Proxy-Authenticate"] {
        let request = suite(vec![rule(name, &[DyeOutcome::Success], None)], vec![]);
        assert!(request.validation_error(true, ALL_CAPS, &[]).is_none());
        let response = suite(vec![], vec![rule(name, &[DyeOutcome::Success], None)]);
        assert_eq!(
            response.validation_error(true, ALL_CAPS, &[]).as_deref(),
            Some("response header mutation targets a protected protocol field")
        );
    }
    for name in ["Content-Type", "Content-Encoding", "Set-Cookie", "X-Region"] {
        let response = suite(vec![], vec![rule(name, &[DyeOutcome::Success], None)]);
        assert!(response.validation_error(true, ALL_CAPS, &[]).is_none());
    }
}

#[test]
fn invalid_and_denied_names_rejected() {
    let s = suite(vec![], vec![rule("Bad Name", &[DyeOutcome::Success], None)]);
    assert!(s
        .validation_error(true, ALL_CAPS, &[])
        .unwrap()
        .contains("invalid header name"));
    for denied in [
        "Host",
        "content-length",
        "Transfer-Encoding",
        "Connection",
        "TE",
        "Upgrade",
        "Trailer",
    ] {
        let s = suite(vec![], vec![rule(denied, &[DyeOutcome::Success], None)]);
        assert!(
            s.validation_error(true, ALL_CAPS, &[]).is_some(),
            "{denied} must be denied"
        );
    }
}

#[test]
fn capability_check_rejects_undeclared_outcome() {
    let s = suite(vec![], vec![rule("X-O", &[DyeOutcome::FailOpen], None)]);
    let err = s.validation_error(true, &[DyeOutcome::Success], &[]).unwrap();
    assert!(err.contains("fail-open"), "{err}");
}

#[test]
fn mixing_success_with_failure_rejected() {
    let s = suite(
        vec![],
        vec![rule("X-O", &[DyeOutcome::Success, DyeOutcome::Reject], None)],
    );
    assert!(s.validation_error(true, ALL_CAPS, &[]).unwrap().contains("mix"));
}

#[test]
fn request_direction_success_only_and_request_stage_only() {
    let s = suite(vec![rule("X-O", &[DyeOutcome::FailOpen], None)], vec![]);
    assert!(s.validation_error(true, ALL_CAPS, &[]).is_some());
    // request-direction rules invalid on non-request-stage entries
    let s2 = suite(vec![rule("X-T", &[DyeOutcome::Success], None)], vec![]);
    assert!(s2.validation_error(false, ALL_CAPS, &[]).is_some());
}

#[test]
fn key_collision_rejected_case_insensitively() {
    let s = suite(vec![rule("X-Backend-Target", &[DyeOutcome::Success], None)], vec![]);
    assert!(s
        .validation_error(true, ALL_CAPS, &["x-backend-target"])
        .unwrap()
        .contains("collide"));
}

#[test]
fn template_values_validated() {
    // valid template
    let s = suite(
        vec![],
        vec![rule("X-D", &[DyeOutcome::FailOpen], Some("v ${header:X-Id}"))],
    );
    assert!(s.validation_error(true, ALL_CAPS, &[]).is_none());
    // parse error
    let s = suite(
        vec![],
        vec![rule("X-D", &[DyeOutcome::FailOpen], Some("bad ${header:X"))],
    );
    assert!(s.validation_error(true, ALL_CAPS, &[]).is_some());
    // secretRef rejected
    let s = suite(
        vec![],
        vec![rule("X-D", &[DyeOutcome::FailOpen], Some("${secretRef:ns/n:k}"))],
    );
    assert!(s.validation_error(true, ALL_CAPS, &[]).unwrap().contains("secretRef"));
    // control bytes in literal rejected
    let s = suite(
        vec![],
        vec![rule("X-D", &[DyeOutcome::FailOpen], Some("evil\r\nInjected: 1"))],
    );
    assert!(s.validation_error(true, ALL_CAPS, &[]).is_some());
}

#[test]
fn sanitize_field_value_strips_control_bytes() {
    assert_eq!(sanitize_field_value("ok value"), "ok value");
    assert_eq!(sanitize_field_value("a\r\nb\u{0}c\u{7f}d"), "abcd");
    assert_eq!(sanitize_field_value("tab\tok"), "tab\tok");
}

#[test]
fn sanitizer_matches_character_filter_for_ascii_and_multibyte_text() {
    let all_ascii: String = (0u8..=127).map(char::from).collect();
    for text in [
        String::new(),
        all_ascii,
        "plain\ttext éñ€©π".into(),
        "a\r\nb\u{0}c\u{7f}é".into(),
    ] {
        let expected: String = text
            .chars()
            .filter(|c| *c == '\t' || (*c as u32 >= 0x20 && *c as u32 != 0x7f))
            .collect();
        assert_eq!(sanitize_field_value(&text), expected);
    }
    for byte in 0u8..=127 {
        let text = char::from(byte).to_string();
        let expected: String = text
            .chars()
            .filter(|c| *c == '\t' || (*c as u32 >= 0x20 && *c as u32 != 0x7f))
            .collect();
        assert_eq!(sanitize_field_value(&text), expected);
    }
}

#[test]
fn sanitizer_borrows_valid_input_and_owns_filtered_input() {
    for input in ["", "plain text", "tab\tok", "éñ€©π"] {
        let sanitized = sanitize_field_value(input);
        assert!(matches!(sanitized, Cow::Borrowed(_)));
        assert_eq!(sanitized.as_ptr(), input.as_ptr());
        assert_eq!(sanitized, input);
    }
    assert!(matches!(sanitize_field_value("a\r\nb"), Cow::Owned(value) if value == "ab"));
}
