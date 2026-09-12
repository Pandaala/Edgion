use crate::lb::BackendSelector;
use crate::plugins::PluginRuntime;
use crate::routes::common::{
    effective_hostnames, filter_accepted_parent_refs, gateways_from_parent_refs, resolve_parent_refs,
};
use crate::routes::http::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::routes::http::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::routes::http::match_unit::{CompiledBackend, HttpRuleRuntime, MatchSpecificity};
use crate::routes::http::routes_mgr::{
    resolved_listeners_for_route, DomainRouteRules, GlobalHttpRouteManagers, RouteRules,
};
use crate::routes::http::{get_global_http_route_managers, HttpRouteRuleUnit};
use crate::runtime::matching::route::ResolvedParentRef;
use crate::MatchInfo;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::matcher::host_match::radix_match::{RadixHost, RadixHostMatchEngine};
use edgion_resources::{HTTPPathMatch, HTTPRoute, HTTPRouteMatch, HTTPRouteRule, ListenerKey, ResourceMeta};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

type DomainStr = String;
/// (normal_routes, regex_routes) tuple for a domain
type RouteRulesPair = (Vec<Arc<HttpRouteRuleUnit>>, Vec<Arc<HttpRouteRuleUnit>>);
/// Domain -> (normal_routes, regex_routes) mapping
type DomainRouteRulesMap = HashMap<DomainStr, RouteRulesPair>;

/// Validated HTTPRoute data extracted from an HTTPRoute resource
struct ValidatedHttpRoute<'a> {
    rules: &'a Vec<edgion_resources::HTTPRouteRule>,
    namespace: String,
    name: String,
    /// Route's raw `metadata.labels`/`metadata.annotations`, resolved once per
    /// route for `AccessLogExtern`'s `from: routeLabel`/`routeAnnotation`.
    /// HTTPRoute carries no operator log labels into `MatchInfo.rsc.labels`
    /// anymore — the `edgion.io/log-label.<suffix>` carrier is narrowed to
    /// TLS resources (see `extract_log_labels`'s doc).
    route_labels_meta: crate::ctx::RouteMeta,
    route_annotations_meta: crate::ctx::RouteMeta,
}

/// Sentinel hostname used when an HTTPRoute has no spec.hostnames.
/// Per Gateway API spec, omitting hostnames means "match all hosts".
const CATCH_ALL_HOSTNAME: &str = "*";

fn get_effective_hostnames(route: &HTTPRoute) -> Vec<String> {
    effective_hostnames(
        route.spec.resolved_hostnames.as_deref(),
        route.spec.hostnames.as_deref(),
    )
}

impl ConfHandler<HTTPRoute> for GlobalHttpRouteManagers {
    // Update order across all ConfHandler entry points:
    //   1. mutate route_cache         — canonical store reflecting the new state.
    //   2. rebuild listener managers  — readers off match engines see the fresh cache.
    // LB policies are no longer sourced from routes: selection reads the per-service
    // EdgionBackendTrafficPolicy store (see `backends::policy::backend_traffic`).
    // Regression tests in this file pin the end-state for full_set / partial_update /
    // mixed scenarios.
    fn full_set(&self, data: &HashMap<String, Arc<HTTPRoute>>) {
        let start_time = Instant::now();
        tracing::info!(component = "http_route_manager", cnt = data.len(), "full set start");

        {
            let _guard = self.lock_rebuild();

            // Step 1: Replace route cache with shared Arcs (no deep clone).
            self.route_cache.clear();
            for (key, route) in data {
                self.route_cache.insert(key.clone(), route.clone()); // Arc clone
            }

            // Step 2: Rebuild all per-listener managers (lock already held above).
            self.rebuild_all_listener_managers_locked();
        }

        let elapsed = Instant::now().saturating_duration_since(start_time);
        tracing::info!(
            component = "http_route_manager",
            ms = elapsed.as_millis(),
            "full set done"
        );
        crate::plugins::publication::global_publisher().replace_routes(
            crate::plugins::publication::inputs::RouteKind::Http,
            data.iter()
                .map(|(key, route)| {
                    (
                        key.clone(),
                        crate::plugins::publication::inputs::RouteInput::from_http(Arc::clone(route)),
                    )
                })
                .collect(),
        );
        if std::ptr::eq(self, get_global_http_route_managers()) {
            crate::ai::replace_adaptive_route_contributions(data);
        }
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<HTTPRoute>>,
        update: HashMap<String, Arc<HTTPRoute>>,
        remove: HashSet<String>,
    ) {
        let policy_inputs = add
            .iter()
            .chain(update.iter())
            .map(|(key, route)| {
                (
                    key.clone(),
                    crate::plugins::publication::inputs::RouteInput::from_http(Arc::clone(route)),
                )
            })
            .collect();
        let policy_removed = remove.clone();
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        // Covers the affected-set computation, route_cache mutation, and rebuild as one
        // critical section so rebuild_all cannot interleave on a stale snapshot.
        let mut add_or_update: HashMap<String, Arc<HTTPRoute>> = add;
        add_or_update.extend(update);

        let start_time = Instant::now();
        tracing::info!(
            component = "http_route_manager",
            add = add_or_update.len(),
            rm = remove.len(),
            "partial update start"
        );

        {
            let _guard = self.lock_rebuild();
            // Compute affected listeners BEFORE updating cache.
            let mut affected_listeners: HashSet<ListenerKey> = HashSet::new();

            for (key, route) in &add_or_update {
                let listeners = resolved_listeners_for_route(route); // &Arc coerces to &HTTPRoute
                if listeners.is_empty() {
                    tracing::warn!(route = %key, "HTTPRoute has no resolved_listeners during partial_update");
                } else {
                    for lk in listeners {
                        affected_listeners.insert(lk.clone());
                    }
                }
                if let Some(old) = self.route_cache.get(key) {
                    for lk in resolved_listeners_for_route(old.value()) {
                        affected_listeners.insert(lk.clone());
                    }
                }
            }
            for key in &remove {
                if let Some(old) = self.route_cache.get(key) {
                    let listeners = resolved_listeners_for_route(old.value());
                    if !listeners.is_empty() {
                        for lk in listeners {
                            affected_listeners.insert(lk.clone());
                        }
                    }
                }
            }

            // Update route cache (store shared Arcs)
            for (key, route) in &add_or_update {
                self.route_cache.insert(key.clone(), Arc::clone(route));
            }
            for key in &remove {
                self.route_cache.remove(key);
            }

            self.rebuild_affected_listener_managers(&affected_listeners);
        }

        tracing::info!(
            component = "http_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "partial update done"
        );
        crate::plugins::publication::global_publisher().update_routes(
            crate::plugins::publication::inputs::RouteKind::Http,
            policy_inputs,
            &policy_removed,
        );
        if std::ptr::eq(self, get_global_http_route_managers()) {
            crate::ai::update_adaptive_route_contributions(&add_or_update, &remove);
        }
    }
}

/// Create a handler for registration with ConfigClient
pub fn create_route_manager_handler() -> Box<dyn ConfHandler<HTTPRoute> + Send + Sync> {
    Box::new(get_global_http_route_managers())
}

// ============================================================================
// Route parsing and DomainRouteRules building
// ============================================================================

/// Extract creation timestamp as milliseconds since epoch from route metadata.
/// Returns 0 if unavailable; 0 is treated as the oldest timestamp in tie-breaking.
fn extract_creation_ts_ms(route: &HTTPRoute) -> i64 {
    route
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| t.0.timestamp_millis())
        .unwrap_or(0)
}

/// Build MatchSpecificity from an HTTPRouteMatch and route-level metadata.
fn build_specificity(
    match_item: &HTTPRouteMatch,
    route_key: &str,
    creation_ts_ms: i64,
    rule_id: usize,
    match_id: usize,
) -> MatchSpecificity {
    MatchSpecificity {
        has_method: match_item.method.is_some(),
        header_count: match_item.headers.as_ref().map(|h| h.len() as u16).unwrap_or(0),
        query_count: match_item.query_params.as_ref().map(|q| q.len() as u16).unwrap_or(0),
        creation_ts_ms,
        route_key: route_key.to_string(),
        rule_index: rule_id as u32,
        match_index: match_id as u32,
    }
}

fn is_regex_path(match_item: &HTTPRouteMatch) -> bool {
    if let Some(ref path_match) = match_item.path {
        if let Some(ref match_type) = path_match.match_type {
            return match_type == "RegularExpression";
        }
    }
    false
}

/// Shared context for building one `HttpRouteRuleUnit` (normal or regex).
struct RouteUnitCtx<'a> {
    namespace: &'a str,
    name: &'a str,
    rule_id: usize,
    match_id: usize,
    resource_key: &'a str,
    match_item: &'a HTTPRouteMatch,
    rule: Arc<HTTPRouteRule>,
    parent_refs: Option<Arc<[ResolvedParentRef]>>,
    creation_ts_ms: i64,
    route_sv: u64,
    /// Per-rule metric matrix, built once above hostname/match fan-out.
    metric_handles: Arc<crate::observe::metrics::route_handles::RouteMetricHandles>,
    /// Rule-level compiled plugin runtime, built ONCE per rule and Arc-shared
    /// into every match-item unit (see `HttpRouteRuleUnit::compile_http_rule`).
    plugin_runtime: Arc<PluginRuntime>,
    /// Per-(rule,hostname) LB selector, Arc-shared across this rule's units.
    backend_finder: Arc<BackendSelector<usize>>,
    /// Per-rule compiled backends (Arc-shared), for by-name lookup.
    backends: Arc<Vec<CompiledBackend>>,
    /// Route's raw metadata maps, resolved once per route, Arc-shared into
    /// each match-item unit's `MatchInfo` for `AccessLogExtern`.
    route_labels_meta: crate::ctx::RouteMeta,
    route_annotations_meta: crate::ctx::RouteMeta,
    /// Shared handle to the full route (for metadata-only template access).
    route: Arc<HTTPRoute>,
}

fn create_regex_route_unit(ctx: RouteUnitCtx<'_>) -> Result<HttpRouteRuleUnit, String> {
    let path_value = ctx
        .match_item
        .path
        .as_ref()
        .and_then(|p| p.value.as_deref())
        .ok_or_else(|| "Regex path must have value".to_string())?;

    let regex = Regex::new(path_value).map_err(|e| format!("Invalid regex '{}': {}", path_value, e))?;
    let (compiled_header_regexes, compiled_query_regexes) = HttpRouteRuleUnit::compile_match_regexes(ctx.match_item);
    let has_query_matchers = ctx.match_item.query_params.as_ref().is_some_and(|qp| !qp.is_empty());
    let specificity = build_specificity(
        ctx.match_item,
        ctx.resource_key,
        ctx.creation_ts_ms,
        ctx.rule_id,
        ctx.match_id,
    );

    let plugin_runtime = ctx.plugin_runtime.clone();
    let backend_finder = ctx.backend_finder.clone();
    Ok(HttpRouteRuleUnit {
        plugin_runtime,
        backends: ctx.backends.clone(),
        backend_finder,
        resource_key: ctx.resource_key.to_string(),
        matched_info: MatchInfo::new(
            ctx.namespace.to_string(),
            ctx.name.to_string(),
            ctx.rule_id,
            ctx.match_id,
            ctx.match_item.clone(),
            ctx.route_sv,
        )
        .with_route_meta(ctx.route_labels_meta.clone(), ctx.route_annotations_meta.clone()),
        rule: ctx.rule.clone(),
        route: Some(ctx.route.clone()),
        path_regex: Some(regex),
        parent_refs: ctx.parent_refs,
        compiled_header_regexes,
        compiled_query_regexes,
        has_query_matchers,
        specificity,
        metric_handles: ctx.metric_handles,
    })
}

fn build_normal_route_unit(ctx: RouteUnitCtx<'_>) -> HttpRouteRuleUnit {
    let (compiled_header_regexes, compiled_query_regexes) = HttpRouteRuleUnit::compile_match_regexes(ctx.match_item);
    let has_query_matchers = ctx.match_item.query_params.as_ref().is_some_and(|qp| !qp.is_empty());
    let specificity = build_specificity(
        ctx.match_item,
        ctx.resource_key,
        ctx.creation_ts_ms,
        ctx.rule_id,
        ctx.match_id,
    );
    let plugin_runtime = ctx.plugin_runtime.clone();
    let backend_finder = ctx.backend_finder.clone();
    HttpRouteRuleUnit {
        plugin_runtime,
        backends: ctx.backends.clone(),
        backend_finder,
        resource_key: ctx.resource_key.to_string(),
        matched_info: MatchInfo::new(
            ctx.namespace.to_string(),
            ctx.name.to_string(),
            ctx.rule_id,
            ctx.match_id,
            ctx.match_item.clone(),
            ctx.route_sv,
        )
        .with_route_meta(ctx.route_labels_meta.clone(), ctx.route_annotations_meta.clone()),
        rule: ctx.rule.clone(),
        route: Some(ctx.route.clone()),
        path_regex: None,
        parent_refs: ctx.parent_refs,
        compiled_header_regexes,
        compiled_query_regexes,
        has_query_matchers,
        specificity,
        metric_handles: ctx.metric_handles,
    }
}

/// Push one match item into the normal or regex bucket for `hostname`.
fn append_match_to_domain(domain_rules: &mut DomainRouteRulesMap, hostname: &str, ctx: RouteUnitCtx<'_>) {
    let resource_key = ctx.resource_key;
    let split = domain_rules
        .entry(hostname.to_string())
        .or_insert_with(|| (Vec::new(), Vec::new()));
    if is_regex_path(ctx.match_item) {
        match create_regex_route_unit(ctx) {
            Ok(unit) => split.1.push(Arc::new(unit)),
            Err(e) => tracing::warn!(route = %resource_key, err = %e, "failed to create regex route"),
        }
    } else {
        split.0.push(Arc::new(build_normal_route_unit(ctx)));
    }
}

/// Rule-owned immutable values shared across every hostname/match unit.
struct SharedHttpRule {
    rule_id: usize,
    rule: Arc<HTTPRouteRule>,
    runtime: HttpRuleRuntime,
    metric_handles: Arc<crate::observe::metrics::route_handles::RouteMetricHandles>,
}

/// Expand all rules of a single route into `hostname`'s domain bucket.
#[allow(clippy::too_many_arguments)] // route-build inputs have distinct lifetimes/ownership; grouping would obscure them
fn append_route_to_hostname(
    domain_rules: &mut DomainRouteRulesMap,
    hostname: &str,
    validated: &ValidatedHttpRoute<'_>,
    resource_key: &str,
    // Resolved once per route by the caller and shared across every hostname
    // bucket, like the runtimes in `shared_rules`.
    resolved_parent_refs: &Arc<[ResolvedParentRef]>,
    creation_ts_ms: i64,
    route_sv: u64,
    shared_rules: &[SharedHttpRule],
    route_arc: &Arc<HTTPRoute>,
) {
    let default_matches = vec![HTTPRouteMatch {
        path: Some(HTTPPathMatch {
            match_type: Some("PathPrefix".to_string()),
            value: Some("/".to_string()),
        }),
        headers: None,
        query_params: None,
        method: None,
    }];

    for shared_rule in shared_rules {
        let rule_id = shared_rule.rule_id;
        let rule = &shared_rule.rule;
        // Compiled runtime is built ONCE per accepted rule;
        // the LB selector is built once per (rule, hostname) and shared across
        // this rule's match-item units.
        let rule_rt = &shared_rule.runtime;
        let backend_finder = HttpRouteRuleUnit::build_backend_selector(&rule_rt.backends);
        let matches = match &rule.matches {
            Some(m) if !m.is_empty() => m,
            _ => &default_matches,
        };
        for (match_id, match_item) in matches.iter().enumerate() {
            append_match_to_domain(
                domain_rules,
                hostname,
                RouteUnitCtx {
                    namespace: &validated.namespace,
                    name: &validated.name,
                    rule_id,
                    match_id,
                    resource_key,
                    match_item,
                    rule: Arc::clone(rule),
                    parent_refs: Some(Arc::clone(resolved_parent_refs)),
                    creation_ts_ms,
                    route_sv,
                    metric_handles: Arc::clone(&shared_rule.metric_handles),
                    plugin_runtime: rule_rt.plugin_runtime.clone(),
                    backend_finder: backend_finder.clone(),
                    backends: rule_rt.backends.clone(),
                    route_labels_meta: validated.route_labels_meta.clone(),
                    route_annotations_meta: validated.route_annotations_meta.clone(),
                    route: route_arc.clone(),
                },
            );
        }
    }
}

/// Parse all HTTPRoutes and collect rules into a domain->rules structure.
///
/// Routes are merged into a single table keyed by hostname.
/// Each route unit carries its parentRefs so gateway validation happens at match time.
fn parse_http_routes_to_domain_rules(data: &HashMap<String, Arc<HTTPRoute>>) -> DomainRouteRulesMap {
    let mut domain_rules: DomainRouteRulesMap = HashMap::new();
    let mut processed_routes = 0;
    let mut skipped_routes = 0;

    for (_key, route) in data.iter() {
        let validated = match validate_http_route(route) {
            Some(v) => v,
            None => {
                skipped_routes += 1;
                continue;
            }
        };
        let accepted_refs = match filter_accepted_parent_refs(
            route.spec.parent_refs.as_deref(),
            route.status.as_ref().map(|s| s.parents.as_slice()),
            Some(&validated.namespace),
        ) {
            Some(refs) => refs,
            None => {
                skipped_routes += 1;
                continue;
            }
        };

        let route_sv = route.get_sync_version();
        let creation_ts_ms = extract_creation_ts_ms(route);
        let resource_key = route.key_name();
        // Resolved ONCE per route: the inputs are
        // loop-invariant across hostnames, and the resulting slice is
        // Arc-shared into every match-item unit of every hostname bucket.
        let resolved_parent_refs: Arc<[ResolvedParentRef]> =
            resolve_parent_refs(&accepted_refs, &validated.namespace).into();

        // invalid_rule_indices refers to spec.rules; when resolved_rules is
        // compiled (delegation), flattening already dropped invalid rules and
        // the indices do not apply.
        let invalid_rule_indices = if route.spec.resolved_rules.is_some() {
            None
        } else {
            route.spec.invalid_rule_indices.as_ref()
        };
        let attached_gateways = gateways_from_parent_refs(&accepted_refs, &validated.namespace);
        let shared_rules: Vec<SharedHttpRule> = validated
            .rules
            .iter()
            .enumerate()
            .filter(|(rule_id, _)| !invalid_rule_indices.is_some_and(|inv| inv.contains(rule_id)))
            .filter_map(|(rule_id, rule)| {
                // Check effective rules independently of Controller status, including
                // resolved delegation rules, before compiling or publishing a runtime.
                if let Some(reason) = rule.unsupported_cors_reason() {
                    tracing::warn!(route = %resource_key, rule_id, %reason, "skipping unsupported HTTPRoute rule");
                    return None;
                }
                // Share one runtime across every hostname and match of this rule.
                let runtime = HttpRouteRuleUnit::compile_http_rule(rule, &validated.namespace);
                let rule = Arc::new(rule.clone());
                let metric_handles = Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::build(
                    &attached_gateways,
                    &validated.namespace,
                    &validated.name,
                    &http_backends_from_rule(&rule, &validated.namespace),
                ));
                Some(SharedHttpRule {
                    rule_id,
                    rule,
                    runtime,
                    metric_handles,
                })
            })
            .collect();
        for hostname in &get_effective_hostnames(route) {
            append_route_to_hostname(
                &mut domain_rules,
                hostname,
                &validated,
                &resource_key,
                &resolved_parent_refs,
                creation_ts_ms,
                route_sv,
                &shared_rules,
                route,
            );
        }
        processed_routes += 1;
    }

    tracing::debug!(
        component = "route_manager",
        proc = processed_routes,
        skip = skipped_routes,
        domains = domain_rules.len(),
        "parsed"
    );

    domain_rules
}

/// Validate HTTPRoute and extract required fields, returning None if any required field is missing.
fn validate_http_route(route: &HTTPRoute) -> Option<ValidatedHttpRoute<'_>> {
    match &route.spec.parent_refs {
        Some(refs) if !refs.is_empty() => refs,
        _ => {
            tracing::warn!(route=%route.key_name(),"no parent_refs");
            return None;
        }
    };

    // Delegation: when the controller flattened a delegation tree into
    // resolved_rules, compile those INSTEAD of the operator-authored rules.
    let rules = match route.effective_rules() {
        Some(rules) if !rules.is_empty() => rules,
        _ => {
            tracing::warn!(route=%route.key_name(),"no rules");
            return None;
        }
    };

    let route_namespace = match &route.metadata.namespace {
        Some(ns) if !ns.is_empty() => ns.clone(),
        _ => {
            tracing::warn!(route=%route.key_name(),"no namespace");
            return None;
        }
    };

    let route_name = match &route.metadata.name {
        Some(name) if !name.is_empty() => name.clone(),
        _ => {
            tracing::warn!(route=%route.key_name(),"no name");
            return None;
        }
    };

    let (route_labels_meta, route_annotations_meta) = crate::ctx::extract_route_meta(&route.metadata);

    Some(ValidatedHttpRoute {
        rules,
        namespace: route_namespace,
        name: route_name,
        route_labels_meta,
        route_annotations_meta,
    })
}

/// Build DomainRouteRules from a set of routes (used by per-port rebuild).
pub(crate) fn build_domain_route_rules_from_routes(data: &HashMap<String, Arc<HTTPRoute>>) -> DomainRouteRules {
    let domain_rules_map = parse_http_routes_to_domain_rules(data);

    let mut exact_domain_map: HashMap<DomainStr, Arc<RouteRules>> = HashMap::new();
    let mut wildcard_hosts: Vec<RadixHost<RouteRules>> = Vec::new();
    let mut catch_all_routes: Option<Arc<RouteRules>> = None;

    for (domain, split) in domain_rules_map.into_iter() {
        if split.0.is_empty() && split.1.is_empty() {
            continue;
        }

        let match_engine = if split.0.is_empty() {
            None
        } else {
            match RadixRouteMatchEngine::build(split.0.clone()) {
                Ok(engine) => Some(Arc::new(engine)),
                Err(e) => {
                    tracing::error!(component="route_manager",domain=%domain,err=?e,"build failed");
                    continue;
                }
            }
        };

        let regex_routes_engine = (!split.1.is_empty()).then(|| Arc::new(RegexRoutesEngine::build(split.1.clone())));

        let mut resource_keys: HashSet<String> = split.0.iter().map(|u| u.resource_key.clone()).collect();
        resource_keys.extend(split.1.iter().map(|u| u.resource_key.clone()));

        let route_rules = Arc::new(RouteRules {
            resource_keys,
            route_rules_list: split.0,
            match_engine,
            regex_routes: split.1,
            regex_routes_engine,
        });

        if domain == CATCH_ALL_HOSTNAME {
            catch_all_routes = Some(route_rules);
        } else if domain.starts_with("*.") {
            wildcard_hosts.push(RadixHost::new(&domain, route_rules));
        } else {
            exact_domain_map.insert(domain.to_lowercase(), route_rules);
        }
    }

    let wildcard_engine = if !wildcard_hosts.is_empty() {
        let mut engine = RadixHostMatchEngine::new();
        if let Err(e) = engine.initialize(wildcard_hosts) {
            tracing::error!(component="route_manager",err=%e,"failed to build RadixHostMatchEngine");
            None
        } else {
            Some(engine)
        }
    } else {
        None
    };

    DomainRouteRules {
        exact_domain_map: Arc::new(exact_domain_map),
        wildcard_engine: wildcard_engine.map(Arc::new),
        catch_all_routes,
    }
}

/// Build a raw-slot-aligned metric identity for every backendRef kind.
/// Backend metrics label namespace/name but not kind, matching the dynamic
/// fallback for Service, EdgionBackend, and invalid selected slots.
fn http_backends_from_rule(rule: &HTTPRouteRule, route_ns: &str) -> Vec<(String, String)> {
    rule.backend_refs
        .as_ref()
        .map(|refs| {
            refs.iter()
                .map(|backend| {
                    (
                        backend.namespace.clone().unwrap_or_else(|| route_ns.to_string()),
                        backend.name.clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::HTTPRoute;

    fn create_test_httproute(
        namespace: &str,
        name: &str,
        hostnames: Vec<&str>,
        gateway_refs: Vec<(&str, &str)>,
    ) -> HTTPRoute {
        let parent_refs_json: Vec<serde_json::Value> = gateway_refs
            .iter()
            .map(|(ns, n)| {
                serde_json::json!({
                    "group": "gateway.networking.k8s.io",
                    "kind": "Gateway",
                    "namespace": ns,
                    "name": n
                })
            })
            .collect();

        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "namespace": namespace,
                "name": name
            },
            "spec": {
                "parentRefs": parent_refs_json,
                "hostnames": hostnames,
                "rules": [{
                    "matches": []
                }]
            }
        });

        serde_json::from_value(json).expect("Failed to create HTTPRoute")
    }

    #[test]
    fn test_get_effective_hostnames_prefers_resolved() {
        let mut route = create_test_httproute(
            "default",
            "route1",
            vec!["original.example.com"],
            vec![("default", "gw1")],
        );
        route.spec.resolved_hostnames = Some(vec!["resolved.example.com".to_string()]);

        let result = get_effective_hostnames(&route);
        assert_eq!(result, vec!["resolved.example.com"]);
    }

    #[test]
    fn test_get_effective_hostnames_falls_back_to_spec_hostnames() {
        let route = create_test_httproute("default", "route1", vec!["spec.example.com"], vec![("default", "gw1")]);
        let result = get_effective_hostnames(&route);
        assert_eq!(result, vec!["spec.example.com"]);
    }

    #[test]
    fn test_get_effective_hostnames_catch_all_when_empty() {
        let route = create_test_httproute("default", "route1", vec![], vec![("default", "gw1")]);
        let result = get_effective_hostnames(&route);
        assert_eq!(result, vec!["*"]);
    }

    #[test]
    fn test_get_effective_hostnames_skips_empty_resolved() {
        let mut route = create_test_httproute(
            "default",
            "route1",
            vec!["fallback.example.com"],
            vec![("default", "gw1")],
        );
        route.spec.resolved_hostnames = Some(vec![]);
        let result = get_effective_hostnames(&route);
        assert_eq!(result, vec!["fallback.example.com"]);
    }

    /// Build a route whose rules each carry one path match, so every compiled
    /// unit contributes exactly one `RadixPath`.
    fn route_with_paths(
        namespace: &str,
        name: &str,
        hostnames: Vec<&str>,
        creation_timestamp: &str,
        rules: serde_json::Value,
    ) -> HTTPRoute {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "namespace": namespace,
                "name": name,
                "creationTimestamp": creation_timestamp
            },
            "spec": {
                "parentRefs": [{
                    "group": "gateway.networking.k8s.io",
                    "kind": "Gateway",
                    "namespace": "default",
                    "name": "gw1"
                }],
                "hostnames": hostnames,
                "rules": rules
            }
        });
        serde_json::from_value(json).expect("route fixture must deserialize")
    }

    /// Routes sharing one hostname whose paths overlap and which differ across
    /// several precedence levels (path length, exact vs prefix, method, header
    /// count, creation timestamp, route name, rule index).
    fn precedence_fixture_units() -> Vec<Arc<HttpRouteRuleUnit>> {
        let older = route_with_paths(
            "default",
            "route-a",
            vec!["rank.example.com"],
            "2024-01-01T00:00:00Z",
            serde_json::json!([
                {"matches": [{"path": {"type": "PathPrefix", "value": "/api"}}]},
                {"matches": [{"path": {"type": "PathPrefix", "value": "/api/v1"}}]},
                {"matches": [{"path": {"type": "Exact", "value": "/api/v1/users"}}]},
                {"matches": [{
                    "path": {"type": "Exact", "value": "/api/v1/orders"},
                    "method": "GET"
                }]},
                {"matches": [{
                    "path": {"type": "Exact", "value": "/api/v1/orders"},
                    "method": "GET",
                    "headers": [{"type": "Exact", "name": "X-A", "value": "1"}]
                }]}
            ]),
        );
        // Same paths as `route-a`, newer, and alphabetically later: exercises the
        // creation-timestamp and route-name levels against identical paths.
        let newer = route_with_paths(
            "default",
            "route-b",
            vec!["rank.example.com"],
            "2025-01-01T00:00:00Z",
            serde_json::json!([
                {"matches": [{"path": {"type": "PathPrefix", "value": "/api"}}]},
                {"matches": [{"path": {"type": "Exact", "value": "/api/v1/users"}}]}
            ]),
        );

        let mut data: HashMap<String, Arc<HTTPRoute>> = HashMap::new();
        data.insert(older.key_name(), Arc::new(older));
        data.insert(newer.key_name(), Arc::new(newer));

        let domain_rules = parse_http_routes_to_domain_rules(&data);
        let (units, _regex_units) = domain_rules
            .get("rank.example.com")
            .expect("fixture hostname must produce route rules");
        assert!(units.len() >= 7, "fixture must produce enough units: {}", units.len());
        units.clone()
    }

    /// The optimization's load-bearing claim: the ranks that
    /// `assign_precedence_ranks` writes must order candidates exactly the way
    /// a direct comparison does (stable sort over build order
    /// by path specificity descending, then `MatchSpecificity::cmp_precedence`).
    ///
    /// This drives the real engine, so a wrong write-back index or a wrong
    /// `routes[route_idx]` lookup fails here — the algebraic unit tests in
    /// `radix_route_match.rs` cannot catch either.
    #[test]
    fn engine_ranks_reproduce_direct_candidate_ordering() {
        let units = precedence_fixture_units();
        let engine = RadixRouteMatchEngine::build(units.clone()).expect("engine must build");
        let paths = engine.paths_for_test();
        assert_eq!(paths.len(), units.len(), "each fixture unit contributes one path");

        // Direct comparison of the current ordering contract.
        let mut expected: Vec<usize> = (0..paths.len()).collect();
        expected.sort_by(|a, b| {
            let (pa, pb) = (&paths[*a], &paths[*b]);
            pb.specificity.cmp(&pa.specificity).then_with(|| {
                units[pa.route_idx]
                    .specificity
                    .cmp_precedence(&units[pb.route_idx].specificity)
            })
        });

        let mut actual: Vec<usize> = (0..paths.len()).collect();
        actual.sort_unstable_by_key(|i| paths[*i].precedence_rank);

        assert_eq!(actual, expected, "build-time rank diverges from direct order");

        // Ranks must be a dense permutation for the request-path integer sort.
        let mut ranks: Vec<u32> = paths.iter().map(|p| p.precedence_rank).collect();
        ranks.sort_unstable();
        assert_eq!(ranks, (0..paths.len() as u32).collect::<Vec<_>>());
    }

    /// Covers the request path's filter-and-order step, which `match_route`
    /// otherwise reaches only with a live `Session`. Without this, dropping the
    /// candidate sort, inverting the `MatchKind` filter, or losing the
    /// `tree_value - 1` bucket offset would all go undetected by unit tests.
    #[test]
    fn engine_filters_and_orders_candidates_for_a_request_path() {
        let units = precedence_fixture_units();
        let engine = RadixRouteMatchEngine::build(units).expect("engine must build");
        let paths = engine.paths_for_test();
        let matched = |path: &str| -> Vec<String> {
            engine
                .ordered_candidates_for_test(path)
                .iter()
                .map(|(_, path_idx)| paths[*path_idx as usize].normalized.clone())
                .collect()
        };

        // Exact beats prefix, longer prefix beats shorter, and both routes'
        // copies of a shared path stay adjacent. Pins the ordering: the
        // traversal yields these in tree order, not precedence order.
        assert_eq!(
            matched("/api/v1/users"),
            vec!["/api/v1/users", "/api/v1/users", "/api/v1", "/api", "/api"]
        );

        // An Exact route must not match a longer path, while the prefixes above
        // it still do.
        assert_eq!(matched("/api/v1/users/extra"), vec!["/api/v1", "/api", "/api"]);

        // A prefix must not match mid-segment: `/api` does not cover `/apifoo`.
        assert!(matched("/apifoo").is_empty());

        // Unrelated paths produce no candidates at all.
        assert!(matched("/other").is_empty());
    }

    /// A route listing the same hostname twice registers its units twice in one
    /// engine, producing candidates that tie through every level of the
    /// precedence chain. `spec.hostnames` is `x-kubernetes-list-type: atomic`
    /// with no `uniqueItems`, and `effective_hostnames` does not dedup, so this
    /// is reachable rather than hypothetical.
    ///
    /// This is the only case where the old stable sort and the new `path_idx`
    /// tie-break could have diverged. They do not: the old code pushed such
    /// candidates consecutively in ascending `path_idx` within one bucket, which
    /// is exactly what the rank now encodes.
    #[test]
    fn tied_candidates_from_a_duplicate_hostname_keep_build_order() {
        let route = route_with_paths(
            "default",
            "dup",
            vec!["dup.example.com", "dup.example.com"],
            "2024-01-01T00:00:00Z",
            serde_json::json!([{"matches": [{"path": {"type": "Exact", "value": "/dup"}}]}]),
        );
        let mut data: HashMap<String, Arc<HTTPRoute>> = HashMap::new();
        data.insert(route.key_name(), Arc::new(route));

        let domain_rules = parse_http_routes_to_domain_rules(&data);
        let (units, _regex_units) = domain_rules
            .get("dup.example.com")
            .expect("duplicated hostname must still produce route rules");
        assert_eq!(units.len(), 2, "the duplicate hostname registers the unit twice");
        assert_eq!(
            units[0].specificity.cmp_precedence(&units[1].specificity),
            std::cmp::Ordering::Equal,
            "the two registrations must be indistinguishable to the precedence chain"
        );

        let engine = RadixRouteMatchEngine::build(units.clone()).expect("engine must build");
        let paths = engine.paths_for_test();
        assert_eq!(paths.len(), 2);
        assert_eq!(
            paths[0].specificity, paths[1].specificity,
            "identical paths must carry identical specificity, so the pair is fully tied"
        );

        // Distinct `route_idx`: `extract_paths` yields at most one path per unit,
        // so tied candidates are duplicate registrations, never one shared route.
        assert_ne!(paths[0].route_idx, paths[1].route_idx);

        // The tie resolves to build order, deterministically.
        assert_eq!(paths[0].precedence_rank, 0);
        assert_eq!(paths[1].precedence_rank, 1);
        let ordered: Vec<u32> = engine
            .ordered_candidates_for_test("/dup")
            .iter()
            .map(|(_, path_idx)| *path_idx)
            .collect();
        assert_eq!(ordered, vec![0, 1]);
    }

    /// Two routes declaring the same normalized path share one radix tree value,
    /// so that bucket must hold both path indices. This is the case the dense
    /// `tree_value - 1` bucket table exists to serve.
    #[test]
    fn engine_groups_identical_paths_into_one_bucket() {
        let units = precedence_fixture_units();
        let engine = RadixRouteMatchEngine::build(units).expect("engine must build");
        let buckets = engine.buckets_for_test();

        assert!(!buckets.is_empty(), "bucket table must be populated");
        assert!(
            buckets.iter().all(|b| !b.is_empty()),
            "dense bucket table must not contain gaps"
        );

        let total: usize = buckets.iter().map(|b| b.len()).sum();
        assert_eq!(
            total,
            engine.paths_for_test().len(),
            "every path lands in exactly one bucket"
        );

        // Three normalized paths are declared more than once: `/api` and
        // `/api/v1/users` by both routes, and `/api/v1/orders` by two rules of
        // `route-a` that differ only in method/header matching.
        // Sorted because the fixture's route order follows `HashMap` iteration,
        // so bucket order is not stable across runs.
        let mut shared: Vec<usize> = buckets.iter().map(|b| b.len()).filter(|n| *n > 1).collect();
        shared.sort_unstable();
        assert_eq!(shared, vec![2, 2, 2], "shared paths must each group both declarations");
    }

    fn named_route(name: &str, kind: &str, path: &str) -> HTTPRoute {
        route_with_paths(
            "default",
            name,
            vec!["named.example.com"],
            "2024-01-01T00:00:00Z",
            serde_json::json!([{"matches": [{"path": {"type": kind, "value": path}}]}]),
        )
    }

    fn named_units(routes: Vec<HTTPRoute>) -> Vec<Arc<HttpRouteRuleUnit>> {
        let data = routes
            .into_iter()
            .map(|route| (route.key_name(), Arc::new(route)))
            .collect();
        parse_http_routes_to_domain_rules(&data)
            .remove("named.example.com")
            .expect("named fixtures must produce a domain")
            .0
    }

    fn named_order(engine: &RadixRouteMatchEngine, units: &[Arc<HttpRouteRuleUnit>], request: &str) -> Vec<String> {
        engine
            .ordered_candidates_for_test(request)
            .iter()
            .map(|(_, path_idx)| {
                let path = &engine.paths_for_test()[*path_idx as usize];
                units[path.route_idx].specificity.route_key.clone()
            })
            .collect()
    }

    fn assert_named_order(routes: Vec<HTTPRoute>, request: &str, expected: &[&str]) {
        let mut units = named_units(routes);
        let expected: Vec<String> = expected.iter().map(|name| format!("default/{name}")).collect();
        // Real parsing and both registration orders; expected winners do not
        // come from the production key or comparator.
        for _ in 0..2 {
            let engine = RadixRouteMatchEngine::build(units.clone()).expect("named engine must build");
            assert_eq!(named_order(&engine, &units, request), expected, "{request}");
            let mut ranks: Vec<u32> = engine.paths_for_test().iter().map(|p| p.precedence_rank).collect();
            ranks.sort_unstable();
            assert_eq!(ranks, (0..units.len() as u32).collect::<Vec<_>>());
            units.reverse();
        }
    }

    #[test]
    fn named_paths_order_all_four_categories_and_request_subsets() {
        let routes = vec![
            named_route("static-exact", "Exact", "/a/b"),
            named_route("param-exact", "Exact", "/a/:identifier"),
            named_route("static-prefix", "PathPrefix", "/a/b"),
            named_route("param-prefix", "PathPrefix", "/a/:customerIdentifier"),
            named_route("root", "PathPrefix", "/"),
        ];
        for (request, expected) in [
            (
                "/a/b",
                vec!["static-exact", "param-exact", "static-prefix", "param-prefix", "root"],
            ),
            ("/a/b/c", vec!["static-prefix", "param-prefix", "root"]),
            ("/a/beyond", vec!["param-exact", "param-prefix", "root"]),
            ("/a", vec!["root"]),
            ("/other", vec!["root"]),
        ] {
            assert_named_order(routes.clone(), request, &expected);
        }
    }

    #[test]
    fn named_exact_beats_shorter_static_prefix() {
        assert_named_order(
            vec![
                named_route("param-exact", "Exact", "/a/:x/:y"),
                named_route("static-prefix", "PathPrefix", "/a/b"),
            ],
            "/a/b/c",
            &["param-exact", "static-prefix"],
        );
    }

    #[test]
    fn named_paths_exercise_each_aggregate_precedence_level() {
        for (kind, request, winner, loser) in [
            ("Exact", "/a/b/c", "/:x/b/c", "/a/:x/:y"),
            ("Exact", "/longsegment/b/c", "/:x/b/c", "/longsegment/:x/:y"),
            ("PathPrefix", "/longsegment/b/c", "/longsegment/:x", "/:x/b/:y"),
            ("PathPrefix", "/a/ab/c", "/a/:x/:y", "/:x/ab"),
            ("PathPrefix", "/a/b", "/a", "/:x/:y"),
            ("PathPrefix", "/a/b", "/:x/:y", "/:x"),
            ("PathPrefix", "/a/b", "/:x", "/"),
            ("PathPrefix", "/a/b", "/a/", "/a"),
        ] {
            assert_named_order(
                vec![
                    named_route("z-winner", kind, winner),
                    named_route("a-loser", kind, loser),
                ],
                request,
                &["z-winner", "a-loser"],
            );
        }
    }

    #[test]
    fn named_paths_with_equal_aggregate_constraints_use_route_name() {
        assert_named_order(
            vec![
                named_route("z-earlier-static", "Exact", "/a/:x/c"),
                named_route("a-later-static", "Exact", "/:x/b/c"),
            ],
            "/a/b/c",
            &["a-later-static", "z-earlier-static"],
        );
    }

    #[test]
    fn named_paths_match_escaped_literals_and_colon_boundaries() {
        for (pattern, request) in [
            ("/a/::literal", "/a/:literal"),
            ("/a/:::x", "/a/::x"),
            ("/a/::", "/a/:"),
            ("/a/foo::x", "/a/foo::x"),
        ] {
            assert_named_order(
                vec![
                    named_route("z-literal", "Exact", pattern),
                    named_route("a-param", "Exact", "/a/:identifier::tail"),
                ],
                request,
                &["z-literal", "a-param"],
            );
        }
    }

    #[test]
    fn named_paths_preserve_normalization_and_trailing_slash_eligibility() {
        assert_named_order(
            vec![
                named_route("z-static", "Exact", "//a///b/"),
                named_route("a-param", "Exact", "/a//:identifier/"),
            ],
            "/a/b/",
            &["z-static", "a-param"],
        );
        assert_named_order(vec![named_route("trailing-exact", "Exact", "/a/:id/")], "/a/b", &[]);
        assert_named_order(vec![named_route("trailing-exact", "Exact", "/a/:id/")], "/a/b/c", &[]);
    }

    #[test]
    fn named_paths_unicode_static_counts_can_tie_across_different_structures() {
        // Both have one static segment, three static characters, two segments.
        // UTF-8 byte counting would incorrectly favor the z route.
        assert_named_order(
            vec![
                named_route("z-multibyte", "Exact", "/:x/€"),
                named_route("a-ascii", "Exact", "/a/:identifier"),
            ],
            "/a/€",
            &["a-ascii", "z-multibyte"],
        );
    }

    #[test]
    fn renamed_parameters_fall_through_every_cross_route_precedence_level() {
        for is_prefix in [false, true] {
            let kind = if is_prefix { "PathPrefix" } else { "Exact" };
            for name in ["customerIdentifier", "é", "€πé", "id::tail"] {
                for level in ["method", "headers", "query", "timestamp", "route-name"] {
                    let mut winner = named_route("z-winner", kind, "/a/:x");
                    let loser_name = if level == "route-name" { "zz-loser" } else { "a-loser" };
                    let mut loser = named_route(loser_name, kind, &format!("/a/:{name}"));
                    let winner_match = &mut winner.spec.rules.as_mut().unwrap()[0].matches.as_mut().unwrap()[0];
                    let loser_match = &mut loser.spec.rules.as_mut().unwrap()[0].matches.as_mut().unwrap()[0];
                    match level {
                        "method" => {
                            winner_match.method = Some("GET".to_string());
                            // A later level favors the loser, proving method wins first.
                            loser_match.headers = Some(
                                serde_json::from_value(serde_json::json!([
                                    {"type": "Exact", "name": "X-A", "value": "1"}
                                ]))
                                .unwrap(),
                            );
                        }
                        "headers" => {
                            winner_match.headers = Some(
                                serde_json::from_value(serde_json::json!([
                                    {"type": "Exact", "name": "X-A", "value": "1"},
                                    {"type": "Exact", "name": "X-B", "value": "2"}
                                ]))
                                .unwrap(),
                            );
                            loser_match.headers = Some(
                                serde_json::from_value(serde_json::json!([
                                    {"type": "Exact", "name": "X-A", "value": "1"}
                                ]))
                                .unwrap(),
                            );
                            loser_match.query_params = Some(
                                serde_json::from_value(serde_json::json!([
                                    {"type": "Exact", "name": "q", "value": "1"}
                                ]))
                                .unwrap(),
                            );
                        }
                        "query" => {
                            winner_match.query_params = Some(
                                serde_json::from_value(serde_json::json!([
                                    {"type": "Exact", "name": "q", "value": "1"},
                                    {"type": "Exact", "name": "r", "value": "2"}
                                ]))
                                .unwrap(),
                            );
                            loser_match.query_params = Some(
                                serde_json::from_value(serde_json::json!([
                                    {"type": "Exact", "name": "q", "value": "1"}
                                ]))
                                .unwrap(),
                            );
                        }
                        "timestamp" => {
                            loser.metadata.creation_timestamp =
                                serde_json::from_value(serde_json::json!("2025-01-01T00:00:00Z")).unwrap();
                        }
                        "route-name" => {}
                        _ => unreachable!("fixture lists every level"),
                    }
                    assert_named_order(vec![loser, winner], "/a/b", &["z-winner", loser_name]);
                }
            }
        }
    }

    #[test]
    fn renamed_parameters_fall_through_rule_match_and_final_path_indices() {
        let route = route_with_paths(
            "default",
            "same",
            vec!["named.example.com", "named.example.com"],
            "2024-01-01T00:00:00Z",
            serde_json::json!([
                {"matches": [
                    {"path": {"type": "Exact", "value": "/a/:x"}},
                    {"path": {"type": "Exact", "value": "/a/:customerIdentifier"}}
                ]},
                {"matches": [{"path": {"type": "Exact", "value": "/a/:é€"}}]}
            ]),
        );
        let mut units = named_units(vec![route]);
        for _ in 0..2 {
            let engine = RadixRouteMatchEngine::build(units.clone()).expect("engine must build");
            let actual: Vec<(u32, u32, u32)> = engine
                .ordered_candidates_for_test("/a/b")
                .iter()
                .map(|(_, idx)| {
                    let spec = &units[engine.paths_for_test()[*idx as usize].route_idx].specificity;
                    (spec.rule_index, spec.match_index, *idx)
                })
                .collect();
            assert_eq!(
                actual.iter().map(|(r, m, _)| (*r, *m)).collect::<Vec<_>>(),
                vec![(0, 0), (0, 0), (0, 1), (0, 1), (1, 0), (1, 0)],
            );
            for pair in actual.chunks_exact(2) {
                assert!(pair[0].2 < pair[1].2, "fully tied registrations use ascending path_idx");
            }
            units.reverse();
        }
    }

    #[test]
    fn named_path_rebuild_preserves_old_snapshot_and_updated_capture_names() {
        let old_units = named_units(vec![
            named_route("a-first", "Exact", "/a/:x"),
            named_route("z-other", "Exact", "/a/:other"),
        ]);
        let old = RadixRouteMatchEngine::build(old_units.clone()).unwrap();
        let new_units = named_units(vec![
            named_route("a-first", "Exact", "/a/:customerIdentifier"),
            named_route("z-other", "Exact", "/a/:other"),
        ]);
        let new = RadixRouteMatchEngine::build(new_units.clone()).unwrap();
        let expected = vec!["default/a-first".to_string(), "default/z-other".to_string()];
        assert_eq!(named_order(&old, &old_units, "/a/b"), expected);
        assert_eq!(named_order(&new, &new_units, "/a/b"), expected);
        for (engine, name) in [(&old, "x"), (&new, "customerIdentifier")] {
            let first = engine.ordered_candidates_for_test("/a/b")[0].1;
            let pattern = &engine.paths_for_test()[first as usize].normalized;
            let captures = crate::ctx::parse_path_params(pattern, "/a/b");
            assert_eq!(captures.len(), 1);
            assert_eq!(captures.get(name).map(String::as_str), Some("b"));
        }
        // A changed static constraint affects only the freshly built snapshot.
        let changed_units = named_units(vec![
            named_route("a-first", "Exact", "/a/:customerIdentifier"),
            named_route("z-other", "Exact", "/a/b"),
        ]);
        let changed = RadixRouteMatchEngine::build(changed_units.clone()).unwrap();
        assert_eq!(
            named_order(&changed, &changed_units, "/a/b"),
            vec!["default/z-other", "default/a-first"]
        );
        assert_eq!(named_order(&old, &old_units, "/a/b"), expected);
    }

    #[test]
    fn metric_backend_slots_include_service_ai_and_invalid_refs() {
        let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
            "backendRefs": [
                {"name": "service"},
                {
                    "group": "edgion.io",
                    "kind": "EdgionBackend",
                    "namespace": "models",
                    "name": "provider"
                },
                {"group": "example.io", "kind": "CustomBackend", "name": "invalid"}
            ]
        }))
        .expect("backend metric fixture must deserialize");

        assert_eq!(
            http_backends_from_rule(&rule, "route-ns"),
            vec![
                ("route-ns".to_string(), "service".to_string()),
                ("models".to_string(), "provider".to_string()),
                ("route-ns".to_string(), "invalid".to_string()),
            ]
        );
    }

    #[test]
    fn test_get_effective_hostnames_multiple_resolved() {
        let mut route = create_test_httproute(
            "default",
            "route1",
            vec!["ignored.example.com"],
            vec![("default", "gw1")],
        );
        route.spec.resolved_hostnames = Some(vec!["a.example.com".to_string(), "b.example.com".to_string()]);
        let result = get_effective_hostnames(&route);
        assert_eq!(result, vec!["a.example.com", "b.example.com"]);
    }

    // ====================================================================
    // ConfHandler ordering regression tests
    //
    // These pin the post-conditions of full_set / partial_update so that the
    // ordering invariant documented at the top of the impl block (sync LB →
    // mutate cache → rebuild) cannot be silently broken by a refactor.
    //
    // The global POLICY_STORE is shared across tests; each scenario uses a
    // distinct namespace and cleans up its own service entries to stay
    // isolation-safe under `cargo test`'s default parallelism.
    // ====================================================================

    fn build_route_with_lb_policy(
        namespace: &str,
        name: &str,
        service: &str,
        kind: &str,
        policy_name: &str,
    ) -> HTTPRoute {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": { "namespace": namespace, "name": name },
            "spec": {
                "parentRefs": [{ "name": "test-gateway", "kind": "Gateway" }],
                "hostnames": ["test.example.com"],
                "rules": [{
                    "backendRefs": [{
                        "name": service,
                        "port": 8080,
                        "kind": "Service",
                        "filters": [{
                            "type": "ExtensionRef",
                            "extensionRef": { "group": "edgion.io", "kind": kind, "name": policy_name }
                        }]
                    }]
                }]
            }
        });
        let mut route: HTTPRoute = serde_json::from_value(json).expect("Failed to build HTTPRoute");
        route.pre_parse();
        route
    }

    fn build_route_with_two_rules(namespace: &str, name: &str) -> HTTPRoute {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": { "namespace": namespace, "name": name },
            "spec": {
                "parentRefs": [{ "name": "test-gateway", "kind": "Gateway" }],
                "hostnames": ["test.example.com"],
                "rules": [
                    { "matches": [{ "path": { "type": "PathPrefix", "value": "/spec-a" } }],
                      "backendRefs": [{ "name": "svc", "port": 8080 }] },
                    { "matches": [{ "path": { "type": "PathPrefix", "value": "/spec-b" } }],
                      "backendRefs": [{ "name": "svc", "port": 8080 }] }
                ]
            }
        });
        let mut route: HTTPRoute = serde_json::from_value(json).expect("Failed to build HTTPRoute");
        route.pre_parse();
        route
    }

    fn domain_unit_paths(domain_rules: &DomainRouteRulesMap, hostname: &str) -> Vec<String> {
        domain_rules
            .get(hostname)
            .map(|(normal, regex)| {
                normal
                    .iter()
                    .chain(regex.iter())
                    .filter_map(|u| u.matched_info.m.path.as_ref().and_then(|p| p.value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Build-site wiring: the units a real HTTPRoute compiles to must carry the
    /// key the request path would have derived inline, i.e. the route's own
    /// namespace must reach `resolve_parent_refs`.
    ///
    /// The unit tests for `resolve_parent_refs` and `check_gateway_listener_match`
    /// each cover one half in isolation; only this test would fail if the build
    /// site ever passed the wrong string (the route's *name*, a listener name, a
    /// parent's namespace) as the fallback.
    #[test]
    fn build_site_resolves_parent_ref_keys_with_the_route_namespace() {
        // `build_route_with_two_rules` declares `parentRefs: [{ name: "test-gateway" }]`
        // with no namespace, so every resolved key must use the route's namespace.
        let route = build_route_with_two_rules("route-ns", "wiring");

        let mut data = HashMap::new();
        data.insert("route-ns/wiring".to_string(), Arc::new(route));
        let domain_rules = parse_http_routes_to_domain_rules(&data);

        let (normal, regex) = domain_rules
            .get("test.example.com")
            .expect("the route must compile to a hostname bucket");
        let units: Vec<_> = normal.iter().chain(regex.iter()).collect();
        assert_eq!(units.len(), 2, "two rules, one match each");

        for unit in &units {
            let resolved = unit
                .parent_refs
                .as_ref()
                .expect("the build site always populates parent_refs");
            assert_eq!(resolved.len(), 1);
            assert_eq!(
                resolved[0].gateway_key, "route-ns/test-gateway",
                "the omitted parentRef namespace must fall back to the ROUTE namespace"
            );
            assert_eq!(resolved[0].parent_ns, "route-ns");
            // The same namespace the request path passes as `route_ns`.
            assert_eq!(unit.matched_info.rsc.ns, "route-ns");
        }

        // All units of one route share a single allocation.
        assert!(
            Arc::ptr_eq(
                units[0].parent_refs.as_ref().unwrap(),
                units[1].parent_refs.as_ref().unwrap()
            ),
            "resolved parentRefs must be Arc-shared across the route's units"
        );
    }

    /// The resolved slice is built once per ROUTE, not once per hostname, so
    /// units in different hostname buckets of the same route share one
    /// allocation. This mirrors how rule runtimes are shared in the same
    /// loop; the inputs are loop-invariant across hostnames.
    #[test]
    fn resolved_parent_refs_are_shared_across_hostname_buckets() {
        let mut route = build_route_with_two_rules("route-ns", "multi-host");
        route.spec.hostnames = Some(vec!["a.example.com".to_string(), "b.example.com".to_string()]);
        route.pre_parse();

        let mut data = HashMap::new();
        data.insert("route-ns/multi-host".to_string(), Arc::new(route));
        let domain_rules = parse_http_routes_to_domain_rules(&data);

        let first_unit_of = |hostname: &str| {
            let (normal, regex) = domain_rules
                .get(hostname)
                .unwrap_or_else(|| panic!("{hostname} must have a bucket"));
            normal
                .iter()
                .chain(regex.iter())
                .next()
                .expect("bucket must hold at least one unit")
                .clone()
        };

        let a = first_unit_of("a.example.com");
        let b = first_unit_of("b.example.com");

        let a_refs = a.parent_refs.as_ref().expect("populated");
        let b_refs = b.parent_refs.as_ref().expect("populated");
        assert_eq!(a_refs[0].gateway_key, "route-ns/test-gateway");
        assert!(
            Arc::ptr_eq(a_refs, b_refs),
            "one route must resolve its parentRefs once, not once per hostname"
        );
    }

    #[test]
    fn rule_and_metric_handles_share_across_fanout_but_selectors_stay_per_hostname() {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": { "namespace": "route-ns", "name": "shared-rule-state" },
            "spec": {
                "parentRefs": [{ "name": "test-gateway", "kind": "Gateway" }],
                "hostnames": ["a.example.com", "b.example.com"],
                "rules": [
                    {
                        "matches": [
                            { "path": { "type": "PathPrefix", "value": "/a" } },
                            { "path": { "type": "PathPrefix", "value": "/b" } }
                        ],
                        "backendRefs": [{ "name": "svc-a", "port": 8080 }]
                    },
                    {
                        "matches": [
                            { "path": { "type": "PathPrefix", "value": "/c" } },
                            { "path": { "type": "PathPrefix", "value": "/d" } }
                        ],
                        "backendRefs": [
                            { "name": "svc-b", "port": 8080 },
                            { "name": "svc-c", "port": 8080 }
                        ]
                    }
                ]
            }
        });
        let mut route: HTTPRoute = serde_json::from_value(json).expect("route fixture must deserialize");
        route.pre_parse();

        let mut data = HashMap::new();
        data.insert("route-ns/shared-rule-state".to_string(), Arc::new(route));
        let domain_rules = parse_http_routes_to_domain_rules(&data);
        let unit = |hostname: &str, rule_id: usize, match_id: usize| {
            let (normal, regex) = domain_rules
                .get(hostname)
                .unwrap_or_else(|| panic!("{hostname} must have a bucket"));
            normal
                .iter()
                .chain(regex.iter())
                .find(|unit| unit.matched_info.rule_id == rule_id && unit.matched_info.match_id == match_id)
                .cloned()
                .unwrap_or_else(|| panic!("missing rule {rule_id} match {match_id} for {hostname}"))
        };

        let a00 = unit("a.example.com", 0, 0);
        let a01 = unit("a.example.com", 0, 1);
        let b00 = unit("b.example.com", 0, 0);
        let a10 = unit("a.example.com", 1, 0);
        let b11 = unit("b.example.com", 1, 1);

        for same_rule in [&a01, &b00] {
            assert!(Arc::ptr_eq(&a00.rule, &same_rule.rule));
            assert!(Arc::ptr_eq(&a00.metric_handles, &same_rule.metric_handles));
        }
        assert!(Arc::ptr_eq(&a10.rule, &b11.rule));
        assert!(Arc::ptr_eq(&a10.metric_handles, &b11.metric_handles));
        assert!(!Arc::ptr_eq(&a00.rule, &a10.rule));
        assert!(!Arc::ptr_eq(&a00.metric_handles, &a10.metric_handles));

        let rule0_matrix = a00
            .metric_handles
            .lookup("route-ns", "test-gateway")
            .expect("rule 0 gateway matrix must exist");
        let rule1_matrix = a10
            .metric_handles
            .lookup("route-ns", "test-gateway")
            .expect("rule 1 gateway matrix must exist");
        assert_eq!(rule0_matrix.counters.len(), 1);
        assert_eq!(rule1_matrix.counters.len(), 2);

        assert!(
            Arc::ptr_eq(&a00.backend_finder, &a01.backend_finder),
            "matches of one rule and hostname must share a selector"
        );
        assert!(
            !Arc::ptr_eq(&a00.backend_finder, &b00.backend_finder),
            "different hostname buckets must retain independent selector state"
        );
        assert!(
            !Arc::ptr_eq(&a00.backend_finder, &a10.backend_finder),
            "different rules must retain independent selector state"
        );
    }

    #[test]
    fn test_resolved_rules_compiled_instead_of_spec_rules() {
        // Delegation contract: when resolved_rules is present the gateway
        // compiles it INSTEAD of spec.rules.
        let mut route = build_route_with_two_rules("test-deleg", "flat");
        route.spec.resolved_rules = Some(
            serde_json::from_value(serde_json::json!([
                { "matches": [{ "path": { "type": "PathPrefix", "value": "/resolved" } }],
                  "backendRefs": [{ "name": "child-svc", "namespace": "test-deleg", "port": 8080 }] }
            ]))
            .unwrap(),
        );
        route.pre_parse();

        let mut data = HashMap::new();
        data.insert("test-deleg/flat".to_string(), Arc::new(route));
        let domain_rules = parse_http_routes_to_domain_rules(&data);

        let paths = domain_unit_paths(&domain_rules, "test.example.com");
        assert_eq!(paths, vec!["/resolved"], "resolved_rules must replace spec.rules");
    }

    #[test]
    fn test_invalid_rule_indices_ignored_when_resolved_rules_present() {
        // invalid_rule_indices refers to spec.rules; a flattened route must
        // not have its (pre-filtered) resolved_rules dropped by stale indices.
        let mut route = build_route_with_two_rules("test-deleg-inv", "flat");
        route.spec.invalid_rule_indices = Some(vec![0]);
        route.spec.resolved_rules = Some(
            serde_json::from_value(serde_json::json!([
                { "matches": [{ "path": { "type": "PathPrefix", "value": "/r-a" } }],
                  "backendRefs": [{ "name": "svc", "port": 8080 }] },
                { "matches": [{ "path": { "type": "PathPrefix", "value": "/r-b" } }],
                  "backendRefs": [{ "name": "svc", "port": 8080 }] }
            ]))
            .unwrap(),
        );
        route.pre_parse();

        let mut data = HashMap::new();
        data.insert("test-deleg-inv/flat".to_string(), Arc::new(route));
        let domain_rules = parse_http_routes_to_domain_rules(&data);
        let mut paths = domain_unit_paths(&domain_rules, "test.example.com");
        paths.sort();
        assert_eq!(paths, vec!["/r-a", "/r-b"], "indices must not apply to resolved_rules");

        // Control: without resolved_rules the same indices DO drop rule 0.
        let mut plain = build_route_with_two_rules("test-deleg-inv", "plain");
        plain.spec.invalid_rule_indices = Some(vec![0]);
        let mut data = HashMap::new();
        data.insert("test-deleg-inv/plain".to_string(), Arc::new(plain));
        let domain_rules = parse_http_routes_to_domain_rules(&data);
        let paths = domain_unit_paths(&domain_rules, "test.example.com");
        assert_eq!(paths, vec!["/spec-b"], "indices must still apply to spec.rules");
    }

    #[test]
    fn credentialed_expose_headers_rejected_before_publication() {
        use serde_json::json;
        for resolved in [false, true] {
            let mut route = route_with_paths(
                "cors-expose",
                "route",
                vec!["cors.example"],
                "2026-09-10T00:00:00Z",
                json!([
                    {"matches": [{"path": {"value": "/invalid"}}], "filters": [{"type": "CORS",
                        "cors": {"allowOrigins": ["https://app.example"], "allowCredentials": true,
                            "exposeHeaders": ["*", "X-Request-Id"]}}]},
                    {"matches": [{"path": {"value": "/valid"}}], "filters": [{"type": "CORS",
                        "cors": {"allowOrigins": ["https://app.example"], "allowCredentials": true,
                            "exposeHeaders": ["X-Request-Id"]}}]}
                ]),
            );
            if resolved {
                route.spec.resolved_rules = route.spec.rules.take();
                route.spec.invalid_rule_indices = Some(vec![1]);
            }
            route.pre_parse();
            assert!(route.status.is_none());
            let domains =
                parse_http_routes_to_domain_rules(&HashMap::from([("cors-expose/route".into(), Arc::new(route))]));
            assert_eq!(domain_unit_paths(&domains, "cors.example"), ["/valid"]);
            assert_eq!(domains["cors.example"].0[0].matched_info.rule_id, 1);
        }
    }

    #[test]
    fn backend_cors_scope_rejected_without_controller_status_or_for_resolved_rules() {
        use serde_json::json;
        let cors = json!({"type": "CORS", "cors": {"allowOrigins": ["https://app.example"]}});
        let rules = json!([
            {"matches": [{"path": {"value": "/backend-only"}}], "backendRefs": [
                {"name": "svc", "port": 80, "filters": [cors.clone()]}
            ]},
            {"matches": [{"path": {"value": "/rule-only"}}], "filters": [cors.clone()],
             "backendRefs": [{"name": "svc", "port": 80}]},
            {"matches": [{"path": {"value": "/mixed"}}], "filters": [cors.clone()],
             "backendRefs": [{"name": "svc", "port": 80, "filters": [cors.clone()]}]},
            {"matches": [{"path": {"value": "/weighted"}}], "backendRefs": [
                {"name": "plain", "port": 80, "weight": 1},
                {"name": "cors", "port": 80, "weight": 0, "filters": [cors]}
            ]},
            {"matches": [{"path": {"value": "/plain"}}], "backendRefs": [{"name": "svc", "port": 80}]}
        ]);
        for resolved in [false, true] {
            let mut route = route_with_paths(
                "cors-scope",
                "route",
                vec!["cors.example"],
                "2026-09-09T00:00:00Z",
                rules.clone(),
            );
            if resolved {
                route.spec.resolved_rules = route.spec.rules.take();
                // These indices address authored rules, never resolved rules.
                route.spec.invalid_rule_indices = Some(vec![1, 4]);
            }
            route.pre_parse();
            assert!(route.status.is_none());
            let data = HashMap::from([("cors-scope/route".to_string(), Arc::new(route))]);
            let domains = parse_http_routes_to_domain_rules(&data);
            let mut paths = domain_unit_paths(&domains, "cors.example");
            paths.sort();
            assert_eq!(paths, ["/plain", "/rule-only"], "resolved={resolved}");
            let (normal, regex) = domains.get("cors.example").unwrap();
            let mut ids: Vec<_> = normal
                .iter()
                .chain(regex)
                .map(|unit| unit.matched_info.rule_id)
                .collect();
            ids.sort();
            assert_eq!(ids, [1, 4], "dropping rules must preserve original rule IDs");
        }
    }

    #[test]
    fn http_route_log_label_annotation_does_not_reach_match_info() {
        // Regression guard: `edgion.io/log-label.<suffix>` was narrowed to
        // TLS resources only (TLSRoute / EdgionTls); HTTPRoute/GRPCRoute log
        // projection is now `AccessLogExtern`'s job. An HTTPRoute carrying the
        // annotation must NOT get it into `MatchInfo.rsc.labels`, or the two
        // mechanisms double-log the same data.
        use edgion_resources::constants::annotations::edgion::LOG_LABEL_PREFIX;

        let mut route = build_route_with_two_rules("test-log-label", "labeled");
        route.metadata.annotations = Some(
            [(format!("{LOG_LABEL_PREFIX}tenant"), "internal".to_string())]
                .into_iter()
                .collect(),
        );
        route.pre_parse();

        let mut data = HashMap::new();
        data.insert("test-log-label/labeled".to_string(), Arc::new(route));
        let domain_rules = parse_http_routes_to_domain_rules(&data);

        let (normal, regex) = domain_rules.get("test.example.com").expect("domain must be present");
        let unit = normal
            .iter()
            .chain(regex.iter())
            .next()
            .expect("at least one unit built");
        assert!(
            unit.matched_info.rsc.labels.is_none(),
            "HTTPRoute's edgion.io/log-label.* annotation must not reach MatchInfo.rsc.labels; \
             got: {:?}",
            unit.matched_info.rsc.labels
        );
    }

    #[test]
    fn test_full_set_replaces_cache() {
        let mgr = GlobalHttpRouteManagers::new();
        let route_key = "test-fs-ord/route1";
        let stale_key = "test-fs-ord/stale";

        let stale = build_route_with_lb_policy("test-fs-ord", "stale", "svc1", "TestExtensionKindB", "default");
        mgr.route_cache
            .insert(stale_key.to_string(), std::sync::Arc::new(stale));

        let mut data = HashMap::new();
        data.insert(
            route_key.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-fs-ord",
                "route1",
                "svc1",
                "TestExtensionKindA",
                "header.x-id",
            )),
        );
        mgr.full_set(&data);

        assert!(
            mgr.route_cache.get(stale_key).is_none(),
            "full_set must clear stale cache entries"
        );
        assert!(
            mgr.route_cache.get(route_key).is_some(),
            "new route must be present in cache"
        );
    }

    #[test]
    fn test_partial_update_route_only_change() {
        let mgr = GlobalHttpRouteManagers::new();
        let route_key = "test-pu-rt/route1";

        let mut add = HashMap::new();
        add.insert(
            route_key.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-pu-rt",
                "route1",
                "svc",
                "TestExtensionKindA",
                "header.x-id",
            )),
        );
        mgr.partial_update(add, HashMap::new(), HashSet::new());

        // Same policy, different hostname — pure route metadata change.
        let mut updated_route =
            build_route_with_lb_policy("test-pu-rt", "route1", "svc", "TestExtensionKindA", "header.x-id");
        updated_route.spec.hostnames = Some(vec!["other.example.com".to_string()]);
        let mut update = HashMap::new();
        update.insert(route_key.to_string(), Arc::new(updated_route));
        mgr.partial_update(HashMap::new(), update, HashSet::new());

        let cached = mgr.route_cache.get(route_key).expect("route must remain in cache");
        assert_eq!(
            cached.spec.hostnames.as_deref(),
            Some(&["other.example.com".to_string()][..]),
            "route_cache must hold the updated route after partial_update"
        );
    }

    #[test]
    fn test_partial_update_simultaneous_add_update_remove() {
        let mgr = GlobalHttpRouteManagers::new();
        let route_a = "test-pu-mix/route-a";
        let route_b = "test-pu-mix/route-b";
        let route_c = "test-pu-mix/route-c";

        let mut init = HashMap::new();
        init.insert(
            route_a.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-pu-mix",
                "route-a",
                "svc-a",
                "TestExtensionKindA",
                "header.x-id",
            )),
        );
        init.insert(
            route_b.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-pu-mix",
                "route-b",
                "svc-b",
                "TestExtensionKindB",
                "default",
            )),
        );
        mgr.partial_update(init, HashMap::new(), HashSet::new());

        let mut add = HashMap::new();
        add.insert(
            route_c.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-pu-mix",
                "route-c",
                "svc-c",
                "TestExtensionKindB",
                "default",
            )),
        );
        let mut update = HashMap::new();
        update.insert(
            route_a.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-pu-mix",
                "route-a",
                "svc-a",
                "TestExtensionKindB",
                "default",
            )),
        );
        let mut remove = HashSet::new();
        remove.insert(route_b.to_string());
        mgr.partial_update(add, update, remove);

        assert!(mgr.route_cache.get(route_a).is_some());
        assert!(mgr.route_cache.get(route_b).is_none(), "removed route must be evicted");
        assert!(mgr.route_cache.get(route_c).is_some());
    }

    #[test]
    fn test_full_set_shares_arc_no_deep_clone() {
        let mgr = GlobalHttpRouteManagers::new();
        let route_key = "test-arc-share/route1";
        let route = std::sync::Arc::new(build_route_with_lb_policy(
            "test-arc-share",
            "route1",
            "svc1",
            "TestExtensionKindA",
            "header.x-id",
        ));

        let mut data = std::collections::HashMap::new();
        data.insert(route_key.to_string(), route.clone());
        mgr.full_set(&data);

        let cached = mgr.route_cache.get(route_key).expect("route must be cached");
        assert!(
            std::sync::Arc::ptr_eq(cached.value(), &route),
            "route_cache must store the same Arc (zero-copy), not a deep clone"
        );
    }

    #[test]
    fn test_built_unit_carries_route_arc() {
        let _mgr = GlobalHttpRouteManagers::new();
        let route_key = "test-unit-route/route1";
        let route = std::sync::Arc::new(build_route_with_lb_policy(
            "test-unit-route",
            "route1",
            "svc1",
            "TestExtensionKindA",
            "header.x-id",
        ));
        let mut data = std::collections::HashMap::new();
        data.insert(route_key.to_string(), route.clone());

        let table = build_domain_route_rules_from_routes(&data);
        // build_route_with_lb_policy sets hostnames: ["test.example.com"] -> exact_domain_map bucket.
        let rules = table
            .exact_domain_map
            .get("test.example.com")
            .cloned()
            .expect("test.example.com rules present");
        let unit = rules
            .route_rules_list
            .first()
            .or_else(|| rules.regex_routes.first())
            .expect("at least one unit built");
        let unit_route = unit.route.as_ref().expect("unit carries its route");
        assert!(
            std::sync::Arc::ptr_eq(unit_route, &route),
            "unit.route must share the cache Arc (zero-copy)"
        );
    }
}
