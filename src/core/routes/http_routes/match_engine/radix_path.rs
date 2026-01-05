#[derive(Debug, Clone, PartialEq)]
enum RawSegment {
    Slash,
    Literal(String),
    Param(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchSegment {
    Literal(String),
    Param(String),
}

#[derive(Debug, Clone)]
pub struct RadixPath {
    pub original: String,
    pub priority_weight: usize,
    pub radix_key: String,
    pub is_prefix_match: bool,
    pub match_segments: Vec<MatchSegment>,
    pub route_idx: usize,
}

impl RadixPath {
    pub fn new(path: &str, route_idx: usize, is_prefix: bool) -> Self {
        let original = path.to_string();
        let is_prefix_match = is_prefix;

        let mut raw_segments = Vec::new();
        let mut current_segment = String::new();

        let process_segment = |segment: String, raw_segment: &mut Vec<RawSegment>| {
            if segment.is_empty() {
                return;
            }
            if segment.starts_with(':') {
                let param_name = &segment[1..];
                if param_name.is_empty() {
                    panic!("Empty param name in path: {}", path);
                }
                raw_segment.push(RawSegment::Param(param_name.to_string()));
            } else {
                raw_segment.push(RawSegment::Literal(segment));
            }
        };

        let mut chars = path.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '/' {
                process_segment(current_segment.clone(), &mut raw_segments);
                current_segment.clear();
                raw_segments.push(RawSegment::Slash);
            } else {
                current_segment.push(c);
            }
        }
        process_segment(current_segment, &mut raw_segments);

        let mut match_segments = Vec::new();
        let mut accumulated_literal = String::new();
        let mut radix_key = String::new();
        let mut radix_key_set = false;

        let flush_literal = |accumulated: String,
                             radix_key: &mut String,
                             radix_key_set: &mut bool,
                             match_segments: &mut Vec<MatchSegment>| {
            if accumulated.is_empty() {
                return;
            }
            if !*radix_key_set {
                *radix_key = accumulated;
                *radix_key_set = true;
            } else {
                match_segments.push(MatchSegment::Literal(accumulated));
            }
        };

        let raw_segments_len = raw_segments.len();

        for raw_seg in raw_segments {
            match raw_seg {
                RawSegment::Slash => {
                    accumulated_literal.push('/');
                }
                RawSegment::Literal(s) => {
                    accumulated_literal.push_str(&s);
                }
                RawSegment::Param(param) => {
                    flush_literal(
                        accumulated_literal.clone(),
                        &mut radix_key,
                        &mut radix_key_set,
                        &mut match_segments,
                    );
                    accumulated_literal.clear();
                    match_segments.push(MatchSegment::Param(param));
                }
            }
        }
        flush_literal(
            accumulated_literal,
            &mut radix_key,
            &mut radix_key_set,
            &mut match_segments,
        );

        if radix_key.is_empty() {
            radix_key = "/".to_string();
        }

        let priority_weight = if is_prefix_match {
            raw_segments_len * 2
        } else {
            raw_segments_len * 2 + 1
        };

        Self {
            original,
            priority_weight,
            radix_key,
            is_prefix_match,
            match_segments,
            route_idx,
        }
    }

    pub fn matches(&self, request_path: &str) -> bool {
        // todo, open this just for test case, normally we do not need this. First check if request_path starts with radix_key
        if !request_path.starts_with(&self.radix_key) {
            return false;
        }

        let remaining_path = &request_path[self.radix_key.len()..];

        if self.match_segments.is_empty() {
            return if self.is_prefix_match {
                true
            } else {
                remaining_path.is_empty()
            };
        }

        let mut path_cursor = 0;

        for segment in &self.match_segments {
            if path_cursor >= remaining_path.len() {
                return false;
            }

            match segment {
                MatchSegment::Literal(literal) => {
                    let segment_path = &remaining_path[path_cursor..];
                    if !segment_path.starts_with(literal) {
                        return false;
                    }
                    path_cursor += literal.len();
                }
                MatchSegment::Param(_param) => {
                    let segment_path = &remaining_path[path_cursor..];
                    let param_end = segment_path.find('/').unwrap_or(segment_path.len());
                    if param_end == 0 {
                        return false;
                    }
                    path_cursor += param_end;
                }
            }
        }

        debug_assert!(
            path_cursor <= remaining_path.len(),
            "Bug: path_cursor ({}) exceeds remaining_path length ({})",
            path_cursor,
            remaining_path.len()
        );

        if path_cursor == remaining_path.len() {
            true
        } else {
            self.is_prefix_match
        }
    }

    pub fn match_type_str(&self) -> &str {
        let has_param = self.match_segments.iter().any(|s| matches!(s, MatchSegment::Param(_)));
        match (self.is_prefix_match, has_param) {
            (true, true) => "ParamPrefix",
            (true, false) => "Prefix",
            (false, true) => "Param",
            (false, false) => "Exact",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match_simple_path() {
        let path = RadixPath::new("/api", 0, false);
        assert_eq!(path.radix_key, "/api");
        assert_eq!(path.is_prefix_match, false);
        assert!(path.match_segments.is_empty());
        assert_eq!(path.match_type_str(), "Exact");

        assert!(path.matches("/api"));
        assert!(!path.matches("/api/users"));
        assert!(!path.matches("/api/"));
        assert!(!path.matches("/"));
    }

    #[test]
    fn test_prefix_match_simple_path() {
        let path = RadixPath::new("/api", 0, true);
        assert_eq!(path.radix_key, "/api");
        assert_eq!(path.is_prefix_match, true);
        assert!(path.match_segments.is_empty());
        assert_eq!(path.match_type_str(), "Prefix");

        assert!(path.matches("/api"));
        assert!(path.matches("/api/users"));
        assert!(path.matches("/api/users/123"));
        // Note: prefix match_engine is string-based, so "/apix" matches "/api" prefix
        assert!(path.matches("/apix"));
        assert!(!path.matches("/"));
        assert!(!path.matches("/ap"));
    }

    #[test]
    fn test_exact_match_with_param() {
        let path = RadixPath::new("/users/:id", 0, false);
        assert_eq!(path.radix_key, "/users/");
        assert_eq!(path.is_prefix_match, false);
        assert_eq!(path.match_segments.len(), 1);
        assert_eq!(path.match_type_str(), "Param");

        match &path.match_segments[0] {
            MatchSegment::Param(name) => assert_eq!(name, "id"),
            _ => panic!("Expected Param segment"),
        }

        assert!(path.matches("/users/123"));
        assert!(path.matches("/users/abc"));
        assert!(!path.matches("/users/"));
        assert!(!path.matches("/users/123/profile"));
        assert!(!path.matches("/users"));
    }

    #[test]
    fn test_prefix_match_with_param() {
        let path = RadixPath::new("/users/:id", 0, true);
        assert_eq!(path.radix_key, "/users/");
        assert_eq!(path.is_prefix_match, true);
        assert_eq!(path.match_type_str(), "ParamPrefix");

        assert!(path.matches("/users/123"));
        assert!(path.matches("/users/123/profile"));
        assert!(path.matches("/users/abc/settings"));
        assert!(!path.matches("/users/"));
        assert!(!path.matches("/users"));
    }

    #[test]
    fn test_multiple_params() {
        let path = RadixPath::new("/api/:version/users/:id", 0, false);
        assert_eq!(path.radix_key, "/api/");
        assert_eq!(path.match_segments.len(), 3);

        assert!(path.matches("/api/v1/users/123"));
        assert!(path.matches("/api/v2/users/abc"));
        assert!(!path.matches("/api/v1/users"));
        assert!(!path.matches("/api/v1/users/123/extra"));
    }

    #[test]
    fn test_param_with_literal_suffix() {
        let path = RadixPath::new("/users/:id/profile", 0, false);
        assert_eq!(path.radix_key, "/users/");
        assert_eq!(path.match_segments.len(), 2);

        match &path.match_segments[0] {
            MatchSegment::Param(name) => assert_eq!(name, "id"),
            _ => panic!("Expected Param segment"),
        }
        match &path.match_segments[1] {
            MatchSegment::Literal(lit) => assert_eq!(lit, "/profile"),
            _ => panic!("Expected Literal segment"),
        }

        assert!(path.matches("/users/123/profile"));
        assert!(!path.matches("/users/123/settings"));
        assert!(!path.matches("/users/123/profile/extra"));
    }

    #[test]
    fn test_root_path_exact() {
        let path = RadixPath::new("/", 0, false);
        assert_eq!(path.radix_key, "/");
        assert!(path.match_segments.is_empty());

        assert!(path.matches("/"));
        assert!(!path.matches("/api"));
    }

    #[test]
    fn test_root_path_prefix() {
        let path = RadixPath::new("/", 0, true);
        assert_eq!(path.radix_key, "/");
        assert!(path.match_segments.is_empty());

        assert!(path.matches("/"));
        assert!(path.matches("/api"));
        assert!(path.matches("/users/123"));
    }

    #[test]
    fn test_priority_weight_calculation() {
        let exact = RadixPath::new("/api/users", 0, false);
        let prefix = RadixPath::new("/api/users", 0, true);

        // Exact match_engine should have higher priority (odd number)
        assert!(exact.priority_weight > prefix.priority_weight);
        assert_eq!(exact.priority_weight % 2, 1); // Odd for exact
        assert_eq!(prefix.priority_weight % 2, 0); // Even for prefix
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

        assert!(path.matches("/api/v1/users/123/posts/456"));
        assert!(!path.matches("/api/v1/users/123/posts"));
        assert!(!path.matches("/api/v1/users/123/posts/456/comments"));
    }

    #[test]
    fn test_trailing_slash_handling() {
        let path_no_slash = RadixPath::new("/api", 0, false);
        let path_with_slash = RadixPath::new("/api/", 0, false);

        assert!(path_no_slash.matches("/api"));
        assert!(!path_no_slash.matches("/api/"));

        assert!(path_with_slash.matches("/api/"));
        assert!(!path_with_slash.matches("/api"));
    }

    #[test]
    fn test_empty_param_name_panics() {
        let result = std::panic::catch_unwind(|| {
            RadixPath::new("/users/:", 0, false);
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_special_characters_in_literal() {
        let path = RadixPath::new("/api-v1/users_list", 0, false);
        assert_eq!(path.radix_key, "/api-v1/users_list");

        assert!(path.matches("/api-v1/users_list"));
        assert!(!path.matches("/api_v1/users_list"));
    }

    #[test]
    fn test_param_cannot_be_empty() {
        let path = RadixPath::new("/users/:id/profile", 0, false);

        // Param must match_engine at least one character
        assert!(!path.matches("/users//profile"));
    }

    #[test]
    fn test_param_stops_at_slash() {
        let path = RadixPath::new("/users/:id/profile", 0, false);

        // Param should capture up to the next slash
        assert!(path.matches("/users/123/profile"));
        assert!(path.matches("/users/abc-def/profile"));
        assert!(!path.matches("/users/123/456/profile"));
    }

    #[test]
    fn test_consecutive_params() {
        // This is an edge case - params right next to each other without separator
        // ":key:value" is treated as a single param with name "key:value"
        let path = RadixPath::new("/data/:key:value", 0, false);

        // Will be parsed as one param segment with name "key:value"
        // This is expected behavior - params need proper separation for clarity
        assert_eq!(path.radix_key, "/data/");
        assert_eq!(path.match_segments.len(), 1);
        match &path.match_segments[0] {
            MatchSegment::Param(name) => assert_eq!(name, "key:value"),
            _ => panic!("Expected Param segment"),
        }
    }

    #[test]
    fn test_radix_key_extraction() {
        // radix_key should be the longest literal prefix before first param
        let test_cases = vec![
            ("/api/users", "/api/users"),
            ("/api/:version", "/api/"),
            ("/:org/repos", "/"),
            ("/a/b/c/:id/d", "/a/b/c/"),
        ];

        for (input, expected_key) in test_cases {
            let path = RadixPath::new(input, 0, false);
            assert_eq!(path.radix_key, expected_key, "Failed for input: {}", input);
        }
    }

    #[test]
    fn test_original_path_preserved() {
        let original = "/users/:userId/posts/:postId";
        let path = RadixPath::new(original, 42, true);

        assert_eq!(path.original, original);
        assert_eq!(path.route_idx, 42);
        assert_eq!(path.is_prefix_match, true);
    }
}
