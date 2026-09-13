/// Immutable build-time path precedence, ordered from least to most specific.
///
/// Exact comes first, followed by aggregate static constraints and path depth.
/// Parameter identifiers are capture keys, never specificity inputs. The static
/// counts extend Gateway API precedence to Edgion's named-parameter syntax;
/// they do not prefer a static segment merely because it occurs earlier.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PathSpecificity {
    pub is_exact: bool,
    pub static_segment_count: usize,
    /// Unicode characters in static content, including slashes, after `/::`
    /// escapes are resolved. Parameter markers and identifiers contribute zero.
    pub static_char_count: usize,
    pub segment_count: usize,
}

impl PathSpecificity {
    fn new(normalized: &str, is_exact: bool) -> Self {
        let mut specificity = Self {
            is_exact,
            static_segment_count: 0,
            static_char_count: 0,
            segment_count: 0,
        };
        for (index, segment) in normalized.split('/').enumerate() {
            if index > 0 {
                specificity.static_char_count += 1;
            }
            if segment.is_empty() {
                continue;
            }
            specificity.segment_count += 1;
            // Match matcher::path::builder::parse_path: only a colon immediately
            // after '/' starts a parameter or escape. An internal ':' is literal.
            let static_segment = if index > 0 && segment.starts_with(':') {
                if let Some(escaped) = segment.strip_prefix("::") {
                    specificity.static_char_count += 1;
                    escaped
                } else {
                    continue;
                }
            } else {
                segment
            };
            specificity.static_segment_count += 1;
            specificity.static_char_count += static_segment.chars().count();
        }
        specificity
    }
}

/// Compiled route path pattern for radix tree matching.
#[derive(Debug)]
pub struct RadixPath {
    /// Normalized path (consecutive slashes merged; non-root Prefix trailing slash removed)
    pub normalized: String,
    /// Path precedence input used only when assigning the snapshot's ranks.
    pub specificity: PathSpecificity,
    /// Whether this is a prefix match (true) or exact match (false)
    pub is_prefix_match: bool,
    /// Index of the associated route
    pub route_idx: usize,
    /// Global precedence rank within the route snapshot (0 = highest priority).
    ///
    /// `RadixPath::new` cannot compute this: the rank depends on every other
    /// path in the same snapshot. It is left at 0 here and filled in by
    /// `RadixRouteMatchEngine::initialize_internal` once all paths are known,
    /// so only read it through a fully built engine.
    pub precedence_rank: u32,
}

impl RadixPath {
    /// Create a new RadixPath from a path pattern.
    ///
    /// # Arguments
    /// * `path` - The path pattern (e.g., "/users/:id/profile")
    /// * `route_idx` - Index of the associated route
    /// * `is_prefix` - Whether this is a prefix match pattern
    pub fn new(path: &str, route_idx: usize, is_prefix: bool) -> Self {
        let is_prefix_match = is_prefix;

        // Normalize path: merge consecutive slashes and validate
        let mut normalized = normalize_path(path);

        // Warn if path was normalized (had consecutive slashes or other issues)
        if normalized != path {
            tracing::warn!(
                "Path normalized: '{}' -> '{}' (consecutive slashes merged)",
                path,
                normalized
            );
        }

        // Validate path starts with '/'
        if !normalized.starts_with('/') && !normalized.is_empty() {
            tracing::warn!("Path should start with '/': '{}'", path);
        }

        // Check for empty parameter names
        if normalized.contains("/:/") || normalized.ends_with("/:") {
            tracing::warn!("Path contains empty parameter name: '{}'", path);
        }

        if is_prefix_match {
            edgion_resources::path_normalization::normalize::canonicalize_path_prefix(&mut normalized);
        }
        let specificity = PathSpecificity::new(&normalized, !is_prefix_match);

        Self {
            normalized,
            specificity,
            is_prefix_match,
            route_idx,
            precedence_rank: 0,
        }
    }

    /// Get the path to use for radix tree insertion.
    /// This returns the normalized path.
    pub fn tree_key(&self) -> &str {
        &self.normalized
    }
}

/// Normalize a path by merging consecutive slashes.
///
/// Examples:
/// - "/api//users" -> "/api/users"
/// - "/api///v1//users/" -> "/api/v1/users/"
/// - "/" -> "/"
fn normalize_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    let mut result = String::with_capacity(path.len());
    let mut prev_was_slash = false;

    for c in path.chars() {
        if c == '/' {
            if !prev_was_slash {
                result.push(c);
            }
            prev_was_slash = true;
        } else {
            result.push(c);
            prev_was_slash = false;
        }
    }

    result
}

#[cfg(test)]
mod tests {
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
}
