use super::*;

#[test]
fn parses_comma_and_space_separated_with_ns_expansion() {
    let got = parse_plugin_ref_list("foo, bar baz/qux ,, foo", "default");
    assert_eq!(
        got,
        vec![
            "default/foo".to_string(),
            "default/bar".to_string(),
            "baz/qux".to_string()
        ]
    );
}

#[test]
fn blank_yields_empty() {
    assert!(parse_plugin_ref_list("   ", "default").is_empty());
    assert!(parse_plugin_ref_list("", "ns").is_empty());
}

#[test]
fn dedups_preserving_first_order() {
    let got = parse_plugin_ref_list("a b a", "ns");
    assert_eq!(got, vec!["ns/a".to_string(), "ns/b".to_string()]);
}
