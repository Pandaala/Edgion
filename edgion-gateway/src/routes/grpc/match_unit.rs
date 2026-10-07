use crate::backends::policy::BackendTlsPolicyLookup;
use crate::backends::runtime::{CompiledBackendTarget, CompiledServiceTarget};
use crate::lb::BackendSelector;
use crate::plugins::PluginRuntime;
use crate::runtime::matching::route::{check_gateway_listener_match, hostname_matches_listener, ResolvedParentRef};
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use crate::runtime::GatewayInfo;
use edgion_resources::err::EdError;
use edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKey;
use edgion_resources::resources::grpc_route::CompiledGrpcMethodMatch;
use edgion_resources::resources::refs::route_fields::GrpcRouteBackendRefs;
use edgion_resources::{GRPCBackendRef, GRPCRoute, GRPCRouteMatch, GRPCRouteRule};
use pingora_proxy::Session;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::sync::Arc;

/// A gRPC backend ref paired with its compiled plugin runtime.
/// Mirrors `CompiledBackend` for HTTP routes (two-type split).
#[derive(Clone)]
pub struct CompiledGrpcBackend {
    /// Pure backend-ref schema owned by the immutable per-rule generation.
    /// Request-specific policy winners are stored on `SelectedGrpcBackend`.
    schema: GRPCBackendRef,
    /// Canonical effective-field classification. A denied Service retains its
    /// target and slot; only permitted_service exposes it to material consumers.
    /// Delegated or otherwise unsupported terminal descriptors are Invalid.
    target: CompiledBackendTarget,
    /// Compiled backend-level plugin chain (built from `schema.filters`).
    pub plugin_runtime: Arc<PluginRuntime>,
}

/// One immutable gRPC backend slot pinned to its compiled route generation.
#[derive(Clone)]
pub struct GrpcBackendHandle {
    generation: Arc<[CompiledGrpcBackend]>,
    index: usize,
}

impl GrpcBackendHandle {
    pub(crate) fn new(generation: Arc<[CompiledGrpcBackend]>, index: usize) -> Option<Self> {
        if generation.get(index).is_some() {
            Some(Self { generation, index })
        } else {
            None
        }
    }

    #[inline]
    pub fn compiled(&self) -> &CompiledGrpcBackend {
        &self.generation[self.index]
    }

    #[inline]
    pub fn slot(&self) -> usize {
        self.index
    }

    #[inline]
    pub fn policy_target_key(&self) -> Option<&PolicyTargetKey> {
        self.compiled()
            .permitted_service()
            .map(CompiledServiceTarget::policy_target)
    }
}

impl std::fmt::Debug for GrpcBackendHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrpcBackendHandle")
            .field("slot", &self.slot())
            .field("name", &self.compiled().name)
            .finish()
    }
}

/// The request-stable gRPC backend selection and its captured TLS policy.
#[derive(Clone, Debug)]
pub struct SelectedGrpcBackend {
    logical: GrpcBackendHandle,
    backend_tls_policy: BackendTlsPolicyLookup,
}

impl SelectedGrpcBackend {
    pub(crate) fn new(logical: GrpcBackendHandle, backend_tls_policy: BackendTlsPolicyLookup) -> Option<Self> {
        logical.compiled().permitted_service()?;
        Some(Self {
            logical,
            backend_tls_policy,
        })
    }

    #[inline]
    pub fn logical(&self) -> &GrpcBackendHandle {
        &self.logical
    }

    #[inline]
    pub fn compiled(&self) -> &CompiledGrpcBackend {
        self.logical.compiled()
    }

    #[inline]
    pub fn slot(&self) -> usize {
        self.logical.slot()
    }

    #[inline]
    pub fn backend_tls_policy(&self) -> &BackendTlsPolicyLookup {
        &self.backend_tls_policy
    }

    #[inline]
    pub fn plugin_runtime(&self) -> Arc<PluginRuntime> {
        Arc::clone(&self.logical.compiled().plugin_runtime)
    }
}

impl std::ops::Deref for SelectedGrpcBackend {
    type Target = CompiledGrpcBackend;

    fn deref(&self) -> &Self::Target {
        self.logical.compiled()
    }
}

impl std::ops::Deref for CompiledGrpcBackend {
    type Target = GRPCBackendRef;
    fn deref(&self) -> &GRPCBackendRef {
        &self.schema
    }
}

impl CompiledGrpcBackend {
    pub fn schema(&self) -> &GRPCBackendRef {
        &self.schema
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

/// Per-rule compiled runtime, built ONCE per rule and Arc-shared across all
/// match-item units of that rule. In-memory RateLimitLocal owns per-rule CMS state,
/// so rebuilding per-unit would split the counter N×.
pub struct GrpcRuleRuntime {
    /// Rule-level compiled plugin chain.
    pub plugin_runtime: Arc<PluginRuntime>,
    /// Compiled backends (each with its own plugin runtime).
    pub backends: Arc<[CompiledGrpcBackend]>,
}

/// Pre-computed match specificity for GRPCRoute Gateway API precedence ordering.
///
/// GRPCRoute precedence after hostname, service length, and method length:
///   header_count > creation_ts > route_key > rule_index > match_index
///
/// GrpcMatchEngine folds this key into its build-time rank, independent of indexes.
#[derive(Clone, Debug)]
pub struct GrpcMatchSpecificity {
    /// Number of header match conditions
    pub header_count: u16,
    /// Route creation timestamp (older = higher priority)
    pub creation_ts_ms: i64,
    /// Route "{namespace}/{name}" for alphabetical tie-breaking
    pub route_key: String,
    /// Rule index within the route (first wins)
    pub rule_index: u32,
    /// Match index within the rule (first wins)
    pub match_index: u32,
}

impl GrpcMatchSpecificity {
    /// Compare two GrpcMatchSpecificity values per Gateway API precedence.
    /// Returns Ordering where Less means `self` has higher priority.
    pub fn cmp_precedence(&self, other: &Self) -> Ordering {
        // More headers = higher priority
        other
            .header_count
            .cmp(&self.header_count)
            // Older route = higher priority
            .then(self.creation_ts_ms.cmp(&other.creation_ts_ms))
            // Alphabetical by namespace/name
            .then(self.route_key.cmp(&other.route_key))
            // First rule/match wins
            .then(self.rule_index.cmp(&other.rule_index))
            .then(self.match_index.cmp(&other.match_index))
    }
}

pub const CATCH_ALL_HOSTNAME: &str = "*";

/// gRPC route level information shared across all rule units
#[derive(Clone, Debug)]
pub struct GrpcRouteInfo {
    /// Parent references with their `GatewayInfoIndex` lookup keys resolved
    /// at snapshot build time. `GrpcRouteInfo` is already `Arc`-shared across
    /// this route's units, so no inner `Arc` is needed.
    pub parent_refs: Option<Vec<ResolvedParentRef>>,
    /// Effective hostnames for this route (from resolved_hostnames or hostnames).
    /// Listener-resolved eligibility constraint; authored Route hostnames
    /// determine engine precedence independently of this intersection.
    pub effective_hostnames: Vec<String>,
}

/// gRPC route match information
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct GrpcMatchInfo {
    /// Resource identifier (kind="GRPCRoute")
    #[schemars(skip)]
    pub rsc: crate::Rsc,
    /// Rule id in GRPCRoute
    pub rule_id: usize,
    /// Rule name (from GRPCRouteRule.name, if set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_name: Option<String>,
    /// Match id at rule id
    pub match_id: usize,
    /// sync_version from gRPC sync (0 = not set)
    #[serde(default, skip_serializing_if = "crate::ctx::is_zero")]
    pub sv: u64,
    /// Matched item (contains service/method in matched.method)
    pub matched: GRPCRouteMatch,
    /// Pre-compiled regex patterns for header matching (index corresponds to matched.headers).
    /// None if match_type is not RegularExpression or if compilation failed.
    /// `Regex` is internally Arc-like; the surrounding `GrpcRouteRuleUnit` is shared via
    /// outer `Arc<GrpcRouteRuleUnit>`, so no extra `Arc` wrapper is needed.
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_header_regexes: Vec<Option<Regex>>,
    /// Canonical shared method compilation outcome. None is invalid, never a
    /// signal to reinterpret a failed regex as an Exact or catch-all matcher.
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_method_match: Option<CompiledGrpcMethodMatch>,
    /// Raw route labels for `AccessLogExtern` `from: routeLabel`. In-process
    /// only — never part of the serialized entry; only declared keys reach `ext`.
    #[serde(skip)]
    #[schemars(skip)]
    pub labels_meta: crate::ctx::RouteMeta,
    /// Raw route annotations for `AccessLogExtern` `from: routeAnnotation`.
    #[serde(skip)]
    #[schemars(skip)]
    pub annotations_meta: crate::ctx::RouteMeta,
}

impl GrpcMatchInfo {
    /// GRPCRoute no longer carries operator log labels (see
    /// `crate::extract_log_labels`'s doc) — `rsc.labels` is always `None`
    /// here. `AccessLogExtern`'s `from: routeLabel` / `from: routeAnnotation`
    /// reads `labels_meta` / `annotations_meta` instead (attached via
    /// [`GrpcMatchInfo::with_route_meta`]).
    pub fn new(
        route_ns: String,
        route_name: String,
        rule_id: usize,
        match_id: usize,
        matched: GRPCRouteMatch,
        sv: u64,
        rule_name: Option<String>,
    ) -> Self {
        // Pre-compile regex patterns for header matching
        let compiled_header_regexes = if let Some(ref headers) = matched.headers {
            headers
                .iter()
                .map(|header_match| {
                    // Only compile if match_type is RegularExpression
                    if header_match.match_type.as_deref() == Some("RegularExpression") {
                        match Regex::new(&header_match.value) {
                            Ok(re) => Some(re),
                            Err(e) => {
                                tracing::warn!(
                                    route = %format!("{}/{}", route_ns, route_name),
                                    rule_id = rule_id,
                                    match_id = match_id,
                                    header = %header_match.name,
                                    pattern = %header_match.value,
                                    error = %e,
                                    "Failed to compile header regex pattern, header match will fail"
                                );
                                None
                            }
                        }
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let compiled_method_match = match matched.compile_method() {
            Ok(compiled) => Some(compiled),
            Err(reason) => {
                tracing::warn!(
                    component = "grpc_route_manager",
                    route_namespace = %route_ns,
                    route_name = %route_name,
                    rule_id,
                    match_id,
                    reason,
                    "Invalid gRPC method matcher; its rule will be skipped"
                );
                None
            }
        };

        Self {
            rsc: crate::Rsc {
                kind: "GRPCRoute".to_string(),
                ns: route_ns,
                name: route_name,
                labels: None,
            },
            rule_id,
            rule_name,
            match_id,
            sv,
            matched,
            compiled_header_regexes,
            compiled_method_match,
            labels_meta: None,
            annotations_meta: None,
        }
    }

    /// Attach the route's raw metadata maps, resolved once at route build.
    pub fn with_route_meta(mut self, labels: crate::ctx::RouteMeta, annotations: crate::ctx::RouteMeta) -> Self {
        self.labels_meta = labels;
        self.annotations_meta = annotations;
        self
    }
}

/// gRPC route rule unit
#[derive(Clone)]
pub struct GrpcRouteRuleUnit {
    pub resource_key: String,
    pub matched_info: GrpcMatchInfo,
    pub rule: Arc<GRPCRouteRule>,
    /// Shared handle to the full matched route, for metadata-only template
    /// access (`${matchedGrpcRoute:...}`). Same `Arc` the route cache holds.
    /// `None` only in unit-test fixtures.
    pub route: Option<Arc<GRPCRoute>>,
    /// Route level information (parent_refs, hostnames, etc.)
    pub route_info: Arc<GrpcRouteInfo>,
    /// Pre-computed match specificity for Gateway API precedence ordering.
    pub specificity: GrpcMatchSpecificity,
    /// Pre-resolved Counter/Histogram handles for the request-end metric
    /// path. The hot path indexes by
    /// `lookup(gw_ns, gw_name).counters[backend_ref_idx][ps_index(...)]`.
    /// Invariant: labels and backend_refs are fixed at build time (init-once
    /// globals); `backend_refs` is never mutated in place, so indices stay
    /// valid for the lifetime of this `Arc<GrpcRouteRuleUnit>`.
    pub metric_handles: Arc<crate::observe::metrics::route_handles::RouteMetricHandles>,
    /// Compiled rule-level plugin chain. Shared across ALL match-item units of
    /// this rule — in-memory RateLimitLocal owns per-rule CMS state.
    pub plugin_runtime: Arc<PluginRuntime>,
    /// Per-rule WRR selector over compiled backends (one per rule; gRPC has no
    /// hostname fan-out so there is no per-hostname duplication).
    pub backend_finder: Arc<BackendSelector<usize>>,
    /// Per-rule compiled backends in declaration order — the FULL list,
    /// including weight-0 refs the WRR selector filters out. Session
    /// persistence resolves an established session across this list before
    /// weighted selection (GEP-1619: weights govern only new sessions).
    pub backends: Arc<[CompiledGrpcBackend]>,
}

impl GrpcRouteRuleUnit {
    /// Compile a rule's runtime ONCE (rule plugin chain + compiled backends).
    /// Call once per rule, above the match-item fan-out, and share the Arc
    /// fields into every unit — RateLimitLocal CMS state must not be split.
    pub fn compile_grpc_rule(route: &GRPCRoute, rule_index: usize) -> Option<GrpcRuleRuntime> {
        Self::compile_with_factories(
            route,
            rule_index,
            |rule, namespace| {
                Arc::new(match &rule.filters {
                    Some(filters) => PluginRuntime::from_grpcroute_filters_for_rule(filters, namespace),
                    None => PluginRuntime::new(),
                })
            },
            |backend, namespace| {
                Arc::new(match &backend.filters {
                    Some(filters) => PluginRuntime::from_grpcroute_filters_for_backend(filters, namespace),
                    None => PluginRuntime::new(),
                })
            },
        )
    }

    fn compile_with_factories(
        route: &GRPCRoute,
        rule_index: usize,
        mut rule_factory: impl FnMut(&GRPCRouteRule, &str) -> Arc<PluginRuntime>,
        mut backend_factory: impl FnMut(&GRPCBackendRef, &str) -> Arc<PluginRuntime>,
    ) -> Option<GrpcRuleRuntime> {
        let references = GrpcRouteBackendRefs::from_route(route).ok()?;
        let rule = route.effective_rules()?.get(rule_index)?;
        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        let plugin_runtime = rule_factory(rule, namespace);
        let backends = rule
            .backend_refs
            .iter()
            .flatten()
            .enumerate()
            .map(|(slot, schema)| {
                let reference = references
                    .effective_at(rule_index, slot)
                    .expect("the backend came from this actual effective rule");
                let target =
                    CompiledBackendTarget::from_reference(reference, edgion_resources::HTTPBackendRefKind::Unsupported);
                let plugin_runtime =
                    if schema.ref_denied.is_none() && matches!(target, CompiledBackendTarget::Service(_)) {
                        backend_factory(schema, namespace)
                    } else {
                        Arc::new(PluginRuntime::new())
                    };
                CompiledGrpcBackend {
                    schema: schema.clone(),
                    target,
                    plugin_runtime,
                }
            })
            .collect::<Vec<_>>();
        Some(GrpcRuleRuntime {
            plugin_runtime,
            backends: backends.into(),
        })
    }

    /// Build a WRR selector over original backend slot indexes.
    /// All matches and hostname indexes share this one selector per rule.
    pub fn build_backend_selector(backends: &[CompiledGrpcBackend]) -> Arc<BackendSelector<usize>> {
        let selector = BackendSelector::new();
        if !backends.is_empty() {
            let weights: Vec<Option<i32>> = backends.iter().map(|b| b.schema.weight.or(Some(1))).collect();
            selector.init((0..backends.len()).collect(), weights);
        }
        Arc::new(selector)
    }

    /// Deep match: check hostname, Gateway/sectionName, and headers.
    ///
    /// The engine already selects the hostname group. Retain the route-level
    /// check here so direct callers cannot bypass the effective hostname scope.
    ///
    /// Returns `Some(GatewayInfo)` of the matched gateway on success, `None` on failure.
    pub fn deep_match(
        &self,
        session: &Session,
        index: &GatewayInfoIndex,
        hostname: &str,
    ) -> Result<Option<Arc<GatewayInfo>>, EdError> {
        let req_header = session.req_header();

        // Preserve the effective hostname scope for direct deep-match callers.
        if !self.matches_hostname(hostname) {
            return Ok(None);
        }

        // Check Gateway/Listener constraints (sectionName + hostname isolation)
        let matched_gi = if let Some(ref parent_refs) = self.route_info.parent_refs {
            match check_gateway_listener_match(parent_refs, index, hostname, &self.matched_info.rsc.ns, "GRPCRoute") {
                Some(gi) => gi,
                None => return Ok(None),
            }
        } else {
            return Ok(None);
        };

        // Check Headers (if specified) - ALL must match (AND logic)
        if let Some(header_matches) = &self.matched_info.matched.headers {
            for (idx, header_match) in header_matches.iter().enumerate() {
                let compiled_regex = self
                    .matched_info
                    .compiled_header_regexes
                    .get(idx)
                    .and_then(|r| r.as_ref());
                if !Self::match_header(req_header, header_match, compiled_regex)? {
                    return Ok(None);
                }
            }
        }

        Ok(Some(matched_gi))
    }

    /// Match gRPC header.
    ///
    /// Controller validates regex patterns at preparse (see ref_grant/validator.rs::check_regex_pattern),
    /// so a missing `compiled_regex` here means controller validation gap or compile-time
    /// invariant violation — fail-closed instead of recompiling on every request.
    fn match_header(
        req_header: &pingora_http::RequestHeader,
        header_match: &edgion_resources::GRPCHeaderMatch,
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
                Some(regex) => Ok(regex.is_match(header_value)),
                None => Ok(false),
            },
            _ => Ok(header_value == header_match.value),
        }
    }

    /// Check if request hostname matches this route's effective hostnames.
    #[inline]
    fn matches_hostname(&self, request_hostname: &str) -> bool {
        let hostnames = &self.route_info.effective_hostnames;
        if hostnames.is_empty() || (hostnames.len() == 1 && hostnames[0] == CATCH_ALL_HOSTNAME) {
            return true;
        }
        hostnames
            .iter()
            .any(|hostname| hostname_matches_listener(request_hostname, hostname))
    }

    /// Get route identifier with rule and match details
    pub fn identifier(&self) -> String {
        if let Some(ref rule_name) = self.matched_info.rule_name {
            format!(
                "{}/{} (rule:{}[{}], match:{})",
                self.matched_info.rsc.ns,
                self.matched_info.rsc.name,
                self.matched_info.rule_id,
                rule_name,
                self.matched_info.match_id
            )
        } else {
            format!(
                "{}/{} (rule:{}, match:{})",
                self.matched_info.rsc.ns,
                self.matched_info.rsc.name,
                self.matched_info.rule_id,
                self.matched_info.match_id
            )
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/grpc/match_unit_test/backend_handle_tests.rs"]
mod backend_handle_tests;

#[cfg(test)]
include!("../../../tests/support/routes/grpc/match_unit_test/compiled_fixtures.rs");
