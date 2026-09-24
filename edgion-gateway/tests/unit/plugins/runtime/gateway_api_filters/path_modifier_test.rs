use super::replace_prefix_match;

#[test]
fn replaces_prefix_on_element_boundaries() {
    let cases = [
        (("/bar", "/", "/xyz"), "/xyz/bar"),
        (("/bar", "/", "/xyz/"), "/xyz/bar"),
        (("/", "/", "/xyz"), "/xyz"),
        (("/bar", "/", ""), "/bar"),
        (("/", "/", ""), "/"),
        (("/foo", "/foo", ""), "/"),
        (("/foo/", "/foo", ""), "/"),
        (("/foo/", "/foo", "/xyz"), "/xyz/"),
        (("/foo/bar", "/foo/", "/xyz"), "/xyz/bar"),
        (("/users/alice/orders", "/users/:id", "/v2"), "/v2/orders"),
        (("/users/aé/orders", "/users/:id", "/v2"), "/v2/orders"),
        (("/café/menu", "/café", "/shop"), "/shop/menu"),
    ];

    for ((request_path, matched_prefix, replacement), expected) in cases {
        assert_eq!(
            replace_prefix_match(request_path, matched_prefix, replacement),
            expected
        );
    }
}
