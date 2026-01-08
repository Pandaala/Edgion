use crate::core::routes::http_routes::match_unit::HttpRouteRuleUnit;
use crate::types::err::EdError;
use pingora_proxy::Session;
use regex::RegexSet;
use std::sync::Arc;

/// Regex routes matching engine
///
/// This engine uses RegexSet for efficient batch matching of multiple regex patterns.
/// Routes are sorted by pattern length (longest first) for priority matching.
/// The engine is immutable after initialization, enabling lock-free concurrent reads.
///
/// Performance: O(M) where M is the input path length, independent of route count.
pub struct RegexRoutesEngine {
    /// Compiled regex set for fast pre-filtering (one-pass matching of all regexes)
    /// None if routes list is empty
    regex_set: Option<RegexSet>,

    /// Regex routes sorted by pattern length (longest first)
    /// This ensures more specific patterns are matched before general ones
    /// Uses HttpRouteRuleUnit with path_regex field set
    routes: Vec<Arc<HttpRouteRuleUnit>>,
}

impl RegexRoutesEngine {
    /// Build a new RegexRoutesEngine with the given regex routes
    pub fn build(routes: Vec<Arc<HttpRouteRuleUnit>>) -> Self {
        if routes.is_empty() {
            tracing::debug!(component = "regex_routes_engine", "Built empty regex routes engine");
            return Self {
                regex_set: None,
                routes: Vec::new(),
            };
        }

        // Sort routes by pattern length (longest first) for priority matching
        // Longer patterns are typically more specific and should be matched first
        let mut routes = routes;
        routes.sort_by(|a, b| {
            let len_a = a.path_regex.as_ref().map(|r| r.as_str().len()).unwrap_or(0);
            let len_b = b.path_regex.as_ref().map(|r| r.as_str().len()).unwrap_or(0);
            len_b.cmp(&len_a) // Descending order (longest first)
        });

        // Extract patterns and build RegexSet for fast batch matching
        let patterns: Vec<&str> = routes
            .iter()
            .filter_map(|r| r.path_regex.as_ref().map(|re| re.as_str()))
            .collect();

        let regex_set = match RegexSet::new(&patterns) {
            Ok(set) => {
                tracing::debug!(
                    component = "regex_routes_engine",
                    count = routes.len(),
                    "Built regex routes engine with RegexSet optimization"
                );
                Some(set)
            }
            Err(e) => {
                tracing::warn!(
                    component = "regex_routes_engine",
                    count = routes.len(),
                    error = %e,
                    "Failed to build RegexSet, falling back to linear matching"
                );
                None
            }
        };

        Self { regex_set, routes }
    }

    /// Get the number of routes in this engine
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Match a route against the request path
    /// Returns the first matching Arc<HttpRouteRuleUnit>, or None if no route matches
    /// Routes are checked in order of pattern length (longest first)
    pub fn match_route(
        &self,
        session: &mut Session,
        listener_name: &str,
    ) -> Result<Option<Arc<HttpRouteRuleUnit>>, EdError> {
        let path = session.req_header().uri.path();

        if let Some(ref regex_set) = self.regex_set {
            // Fast path: Use RegexSet for O(M) batch matching
            let matches: Vec<usize> = regex_set.matches(path).into_iter().collect();

            if matches.is_empty() {
                // No regex matched the path
                return Ok(None);
            }

            // Check matched routes in sorted order (longest first)
            // Only iterate over pre-filtered matches, not all routes
            for &idx in &matches {
                let regex_route = &self.routes[idx];

                // Path already matched by RegexSet, now check deep match
                // (headers, query params, method, sectionName)
                if regex_route.deep_match(session, listener_name)? {
                    tracing::debug!(
                        path = %path,
                        regex = %regex_route.path_regex.as_ref().map(|r| r.as_str()).unwrap_or(""),
                        "Regex match succeeded (RegexSet fast path)"
                    );
                    return Ok(Some(regex_route.clone()));
                }
            }
        } else {
            // Fallback path: Linear scan if RegexSet failed to build
            for regex_route in &self.routes {
                if regex_route.matches_path(path) {
                    // Path matches, check deep match (headers, query params, method, sectionName)
                    if regex_route.deep_match(session, listener_name)? {
                        tracing::debug!(
                            path = %path,
                            regex = %regex_route.path_regex.as_ref().map(|r| r.as_str()).unwrap_or(""),
                            "Regex match succeeded (linear fallback)"
                        );
                        return Ok(Some(regex_route.clone()));
                    }
                }
            }
        }

        // No route matched
        Ok(None)
    }
}

// RegexRoutesEngine is thread-safe with lock-free reads!
// The routes vector is immutable after initialization.
unsafe impl Sync for RegexRoutesEngine {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lb::BackendSelector;
    use crate::core::plugins::PluginRuntime;
    use crate::types::{HTTPPathMatch, HTTPRouteMatch, HTTPRouteRule, MatchInfo};
    use regex::Regex;

    /// Helper function to create a test HttpRouteRuleUnit with regex pattern
    fn create_test_route_unit(
        namespace: &str,
        name: &str,
        regex_pattern: &str,
        rule_id: usize,
    ) -> Arc<HttpRouteRuleUnit> {
        let path_match = HTTPPathMatch {
            match_type: Some("RegularExpression".to_string()),
            value: Some(regex_pattern.to_string()),
        };

        let match_item = HTTPRouteMatch {
            path: Some(path_match),
            headers: None,
            query_params: None,
            method: None,
        };

        let rule = Arc::new(HTTPRouteRule {
            matches: Some(vec![match_item.clone()]),
            filters: None,
            backend_refs: None,
            timeouts: None,
            retry: None,
            session_persistence: None,
            backend_finder: BackendSelector::new(),
            plugin_runtime: Arc::new(PluginRuntime::new()),
            parsed_timeouts: None,
            parsed_max_retries: None,
        });

        let regex = Regex::new(regex_pattern).expect(&format!("Invalid regex pattern: {}", regex_pattern));

        Arc::new(HttpRouteRuleUnit {
            resource_key: format!("{}/{}", namespace, name),
            matched_info: MatchInfo::new(namespace.to_string(), name.to_string(), rule_id, 0, match_item),
            rule,
            path_regex: Some(regex),
            parent_refs: None,
        })
    }

    #[test]
    fn test_empty_routes() {
        let engine = RegexRoutesEngine::build(Vec::new());

        assert_eq!(engine.route_count(), 0);
        assert!(engine.regex_set.is_none());
        assert!(engine.routes.is_empty());
    }

    #[test]
    fn test_single_route() {
        let route = create_test_route_unit("default", "route1", r"^/api/v1/.*$", 0);
        let engine = RegexRoutesEngine::build(vec![route.clone()]);

        assert_eq!(engine.route_count(), 1);
        assert!(engine.regex_set.is_some());

        // Verify the route is stored
        assert_eq!(engine.routes.len(), 1);
        assert_eq!(engine.routes[0].matched_info.rns, "default");
        assert_eq!(engine.routes[0].matched_info.rn, "route1");
    }

    #[test]
    fn test_routes_sorted_by_length() {
        // Create routes with different pattern lengths
        let route1 = create_test_route_unit("default", "short", r"^/a$", 0);
        let route2 = create_test_route_unit("default", "medium", r"^/api/v1$", 1);
        let route3 = create_test_route_unit("default", "long", r"^/api/v1/users/[0-9]+$", 2);

        // Add routes in random order
        let routes = vec![route2.clone(), route1.clone(), route3.clone()];
        let engine = RegexRoutesEngine::build(routes);

        assert_eq!(engine.route_count(), 3);

        // Verify sorted by pattern length (longest first)
        assert_eq!(engine.routes[0].matched_info.rn, "long"); // longest pattern
        assert_eq!(engine.routes[1].matched_info.rn, "medium");
        assert_eq!(engine.routes[2].matched_info.rn, "short"); // shortest pattern

        // Verify pattern lengths
        let len0 = engine.routes[0].path_regex.as_ref().unwrap().as_str().len();
        let len1 = engine.routes[1].path_regex.as_ref().unwrap().as_str().len();
        let len2 = engine.routes[2].path_regex.as_ref().unwrap().as_str().len();

        assert!(len0 >= len1);
        assert!(len1 >= len2);
    }

    #[test]
    fn test_regex_set_creation() {
        let routes = vec![
            create_test_route_unit("default", "route1", r"^/api/.*$", 0),
            create_test_route_unit("default", "route2", r"^/users/[0-9]+$", 1),
            create_test_route_unit("default", "route3", r"^/admin/.*$", 2),
        ];

        let engine = RegexRoutesEngine::build(routes);

        // RegexSet should be created successfully
        assert!(engine.regex_set.is_some());

        let regex_set = engine.regex_set.as_ref().unwrap();

        // Test that RegexSet matches correctly
        assert!(regex_set.is_match("/api/test"));
        assert!(regex_set.is_match("/users/123"));
        assert!(regex_set.is_match("/admin/dashboard"));
        assert!(!regex_set.is_match("/other/path"));
    }

    #[test]
    fn test_regex_set_matches_indices() {
        // Create routes with different patterns
        let routes = vec![
            create_test_route_unit("default", "api", r"^/api/.*$", 0),
            create_test_route_unit("default", "users", r"^/users/[0-9]+$", 1),
            create_test_route_unit("default", "admin", r"^/admin/.*$", 2),
        ];

        let engine = RegexRoutesEngine::build(routes);
        let regex_set = engine.regex_set.as_ref().unwrap();

        // Test /api/test - should match index for "api" route
        let matches: Vec<usize> = regex_set.matches("/api/test").into_iter().collect();
        assert_eq!(matches.len(), 1);
        assert!(engine.routes[matches[0]].matched_info.rn.contains("api"));

        // Test /users/123 - should match index for "users" route
        let matches: Vec<usize> = regex_set.matches("/users/123").into_iter().collect();
        assert_eq!(matches.len(), 1);
        assert!(engine.routes[matches[0]].matched_info.rn.contains("users"));

        // Test /admin/panel - should match index for "admin" route
        let matches: Vec<usize> = regex_set.matches("/admin/panel").into_iter().collect();
        assert_eq!(matches.len(), 1);
        assert!(engine.routes[matches[0]].matched_info.rn.contains("admin"));
    }

    #[test]
    fn test_multiple_regex_matches() {
        // Create routes where multiple patterns can match the same path
        let routes = vec![
            create_test_route_unit("default", "general", r"^/api/.*$", 0),
            create_test_route_unit("default", "specific", r"^/api/v1/users$", 1),
            create_test_route_unit("default", "wildcard", r"^/.*$", 2),
        ];

        let engine = RegexRoutesEngine::build(routes);
        let regex_set = engine.regex_set.as_ref().unwrap();

        // Path "/api/v1/users" should match all three patterns
        let matches: Vec<usize> = regex_set.matches("/api/v1/users").into_iter().collect();
        assert!(matches.len() >= 2); // At least 2 should match

        // The longest pattern should be first in routes (due to sorting)
        // In this case "^/api/v1/users$" is longest
        let longest_idx = matches
            .iter()
            .max_by_key(|&&idx| engine.routes[idx].path_regex.as_ref().unwrap().as_str().len())
            .unwrap();

        assert!(engine.routes[*longest_idx].matched_info.rn.contains("specific"));
    }

    #[test]
    fn test_path_matching_correctness() {
        let routes = vec![
            create_test_route_unit("default", "digits", r"^/users/[0-9]+$", 0),
            create_test_route_unit("default", "alpha", r"^/users/[a-z]+$", 1),
        ];

        let engine = RegexRoutesEngine::build(routes);

        // Test digit path
        assert!(engine.routes[0].matches_path("/users/123"));
        assert!(!engine.routes[0].matches_path("/users/abc"));

        // Test alpha path
        assert!(engine.routes[1].matches_path("/users/abc"));
        assert!(!engine.routes[1].matches_path("/users/123"));
    }

    #[test]
    fn test_complex_regex_patterns() {
        let routes = vec![
            create_test_route_unit(
                "default",
                "uuid",
                r"^/items/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
                0,
            ),
            create_test_route_unit(
                "default",
                "date",
                r"^/logs/\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])$",
                1,
            ),
            create_test_route_unit("default", "version", r"^/api/v[0-9]+/.*$", 2),
        ];

        let engine = RegexRoutesEngine::build(routes);
        assert!(engine.regex_set.is_some());

        // Find routes by name (since they're sorted by length)
        let uuid_route = engine.routes.iter().find(|r| r.matched_info.rn == "uuid").unwrap();
        let date_route = engine.routes.iter().find(|r| r.matched_info.rn == "date").unwrap();
        let version_route = engine.routes.iter().find(|r| r.matched_info.rn == "version").unwrap();

        // Test UUID pattern
        assert!(uuid_route.matches_path("/items/550e8400-e29b-41d4-a716-446655440000"));
        assert!(!uuid_route.matches_path("/items/not-a-uuid"));

        // Test date pattern (with valid month/day validation)
        assert!(date_route.matches_path("/logs/2025-12-30"));
        assert!(!date_route.matches_path("/logs/2025-13-40"));

        // Test version pattern
        assert!(version_route.matches_path("/api/v1/users"));
        assert!(version_route.matches_path("/api/v2/posts"));
        assert!(!version_route.matches_path("/api/vX/users"));
    }

    #[test]
    fn test_edge_cases() {
        let routes = vec![
            create_test_route_unit("default", "root", r"^/$", 0),
            create_test_route_unit("default", "empty", r"^$", 1),
            create_test_route_unit("default", "any", r".*", 2),
        ];

        let engine = RegexRoutesEngine::build(routes);

        // Test root path
        assert!(engine.routes[0].matches_path("/"));
        assert!(!engine.routes[0].matches_path("/anything"));

        // Test empty pattern
        assert!(engine.routes[1].matches_path(""));
        assert!(!engine.routes[1].matches_path("/"));

        // Test match-all pattern
        assert!(engine.routes[2].matches_path(""));
        assert!(engine.routes[2].matches_path("/"));
        assert!(engine.routes[2].matches_path("/anything"));
    }

    #[test]
    fn test_no_matches() {
        let routes = vec![
            create_test_route_unit("default", "route1", r"^/api/.*$", 0),
            create_test_route_unit("default", "route2", r"^/admin/.*$", 1),
        ];

        let engine = RegexRoutesEngine::build(routes);
        let regex_set = engine.regex_set.as_ref().unwrap();

        // Paths that don't match any pattern
        let matches: Vec<usize> = regex_set.matches("/other/path").into_iter().collect();
        assert_eq!(matches.len(), 0);

        let matches: Vec<usize> = regex_set.matches("/users/123").into_iter().collect();
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_special_characters_in_regex() {
        let routes = vec![
            create_test_route_unit("default", "dots", r"^/api\.json$", 0),
            create_test_route_unit("default", "question", r"^/search\?.*$", 1),
            create_test_route_unit("default", "brackets", r"^/items\[[0-9]\]$", 2),
        ];

        let engine = RegexRoutesEngine::build(routes);

        // Find routes by name (since they're sorted by length)
        let dots_route = engine.routes.iter().find(|r| r.matched_info.rn == "dots").unwrap();
        let question_route = engine.routes.iter().find(|r| r.matched_info.rn == "question").unwrap();
        let brackets_route = engine.routes.iter().find(|r| r.matched_info.rn == "brackets").unwrap();

        // Test escaped dot - literal dot, not any character
        assert!(dots_route.matches_path("/api.json"));
        assert!(!dots_route.matches_path("/apixjson"));

        // Test escaped question mark in path
        assert!(question_route.matches_path("/search?query=test"));

        // Test escaped brackets with digit inside
        assert!(brackets_route.matches_path("/items[5]"));
        assert!(!brackets_route.matches_path("/items[a]"));
    }

    #[test]
    fn test_case_sensitive_matching() {
        let routes = vec![
            create_test_route_unit("default", "lower", r"^/api/users$", 0),
            create_test_route_unit("default", "upper", r"^/API/USERS$", 1),
        ];

        let engine = RegexRoutesEngine::build(routes);

        // Regex matching is case-sensitive by default
        assert!(engine.routes[0].matches_path("/api/users"));
        assert!(!engine.routes[0].matches_path("/API/USERS"));

        assert!(engine.routes[1].matches_path("/API/USERS"));
        assert!(!engine.routes[1].matches_path("/api/users"));
    }

    #[test]
    fn test_large_number_of_routes() {
        // Test with many routes to verify RegexSet efficiency
        let mut routes = Vec::new();
        for i in 0..100 {
            let pattern = format!(r"^/route{}/.*$", i);
            routes.push(create_test_route_unit("default", &format!("route{}", i), &pattern, i));
        }

        let engine = RegexRoutesEngine::build(routes);

        assert_eq!(engine.route_count(), 100);
        assert!(engine.regex_set.is_some());

        // Verify some matches
        assert!(engine.routes.iter().any(|r| r.matches_path("/route0/test")));
        assert!(engine.routes.iter().any(|r| r.matches_path("/route50/test")));
        assert!(engine.routes.iter().any(|r| r.matches_path("/route99/test")));
    }
}
