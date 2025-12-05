use crate::core::routes::regex_match_unit::HttpRouteRuleRegexUnit;
use crate::core::lb::{ERR_NO_BACKEND_REFS, ERR_INCONSISTENT_WEIGHT};
use crate::types::err::EdError;
use crate::types::{HTTPRouteRule, MatchInfo, HTTPBackendRef};
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
    routes: Vec<Arc<HttpRouteRuleRegexUnit>>,
}

impl RegexRoutesEngine {
    /// Build a new RegexRoutesEngine with the given regex routes
    pub fn build(mut routes: Vec<HttpRouteRuleRegexUnit>) -> Self {
        // Sort routes by pattern length (longest first) for priority matching
        // Longer patterns are typically more specific and should be matched first
        routes.sort_by(|a, b| {
            let len_a = a.path_regex.as_str().len();
            let len_b = b.path_regex.as_str().len();
            len_b.cmp(&len_a) // Descending order (longest first)
        });

        let routes: Vec<Arc<HttpRouteRuleRegexUnit>> = routes
            .into_iter()
            .map(|r| Arc::new(r))
            .collect();

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

    /// Select backend from the matched route rule
    fn select_backend(rule: &Arc<HTTPRouteRule>) -> Result<HTTPBackendRef, EdError> {
        // Initialize selector if not yet initialized
        if !rule.backend_finder.is_initialized() {
            let (items, weights) = match &rule.backend_refs {
                Some(refs) if !refs.is_empty() => {
                    let items: Vec<HTTPBackendRef> = refs.clone();
                    // Default weight to 1 if not specified
                    let weights: Vec<Option<i32>> = refs.iter().map(|br| br.weight.or(Some(1))).collect();
                    (items, weights)
                }
                _ => (vec![], vec![]),
            };
            rule.backend_finder.init(items, weights);
        }

        // Select backend
        rule.backend_finder.select().map_err(|err_code| {
            match err_code {
                ERR_NO_BACKEND_REFS => EdError::BackendNotFound(),
                ERR_INCONSISTENT_WEIGHT => EdError::InconsistentWeight(),
                _ => EdError::BackendNotFound(),
            }
        })
    }

    /// Match a route against the request path
    /// Returns the first matching (MatchInfo, HTTPBackendRef), or None if no route matches
    /// Routes are checked in order of pattern length (longest first)
    pub fn match_route(
        &self,
        session: &mut Session,
    ) -> Result<Option<(MatchInfo, HTTPBackendRef)>, EdError> {
        let path = session.req_header().uri.path();

        // Try each regex route in order (already sorted by length, longest first)
        for regex_route in &self.routes {
            if regex_route.matches_path(path) {
                // Path matches, check deep match (headers, query params, method)
                if regex_route.deep_match(session)? {
                    tracing::debug!(
                        path = %path,
                        regex = %regex_route.path_regex.as_str(),
                        "Regex match succeeded"
                    );
                    let backend = Self::select_backend(&regex_route.rule)?;
                    return Ok(Some(((*regex_route.matched_info).clone(), backend)));
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

