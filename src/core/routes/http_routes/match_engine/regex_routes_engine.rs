use crate::core::routes::http_routes::match_unit::HttpRouteRuleUnit;
use crate::types::err::EdError;
use pingora_proxy::Session;
use std::sync::Arc;

/// Regex routes matching engine
/// 
/// This engine stores regex routes sorted by pattern length (longest first)
/// for efficient matching. It's immutable after initialization, enabling
/// lock-free concurrent reads.
pub struct RegexRoutesEngine {
    /// Regex routes sorted by pattern length (longest first)
    /// This ensures more specific patterns are matched before general ones
    /// Uses HttpRouteRuleUnit with path_regex field set
    routes: Vec<Arc<HttpRouteRuleUnit>>,
}

impl RegexRoutesEngine {
    /// Build a new RegexRoutesEngine with the given regex routes
    pub fn build(routes: Vec<Arc<HttpRouteRuleUnit>>) -> Self {
        // Sort routes by pattern length (longest first) for priority matching
        // Longer patterns are typically more specific and should be matched first
        let mut routes = routes;
        routes.sort_by(|a, b| {
            let len_a = a.path_regex.as_ref().map(|r| r.as_str().len()).unwrap_or(0);
            let len_b = b.path_regex.as_ref().map(|r| r.as_str().len()).unwrap_or(0);
            len_b.cmp(&len_a) // Descending order (longest first)
        });

        tracing::debug!(
            component = "regex_routes_engine",
            count = routes.len(),
            "Built regex routes engine"
        );

        Self { routes }
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
    ) -> Result<Option<Arc<HttpRouteRuleUnit>>, EdError> {
        let path = session.req_header().uri.path();

        // Try each regex route in order (already sorted by length, longest first)
        for regex_route in &self.routes {
            if regex_route.matches_path(path) {
                // Path matches, check deep match (headers, query params, method)
                if regex_route.deep_match(session)? {
                    tracing::debug!(
                        path = %path,
                        regex = %regex_route.path_regex.as_ref().map(|r| r.as_str()).unwrap_or(""),
                        "Regex match succeeded"
                    );
                    return Ok(Some(regex_route.clone()));
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

