use super::*;

#[test]
fn body_predicate_requires_explicit_max_response_bytes() {
    // A success.body predicate on a connection left at the 1KB default must be rejected.
    let c: ExternalHttpConnection = yaml_serde::from_str(
            "target:\n  url: https://api.example.com\nmaxResponseBytes: 512\nsuccess:\n  body:\n    - pointer: /code\n      equals: 0\n",
        )
        .unwrap();
    let err = validate_connection_target(&c).expect("must require explicit maxResponseBytes");
    assert!(err.contains("maxResponseBytes"), "got: {err}");
}

#[test]
fn body_predicate_passes_with_adequate_max_response_bytes() {
    let c: ExternalHttpConnection = yaml_serde::from_str(
            "target:\n  url: https://api.example.com\nmaxResponseBytes: 8192\nsuccess:\n  body:\n    - pointer: /code\n      equals: 0\n",
        )
        .unwrap();
    assert!(validate_connection_target(&c).is_none());
}

#[test]
fn no_body_predicate_allows_default_max_response_bytes() {
    // The default 1KB cap is fine when there's no body predicate to inspect.
    let c: ExternalHttpConnection = yaml_serde::from_str("target:\n  url: https://api.example.com\n").unwrap();
    assert!(validate_connection_target(&c).is_none());
}
