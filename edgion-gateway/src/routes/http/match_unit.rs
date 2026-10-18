use super::match_engine::radix_path::RadixPath;
use crate::backends::runtime::{CompiledBackendTarget, CompiledServiceTarget};
use crate::ctx::EdgionHttpContext;
use crate::lb::BackendSelector;
use crate::plugins::PluginRuntime;
use crate::runtime::matching::route::{check_gateway_listener_match, ResolvedParentRef};
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use crate::runtime::GatewayInfo;
use crate::MatchInfo;
use edgion_algo::matcher::radix_path::{PathMatchKind, PathRadixBuilder, PathRadixTree};
use edgion_resources::err::EdError;
use edgion_resources::path_normalization::{
    get_path_normalization_config, normalize_config_path, PathNormalizationConfig,
};
use edgion_resources::resources::http_route::{ParentMatch, ResolvedParentMatch, MAX_HTTP_ROUTE_DELEGATION_DEPTH};
use edgion_resources::resources::refs::route_fields::HttpRouteBackendRefs;
use edgion_resources::{HTTPBackendRef, HTTPRoute, HTTPRouteRule};
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
/// Only the actual Route compiler can construct a compiled slot.
/// ```compile_fail
/// use edgion_gateway::routes::http::match_unit::CompiledBackend;
/// fn overwrite(compiled: &mut CompiledBackend) {
///     compiled.schema.name = "replacement".into();
/// }
/// ```
#[derive(Clone)]
pub struct CompiledBackend {
    /// The pure backend-ref schema (name, port, weight, …). Owned by the
    /// immutable compiled generation. Per-request policy winners are stored on
    /// selected handles and never mutate this schema.
    schema: HTTPBackendRef,
    /// Original position in `HTTPRouteRule.backendRefs`. This remains stable
    /// across typed filtering and is the conformance/metrics slot identity.
    slot: usize,
    /// Canonical typed identity shared with Controller contextual admission.
    identity: edgion_resources::LogicalBackendSlotIdentity,
    /// Cold-path classification. Service-only consumers must explicitly match
    /// `Service`; AI routing consumes the full ordered slot list separately.
    target: CompiledBackendTarget,
    /// Compiled backend-level plugin chain (built from `schema.filters`).
    pub plugin_runtime: Arc<PluginRuntime>,
}

impl CompiledBackend {
    pub fn is_service(&self) -> bool {
        matches!(self.target, CompiledBackendTarget::Service(_))
    }

    /// Whether this slot participates in ordinary HTTP backend selection.
    /// Invalid refs retain their declared Gateway API traffic share and fail
    /// after selection. AI dispatch is separate; ordinary HTTP rejects an AI slot.
    fn is_generic_selectable(&self) -> bool {
        matches!(
            self.target,
            CompiledBackendTarget::Service(_)
                | CompiledBackendTarget::EdgionBackend(_)
                | CompiledBackendTarget::Invalid { .. }
        )
    }

    /// Whether this backend is an authorized EdgionBackend reference.
    ///
    /// AI routing bypasses the generic Service selector, so its callers must
    /// use this predicate rather than kind-only matching to preserve the
    /// Controller's ReferenceGrant decision.
    pub fn is_permitted_ai_backend(&self) -> bool {
        self.schema.ref_denied.is_none() && matches!(self.target, CompiledBackendTarget::EdgionBackend(_))
    }

    pub fn schema(&self) -> &HTTPBackendRef {
        &self.schema
    }
    pub fn slot(&self) -> usize {
        self.slot
    }
    pub fn identity(&self) -> &edgion_resources::LogicalBackendSlotIdentity {
        &self.identity
    }
    pub fn target(&self) -> &CompiledBackendTarget {
        &self.target
    }
    pub fn permitted_service(&self) -> Option<&CompiledServiceTarget> {
        if self.ref_denied.is_some() {
            return None;
        }
        match &self.target {
            CompiledBackendTarget::Service(service) => Some(service),
            _ => None,
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

/// Canonical PathPrefix conjunction for one delegated effective match.
/// At most eight IDs fit the radix result's inline capacity; at most eight
/// root-to-terminal paths also bound its DFS frontier below inline capacity 16.
pub struct CompiledParentMatch {
    tree: PathRadixTree,
    required: usize,
}

impl CompiledParentMatch {
    /// Returns the guard and whether every canonical ancestor is literal.
    fn compile(scope: &ParentMatch, config: &PathNormalizationConfig) -> Option<(Self, bool)> {
        if scope.raw_path_prefixes.is_empty() || scope.raw_path_prefixes.len() > MAX_HTTP_ROUTE_DELEGATION_DEPTH {
            return None;
        }
        let mut builder = PathRadixBuilder::new();
        let mut all_literal = true;
        for (id, prefix) in scope.raw_path_prefixes.iter().enumerate() {
            if !prefix.starts_with('/') {
                return None;
            }
            let normalized = normalize_config_path(prefix, config);
            let path = RadixPath::new(&normalized, 0, true);
            all_literal &= !path.tree_key().contains("/:");
            builder.insert(path.tree_key(), id).ok()?;
        }
        Some((
            Self {
                tree: builder.freeze().ok()?,
                required: scope.raw_path_prefixes.len(),
            },
            all_literal,
        ))
    }

    fn matches(&self, path: &str) -> bool {
        let matches = self.tree.match_all_ext(path);
        matches.len() == self.required && matches.iter().all(|(_, kind)| *kind != PathMatchKind::PartialSegment)
    }
}

/// Validate provenance before constructing any plugins or publishing any units.
fn compile_parent_matches(
    route: &HTTPRoute,
    rule: &HTTPRouteRule,
    config: &PathNormalizationConfig,
) -> Option<Vec<Option<Arc<CompiledParentMatch>>>> {
    match (&rule.resolved_parent_match, route.spec.resolved_rules.is_some()) {
        (None, false) | (Some(ResolvedParentMatch::Direct {}), _) => Some(Vec::new()),
        (Some(ResolvedParentMatch::Delegated { matches }), true) => {
            let effective = rule.matches.as_ref()?;
            if effective.is_empty() || matches.len() != effective.len() {
                return None;
            }
            matches
                .iter()
                .zip(effective)
                .map(|(scope, child)| {
                    let (guard, all_literal) = CompiledParentMatch::compile(scope, config)?;
                    let redundant = all_literal
                        && child.path.as_ref().is_some_and(|path| {
                            let kind = path.match_type.as_deref().unwrap_or("PathPrefix");
                            if !matches!(kind, "Exact" | "PathPrefix") {
                                return false;
                            }
                            // Child paths already passed Gateway preparse. Reuse the
                            // canonical matcher, without another BASE decode. For
                            // literal prefixes, matching the child root proves every
                            // slash-delimited extension stays inside all ancestors.
                            let child = RadixPath::new(path.value.as_deref().unwrap_or("/"), 0, kind == "PathPrefix");
                            child.tree_key().starts_with('/')
                                && !child.tree_key().contains("/:")
                                && guard.matches(child.tree_key())
                        });
                    Some(if redundant { None } else { Some(Arc::new(guard)) })
                })
                .collect()
        }
        _ => None,
    }
}

#[derive(Clone)]
pub struct HttpRouteRuleUnit {
    /// Parent path constraints from the same immutable effective rule generation.
    pub parent_match: Option<Arc<CompiledParentMatch>>,
    /// Validated lifetime from this effective rule generation, resolved before publication.
    pub session_persistence_ttl_secs: Option<u64>,
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
    /// Empty for direct rules; otherwise aligned with effective matches.
    /// A None entry is a proven redundant guard, never an omitted scope.
    pub parent_matches: Vec<Option<Arc<CompiledParentMatch>>>,
    /// Validated lifetime from this effective rule generation, resolved before publication.
    pub session_persistence_ttl_secs: Option<u64>,
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
    pub fn compile_http_rule(
        route: &HTTPRoute,
        rule_index: usize,
    ) -> Result<HttpRuleRuntime, edgion_resources::RequestTransformViolation> {
        let rule = route
            .effective_rules()
            .and_then(|rules| rules.get(rule_index))
            .ok_or_else(|| edgion_resources::RequestTransformViolation {
                path: "spec.rules".to_string(),
                message: "effective rule is unavailable".to_string(),
            })?;
        let violations = edgion_resources::effective_request_transform_violations(rule).map_err(|_| {
            edgion_resources::RequestTransformViolation {
                path: String::new(),
                message: "request transform validation contract is unavailable".to_string(),
            }
        })?;
        if let Some(violation) = violations.into_iter().next() {
            return Err(violation);
        }
        Self::compile_prevalidated_http_rule(route, rule_index).ok_or_else(|| {
            edgion_resources::RequestTransformViolation {
                path: String::new(),
                message: "invalid effective rule or Route reference source".to_string(),
            }
        })
    }

    pub(super) fn compile_prevalidated_http_rule(route: &HTTPRoute, rule_index: usize) -> Option<HttpRuleRuntime> {
        Self::compile_with_factories(
            route,
            rule_index,
            |route, rule_index| {
                let tuning = route
                    .effective_rules()
                    .and_then(|rules| rules.get(rule_index))
                    .and_then(|rule| rule.parsed_mirror_tuning.clone())
                    .unwrap_or_default();
                Arc::new(PluginRuntime::from_httproute_filters_for_rule(
                    route, rule_index, &tuning,
                ))
            },
            |route, rule_index, backend_index| {
                let tuning = route
                    .effective_rules()
                    .and_then(|rules| rules.get(rule_index))
                    .and_then(|rule| rule.parsed_mirror_tuning.clone())
                    .unwrap_or_default();
                Arc::new(PluginRuntime::from_httproute_filters_for_backend(
                    route,
                    rule_index,
                    backend_index,
                    &tuning,
                ))
            },
        )
    }

    fn compile_with_factories(
        route: &HTTPRoute,
        rule_index: usize,
        mut rule_factory: impl FnMut(&HTTPRoute, usize) -> Arc<PluginRuntime>,
        mut backend_factory: impl FnMut(&HTTPRoute, usize, usize) -> Arc<PluginRuntime>,
    ) -> Option<HttpRuleRuntime> {
        let references = HttpRouteBackendRefs::from_route(route).ok()?;
        let rule = route.effective_rules()?.get(rule_index)?;
        let parent_matches = compile_parent_matches(route, rule, get_path_normalization_config())?;
        let session_persistence_ttl_secs = rule
            .session_persistence
            .as_ref()
            .map(|policy| policy.validated_lifetime_secs())
            .transpose()
            .ok()?
            .flatten();
        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        let plugin_runtime = rule_factory(route, rule_index);
        let backends = rule.backend_refs.as_ref().map_or_else(Vec::new, |refs| {
            let identities = edgion_resources::logical_backend_slot_identities(refs, namespace);
            refs.iter()
                .zip(identities)
                .enumerate()
                .map(|(slot, (schema, identity))| {
                    let reference = references
                        .effective_at(rule_index, slot)
                        .expect("the backend came from this actual effective rule");
                    let target = CompiledBackendTarget::from_reference(reference, schema.backend_kind());
                    let plugin_runtime =
                        if schema.ref_denied.is_none() && !matches!(target, CompiledBackendTarget::Invalid { .. }) {
                            backend_factory(route, rule_index, slot)
                        } else {
                            Arc::new(PluginRuntime::new())
                        };
                    CompiledBackend {
                        schema: schema.clone(),
                        slot,
                        identity,
                        target,
                        plugin_runtime,
                    }
                })
                .collect::<Vec<_>>()
        });
        Some(HttpRuleRuntime {
            parent_matches,
            session_persistence_ttl_secs,
            plugin_runtime,
            backends: backends.into(),
        })
    }

    /// Build a per-rule LB selector over original backend slots. Its WRR cursor
    /// and indexes are shared across the rule's match items and hostname groups.
    pub fn build_backend_selector(backends: &[CompiledBackend]) -> Arc<BackendSelector<usize>> {
        let generic_backends = backends
            .iter()
            .filter(|backend| backend.is_generic_selectable())
            .collect::<Vec<_>>();
        let weights = generic_backends
            .iter()
            .map(|backend| backend.schema.weight.or(Some(1)))
            .collect();
        Arc::new(BackendSelector::new(
            generic_backends.iter().map(|backend| backend.slot).collect(),
            weights,
        ))
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
        if self
            .parent_match
            .as_ref()
            .is_some_and(|parent| !parent.matches(ctx.path()))
        {
            return Ok(None);
        }
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
        // The context owns the frozen entry query shared by all readers.
        if has_query_matchers {
            if let Some(query_param_matches) = &match_item.query_params {
                let query_params = ctx.cached_query();
                for (idx, query_param_match) in query_param_matches.iter().enumerate() {
                    let pre = compiled_query_regexes.get(idx).and_then(|r| r.as_ref());
                    if !Self::match_query_param(query_params, query_param_match, pre)? {
                        return Ok(None);
                    }
                }
            }
        }

        Ok(Some(Arc::clone(matched_gi)))
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
#[path = "../../../tests/unit/routes/http/match_unit_test.rs"]
mod tests;

#[cfg(test)]
include!("../../../tests/support/routes/http/match_unit_test/compiled_fixtures.rs");

#[cfg(test)]
use edgion_resources::HTTPBackendRefKind;
