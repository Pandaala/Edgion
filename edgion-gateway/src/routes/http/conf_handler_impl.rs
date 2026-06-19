use crate::lb::BackendSelector;
use crate::plugins::PluginRuntime;
use crate::routes::common::{effective_hostnames, filter_accepted_parent_refs, gateways_from_parent_refs};
use crate::routes::http::lb_policy_sync::{cleanup_lb_policies_for_routes, sync_lb_policies_for_routes};
use crate::routes::http::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::routes::http::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::routes::http::match_unit::{CompiledBackend, HttpRuleRuntime, MatchSpecificity};
use crate::routes::http::routes_mgr::{
    resolved_listeners_for_route, DomainRouteRules, GlobalHttpRouteManagers, RouteRules,
};
use crate::routes::http::{get_global_http_route_managers, HttpRouteRuleUnit};
use crate::MatchInfo;
use edgion_common::conf_sync::traits::{arc_map_into_owned, arc_map_to_owned, ConfHandler};
use edgion_resources::matcher::host_match::radix_match::{RadixHost, RadixHostMatchEngine};
use edgion_resources::resources::common::ParentReference;
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
    /// Operator-defined log labels, resolved once per route at build time and
    /// shared (Arc) into every match-item unit's `MatchInfo`.
    labels: Option<crate::LogLabels>,
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
    // Update order is load-bearing across all ConfHandler entry points:
    //   1. sync/cleanup LB policies   — populate global policy store first so any
    //      lookup driven by the new route_cache can resolve its policy.
    //   2. mutate route_cache         — canonical store reflecting the new state.
    //   3. rebuild listener managers  — readers off match engines see (cache, policy)
    //      that are already mutually consistent.
    // Reordering risks a transient window where matched routes reference services
    // whose LB policy is still stale (or missing). Regression tests in this file
    // pin the end-state for full_set / partial_update / mixed scenarios.
    fn full_set(&self, data: &HashMap<String, Arc<HTTPRoute>>) {
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        let data_owned = arc_map_to_owned(data);
        // Step 1: sync LB policies (must precede cache mutation — see invariant above).
        sync_lb_policies_for_routes(&data_owned);

        let start_time = Instant::now();
        tracing::info!(component = "http_route_manager", cnt = data.len(), "full set start");

        // Step 2: Replace route cache (reuse data_owned to avoid double clone)
        self.route_cache.clear();
        for (key, route) in data_owned {
            self.route_cache.insert(key, route);
        }

        // Step 3: Rebuild all per-listener managers (lock already held above).
        self.rebuild_all_listener_managers_locked();

        let elapsed = Instant::now().saturating_duration_since(start_time);
        tracing::info!(
            component = "http_route_manager",
            ms = elapsed.as_millis(),
            "full set done"
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<HTTPRoute>>,
        update: HashMap<String, Arc<HTTPRoute>>,
        remove: HashSet<String>,
    ) {
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        // Covers the affected-set computation, route_cache mutation, and rebuild as one
        // critical section so rebuild_all cannot interleave on a stale snapshot.
        let _guard = self.lock_rebuild();
        let mut add_or_update_owned = arc_map_into_owned(add);
        add_or_update_owned.extend(arc_map_into_owned(update));
        // Step 1: sync/cleanup LB policies before touching route_cache (see invariant
        // above on full_set). cleanup runs after sync so that an updated route which
        // still references an old service does not have its policy dropped here.
        sync_lb_policies_for_routes(&add_or_update_owned);
        cleanup_lb_policies_for_routes(&remove);

        let start_time = Instant::now();
        tracing::info!(
            component = "http_route_manager",
            add = add_or_update_owned.len(),
            rm = remove.len(),
            "partial update start"
        );

        // Compute affected listeners BEFORE updating cache.
        let mut affected_listeners: HashSet<ListenerKey> = HashSet::new();

        for (key, route) in &add_or_update_owned {
            let listeners = resolved_listeners_for_route(route);
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

        // Update route cache (reuse add_or_update_owned to avoid double clone)
        for (key, route) in add_or_update_owned {
            self.route_cache.insert(key, route);
        }
        for key in &remove {
            self.route_cache.remove(key);
        }

        self.rebuild_affected_listener_managers(&affected_listeners);

        tracing::info!(
            component = "http_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "partial update done"
        );
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
    parent_refs: Option<Vec<ParentReference>>,
    creation_ts_ms: i64,
    route_sv: u64,
    /// Concrete `(gateway_ns, gateway_name)` pairs this route attaches to,
    /// pre-resolved from `parent_refs` against the accepted status.
    /// Drives `RouteMetricHandles::build`.
    attached_gateways: &'a [(String, String)],
    /// Rule-level compiled plugin runtime, built ONCE per rule and Arc-shared
    /// into every match-item unit (see `HttpRouteRuleUnit::compile_http_rule`).
    plugin_runtime: Arc<PluginRuntime>,
    /// Per-(rule,hostname) LB selector, Arc-shared across this rule's units.
    backend_finder: Arc<BackendSelector<CompiledBackend>>,
    /// Per-rule compiled backends (Arc-shared), for by-name lookup.
    backends: Arc<Vec<CompiledBackend>>,
    /// Operator-defined log labels, resolved once per route, Arc-shared into
    /// each match-item unit's `MatchInfo`.
    labels: Option<crate::LogLabels>,
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
            ctx.labels.clone(),
        ),
        rule: ctx.rule.clone(),
        path_regex: Some(regex),
        parent_refs: ctx.parent_refs,
        compiled_header_regexes,
        compiled_query_regexes,
        has_query_matchers,
        specificity,
        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::build(
            ctx.attached_gateways,
            ctx.namespace,
            ctx.name,
            &http_backends_from_rule(&ctx.rule, ctx.namespace),
        )),
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
            ctx.labels.clone(),
        ),
        rule: ctx.rule.clone(),
        path_regex: None,
        parent_refs: ctx.parent_refs,
        compiled_header_regexes,
        compiled_query_regexes,
        has_query_matchers,
        specificity,
        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::build(
            ctx.attached_gateways,
            ctx.namespace,
            ctx.name,
            &http_backends_from_rule(&ctx.rule, ctx.namespace),
        )),
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

/// Expand all rules of a single route into `hostname`'s domain bucket.
#[allow(clippy::too_many_arguments)] // validated route context has 8 interrelated params; grouping would obscure them
fn append_route_to_hostname(
    domain_rules: &mut DomainRouteRulesMap,
    hostname: &str,
    validated: &ValidatedHttpRoute<'_>,
    resource_key: &str,
    parent_refs: &[ParentReference],
    invalid_rule_indices: Option<&Vec<usize>>,
    creation_ts_ms: i64,
    route_sv: u64,
    rule_runtimes: &[HttpRuleRuntime],
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

    let attached_gateways = gateways_from_parent_refs(parent_refs, &validated.namespace);

    for (rule_id, rule) in validated.rules.iter().enumerate() {
        if invalid_rule_indices.is_some_and(|inv| inv.contains(&rule_id)) {
            continue;
        }
        let rule_arc = Arc::new(rule.clone());
        // Compiled runtime is built ONCE per rule (`rule_runtimes[rule_id]`);
        // the LB selector is built once per (rule, hostname) and shared across
        // this rule's match-item units.
        let rule_rt = &rule_runtimes[rule_id];
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
                    rule: rule_arc.clone(),
                    parent_refs: Some(parent_refs.to_vec()),
                    creation_ts_ms,
                    route_sv,
                    attached_gateways: &attached_gateways,
                    plugin_runtime: rule_rt.plugin_runtime.clone(),
                    backend_finder: backend_finder.clone(),
                    backends: rule_rt.backends.clone(),
                    labels: validated.labels.clone(),
                },
            );
        }
    }
}

/// Parse all HTTPRoutes and collect rules into a domain->rules structure.
///
/// Routes are merged into a single table keyed by hostname.
/// Each route unit carries its parentRefs so gateway validation happens at match time.
fn parse_http_routes_to_domain_rules(data: &HashMap<String, HTTPRoute>) -> DomainRouteRulesMap {
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
        // Compile each rule's runtime ONCE per route (shared across all hostnames
        // and match-items). Correctness-critical: in-memory RateLimit owns per-rule
        // state, so building a runtime per hostname/match would split that state.
        let rule_runtimes: Vec<HttpRuleRuntime> = validated
            .rules
            .iter()
            .map(|r| HttpRouteRuleUnit::compile_http_rule(r, &validated.namespace))
            .collect();

        for hostname in &get_effective_hostnames(route) {
            append_route_to_hostname(
                &mut domain_rules,
                hostname,
                &validated,
                &resource_key,
                &accepted_refs,
                route.spec.invalid_rule_indices.as_ref(),
                creation_ts_ms,
                route_sv,
                &rule_runtimes,
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

    let rules = match &route.spec.rules {
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

    let labels = crate::extract_log_labels(&route.metadata);

    Some(ValidatedHttpRoute {
        rules,
        namespace: route_namespace,
        name: route_name,
        labels,
    })
}

/// Build DomainRouteRules from a set of routes (used by per-port rebuild).
pub(crate) fn build_domain_route_rules_from_routes(data: &HashMap<String, HTTPRoute>) -> DomainRouteRules {
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

/// Extract `(backend_ns, backend_name)` tuples in `backend_refs` order, with
/// fallback to `route_ns` when the backend ref omits its namespace.
fn http_backends_from_rule(rule: &HTTPRouteRule, route_ns: &str) -> Vec<(String, String)> {
    rule.backend_refs
        .as_ref()
        .map(|refs| {
            refs.iter()
                .map(|b| {
                    (
                        b.namespace.clone().unwrap_or_else(|| route_ns.to_string()),
                        b.name.clone(),
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

    #[test]
    fn test_full_set_replaces_cache_and_syncs_policies() {
        use crate::lb::lb_policy::{get_global_policy_store, LbPolicy};

        let mgr = GlobalHttpRouteManagers::new();
        let policy_store = get_global_policy_store();
        let svc_key = "test-fs-ord/svc1";
        let route_key = "test-fs-ord/route1";
        let stale_key = "test-fs-ord/stale";

        let stale = build_route_with_lb_policy("test-fs-ord", "stale", "svc1", "LBPolicyLeastConn", "default");
        mgr.route_cache.insert(stale_key.to_string(), stale);

        let mut data = HashMap::new();
        data.insert(
            route_key.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-fs-ord",
                "route1",
                "svc1",
                "LBPolicyConsistentHash",
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
        assert!(
            policy_store.get(svc_key).contains(&LbPolicy::Consistent),
            "policy must be visible in store after full_set returns"
        );

        policy_store.delete_lb_policies_by_resource_key(route_key);
        policy_store.delete_lb_policies_by_resource_key(stale_key);
    }

    #[test]
    fn test_partial_update_policy_only_change() {
        use crate::lb::lb_policy::{get_global_policy_store, LbPolicy};

        let mgr = GlobalHttpRouteManagers::new();
        let policy_store = get_global_policy_store();
        let svc_key = "test-pu-pol/svc";
        let route_key = "test-pu-pol/route1";

        let mut add = HashMap::new();
        add.insert(
            route_key.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-pu-pol",
                "route1",
                "svc",
                "LBPolicyConsistentHash",
                "header.x-id",
            )),
        );
        mgr.partial_update(add, HashMap::new(), HashSet::new());
        assert!(policy_store.get(svc_key).contains(&LbPolicy::Consistent));

        let mut update = HashMap::new();
        update.insert(
            route_key.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-pu-pol",
                "route1",
                "svc",
                "LBPolicyLeastConn",
                "default",
            )),
        );
        mgr.partial_update(HashMap::new(), update, HashSet::new());

        assert!(mgr.route_cache.get(route_key).is_some());
        let policies = policy_store.get(svc_key);
        assert!(
            policies.contains(&LbPolicy::LeastConnection),
            "updated policy must reach the store"
        );
        // Union semantics: prior policy persists until the last referencing route is removed.
        assert!(
            policies.contains(&LbPolicy::Consistent),
            "union semantics: prior policy retained"
        );

        policy_store.delete_lb_policies_by_resource_key(route_key);
    }

    #[test]
    fn test_partial_update_route_only_change() {
        use crate::lb::lb_policy::get_global_policy_store;

        let mgr = GlobalHttpRouteManagers::new();
        let policy_store = get_global_policy_store();
        let svc_key = "test-pu-rt/svc";
        let route_key = "test-pu-rt/route1";

        let mut add = HashMap::new();
        add.insert(
            route_key.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-pu-rt",
                "route1",
                "svc",
                "LBPolicyConsistentHash",
                "header.x-id",
            )),
        );
        mgr.partial_update(add, HashMap::new(), HashSet::new());
        let policies_before = policy_store.get(svc_key);
        assert_eq!(policies_before.len(), 1);

        // Same policy, different hostname — pure route metadata change, no policy change.
        let mut updated_route =
            build_route_with_lb_policy("test-pu-rt", "route1", "svc", "LBPolicyConsistentHash", "header.x-id");
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
        assert_eq!(
            policy_store.get(svc_key).len(),
            policies_before.len(),
            "policies untouched when route content changes but policy doesn't"
        );

        policy_store.delete_lb_policies_by_resource_key(route_key);
    }

    #[test]
    fn test_partial_update_simultaneous_add_update_remove() {
        use crate::lb::lb_policy::{get_global_policy_store, LbPolicy};

        let mgr = GlobalHttpRouteManagers::new();
        let policy_store = get_global_policy_store();
        let route_a = "test-pu-mix/route-a";
        let route_b = "test-pu-mix/route-b";
        let route_c = "test-pu-mix/route-c";
        let svc_a = "test-pu-mix/svc-a";
        let svc_b = "test-pu-mix/svc-b";
        let svc_c = "test-pu-mix/svc-c";

        let mut init = HashMap::new();
        init.insert(
            route_a.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-pu-mix",
                "route-a",
                "svc-a",
                "LBPolicyConsistentHash",
                "header.x-id",
            )),
        );
        init.insert(
            route_b.to_string(),
            Arc::new(build_route_with_lb_policy(
                "test-pu-mix",
                "route-b",
                "svc-b",
                "LBPolicyLeastConn",
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
                "LBPolicyLeastConn",
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
                "LBPolicyLeastConn",
                "default",
            )),
        );
        let mut remove = HashSet::new();
        remove.insert(route_b.to_string());
        mgr.partial_update(add, update, remove);

        assert!(mgr.route_cache.get(route_a).is_some());
        assert!(mgr.route_cache.get(route_b).is_none(), "removed route must be evicted");
        assert!(mgr.route_cache.get(route_c).is_some());

        let pa = policy_store.get(svc_a);
        assert!(
            pa.contains(&LbPolicy::LeastConnection),
            "updated policy on route-a present"
        );
        assert!(
            policy_store.get(svc_b).is_empty(),
            "policy for removed route's service must be cleaned up"
        );
        assert!(policy_store.get(svc_c).contains(&LbPolicy::LeastConnection));

        policy_store.delete_lb_policies_by_resource_key(route_a);
        policy_store.delete_lb_policies_by_resource_key(route_c);
    }
}
