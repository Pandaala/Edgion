use super::prefix_replacement_parts;

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
        (("/foo///bar", "//foo//", "/xyz///"), "/xyz/bar"),
        (("/foo///", "/foo", "/xyz///"), "/xyz/"),
        (("/foo", "/foo", "/xyz///"), "/xyz///"),
        (("/foo", "/foo/bar", "/xyz///"), "/xyz///"),
        (("", "/", ""), "/"),
    ];

    for ((request_path, matched_prefix, replacement), expected) in cases {
        let (parts, allocations) = crate::test_allocation_counter::count_allocations(|| {
            prefix_replacement_parts(request_path, matched_prefix, replacement)
        });
        assert_eq!(allocations, 0);
        assert_eq!(parts.concat(), expected);
    }
}
