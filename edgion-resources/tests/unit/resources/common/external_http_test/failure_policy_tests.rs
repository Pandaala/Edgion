use super::*;

#[test]
fn failure_policy_defaults() {
    let c = ExternalHttpConnection::default();
    assert_eq!(c.status_on_error, 503);
}

#[test]
fn failure_policy_parses_top_level() {
    let c: ExternalHttpConnection =
        yaml_serde::from_str("target:\n  url: https://api.example.com\nstatusOnError: 502\n").unwrap();
    assert_eq!(c.status_on_error, 502);
}
