use super::*;

#[test]
fn parses_root_members_indices_and_quoted_members() {
    let path = DefiniteJsonPath::parse(r#"$.user[0]["name.with.dots"]"#).unwrap();
    assert_eq!(
        path.segments(),
        &[
            JsonPathSegment::Member("user".to_string()),
            JsonPathSegment::Index(0),
            JsonPathSegment::Member("name.with.dots".to_string()),
        ]
    );
}

#[test]
fn parses_json_escapes_in_quoted_member() {
    let path = DefiniteJsonPath::parse(r#"$["quote\"and\u0020space"]"#).unwrap();
    assert_eq!(
        path.segments(),
        &[JsonPathSegment::Member("quote\"and space".to_string())]
    );
}

#[test]
fn root_path_is_valid() {
    let path = DefiniteJsonPath::parse("$").unwrap();
    assert!(path.segments().is_empty());
}

#[test]
fn rejects_indefinite_or_legacy_paths() {
    for invalid in [
        "user.name",
        "$.*",
        "$..name",
        "$[?(@.ok)]",
        "$[0:2]",
        "$[-1]",
        "$['name']",
        "$[0,1]",
    ] {
        assert!(
            DefiniteJsonPath::parse(invalid).is_err(),
            "{invalid:?} should be rejected"
        );
    }
}

#[test]
fn rejects_invalid_or_overflowing_indices() {
    assert!(DefiniteJsonPath::parse("$[]").is_err());
    assert!(DefiniteJsonPath::parse("$[18446744073709551616]").is_err());
}

#[test]
fn enforces_encoded_length_and_depth_limits() {
    let too_long = format!("$[\"{}\"]", "a".repeat(MAX_JSON_PATH_LEN));
    assert!(DefiniteJsonPath::parse(&too_long).is_err());

    let too_deep = format!("${}", ".a".repeat(MAX_JSON_PATH_DEPTH + 1));
    assert!(DefiniteJsonPath::parse(&too_deep).is_err());
}
