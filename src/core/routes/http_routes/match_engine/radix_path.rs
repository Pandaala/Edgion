/// Compiled route path pattern for radix tree matching.
///
/// After the radix tree now supports `:param` parameter matching internally,
/// this struct is greatly simplified - no longer needs `radix_key` extraction
/// or `match_segments` parsing.
#[derive(Debug, Clone)]
pub struct RadixPath {
    /// Original path pattern (e.g., "/api/:version/users")
    pub original: String,
    /// Normalized path (consecutive slashes merged)
    pub normalized: String,
    /// Priority weight for sorting (higher = more specific)
    /// Order: static exact > param exact > static prefix > param prefix
    pub priority_weight: usize,
    /// Whether this is a prefix match (true) or exact match (false)
    pub is_prefix_match: bool,
    /// Index of the associated route
    pub route_idx: usize,
    /// Number of path segments (for exact match verification)
    pub segment_count: usize,
    /// Whether the path contains parameter segments
    pub has_params: bool,
}

impl RadixPath {
    /// Create a new RadixPath from a path pattern.
    ///
    /// # Arguments
    /// * `path` - The path pattern (e.g., "/users/:id/profile")
    /// * `route_idx` - Index of the associated route
    /// * `is_prefix` - Whether this is a prefix match pattern
    pub fn new(path: &str, route_idx: usize, is_prefix: bool) -> Self {
        let original = path.to_string();
        let is_prefix_match = is_prefix;

        // Normalize path: merge consecutive slashes and validate
        let normalized = normalize_path(path);

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

        // Count segments and detect parameters
        let (segment_count, has_params) = count_segments_and_params(&normalized);

        // Priority calculation:
        // - More segments = higher priority (more specific path)
        // - Static > Param (static routes are more specific)
        // - Exact > Prefix (exact matches are more specific)
        //
        // Formula: segment_count * 4 + type_bonus
        // type_bonus: static_exact=3, param_exact=2, static_prefix=1, param_prefix=0
        let type_bonus = match (is_prefix_match, has_params) {
            (false, false) => 3, // static exact (highest)
            (false, true) => 2,  // param exact
            (true, false) => 1,  // static prefix
            (true, true) => 0,   // param prefix (lowest)
        };
        let priority_weight = segment_count * 4 + type_bonus;

        Self {
            original,
            normalized,
            priority_weight,
            is_prefix_match,
            route_idx,
            segment_count,
            has_params,
        }
    }

    /// Get the path to use for radix tree insertion.
    /// This returns the normalized path.
    pub fn tree_key(&self) -> &str {
        &self.normalized
    }

    /// Return a string describing the match type for debugging.
    pub fn match_type_str(&self) -> &str {
        match (self.is_prefix_match, self.has_params) {
            (true, true) => "ParamPrefix",
            (true, false) => "Prefix",
            (false, true) => "Param",
            (false, false) => "Exact",
        }
    }

    /// Check if a request path matches this pattern for exact match validation.
    ///
    /// This is only used for non-prefix patterns to verify the request path
    /// has the same number of segments as the pattern.
    ///
    /// For prefix patterns, this always returns true.
    pub fn matches_exact(&self, request_path: &str) -> bool {
        if self.is_prefix_match {
            return true;
        }

        // For exact match, verify segment count matches
        let request_segments = count_path_segments(request_path);
        request_segments == self.segment_count
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

/// Count the number of path segments in a request path.
///
/// Examples:
/// - "/" -> 0
/// - "/api" -> 1
/// - "/api/users" -> 2
/// - "/api/users/" -> 2
#[inline]
fn count_path_segments(path: &str) -> usize {
    if path.is_empty() || path == "/" {
        return 0;
    }

    path.split('/').filter(|s| !s.is_empty()).count()
}

/// Count segments and detect if path contains parameters.
///
/// This handles the `::` escape sequence (double colon becomes literal colon),
/// matching the behavior of radix tree's parse_path:
/// - `/:name` is a parameter segment
/// - `/::name` is escaped to `/:name` (literal colon, not a parameter)
///
/// Returns (segment_count, has_params)
fn count_segments_and_params(path: &str) -> (usize, bool) {
    let mut segment_count = 0;
    let mut has_params = false;
    let bytes = path.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Skip leading slash
        if bytes[i] == b'/' {
            i += 1;
            continue;
        }

        // Start of a new segment
        segment_count += 1;

        // Check if this segment is a parameter (starts with single `:`)
        // This matches radix tree's parse_path logic:
        // - `:name` after `/` is a parameter
        // - `::name` after `/` is escaped (literal `:name`)
        if bytes[i] == b':' {
            // Check for `::` escape (not a parameter)
            if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                // `::` is escaped, not a parameter
            } else {
                has_params = true;
            }
        }

        // Skip to next slash or end
        while i < bytes.len() && bytes[i] != b'/' {
            i += 1;
        }
    }

    (segment_count, has_params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match_simple_path() {
        let path = RadixPath::new("/api", 0, false);
        assert_eq!(path.original, "/api");
        assert_eq!(path.normalized, "/api");
        assert!(!path.is_prefix_match);
        assert_eq!(path.segment_count, 1);
        assert!(!path.has_params);
        assert_eq!(path.match_type_str(), "Exact");

        // Exact match validation
        assert!(path.matches_exact("/api"));
        assert!(!path.matches_exact("/api/users"));
        assert!(!path.matches_exact("/"));
    }

    #[test]
    fn test_prefix_match_simple_path() {
        let path = RadixPath::new("/api", 0, true);
        assert_eq!(path.original, "/api");
        assert_eq!(path.normalized, "/api");
        assert!(path.is_prefix_match);
        assert_eq!(path.segment_count, 1);
        assert!(!path.has_params);
        assert_eq!(path.match_type_str(), "Prefix");

        // Prefix match always returns true for matches_exact
        assert!(path.matches_exact("/api"));
        assert!(path.matches_exact("/api/users"));
        assert!(path.matches_exact("/"));
    }

    #[test]
    fn test_exact_match_with_param() {
        let path = RadixPath::new("/users/:id", 0, false);
        assert_eq!(path.original, "/users/:id");
        assert_eq!(path.normalized, "/users/:id");
        assert!(!path.is_prefix_match);
        assert_eq!(path.segment_count, 2);
        assert!(path.has_params);
        assert_eq!(path.match_type_str(), "Param");

        // Segment count validation
        assert!(path.matches_exact("/users/123"));
        assert!(!path.matches_exact("/users/123/profile"));
        assert!(!path.matches_exact("/users"));
    }

    #[test]
    fn test_prefix_match_with_param() {
        let path = RadixPath::new("/users/:id", 0, true);
        assert_eq!(path.original, "/users/:id");
        assert_eq!(path.normalized, "/users/:id");
        assert!(path.is_prefix_match);
        assert_eq!(path.segment_count, 2);
        assert!(path.has_params);
        assert_eq!(path.match_type_str(), "ParamPrefix");

        // Prefix match always allows
        assert!(path.matches_exact("/users/123"));
        assert!(path.matches_exact("/users/123/profile"));
    }

    #[test]
    fn test_multiple_params() {
        let path = RadixPath::new("/api/:version/users/:id", 0, false);
        assert_eq!(path.segment_count, 4);
        assert!(path.has_params);

        assert!(path.matches_exact("/api/v1/users/123"));
        assert!(!path.matches_exact("/api/v1/users"));
        assert!(!path.matches_exact("/api/v1/users/123/extra"));
    }

    #[test]
    fn test_param_with_literal_suffix() {
        let path = RadixPath::new("/users/:id/profile", 0, false);
        assert_eq!(path.segment_count, 3);
        assert!(path.has_params);

        assert!(path.matches_exact("/users/123/profile"));
        assert!(!path.matches_exact("/users/123/profile/extra"));
    }

    #[test]
    fn test_root_path_exact() {
        let path = RadixPath::new("/", 0, false);
        assert_eq!(path.original, "/");
        assert_eq!(path.normalized, "/");
        assert_eq!(path.segment_count, 0);
        assert!(!path.has_params);

        assert!(path.matches_exact("/"));
        assert!(!path.matches_exact("/api"));
    }

    #[test]
    fn test_root_path_prefix() {
        let path = RadixPath::new("/", 0, true);
        assert_eq!(path.original, "/");
        assert_eq!(path.normalized, "/");
        assert_eq!(path.segment_count, 0);

        // Prefix always allows
        assert!(path.matches_exact("/"));
        assert!(path.matches_exact("/api"));
    }

    #[test]
    fn test_priority_weight_calculation() {
        let exact = RadixPath::new("/api/users", 0, false);
        let prefix = RadixPath::new("/api/users", 0, true);

        // Exact match should have higher priority
        assert!(exact.priority_weight > prefix.priority_weight);
    }

    #[test]
    fn test_static_vs_param_priority() {
        // Static exact should have highest priority
        let static_exact = RadixPath::new("/users/admin", 0, false);
        let param_exact = RadixPath::new("/users/:id", 1, false);
        let static_prefix = RadixPath::new("/users/admin", 2, true);
        let param_prefix = RadixPath::new("/users/:id", 3, true);

        // Priority order: static_exact > param_exact > static_prefix > param_prefix
        assert!(static_exact.priority_weight > param_exact.priority_weight);
        assert!(param_exact.priority_weight > static_prefix.priority_weight);
        assert!(static_prefix.priority_weight > param_prefix.priority_weight);
    }

    #[test]
    fn test_longer_path_higher_priority() {
        let short = RadixPath::new("/api", 0, false);
        let long = RadixPath::new("/api/users/profile", 0, false);

        assert!(long.priority_weight > short.priority_weight);
    }

    #[test]
    fn test_match_type_str_variants() {
        assert_eq!(RadixPath::new("/api", 0, false).match_type_str(), "Exact");
        assert_eq!(RadixPath::new("/api", 0, true).match_type_str(), "Prefix");
        assert_eq!(RadixPath::new("/users/:id", 0, false).match_type_str(), "Param");
        assert_eq!(RadixPath::new("/users/:id", 0, true).match_type_str(), "ParamPrefix");
    }

    #[test]
    fn test_complex_pattern() {
        let path = RadixPath::new("/api/:version/users/:userId/posts/:postId", 0, false);
        assert_eq!(path.segment_count, 6);
        assert!(path.has_params);

        assert!(path.matches_exact("/api/v1/users/123/posts/456"));
        assert!(!path.matches_exact("/api/v1/users/123/posts"));
        assert!(!path.matches_exact("/api/v1/users/123/posts/456/comments"));
    }

    #[test]
    fn test_trailing_slash_handling() {
        let path_no_slash = RadixPath::new("/api", 0, false);
        let path_with_slash = RadixPath::new("/api/", 0, false);

        // Both have 1 segment
        assert_eq!(path_no_slash.segment_count, 1);
        assert_eq!(path_with_slash.segment_count, 1);
    }

    #[test]
    fn test_special_characters_in_literal() {
        let path = RadixPath::new("/api-v1/users_list", 0, false);
        assert_eq!(path.segment_count, 2);
        assert!(!path.has_params);
    }

    #[test]
    fn test_escaped_colon() {
        // Double colon `::` is treated as literal colon, not a parameter
        // This matches radix tree's parse_path behavior
        let path = RadixPath::new("/api/::version/data", 0, false);
        assert_eq!(path.segment_count, 3);
        assert!(!path.has_params); // `::version` is not a parameter
    }

    #[test]
    fn test_consecutive_params() {
        // ":key:value" is treated as a single segment starting with `:`
        let path = RadixPath::new("/data/:key:value", 0, false);
        assert_eq!(path.segment_count, 2);
        assert!(path.has_params);
    }

    #[test]
    fn test_original_path_preserved() {
        let original = "/users/:userId/posts/:postId";
        let path = RadixPath::new(original, 42, true);

        assert_eq!(path.original, original);
        assert_eq!(path.normalized, original);
        assert_eq!(path.route_idx, 42);
        assert!(path.is_prefix_match);
    }

    #[test]
    fn test_count_path_segments() {
        assert_eq!(count_path_segments("/"), 0);
        assert_eq!(count_path_segments(""), 0);
        assert_eq!(count_path_segments("/api"), 1);
        assert_eq!(count_path_segments("/api/"), 1);
        assert_eq!(count_path_segments("/api/users"), 2);
        assert_eq!(count_path_segments("/api/users/"), 2);
        assert_eq!(count_path_segments("/api/v1/users/123"), 4);
    }

    #[test]
    fn test_count_segments_and_params() {
        assert_eq!(count_segments_and_params("/"), (0, false));
        assert_eq!(count_segments_and_params("/api"), (1, false));
        assert_eq!(count_segments_and_params("/api/:id"), (2, true));
        assert_eq!(count_segments_and_params("/api/::id"), (2, false)); // escaped
        assert_eq!(count_segments_and_params("/api/:v/users/:id"), (4, true));
    }

    // ===== New edge case tests =====

    #[test]
    fn test_normalize_consecutive_slashes() {
        let path = RadixPath::new("/api//users", 0, false);
        assert_eq!(path.original, "/api//users");
        assert_eq!(path.normalized, "/api/users");
        assert_eq!(path.segment_count, 2);
    }

    #[test]
    fn test_normalize_multiple_consecutive_slashes() {
        let path = RadixPath::new("/api///v1//users/", 0, false);
        assert_eq!(path.original, "/api///v1//users/");
        assert_eq!(path.normalized, "/api/v1/users/");
        assert_eq!(path.segment_count, 3);
    }

    #[test]
    fn test_normalize_path_function() {
        assert_eq!(normalize_path("/api//users"), "/api/users");
        assert_eq!(normalize_path("/api///v1"), "/api/v1");
        assert_eq!(normalize_path("//api"), "/api");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path(""), "");
    }

    #[test]
    fn test_same_path_different_match_types() {
        let exact = RadixPath::new("/api/v1", 0, false);
        let prefix = RadixPath::new("/api/v1", 1, true);

        // Exact should have higher priority than prefix
        assert!(exact.priority_weight > prefix.priority_weight);

        // Both have same segment count
        assert_eq!(exact.segment_count, prefix.segment_count);
    }

    #[test]
    fn test_tree_key_returns_normalized() {
        let path = RadixPath::new("/api//users", 0, false);
        assert_eq!(path.tree_key(), "/api/users");
    }

    #[test]
    fn test_priority_order_comprehensive() {
        // Test all combinations with same segment count
        let static_exact_2seg = RadixPath::new("/a/b", 0, false);
        let param_exact_2seg = RadixPath::new("/a/:b", 1, false);
        let static_prefix_2seg = RadixPath::new("/a/b", 2, true);
        let param_prefix_2seg = RadixPath::new("/a/:b", 3, true);

        // All have 2 segments
        assert_eq!(static_exact_2seg.segment_count, 2);
        assert_eq!(param_exact_2seg.segment_count, 2);
        assert_eq!(static_prefix_2seg.segment_count, 2);
        assert_eq!(param_prefix_2seg.segment_count, 2);

        // Priority order with same segment count
        assert!(static_exact_2seg.priority_weight > param_exact_2seg.priority_weight);
        assert!(param_exact_2seg.priority_weight > static_prefix_2seg.priority_weight);
        assert!(static_prefix_2seg.priority_weight > param_prefix_2seg.priority_weight);

        // Verify exact weights: segment_count * 4 + type_bonus
        assert_eq!(static_exact_2seg.priority_weight, 2 * 4 + 3); // 11
        assert_eq!(param_exact_2seg.priority_weight, 2 * 4 + 2); // 10
        assert_eq!(static_prefix_2seg.priority_weight, 2 * 4 + 1); // 9
        assert_eq!(param_prefix_2seg.priority_weight, (2 * 4)); // 8
    }

    #[test]
    fn test_longer_path_beats_shorter_even_with_lower_type() {
        // A longer param route should beat a shorter static route
        let short_static = RadixPath::new("/a", 0, false); // 1*4+3 = 7
        let long_param = RadixPath::new("/a/:b/c", 1, false); // 3*4+2 = 14

        assert!(long_param.priority_weight > short_static.priority_weight);
    }
}
