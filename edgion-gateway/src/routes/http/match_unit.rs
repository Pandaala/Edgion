use crate::ctx::EdgionHttpContext;
use crate::lb::BackendSelector;
use crate::plugins::PluginRuntime;
use crate::runtime::matching::route::{check_gateway_listener_match, ResolvedParentRef};
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use crate::runtime::GatewayInfo;
use crate::MatchInfo;
use edgion_resources::err::EdError;
use edgion_resources::{HTTPBackendRef, HTTPBackendRefKind, HTTPRoute, HTTPRouteRule};
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
/// Path-level precedence (Exact vs Prefix, static constraints, depth) is handled separately by
/// RadixPath.specificity, which sorts *before* this struct.
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
    /// compare RadixPath.specificity first.
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
    pub matched_gateway: Arc<GatewayInfo>,
}

/// A backend ref paired with its compiled plugin runtime. The LB selector stores
/// only original slot indexes; request handles borrow this object from the
/// immutable per-rule generation. This keeps the gateway-compiled
/// `PluginRuntime` off the `HTTPBackendRef` schema (two-type split).
#[derive(Clone)]
pub struct CompiledBackend {
    /// The pure backend-ref schema (name, port, weight, …). Owned by the
    /// immutable compiled generation. Per-request policy winners are stored on
    /// selected handles and never mutate this schema.
    pub schema: HTTPBackendRef,
    /// Original position in `HTTPRouteRule.backendRefs`. This remains stable
    /// across typed filtering and is the conformance/metrics slot identity.
    pub slot: usize,
    /// Canonical typed identity shared with Controller contextual admission.
    pub identity: edgion_resources::LogicalBackendSlotIdentity,
    /// Cold-path classification. Service-only consumers must explicitly match
    /// `Service`; AI routing consumes the full ordered slot list separately.
    pub target: CompiledBackendTarget,
    /// Precompiled Service authorization for direct endpoint overrides.
    pub direct_endpoint: Option<crate::backends::validation::DirectEndpointAuthorization>,
    /// Compiled backend-level plugin chain (built from `schema.filters`).
    pub plugin_runtime: Arc<PluginRuntime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompiledBackendTarget {
    Service { namespace: String, name: String },
    EdgionBackend { namespace: String, name: String },
    Invalid { classified_kind: HTTPBackendRefKind },
}

impl CompiledBackend {
    pub fn is_service(&self) -> bool {
        matches!(self.target, CompiledBackendTarget::Service { .. })
    }

    /// Whether this slot participates in ordinary HTTP backend selection.
    /// Invalid refs retain their declared Gateway API traffic share and fail
    /// after selection; valid AI refs are owned by the AI dispatch path.
    fn is_generic_selectable(&self) -> bool {
        matches!(
            self.target,
            CompiledBackendTarget::Service { .. }
                | CompiledBackendTarget::EdgionBackend { .. }
                | CompiledBackendTarget::Invalid { .. }
        )
    }

    /// Whether this backend is an authorized EdgionBackend reference.
    ///
    /// AI routing bypasses the generic Service selector, so its callers must
    /// use this predicate rather than kind-only matching to preserve the
    /// Controller's ReferenceGrant decision.
    pub fn is_permitted_ai_backend(&self) -> bool {
        self.schema.ref_denied.is_none() && matches!(self.target, CompiledBackendTarget::EdgionBackend { .. })
    }

    pub fn compile(
        schema: HTTPBackendRef,
        route_namespace: &str,
        slot: usize,
        plugin_runtime: Arc<PluginRuntime>,
    ) -> Self {
        let identity =
            edgion_resources::logical_backend_slot_identities(std::slice::from_ref(&schema), route_namespace)
                .into_iter()
                .next()
                .expect("one backend ref produces one identity");
        Self::compile_with_identity(schema, route_namespace, slot, identity, plugin_runtime)
    }

    fn compile_with_identity(
        schema: HTTPBackendRef,
        route_namespace: &str,
        slot: usize,
        identity: edgion_resources::LogicalBackendSlotIdentity,
        plugin_runtime: Arc<PluginRuntime>,
    ) -> Self {
        let target = Self::compile_target(&schema, route_namespace);
        let direct_endpoint =
            crate::backends::validation::DirectEndpointAuthorization::compile(&schema, route_namespace);
        Self {
            schema,
            slot,
            identity,
            target,
            direct_endpoint,
            plugin_runtime,
        }
    }

    fn compile_target(schema: &HTTPBackendRef, route_namespace: &str) -> CompiledBackendTarget {
        let namespace = schema.namespace.as_deref().unwrap_or(route_namespace).to_string();
        match schema.backend_kind() {
            HTTPBackendRefKind::Service => CompiledBackendTarget::Service {
                namespace,
                name: schema.name.clone(),
            },
            HTTPBackendRefKind::EdgionBackend if schema.port.is_none() => CompiledBackendTarget::EdgionBackend {
                namespace,
                name: schema.name.clone(),
            },
            classified_kind => CompiledBackendTarget::Invalid { classified_kind },
        }
    }
}

impl std::ops::Deref for CompiledBackend {
    type Target = HTTPBackendRef;
    /// Deref to the immutable backend schema so identity, port, and filter
    /// consumers share the compiled generation.
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
    /// Shared handle to the full matched route, for metadata-only template
    /// access (`${matchedHttpRoute:...}`). Same `Arc` the route cache holds —
    /// zero-copy. `None` only in unit-test fixtures that build the unit directly.
    pub route: Option<Arc<HTTPRoute>>,
    /// Compiled regex for path matching (only for RegularExpression path type)
    pub path_regex: Option<Regex>,
    /// Parent references for sectionName matching, with their
    /// `GatewayInfoIndex` lookup keys resolved at snapshot build time.
    /// `Arc`-shared across every match-item unit of the same route.
    pub parent_refs: Option<Arc<[ResolvedParentRef]>>,
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
    /// Pre-resolved Counter/Histogram handles for the request-end metric path.
    /// The hot path indexes by `lookup(gw_ns, gw_name).counters[backend_ref_idx][ps_index(...)]`.
    /// Invariants: label configs are read once at build time (init-once globals);
    /// `backend_refs` is never mutated in place, so indices stay valid for the
    /// lifetime of this `Arc<HttpRouteRuleUnit>`.
    pub metric_handles: Arc<crate::observe::metrics::route_handles::RouteMetricHandles>,
    /// Compiled rule-level plugin chain (built from `rule.filters`). Lives here,
    /// not on the `HTTPRouteRule` schema (two-type split).
    pub plugin_runtime: Arc<PluginRuntime>,
    /// Per-rule compiled backends (each with its own plugin runtime), shared
    /// across the rule's units. Used for by-name lookup (Internal upstream jumps).
    pub backends: Arc<[CompiledBackend]>,
    /// LB selector over original compiled-backend slot indexes, built once per
    /// rule and shared across its match items and declared hostname groups.
    /// `Arc` so the `#[derive(Clone)]` unit stays cheap to clone.
    pub backend_finder: Arc<BackendSelector<usize>>,
}

/// Per-rule compiled runtime, built ONCE per rule. The `Arc<PluginRuntime>` and
/// the per-backend `plugin_runtime`s MUST be shared across all of a rule's
/// match-item units — some plugins (in-memory RateLimitLocal) own per-rule mutable
/// state, so a fresh runtime per match-item would split that state. See
/// `compile_http_rule`.
pub struct HttpRuleRuntime {
    /// Rule-level compiled plugin chain (one per rule, shared across all units).
    pub plugin_runtime: Arc<PluginRuntime>,
    /// Compiled backends (each with its own per-rule-shared plugin runtime). The
    /// per-rule LB selector is built over these backend slots. `Arc` so it
    /// is shared (not deep-cloned) into each of the rule's units.
    pub backends: Arc<[CompiledBackend]>,
}

impl HttpRouteRuleUnit {
    /// Compile a rule's runtime ONCE (rule plugin chain + compiled backends) from
    /// the rule/backend `filters`. Call once per rule (above the hostname/match
    /// fan-out) and share the resulting `Arc`s into every unit — some plugins
    /// (in-memory RateLimitLocal) own per-rule state, so the runtime must NOT be rebuilt
    /// per match-item/hostname.
    pub fn compile_http_rule(rule: &HTTPRouteRule, namespace: &str) -> HttpRuleRuntime {
        // Route-level `edgion.io/mirror-*` annotation values, pre-parsed onto
        // the rule; defaults when no mirror annotation is present.
        let mirror_tuning = rule.parsed_mirror_tuning.clone().unwrap_or_default();
        let plugin_runtime = Arc::new(match &rule.filters {
            Some(filters) => PluginRuntime::from_httproute_filters_for_rule(filters, namespace, &mirror_tuning),
            None => PluginRuntime::new(),
        });
        let backends: Vec<CompiledBackend> = rule.backend_refs.as_ref().map_or_else(Vec::new, |refs| {
            let identities = edgion_resources::logical_backend_slot_identities(refs, namespace);
            refs.iter()
                .zip(identities)
                .enumerate()
                .map(|(slot, (br, identity))| {
                    let plugin_runtime = Arc::new(match &br.filters {
                        Some(filters) => {
                            PluginRuntime::from_httproute_filters_for_backend(filters, namespace, &mirror_tuning)
                        }
                        None => PluginRuntime::new(),
                    });
                    CompiledBackend::compile_with_identity(br.clone(), namespace, slot, identity, plugin_runtime)
                })
                .collect()
        });
        HttpRuleRuntime {
            plugin_runtime,
            backends: Arc::from(backends),
        }
    }

    /// Build a per-rule LB selector over original backend slots. Its WRR cursor
    /// and indexes are shared across the rule's match items and hostname groups.
    pub fn build_backend_selector(backends: &[CompiledBackend]) -> Arc<BackendSelector<usize>> {
        let selector = BackendSelector::new();
        let generic_backends = backends
            .iter()
            .filter(|backend| backend.is_generic_selectable())
            .collect::<Vec<_>>();
        if !generic_backends.is_empty() {
            let weights = generic_backends
                .iter()
                .map(|backend| backend.schema.weight.or(Some(1)))
                .collect();
            selector.init(generic_backends.iter().map(|backend| backend.slot).collect(), weights);
        }
        Arc::new(selector)
    }

    /// Pre-compile RegularExpression regexes for header and query param matchers.
    /// Call once at route creation time; the returned vecs are positionally aligned
    /// with the `headers` / `query_params` slices in `HTTPRouteMatch`.
    pub fn compile_match_regexes(
        match_item: &edgion_resources::HTTPRouteMatch,
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
    ) -> Result<Option<Arc<GatewayInfo>>, EdError> {
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

    /// Match HTTP header.
    /// `compiled_regex`: pre-compiled regex for this header matcher (if RegularExpression).
    pub(crate) fn match_header(
        req_header: &pingora_http::RequestHeader,
        header_match: &edgion_resources::HTTPHeaderMatch,
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
        query_param_match: &edgion_resources::HTTPQueryParamMatch,
        compiled_regex: Option<&Regex>,
    ) -> Result<bool, EdError> {
        let Some(param_value) = query_params.get(&query_param_match.name) else {
            return Ok(false);
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
        parent_refs: &Option<Arc<[ResolvedParentRef]>>,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
        compiled_header_regexes: &[Option<Regex>],
        compiled_query_regexes: &[Option<Regex>],
        has_query_matchers: bool,
    ) -> Result<Option<Arc<GatewayInfo>>, EdError> {
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

    /// Extract the optional path pattern from this route with its match type.
    pub fn extract_paths(&self) -> impl Iterator<Item = (&str, bool)> {
        self.matched_info
            .m
            .path
            .as_ref()
            .and_then(|path| {
                let is_prefix = path.match_type.as_deref().is_some_and(|t| t == "PathPrefix");
                path.value.as_deref().map(|value| (value, is_prefix))
            })
            .into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_backend_filter_remains_backend_scoped() {
        let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
            "backendRefs": [
                {
                    "name": "invalid",
                    "port": 8080,
                    "filters": [{"type": "ExtensionRef"}]
                },
                {"name": "valid", "port": 8080}
            ]
        }))
        .unwrap();

        let runtime = HttpRouteRuleUnit::compile_http_rule(&rule, "route-ns");

        assert!(runtime.backends[0].plugin_runtime.rejects_invalid_config());
        assert!(!runtime.backends[1].plugin_runtime.rejects_invalid_config());
        assert!(!runtime.plugin_runtime.rejects_invalid_config());
    }

    #[test]
    fn compile_http_rule_preserves_slots_and_types_backends() {
        let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
            "backendRefs": [
                {"name": "service-a", "port": 8080, "weight": 7},
                {"kind": "ServiceExternalName", "name": "dns-svc", "port": 30001, "weight": 1},
                {"kind": "ServiceImport", "name": "imported-svc", "port": 8080, "weight": 2},
                {"group": "edgion.io", "kind": "EdgionBackend", "name": "ai-a", "weight": 3},
                {"group": "edgion.io", "kind": "EdgionBackend", "name": "ai-bad", "port": 443},
                {"group": "example.io", "kind": "Service", "name": "wrong-group-service"}
            ]
        }))
        .unwrap();

        let runtime = HttpRouteRuleUnit::compile_http_rule(&rule, "route-ns");
        assert_eq!(runtime.backends.len(), 6);
        assert_eq!(
            runtime.backends.iter().map(|backend| backend.slot).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5]
        );
        assert_eq!(runtime.backends[0].schema.weight, Some(7));
        assert_eq!(
            runtime.backends[0].target,
            CompiledBackendTarget::Service {
                namespace: "route-ns".to_string(),
                name: "service-a".to_string()
            }
        );
        assert_eq!(
            runtime.backends[1].target,
            CompiledBackendTarget::Service {
                namespace: "route-ns".to_string(),
                name: "dns-svc".to_string()
            }
        );
        assert_eq!(
            runtime.backends[2].target,
            CompiledBackendTarget::Service {
                namespace: "route-ns".to_string(),
                name: "imported-svc".to_string()
            }
        );
        assert_eq!(
            runtime.backends[3].target,
            CompiledBackendTarget::EdgionBackend {
                namespace: "route-ns".to_string(),
                name: "ai-a".to_string()
            }
        );
        assert_eq!(
            runtime.backends[4].target,
            CompiledBackendTarget::Invalid {
                classified_kind: HTTPBackendRefKind::EdgionBackend
            }
        );
        assert_eq!(
            runtime.backends[5].target,
            CompiledBackendTarget::Invalid {
                classified_kind: HTTPBackendRefKind::Unsupported
            }
        );

        let selector = HttpRouteRuleUnit::build_backend_selector(&runtime.backends);
        // Service, EdgionBackend, and invalid slots retain their declared
        // traffic share. Two complete WRR cycles make the assertion independent
        // of the smooth sequence's partial-cycle ordering.
        let mut seen = [0u32; 6];
        for _ in 0..30 {
            let selected = selector.select().unwrap();
            seen[selected] += 1;
        }
        assert_eq!(seen, [14, 2, 4, 6, 2, 2]);
    }

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
