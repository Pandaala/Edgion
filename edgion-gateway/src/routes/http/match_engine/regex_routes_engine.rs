use crate::ctx::EdgionHttpContext;
use crate::routes::delegation::descent::{self, ChildHit, DescentState};
use crate::routes::http::match_unit::{HttpRouteRuleUnit, RouteMatchResult};
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use edgion_resources::err::EdError;
use pingora_proxy::Session;
use regex::RegexSet;
use std::sync::Arc;

/// Regex routes matching engine
///
/// This engine uses RegexSet for efficient batch matching of multiple regex patterns.
/// Routes are sorted by pattern length (longest first) for priority matching.
/// The engine is immutable after initialization, enabling lock-free concurrent reads.
///
/// Performance: O(P * M), where P is the combined pattern-set size and M is
/// the input path length.
pub struct RegexRoutesEngine {
    /// Compiled regex set for fast pre-filtering (one-pass matching of all regexes)
    /// None if routes list is empty
    regex_set: Option<RegexSet>,

    /// Regex routes sorted by pattern length (longest first) for priority matching.
    routes: Vec<Arc<HttpRouteRuleUnit>>,
}

impl RegexRoutesEngine {
    /// Build a new RegexRoutesEngine with the given regex routes
    pub fn build(routes: Vec<Arc<HttpRouteRuleUnit>>) -> Self {
        if routes.is_empty() {
            return Self {
                regex_set: None,
                routes: Vec::new(),
            };
        }

        // Sort routes by pattern length (longest first) for priority matching,
        // with specificity tie-breaking per Gateway API spec.
        //
        // Invariant relied on by `match_route`: `RegexSet::matches()` returns matched
        // indices in ascending order (regex crate contract). Because `routes` is
        // sorted longest-first here, that ascending subsequence is already in
        // highest-precedence-first order. `match_route` MUST NOT re-sort the matched
        // indices — doing so would be a per-request no-op on the hot path.
        let mut routes = routes;
        routes.sort_by(|a, b| {
            let len_a = a.path_regex.as_ref().map(|r| r.as_str().len()).unwrap_or(0);
            let len_b = b.path_regex.as_ref().map(|r| r.as_str().len()).unwrap_or(0);
            let len_cmp = len_b.cmp(&len_a);
            if len_cmp != std::cmp::Ordering::Equal {
                return len_cmp;
            }
            a.specificity.cmp_precedence(&b.specificity)
        });

        let patterns: Vec<&str> = routes
            .iter()
            .filter_map(|r| r.path_regex.as_ref().map(|re| re.as_str()))
            .collect();

        let regex_set = match RegexSet::new(&patterns) {
            Ok(set) => Some(set),
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

    /// Match a route against the request path.
    /// Returns the first matching `RouteMatchResult`, or `None` if no route matches.
    /// Routes are checked in order of pattern length (longest first).
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `ctx`: Request context containing hostname and other request info
    /// - `index`: Pre-built gateway/listener index for this port
    pub fn match_route(
        &self,
        session: &mut Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
    ) -> Result<Option<RouteMatchResult>, EdError> {
        let path = ctx.path();

        if let Some(ref regex_set) = self.regex_set {
            // A singleton can only yield index zero; avoid collecting match IDs.
            if regex_set.len() == 1 {
                if !regex_set.is_match(path) {
                    return Ok(None);
                }
                let route = &self.routes[0];
                return Ok(route
                    .deep_match(session, ctx, index)?
                    .map(|matched_gateway| RouteMatchResult {
                        route_unit: Arc::clone(route),
                        matched_gateway,
                        delegated: None,
                    }));
            }
            // Indices from `RegexSet::matches()` are ascending; combined with the
            // build-time longest-first sort of `routes`, this iteration is already
            // in highest-precedence-first order. See the invariant note in `build`.
            for idx in regex_set.matches(path) {
                let regex_route = &self.routes[idx];
                if let Some(matched_gi) = regex_route.deep_match(session, ctx, index)? {
                    return Ok(Some(RouteMatchResult {
                        route_unit: regex_route.clone(),
                        matched_gateway: matched_gi,
                        delegated: None,
                    }));
                }
            }
        } else {
            for regex_route in &self.routes {
                if regex_route.matches_path(path) {
                    if let Some(matched_gi) = regex_route.deep_match(session, ctx, index)? {
                        return Ok(Some(RouteMatchResult {
                            route_unit: regex_route.clone(),
                            matched_gateway: matched_gi,
                            delegated: None,
                        }));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Child-level match inside a delegation subtree, in the same precedence
    /// order as [`Self::match_route`]. The subtree engine supplies the child's
    /// own regex; the parent's prefix already matched one level up, so the
    /// effective scope is their intersection.
    pub(crate) fn match_child(
        &self,
        session: &Session,
        ctx: &EdgionHttpContext,
        state: &mut DescentState,
    ) -> Result<Option<ChildHit>, EdError> {
        let path = ctx.path();
        if let Some(ref regex_set) = self.regex_set {
            for idx in regex_set.matches(path) {
                if let Some(hit) = descent::match_child_unit(&self.routes[idx], session, ctx, state)? {
                    return Ok(Some(hit));
                }
            }
        } else {
            for regex_route in &self.routes {
                if regex_route.matches_path(path) {
                    if let Some(hit) = descent::match_child_unit(regex_route, session, ctx, state)? {
                        return Ok(Some(hit));
                    }
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/match_engine/regex_routes_engine_test.rs"]
mod tests;
