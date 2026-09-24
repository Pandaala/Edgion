use super::*;

#[test]
fn accepts_single_char() {
    assert!(validate_group_name("a").is_ok());
    assert!(validate_group_name("0").is_ok());
}

#[test]
fn accepts_typical_label() {
    assert!(validate_group_name("partner-a").is_ok());
    assert!(validate_group_name("internal_keys").is_ok());
    assert!(validate_group_name("v1.0").is_ok());
}

#[test]
fn rejects_empty() {
    assert_eq!(validate_group_name("").unwrap_err(), "group name is empty");
}

#[test]
fn rejects_too_long() {
    let name = "a".repeat(64);
    assert!(validate_group_name(&name).unwrap_err().contains("exceeds 63 chars"));
}

#[test]
fn rejects_leading_hyphen() {
    assert!(validate_group_name("-foo")
        .unwrap_err()
        .contains("start with alphanumeric"));
}

#[test]
fn rejects_trailing_dot() {
    assert!(validate_group_name("foo.")
        .unwrap_err()
        .contains("end with alphanumeric"));
}

#[test]
fn rejects_invalid_char() {
    assert!(validate_group_name("foo bar")
        .unwrap_err()
        .contains("invalid character"));
    assert!(validate_group_name("foo/bar")
        .unwrap_err()
        .contains("invalid character"));
}
