use super::*;
#[test]
fn source_urls_send_bounded_defaults_and_encode_continuation() {
    let url =
        build_list_url_with_pagination("http://localhost/api/v1/namespaced/HTTPRoute", None, Some("next+/?=")).unwrap();
    let parsed = reqwest::Url::parse(&url).unwrap();
    let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(pairs["limit"], "10");
    assert_eq!(pairs["continue"], "next+/?=");
    for limit in [0, 101] {
        assert!(
            build_list_url_with_pagination("http://localhost/api/v1/keys/cluster/GatewayClass", Some(limit), None)
                .is_err()
        );
    }
    let diagnostic =
        build_list_url_with_pagination("http://localhost/configclient/HTTPRoute/list", None, None).unwrap();
    assert!(!diagnostic.contains("limit="));
}
