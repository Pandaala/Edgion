use crate::core::gateway::ctx::EdgionHttpContext;
use crate::core::gateway::lb::BackendSelector;
use crate::core::gateway::plugins::PluginRuntime;
use crate::core::gateway::runtime::matching::route::check_gateway_listener_match;
use crate::core::gateway::runtime::store::listener_gateway_info::GatewayInfoIndex;
use crate::core::gateway::runtime::GatewayInfo;
use crate::core::gateway::MatchInfo;
use crate::types::err::EdError;
use crate::types::resources::common::ParentReference;
use crate::types::{HTTPBackendRef, HTTPRouteRule};
use pingora_proxy::Session;
use regex::Regex;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

/// Pre-computed match specificity for Gateway API precedence ordering.
///
/// Fields are computed once at route creation time.
/// Comparison order (highest priority first):
///   has_method > header_count > query_count > creation_ts > route_key > rule_index
///
/// Path-level precedence (Exact vs Prefix, char count) is handled separately by
/// RadixPath.priority_weight, which sorts *before* this struct.
#[derive(Clone, Debug)]
pub struct MatchSpecificity {
    /// Whether this match specifies an HTTP method (true = more specific)
    pub has_method: bool,
    /// Number of header match conditions
    pub header_count: u16,
    /// Number of query parameter match conditions
    pub query_count: u16,
    /// Route creation timestamp for cross-route tie-breaking (older = higher priority).
    /// Milliseconds since epoch; 0 = unknown/missing.
    pub creation_ts_ms: i64,
    /// Route "{namespace}/{name}" for final alphabetical tie-breaking
    pub route_key: String,
    /// Rule index within the route (for intra-route tie-breaking: first wins)
    pub rule_index: u32,
    /// Match index within the rule (for intra-rule tie-breaking: first wins)
    pub match_index: u32,
}

impl MatchSpecificity {
    /// Compare two MatchSpecificity values per Gateway API precedence.
    /// Returns Ordering where Less means `self` has higher priority.
    ///
    /// This does NOT include path-level comparison — the caller must
    /// compare RadixPath.priority_weight first.
    pub fn cmp_precedence(&self, other: &Self) -> Ordering {
        // Level 3: method match (has_method = true sorts first)
        other
            .has_method
            .cmp(&self.has_method)
            // Level 4: header count (more = higher priority)
            .then(other.header_count.cmp(&self.header_count))
            // Level 5: query param count (more = higher priority)
            .then(other.query_count.cmp(&self.query_count))
            // Level 6: creation timestamp (older = higher priority, ascending)
            .then(self.creation_ts_ms.cmp(&other.creation_ts_ms))
            // Level 7: alphabetical by namespace/name (ascending)
            .then(self.route_key.cmp(&other.route_key))
            // Intra-route: first rule wins (ascending)
            .then(self.rule_index.cmp(&other.rule_index))
            // Intra-rule: first match wins (ascending)
            .then(self.match_index.cmp(&other.match_index))
    }
}

/// Result of a successful route match, bundling the matched route and the
/// gateway/listener context that satisfied the parentRef constraints.
pub struct RouteMatchResult {
    pub route_unit: Arc<HttpRouteRuleUnit>,
    pub matched_gateway: GatewayInfo,
}

/// A backend ref paired with its compiled plugin runtime — the unit the LB
/// selector returns. Keeps the gateway-compiled `PluginRuntime` off the
/// `HTTPBackendRef` schema (two-type split). Both fields are `Arc`, so cloning
/// on `select()` is cheap.
#[derive(Clone)]
pub struct CompiledBackend {
    /// The pure backend-ref schema (name, port, weight, extension_info, …). Owned
    /// (not `Arc`) because the selected backend's per-request `backend_tls_policy`
    /// is mutated after selection; clone-on-`select()` matches today's cost (the
    /// old selector also cloned `HTTPBackendRef`).
    pub schema: HTTPBackendRef,
    /// Compiled backend-level plugin chain (built from `schema.filters`).
    pub plugin_runtime: Arc<PluginRuntime>,
}

impl std::ops::Deref for CompiledBackend {
    type Target = HTTPBackendRef;
    /// Deref to the backend schema so existing `selected_backend.name` / `.port` /
    /// `.extension_info` / `.backend_tls_policy` accesses keep working unchanged.
    /// `plugin_runtime` is `CompiledBackend`'s own field, so it shadows correctly.
    fn deref(&self) -> &HTTPBackendRef {
        &self.schema
    }
}

#[derive(Clone)]
pub struct HttpRouteRuleUnit {
    pub resource_key: String,
    /// Match info containing namespace, name and match item
    pub matched_info: MatchInfo,
    /// Reference to the original rule (for backend_refs, plugins, etc.)
    pub rule: Arc<HTTPRouteRule>,
    /// Compiled regex for path matching (only for RegularExpression path type)
    pub path_regex: Option<Regex>,
    /// Parent references for sectionName matching
    pub parent_refs: Option<Vec<ParentReference>>,
    /// Pre-compiled regexes for header RegularExpression matchers (aligned with headers vec).
    /// `Regex` is internally Arc-like; the surrounding `HttpRouteRuleUnit` is shared via
    /// outer `Arc<HttpRouteRuleUnit>`, so no extra `Arc` wrapper is needed.
    pub compiled_header_regexes: Vec<Option<Regex>>,
    /// Pre-compiled regexes for query param RegularExpression matchers (aligned with query_params vec).
    pub compiled_query_regexes: Vec<Option<Regex>>,
    /// True when the match item has non-empty query_params conditions.
    /// Used to skip query string parsing on the hot path for routes that don't need it.
    pub has_query_matchers: bool,
    /// Pre-computed match specificity for Gateway API precedence ordering.
    pub specificity: MatchSpecificity,
    /// Pre-resolved Counter/Histogram handles for the request-end metric
    /// path. The hot path indexes by
    /// `lookup(gw_ns, gw_name).counters[backend_ref_idx][ps_index(...)]`.
    /// Invariants: `BackendRequestsLabels` / `RequestDurationConfig.labels`
    /// are read once at build time (init-once globals — see
    /// `docs/superpowers/specs/2026-06-10-observe-f002-route-unit-metric-handles-design.md`
    /// §6); `backend_refs` is never mutated in place, so indices stay valid
    /// for the lifetime of this `Arc<HttpRouteRuleUnit>`.
    pub metric_handles: Arc<crate::core::gateway::observe::metrics::route_handles::RouteMetricHandles>,
    /// Compiled rule-level plugin chain (built from `rule.filters`). Lives here,
    /// not on the `HTTPRouteRule` schema (two-type split).
    pub plugin_runtime: Arc<PluginRuntime>,
    /// Per-rule compiled backends (each with its own plugin runtime), shared
    /// across the rule's units. Used for by-name lookup (Internal upstream jumps).
    pub backends: Arc<Vec<CompiledBackend>>,
    /// LB backend selector over compiled backends, built at unit construction.
    /// `Arc` so the `#[derive(Clone)]` unit stays cheap to clone.
    pub backend_finder: Arc<BackendSelector<CompiledBackend>>,
}

/// Per-rule compiled runtime, built ONCE per rule (mirroring today's `preparse`,
/// which builds `rule.plugin_runtime` once per rule). The `Arc<PluginRuntime>` and
/// the per-backend `plugin_runtime`s MUST be shared across all of a rule's
/// match-item units — some plugins (in-memory RateLimit) own per-rule mutable
/// state, so a fresh runtime per match-item would split that state. See
/// `compile_http_rule`.
pub struct HttpRuleRuntime {
    /// Rule-level compiled plugin chain (one per rule, shared across all units).
    pub plugin_runtime: Arc<PluginRuntime>,
    /// Compiled backends (each with its own per-rule-shared plugin runtime). The
    /// per-(rule,hostname) LB selector is built over a clone of these. `Arc` so it
    /// is shared (not deep-cloned) into each of the rule's units.
    pub backends: Arc<Vec<CompiledBackend>>,
}

impl HttpRouteRuleUnit {
    /// Compile a rule's runtime ONCE (rule plugin chain + compiled backends) from
    /// the rule/backend `filters`. Call once per rule (above the hostname/match
    /// fan-out) and share the resulting `Arc`s into every unit — some plugins
    /// (in-memory RateLimit) own per-rule state, so the runtime must NOT be rebuilt
    /// per match-item/hostname. Replaces the runtime-building that used to live in
    /// `HTTPRoute::preparse()`.
    pub fn compile_http_rule(rule: &HTTPRouteRule, namespace: &str) -> HttpRuleRuntime {
        let plugin_runtime = Arc::new(match &rule.filters {
            Some(filters) => PluginRuntime::from_httproute_filters_for_rule(filters, namespace),
            None => PluginRuntime::new(),
        });
        let backends: Vec<CompiledBackend> = rule.backend_refs.as_ref().map_or_else(Vec::new, |refs| {
            refs.iter()
                .map(|br| {
                    let plugin_runtime = Arc::new(match &br.filters {
                        Some(filters) => PluginRuntime::from_httproute_filters_for_backend(filters, namespace),
                        None => PluginRuntime::new(),
                    });
                    CompiledBackend {
                        schema: br.clone(),
                        plugin_runtime,
                    }
                })
                .collect()
        });
        HttpRuleRuntime {
            plugin_runtime,
            backends: Arc::new(backends),
        }
    }

    /// Build a per-(rule,hostname) LB selector over a rule's compiled backends.
    /// Mirrors today's per-hostname `backend_finder` (each `rule.clone()` per
    /// hostname gets a fresh selector). The compiled backends are Arc-shared, so
    /// only the WRR cursor is per-hostname.
    pub fn build_backend_selector(backends: &[CompiledBackend]) -> Arc<BackendSelector<CompiledBackend>> {
        let selector = BackendSelector::new();
        if !backends.is_empty() {
            let weights: Vec<Option<i32>> = backends.iter().map(|b| b.schema.weight.or(Some(1))).collect();
            selector.init(backends.to_vec(), weights);
        }
        Arc::new(selector)
    }

    /// Pre-compile RegularExpression regexes for header and query param matchers.
    /// Call once at route creation time; the returned vecs are positionally aligned
    /// with the `headers` / `query_params` slices in `HTTPRouteMatch`.
    pub fn compile_match_regexes(
        match_item: &crate::types::HTTPRouteMatch,
    ) -> (Vec<Option<Regex>>, Vec<Option<Regex>>) {
        // Controller validates regex patterns at preparse (see ref_grant/validator.rs::check_regex_pattern),
        // so compilation failures here indicate a controller/gateway sync invariant violation.
        let header_regexes = match_item.headers.as_ref().map_or_else(Vec::new, |headers| {
            headers
                .iter()
                .map(|hm| {
                    if hm.match_type.as_deref() == Some("RegularExpression") {
                        match Regex::new(&hm.value) {
                            Ok(r) => Some(r),
                            Err(e) => {
                                tracing::error!(header = %hm.name, pattern = %hm.value, error = %e,
                                    "Header regex failed to compile at gateway despite controller validation; matcher will fail-closed");
                                None
                            }
                        }
                    } else {
                        None
                    }
                })
                .collect()
        });
        let query_regexes = match_item.query_params.as_ref().map_or_else(Vec::new, |params| {
            params
                .iter()
                .map(|qm| {
                    if qm.match_type.as_deref() == Some("RegularExpression") {
                        match Regex::new(&qm.value) {
                            Ok(r) => Some(r),
                            Err(e) => {
                                tracing::error!(query = %qm.name, pattern = %qm.value, error = %e,
                                    "Query regex failed to compile at gateway despite controller validation; matcher will fail-closed");
                                None
                            }
                        }
                    } else {
                        None
                    }
                })
                .collect()
        });
        (header_regexes, query_regexes)
    }

    /// Check if this is a regex route
    pub fn is_regex_route(&self) -> bool {
        self.path_regex.is_some()
    }

    /// Try to match the request path against the regex pattern (if this is a regex route)
    pub fn matches_path(&self, path: &str) -> bool {
        if let Some(ref regex) = self.path_regex {
            regex.is_match(path)
        } else {
            false
        }
    }

    /// Perform deep match (headers, query params, method, sectionName/Gateway).
    ///
    /// Returns `Some(GatewayInfo)` of the matched gateway on success, `None` on failure.
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `ctx`: Request context containing hostname and other request info
    /// - `index`: Pre-built gateway/listener index for this port
    pub fn deep_match(
        &self,
        session: &Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
    ) -> Result<Option<GatewayInfo>, EdError> {
        let req_header = session.req_header();
        Self::deep_match_common(
            &self.matched_info,
            req_header,
            &self.parent_refs,
            ctx,
            index,
            &self.compiled_header_regexes,
            &self.compiled_query_regexes,
            self.has_query_matchers,
        )
    }

    /// Get route identifier
    pub fn identifier(&self) -> String {
        format!("{}/{}", self.matched_info.rsc.ns, self.matched_info.rsc.name)
    }

    /// Match HTTP header.
    /// `compiled_regex`: pre-compiled regex for this header matcher (if RegularExpression).
    pub(crate) fn match_header(
        req_header: &pingora_http::RequestHeader,
        header_match: &crate::types::HTTPHeaderMatch,
        compiled_regex: Option<&Regex>,
    ) -> Result<bool, EdError> {
        let header_value = match req_header.headers.get(&header_match.name) {
            Some(value) => value.to_str().unwrap_or(""),
            None => return Ok(false),
        };

        let match_type = header_match.match_type.as_deref().unwrap_or("Exact");

        match match_type {
            "Exact" => Ok(header_value == header_match.value),
            "RegularExpression" => match compiled_regex {
                Some(re) => Ok(re.is_match(header_value)),
                None => Ok(false),
            },
            _ => Ok(false),
        }
    }

    /// Match query parameter.
    /// `compiled_regex`: pre-compiled regex for this query matcher (if RegularExpression).
    pub(crate) fn match_query_param(
        query_params: &HashMap<String, String>,
        query_param_match: &crate::types::HTTPQueryParamMatch,
        compiled_regex: Option<&Regex>,
    ) -> Result<bool, EdError> {
        let param_value = match query_params.get(&query_param_match.name) {
            Some(value) => value,
            None => return Ok(false),
        };

        let match_type = query_param_match.match_type.as_deref().unwrap_or("Exact");

        match match_type {
            "Exact" => Ok(param_value == &query_param_match.value),
            "RegularExpression" => match compiled_regex {
                Some(re) => Ok(re.is_match(param_value)),
                None => Ok(false),
            },
            _ => Ok(false),
        }
    }

    /// Common deep match logic for checking Gateway/sectionName, method, headers, and query parameters.
    ///
    /// Returns `Some(GatewayInfo)` of the matched gateway on success, `None` on failure.
    #[allow(clippy::too_many_arguments)] // all 8 params are distinct match-time inputs; a context struct would add indirection
    pub(crate) fn deep_match_common(
        matched_info: &MatchInfo,
        req_header: &pingora_http::RequestHeader,
        parent_refs: &Option<Vec<ParentReference>>,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
        compiled_header_regexes: &[Option<Regex>],
        compiled_query_regexes: &[Option<Regex>],
        has_query_matchers: bool,
    ) -> Result<Option<GatewayInfo>, EdError> {
        let method = req_header.method.as_str();
        let match_item = &matched_info.m;

        // 0. Check Gateway/Listener constraints (sectionName + hostname isolation)
        let matched_gi = if let Some(ref parent_refs) = parent_refs {
            match check_gateway_listener_match(
                parent_refs,
                index,
                &ctx.request_info.hostname,
                &matched_info.rsc.ns,
                "HTTPRoute",
            ) {
                Some(gi) => gi,
                None => return Ok(None),
            }
        } else {
            return Ok(None);
        };

        // 1. Check HTTP Method (if specified)
        if let Some(match_method) = &match_item.method {
            if method != match_method.as_str() {
                return Ok(None);
            }
        }

        // 2. Check Headers (if specified) - ALL must match (AND logic)
        if let Some(header_matches) = &match_item.headers {
            for (idx, header_match) in header_matches.iter().enumerate() {
                let pre = compiled_header_regexes.get(idx).and_then(|r| r.as_ref());
                if !Self::match_header(req_header, header_match, pre)? {
                    return Ok(None);
                }
            }
        }

        // 3. Check Query Parameters — lazy parse: only when this route has query matchers.
        // ctx.cached_query() ensures the query string is parsed at most once per request
        // across all route candidates, sharing the result with PluginSession::get_query_param.
        // Use entry_raw_query (frozen at request entry) so route matching observes the
        // client-sent query, not any rewritten upstream URI.
        if has_query_matchers {
            if let Some(query_param_matches) = &match_item.query_params {
                let query_params = ctx.cached_query(ctx.request_info.entry_raw_query.as_deref());
                for (idx, query_param_match) in query_param_matches.iter().enumerate() {
                    let pre = compiled_query_regexes.get(idx).and_then(|r| r.as_ref());
                    if !Self::match_query_param(query_params, query_param_match, pre)? {
                        return Ok(None);
                    }
                }
            }
        }

        Ok(Some(matched_gi))
    }

    /// Extract all path patterns from this route with their match types
    /// Returns Vec<(path, is_prefix)>
    pub fn extract_paths(&self) -> Vec<(String, bool)> {
        let mut paths = Vec::new();

        // Extract path from the single match_item
        if let Some(path) = &self.matched_info.m.path {
            if let Some(value) = &path.value {
                let is_prefix = path.match_type.as_deref().map(|t| t == "PathPrefix").unwrap_or(false);
                paths.push((value.clone(), is_prefix));
            }
        }

        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_specificity(
        has_method: bool,
        header_count: u16,
        query_count: u16,
        creation_ts_ms: i64,
        route_key: &str,
        rule_index: u32,
        match_index: u32,
    ) -> MatchSpecificity {
        MatchSpecificity {
            has_method,
            header_count,
            query_count,
            creation_ts_ms,
            route_key: route_key.to_string(),
            rule_index,
            match_index,
        }
    }

    #[test]
    fn test_specificity_method_wins() {
        let with_method = make_specificity(true, 0, 0, 1000, "ns/a", 0, 0);
        let without_method = make_specificity(false, 0, 0, 1000, "ns/a", 0, 0);
        assert_eq!(with_method.cmp_precedence(&without_method), Ordering::Less);
        assert_eq!(without_method.cmp_precedence(&with_method), Ordering::Greater);
    }

    #[test]
    fn test_specificity_more_headers_wins() {
        let h3 = make_specificity(true, 3, 0, 1000, "ns/a", 0, 0);
        let h2 = make_specificity(true, 2, 0, 1000, "ns/a", 0, 0);
        let h0 = make_specificity(true, 0, 0, 1000, "ns/a", 0, 0);
        assert_eq!(h3.cmp_precedence(&h2), Ordering::Less);
        assert_eq!(h2.cmp_precedence(&h0), Ordering::Less);
        assert_eq!(h3.cmp_precedence(&h0), Ordering::Less);
    }

    #[test]
    fn test_specificity_more_query_params_wins() {
        let q2 = make_specificity(true, 1, 2, 1000, "ns/a", 0, 0);
        let q1 = make_specificity(true, 1, 1, 1000, "ns/a", 0, 0);
        let q0 = make_specificity(true, 1, 0, 1000, "ns/a", 0, 0);
        assert_eq!(q2.cmp_precedence(&q1), Ordering::Less);
        assert_eq!(q1.cmp_precedence(&q0), Ordering::Less);
    }

    #[test]
    fn test_specificity_older_timestamp_wins() {
        let old = make_specificity(true, 1, 1, 100, "ns/a", 0, 0);
        let new = make_specificity(true, 1, 1, 200, "ns/a", 0, 0);
        assert_eq!(old.cmp_precedence(&new), Ordering::Less);
        assert_eq!(new.cmp_precedence(&old), Ordering::Greater);
    }

    #[test]
    fn test_specificity_alphabetical_route_key() {
        let a = make_specificity(true, 1, 1, 100, "ns/aaa", 0, 0);
        let b = make_specificity(true, 1, 1, 100, "ns/bbb", 0, 0);
        assert_eq!(a.cmp_precedence(&b), Ordering::Less);
        assert_eq!(b.cmp_precedence(&a), Ordering::Greater);
    }

    #[test]
    fn test_specificity_rule_index_tiebreak() {
        let r0 = make_specificity(true, 1, 1, 100, "ns/a", 0, 0);
        let r1 = make_specificity(true, 1, 1, 100, "ns/a", 1, 0);
        assert_eq!(r0.cmp_precedence(&r1), Ordering::Less);
    }

    #[test]
    fn test_specificity_match_index_tiebreak() {
        let m0 = make_specificity(true, 1, 1, 100, "ns/a", 0, 0);
        let m1 = make_specificity(true, 1, 1, 100, "ns/a", 0, 1);
        assert_eq!(m0.cmp_precedence(&m1), Ordering::Less);
    }

    #[test]
    fn test_specificity_equal() {
        let a = make_specificity(true, 1, 1, 100, "ns/a", 0, 0);
        let b = make_specificity(true, 1, 1, 100, "ns/a", 0, 0);
        assert_eq!(a.cmp_precedence(&b), Ordering::Equal);
    }

    #[test]
    fn test_specificity_method_trumps_headers() {
        let with_method = make_specificity(true, 0, 0, 1000, "ns/a", 0, 0);
        let many_headers = make_specificity(false, 10, 10, 100, "ns/a", 0, 0);
        assert_eq!(with_method.cmp_precedence(&many_headers), Ordering::Less);
    }
}
