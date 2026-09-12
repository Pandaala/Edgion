use crate::backends::policy::BackendTlsPolicyLookup;
use crate::lb::BackendSelector;
use crate::plugins::PluginRuntime;
use crate::runtime::matching::route::{check_gateway_listener_match, hostname_matches_listener, ResolvedParentRef};
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use crate::runtime::GatewayInfo;
use edgion_resources::err::EdError;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
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
    pub schema: GRPCBackendRef,
    /// Canonical policy identity proven once from the authorized Service ref.
    /// `None` for denied, delegated, or otherwise unsupported backend refs.
    policy_target_key: Option<PolicyTargetKey>,
    /// Compiled backend-level plugin chain (built from `schema.filters`).
    pub plugin_runtime: Arc<PluginRuntime>,
}

/// One immutable gRPC backend slot pinned to its compiled route generation.
#[derive(Clone)]
pub struct GrpcBackendHandle {
    generation: Arc<Vec<CompiledGrpcBackend>>,
    index: usize,
}

impl GrpcBackendHandle {
    pub(crate) fn new(generation: Arc<Vec<CompiledGrpcBackend>>, index: usize) -> Option<Self> {
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
        self.compiled().policy_target_key.as_ref()
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
    pub(crate) fn new(logical: GrpcBackendHandle, backend_tls_policy: BackendTlsPolicyLookup) -> Self {
        Self {
            logical,
            backend_tls_policy,
        }
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
    fn compile(schema: GRPCBackendRef, route_namespace: &str, plugin_runtime: Arc<PluginRuntime>) -> Self {
        let policy_target_key = Self::compile_policy_target_key(&schema, route_namespace);
        Self {
            schema,
            policy_target_key,
            plugin_runtime,
        }
    }

    fn compile_policy_target_key(schema: &GRPCBackendRef, route_namespace: &str) -> Option<PolicyTargetKey> {
        if schema.ref_denied.is_some() {
            return None;
        }
        if !matches!(schema.group.as_deref(), None | Some("") | Some("core"))
            || !matches!(schema.kind.as_deref(), None | Some("Service"))
        {
            return None;
        }
        Some(PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: schema.namespace.as_deref().unwrap_or(route_namespace).to_string(),
            name: schema.name.clone(),
        })
    }
}

/// Per-rule compiled runtime, built ONCE per rule and Arc-shared across all
/// match-item units of that rule. In-memory RateLimitLocal owns per-rule CMS state,
/// so rebuilding per-unit would split the counter N×.
pub struct GrpcRuleRuntime {
    /// Rule-level compiled plugin chain.
    pub plugin_runtime: Arc<PluginRuntime>,
    /// Compiled backends (each with its own plugin runtime).
    pub backends: Arc<Vec<CompiledGrpcBackend>>,
}

/// Pre-computed match specificity for GRPCRoute Gateway API precedence ordering.
///
/// GRPCRoute precedence (within the same service/method bucket):
///   header_count > creation_ts > route_key > rule_index > match_index
///
/// Service/method-level precedence is handled by the bucket structure in GrpcMatchEngine.
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
    /// Unlike HTTP routes (bucketed by domain), gRPC routes share a single
    /// match engine so we must verify hostname match explicitly.
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
    /// Pre-compiled regex for service name matching (when match_type is RegularExpression)
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_service_regex: Option<Regex>,
    /// Pre-compiled regex for method name matching (when match_type is RegularExpression)
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_method_regex: Option<Regex>,
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

        // Pre-compile regex patterns for service/method matching
        use edgion_resources::resources::grpc_route::GRPCMethodMatchType;
        let (compiled_service_regex, compiled_method_regex) = if let Some(ref method_match) = matched.method {
            let is_regex = method_match.match_type.as_ref() == Some(&GRPCMethodMatchType::RegularExpression);
            let compile = |field: &Option<String>, label: &str| -> Option<Regex> {
                if !is_regex {
                    return None;
                }
                field.as_ref().and_then(|pattern| match Regex::new(pattern) {
                    Ok(re) => Some(re),
                    Err(e) => {
                        tracing::warn!(
                            route = %format!("{}/{}", route_ns, route_name),
                            rule_id = rule_id,
                            match_id = match_id,
                            field = label,
                            pattern = %pattern,
                            error = %e,
                            "Failed to compile gRPC method regex pattern, match will fail"
                        );
                        None
                    }
                })
            };
            (
                compile(&method_match.service, "service"),
                compile(&method_match.method, "method"),
            )
        } else {
            (None, None)
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
            compiled_service_regex,
            compiled_method_regex,
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
    pub backends: Arc<Vec<CompiledGrpcBackend>>,
}

impl GrpcRouteRuleUnit {
    /// Compile a rule's runtime ONCE (rule plugin chain + compiled backends).
    /// Call once per rule, above the match-item fan-out, and share the Arc
    /// fields into every unit — RateLimitLocal CMS state must not be split.
    pub fn compile_grpc_rule(rule: &GRPCRouteRule, namespace: &str) -> GrpcRuleRuntime {
        let plugin_runtime = Arc::new(match &rule.filters {
            Some(filters) => PluginRuntime::from_grpcroute_filters_for_rule(filters, namespace),
            None => PluginRuntime::new(),
        });
        let backends: Vec<CompiledGrpcBackend> = rule.backend_refs.as_ref().map_or_else(Vec::new, |refs| {
            refs.iter()
                .map(|br| {
                    let plugin_runtime = Arc::new(match &br.filters {
                        Some(filters) => PluginRuntime::from_grpcroute_filters_for_backend(filters, namespace),
                        None => PluginRuntime::new(),
                    });
                    CompiledGrpcBackend::compile(br.clone(), namespace, plugin_runtime)
                })
                .collect()
        });
        GrpcRuleRuntime {
            plugin_runtime,
            backends: Arc::new(backends),
        }
    }

    /// Build a WRR selector over original backend slot indexes.
    /// gRPC has no hostname fan-out, so one selector per rule suffices.
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
    /// Unlike HTTP routes which are bucketed by domain, gRPC routes share a single
    /// match engine indexed by service/method. We must explicitly verify that the
    /// request hostname matches the route's effective hostnames.
    ///
    /// Returns `Some(GatewayInfo)` of the matched gateway on success, `None` on failure.
    pub fn deep_match(
        &self,
        session: &Session,
        index: &GatewayInfoIndex,
        hostname: &str,
    ) -> Result<Option<Arc<GatewayInfo>>, EdError> {
        let req_header = session.req_header();

        // Check route-level hostname constraint: gRPC routes are NOT bucketed
        // by domain so we verify here that the request host is allowed.
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
        for h in hostnames {
            if h.starts_with("*.") {
                if hostname_matches_listener(request_hostname, h) {
                    return true;
                }
            } else if request_hostname.eq_ignore_ascii_case(h) {
                return true;
            }
        }
        false
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
mod backend_handle_tests {
    use super::*;
    use edgion_resources::resources::backend_tls_policy::{
        BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation,
    };
    use edgion_resources::resources::{BackendTLSPolicy, RefDenied};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    #[test]
    fn malformed_backend_filter_remains_backend_scoped() {
        let rule: GRPCRouteRule = serde_json::from_value(serde_json::json!({
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

        let runtime = GrpcRouteRuleUnit::compile_grpc_rule(&rule, "route-ns");

        assert!(runtime.backends[0].plugin_runtime.rejects_invalid_config());
        assert!(!runtime.backends[1].plugin_runtime.rejects_invalid_config());
        assert!(!runtime.plugin_runtime.rejects_invalid_config());
    }

    fn compiled(name: &str, weight: i32, runtime: Arc<PluginRuntime>) -> CompiledGrpcBackend {
        CompiledGrpcBackend::compile(
            GRPCBackendRef {
                name: name.to_string(),
                namespace: Some("backend-ns".to_string()),
                port: Some(8080),
                weight: Some(weight),
                group: None,
                kind: Some("Service".to_string()),
                filters: None,
                backend_tls_policy: None,
                ref_denied: None,
            },
            "route-ns",
            runtime,
        )
    }

    fn compile_one_backend(schema: GRPCBackendRef, route_namespace: &str) -> Arc<Vec<CompiledGrpcBackend>> {
        GrpcRouteRuleUnit::compile_grpc_rule(
            &GRPCRouteRule {
                name: None,
                matches: None,
                filters: None,
                backend_refs: Some(vec![schema]),
                timeouts: None,
                retry: None,
                session_persistence: None,
                parsed_timeouts: None,
                parsed_retry: None,
                parsed_allow_non_idempotent_retry: None,
            },
            route_namespace,
        )
        .backends
    }

    fn backend_ref(group: Option<&str>, kind: Option<&str>, namespace: Option<&str>, denied: bool) -> GRPCBackendRef {
        GRPCBackendRef {
            name: "api".to_string(),
            namespace: namespace.map(str::to_string),
            port: Some(8080),
            weight: Some(1),
            group: group.map(str::to_string),
            kind: kind.map(str::to_string),
            filters: None,
            backend_tls_policy: None,
            ref_denied: denied.then(|| RefDenied {
                target_namespace: namespace.unwrap_or("route-ns").to_string(),
                target_name: "api".to_string(),
                reason: Some("NoMatchingReferenceGrant".to_string()),
            }),
        }
    }

    fn tls_policy(name: &str, hostname: &str) -> Arc<BackendTLSPolicy> {
        Arc::new(BackendTLSPolicy {
            metadata: ObjectMeta {
                namespace: Some("backend-ns".to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: BackendTLSPolicySpec {
                target_refs: vec![BackendTLSPolicyTargetRef {
                    group: String::new(),
                    kind: "Service".to_string(),
                    name: "api".to_string(),
                    section_name: None,
                }],
                resolved_target_class: None,
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: hostname.to_string(),
                    subject_alt_names: None,
                    well_known_ca_certificates: None,
                },
                options: None,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                use_system_ca: true,
            },
            status: None,
        })
    }

    #[test]
    fn grpc_backend_handles_stay_within_hot_path_budget() {
        assert_eq!(std::mem::size_of::<GrpcBackendHandle>(), 16);
        assert_eq!(std::mem::size_of::<SelectedGrpcBackend>(), 32);
        assert_eq!(std::mem::size_of::<Option<SelectedGrpcBackend>>(), 32);
    }

    #[test]
    fn handle_projects_exact_duplicate_slot_and_pins_generation_runtime() {
        let old_runtime = Arc::new(PluginRuntime::new());
        let generation = Arc::new(vec![
            compiled("api", 0, Arc::new(PluginRuntime::new())),
            compiled("api", 1, Arc::clone(&old_runtime)),
        ]);
        let logical = GrpcBackendHandle::new(Arc::clone(&generation), 1).unwrap();
        let selected = SelectedGrpcBackend::new(logical, BackendTlsPolicyLookup::Absent);
        let replacement = Arc::new(vec![compiled("new", 1, Arc::new(PluginRuntime::new()))]);
        drop(generation);

        assert_eq!(selected.slot(), 1);
        assert_eq!(selected.name, "api");
        assert_eq!(selected.logical().policy_target_key().unwrap().name, "api");
        assert!(Arc::ptr_eq(&selected.plugin_runtime(), &old_runtime));
        assert!(!Arc::ptr_eq(&selected.logical.generation, &replacement));
        let replacement_handle = GrpcBackendHandle::new(replacement, 0).unwrap();
        assert_eq!(replacement_handle.policy_target_key().unwrap().name, "new");
    }

    #[test]
    fn handle_rejects_out_of_range_index() {
        let generation = Arc::new(vec![compiled("api", 1, Arc::new(PluginRuntime::new()))]);
        assert!(GrpcBackendHandle::new(generation, 1).is_none());
    }

    #[test]
    fn selector_returns_original_slot_and_filters_weight_zero_only_for_new_requests() {
        let generation = vec![
            compiled("sticky", 0, Arc::new(PluginRuntime::new())),
            compiled("active", 1, Arc::new(PluginRuntime::new())),
        ];
        let selector = GrpcRouteRuleUnit::build_backend_selector(&generation);

        for _ in 0..8 {
            assert_eq!(selector.select().unwrap(), 1);
        }
    }

    #[test]
    fn compiled_policy_target_key_accepts_only_authorized_service_refs() {
        struct Case {
            label: &'static str,
            group: Option<&'static str>,
            kind: Option<&'static str>,
            namespace: Option<&'static str>,
            denied: bool,
            expected_namespace: Option<&'static str>,
        }

        let cases = [
            Case {
                label: "defaults",
                group: None,
                kind: None,
                namespace: None,
                denied: false,
                expected_namespace: Some("route-ns"),
            },
            Case {
                label: "empty-group-default-kind",
                group: Some(""),
                kind: None,
                namespace: None,
                denied: false,
                expected_namespace: Some("route-ns"),
            },
            Case {
                label: "core-group-default-kind",
                group: Some("core"),
                kind: None,
                namespace: None,
                denied: false,
                expected_namespace: Some("route-ns"),
            },
            Case {
                label: "default-group-service-kind",
                group: None,
                kind: Some("Service"),
                namespace: None,
                denied: false,
                expected_namespace: Some("route-ns"),
            },
            Case {
                label: "empty-group-service-kind",
                group: Some(""),
                kind: Some("Service"),
                namespace: None,
                denied: false,
                expected_namespace: Some("route-ns"),
            },
            Case {
                label: "core-service-explicit-namespace",
                group: Some("core"),
                kind: Some("Service"),
                namespace: Some("backend-ns"),
                denied: false,
                expected_namespace: Some("backend-ns"),
            },
            Case {
                label: "group-is-case-sensitive",
                group: Some("Core"),
                kind: Some("Service"),
                namespace: None,
                denied: false,
                expected_namespace: None,
            },
            Case {
                label: "kind-is-case-sensitive",
                group: Some("core"),
                kind: Some("service"),
                namespace: None,
                denied: false,
                expected_namespace: None,
            },
            Case {
                label: "unsupported-kind",
                group: None,
                kind: Some("Other"),
                namespace: None,
                denied: false,
                expected_namespace: None,
            },
            Case {
                label: "grpc-route-delegation",
                group: Some("gateway.networking.k8s.io"),
                kind: Some("GRPCRoute"),
                namespace: Some("child-ns"),
                denied: false,
                expected_namespace: None,
            },
            Case {
                label: "denied-service",
                group: None,
                kind: Some("Service"),
                namespace: Some("backend-ns"),
                denied: true,
                expected_namespace: None,
            },
        ];

        for case in cases {
            let generation = compile_one_backend(
                backend_ref(case.group, case.kind, case.namespace, case.denied),
                "route-ns",
            );
            let handle = GrpcBackendHandle::new(generation, 0).unwrap();
            match (handle.policy_target_key(), case.expected_namespace) {
                (Some(target), Some(expected_namespace)) => {
                    assert_eq!(target.target_kind, PolicyTargetKind::Service, "{}", case.label);
                    assert_eq!(target.namespace, expected_namespace, "{}", case.label);
                    assert_eq!(target.name, "api", "{}", case.label);
                }
                (None, None) => {}
                (actual, expected) => panic!("{}: target {actual:?}, expected namespace {expected:?}", case.label),
            }
        }
    }

    #[test]
    fn handle_policy_target_projection_is_borrowed_stable_and_allocation_free() {
        let generation = compile_one_backend(backend_ref(None, None, None, false), "route-ns");
        let stored = generation[0].policy_target_key.as_ref().unwrap() as *const PolicyTargetKey;

        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            let handle = GrpcBackendHandle::new(Arc::clone(&generation), 0).unwrap();
            let first = handle.policy_target_key().unwrap();
            let second = handle.policy_target_key().unwrap();
            assert!(std::ptr::eq(first, second));
            assert_eq!(first as *const PolicyTargetKey, stored);
            std::hint::black_box(first);
        });

        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }

    #[test]
    fn selected_backend_pins_tls_policy_and_selection_is_allocation_free() {
        let runtime = Arc::new(PluginRuntime::new());
        let generation = Arc::new(vec![compiled("api", 1, Arc::clone(&runtime))]);
        let old_policy = tls_policy("old", "old.example.com");
        let current_policy = tls_policy("new", "new.example.com");

        let captured = BackendTlsPolicyLookup::effective_from_policy(Arc::clone(&old_policy));
        let logical = GrpcBackendHandle::new(Arc::clone(&generation), 0).unwrap();
        let selected = SelectedGrpcBackend::new(logical, captured);
        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            let logical = GrpcBackendHandle::new(Arc::clone(&generation), 0).unwrap();
            let selected = SelectedGrpcBackend::new(logical, BackendTlsPolicyLookup::Absent);
            let projected_runtime = selected.plugin_runtime();
            drop(projected_runtime);
            selected
        });

        let retained = selected
            .backend_tls_policy()
            .effective()
            .expect("fixture policy must stay effective");
        assert!(Arc::ptr_eq(&retained.policy, &old_policy));
        assert!(!Arc::ptr_eq(&retained.policy, &current_policy));
        assert_eq!(retained.policy.spec.validation.hostname, "old.example.com");
        assert!(selected.logical().compiled().schema.backend_tls_policy.is_none());
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }
}
