use super::*;

fn key(path: &str, is_prefix: bool) -> PathSpecificity {
    RadixPath::new(path, 0, is_prefix).specificity
}

#[test]
fn compiled_path_preserves_match_and_route_metadata() {
    for is_prefix in [false, true] {
        let path = RadixPath::new("/users/:userId/posts/:postId", 42, is_prefix);
        assert_eq!(path.tree_key(), "/users/:userId/posts/:postId");
        assert_eq!(path.route_idx, 42);
        assert_eq!(path.is_prefix_match, is_prefix);
        assert_eq!(path.specificity.is_exact, !is_prefix);
        assert_eq!(path.specificity.static_segment_count, 2);
        assert_eq!(path.specificity.segment_count, 4);
    }
}

#[test]
fn parameter_identifiers_never_contribute_to_specificity() {
    for is_prefix in [false, true] {
        let expected = key("/users/:x/posts/:y", is_prefix);
        for pattern in [
            "/users/:customerIdentifier/posts/:postIdentifier",
            "/users/:é/posts/:€",
            "/users/:ééé/posts/:ππ",
            "/users/:key:value/posts/:id::tail",
            "/users/:/posts/:",
        ] {
            assert_eq!(key(pattern, is_prefix), expected, "{pattern}");
        }
        assert_eq!(expected.static_segment_count, 2);
        assert_eq!(expected.static_char_count, 14);
        assert_eq!(expected.segment_count, 4);
    }
}

#[test]
fn exact_always_precedes_prefix_even_with_fewer_static_constraints() {
    assert!(key("/:x", false) > key("/long-static-segment", true));
    assert!(key("/a/:x", false) > key("/a/b", true));
    assert!(key("/a/b", false) > key("/a/:customerIdentifier", true));
}

#[test]
fn static_exact_precedes_parameter_exact() {
    assert!(key("/a/b", false) > key("/a/:identifier", false));
}

#[test]
fn static_prefix_precedes_parameter_prefix() {
    assert!(key("/a/b", true) > key("/a/:identifier", true));
}

#[test]
fn all_four_match_categories_have_the_expected_order() {
    let ordered = [
        key("/a/b", false),
        key("/a/:identifier", false),
        key("/a/b", true),
        key("/a/:identifier", true),
    ];
    assert!(ordered.windows(2).all(|pair| pair[0] > pair[1]));
}

#[test]
fn aggregate_static_segments_precede_earlier_static_positions() {
    // Both patterns match /a/b/c. The second has more static constraints.
    assert!(key("/:x/b/c", false) > key("/a/:x/:y", false));
    // Equal aggregate constraints intentionally defer to non-path precedence.
    assert_eq!(key("/a/:x/c", false), key("/:x/b/c", false));
}

#[test]
fn static_segment_count_precedes_static_character_count() {
    // Both match /longsegment/b/c.
    assert!(key("/:x/b/c", false) > key("/longsegment/:x/:y", false));
}

#[test]
fn static_character_count_precedes_depth() {
    // Both prefixes match /longsegment/b/c.
    assert!(key("/longsegment/:x", true) > key("/:x/b/:y", true));
}

#[test]
fn depth_breaks_equal_static_counts() {
    // Both prefixes match /a/ab/c; static counts are (1, 4).
    assert!(key("/a/:x/:y", true) > key("/:x/ab", true));
}

#[test]
fn root_and_parameter_only_prefixes_are_ordered_by_depth() {
    assert_eq!(
        key("/", true),
        PathSpecificity {
            is_exact: false,
            static_segment_count: 0,
            static_char_count: 1,
            segment_count: 0,
        }
    );
    assert!(key("/:x", true) > key("/", true));
    assert!(key("/:x/:y", true) > key("/:x", true));
    // An existing static constraint takes precedence over parameter depth.
    assert!(key("/a", true) > key("/:x/:y", true));
}

// Retains gw-http-match-02's character-unit guard under the new path key.
#[test]
fn static_specificity_counts_unicode_characters_not_bytes() {
    assert_eq!(key("/café", true), key("/cafe", true));
    assert_eq!(key("/€", true), key("/a", true));
    assert_eq!(key("/café", true).static_char_count, 5);
    assert!(key("/caféx", true) > key("/café", true));
}

#[test]
fn escaped_colons_count_only_the_matched_literal() {
    for (pattern, literal) in [
        ("/api/::version/data", "/api/:version/data"),
        ("/:::x", "/::x"),
        ("/::", "/:"),
        ("/::x/::y", "/:x/:y"),
    ] {
        let specificity = key(pattern, false);
        assert_eq!(specificity.static_char_count, literal.chars().count(), "{pattern}");
        assert_eq!(specificity.static_segment_count, specificity.segment_count);
    }
    assert!(key("/a/::literal", false) > key("/a/:identifier", false));
}

#[test]
fn internal_colons_and_missing_leading_slash_follow_radix_parser() {
    for pattern in ["/foo::x", ":id", "::literal", "/foo:bar"] {
        let specificity = key(pattern, false);
        assert_eq!(specificity.static_segment_count, 1, "{pattern}");
        assert_eq!(specificity.static_char_count, pattern.chars().count(), "{pattern}");
    }
    assert_eq!(key("/:id::tail", false), key("/:x", false));
}

#[test]
fn repeated_slashes_normalize_before_specificity() {
    for (input, normalized) in [
        ("/api//users", "/api/users"),
        ("/api///v1//users/", "/api/v1/users"),
        ("//api", "/api"),
        ("/a//:long///c", "/a/:long/c"),
        ("///", "/"),
    ] {
        let path = RadixPath::new(input, 0, true);
        assert_eq!(path.tree_key(), normalized);
        assert_eq!(path.specificity, key(normalized, true));
    }
    assert_eq!(normalize_path(""), "");
    assert_eq!(normalize_path("/"), "/");
}

#[test]
fn trailing_slash_only_contributes_to_exact_specificity() {
    for pattern in ["/api", "/api/:id"] {
        let with_slash = format!("{pattern}/");
        let a = RadixPath::new(pattern, 0, true);
        let b = RadixPath::new(&with_slash, 0, true);
        assert_eq!(a.tree_key(), pattern);
        assert_eq!(b.tree_key(), pattern);
        assert_eq!(a.specificity, b.specificity);
        let a = RadixPath::new(pattern, 0, false);
        let b = RadixPath::new(&with_slash, 0, false);
        assert_eq!(b.tree_key(), with_slash);
        assert_eq!(a.specificity.segment_count, b.specificity.segment_count);
        assert_eq!(b.specificity.static_char_count, a.specificity.static_char_count + 1);
        assert!(b.specificity > a.specificity);
    }
}

#[test]
fn standard_literal_prefix_extensions_keep_longest_path_precedence() {
    let paths = ["/", "/api", "/api/users", "/api/users/profile"];
    assert!(paths.windows(2).all(|pair| key(pair[1], true) > key(pair[0], true)));
    assert!(key("/api/users", false) > key("/api/users", true));
    assert_eq!(key("/api-v1/users_list", false).static_segment_count, 2);
}
