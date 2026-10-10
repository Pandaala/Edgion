use crate::lb::BackendSelector;
use crate::plugins::PluginRuntime;
use crate::routes::common::{filter_accepted_parent_refs, gateways_from_parent_refs, resolve_parent_refs};
use crate::routes::http::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::routes::http::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::routes::http::match_unit::{CompiledBackend, HttpRuleRuntime, MatchSpecificity};
use crate::routes::http::routes_mgr::{
    resolved_listeners_for_route, DomainRouteRules, GlobalHttpRouteManagers, RouteRules,
};
use crate::routes::http::{get_global_http_route_managers, HttpRouteRuleUnit};
use crate::runtime::matching::route::ResolvedParentRef;
use crate::MatchInfo;
use edgion_algo::matcher::radix_hostname::{RadixHostnameChange, RadixHostnameMatcher, RadixHostnamePattern};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::{HTTPRoute, HTTPRouteMatch, HTTPRouteRule, ListenerKey, ResourceMeta};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::sync::Arc;
use std::time::Instant;

type DomainStr = String;
/// (normal_routes, regex_routes) tuple for a domain
type RouteRulesPair = (Vec<Arc<HttpRouteRuleUnit>>, Vec<Arc<HttpRouteRuleUnit>>);
/// Domain -> (normal_routes, regex_routes) mapping
type DomainRouteRulesMap = HashMap<DomainStr, RouteRulesPair>;

fn canonicalize_unchanged_routes(
    current: &dashmap::DashMap<String, Arc<HTTPRoute>>,
    incoming: &mut HashMap<String, Arc<HTTPRoute>>,
) {
    for (key, route) in incoming {
        if let Some(old) = current
            .get(key)
            .filter(|old| crate::plugins::publication::same_source(old.value(), route))
        {
            *route = Arc::clone(old.value());
        }
    }
}

fn hostname_table_or_fail_closed(
    result: Result<RadixHostnameMatcher<RouteRules>, impl Display>,
    catch_all_routes: Option<Arc<RouteRules>>,
    pattern_count: usize,
) -> DomainRouteRules {
    match result {
        Ok(hosts) => DomainRouteRules {
            hosts,
            catch_all_routes,
            hostname_build_failed: false,
        },
        Err(error) => {
            tracing::error!(
                component = "http_route_hostname_index",
                pattern_count,
                err = %error,
                "Failed to build the HTTP Route hostname index; publishing an empty Listener table"
            );
            DomainRouteRules {
                hostname_build_failed: true,
                ..DomainRouteRules::new()
            }
        }
    }
}

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

fn get_declared_hostnames(route: &HTTPRoute) -> Vec<String> {
    match route.spec.hostnames.as_ref().filter(|hosts| !hosts.is_empty()) {
        Some(hosts) => {
            let mut hosts = hosts.clone();
            hosts.sort();
            hosts.dedup();
            hosts
        }
        None => vec![CATCH_ALL_HOSTNAME.to_string()],
    }
}

/// Cold change discovery; payload reconstruction visits only touched groups.
#[derive(Default)]
pub(crate) struct HttpRouteChanges {
    pub(crate) resource_keys: HashSet<String>,
    hostnames: HashSet<String>,
}

impl HttpRouteChanges {
    fn record(&mut self, key: &str, route: &HTTPRoute) {
        self.resource_keys.insert(key.to_string());
        self.hostnames.extend(get_declared_hostnames(route));
    }
}

impl ConfHandler<HTTPRoute> for GlobalHttpRouteManagers {
    // Update order across all ConfHandler entry points:
    //   1. mutate route_cache         — canonical store reflecting the new state.
    //   2. rebuild listener managers  — prepare compiled tables under the rebuild lock.
    //   3. commit tables and admissions together before releasing that lock.
    // LB policies are no longer sourced from routes: selection reads the per-service
    // EdgionBackendTrafficPolicy store (see `backends::policy::backend_traffic`).
    // Regression tests in this file pin the end-state for full_set / partial_update /
    // mixed scenarios.
    fn full_set(&self, data: &HashMap<String, Arc<HTTPRoute>>) {
        let start_time = Instant::now();
        tracing::info!(component = "http_route_manager", cnt = data.len(), "full set start");

        let mut canonical = data.clone();

        let guard = self.lock_rebuild();
        {
            // A conf-sync replay may deserialize an unchanged Route into a new
            // Arc. Keep the canonical source identity shared by the route table
            // and plugin publisher so the replay does not create a false
            // cross-generation admission window.
            canonicalize_unchanged_routes(&self.route_cache, &mut canonical);

            // Step 1: Replace route cache with shared Arcs (no deep clone).
            self.route_cache.clear();
            for (key, route) in &canonical {
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
        crate::plugins::publication::global_publisher().replace_http_routes(
            canonical
                .iter()
                .map(|(key, route)| {
                    (
                        key.clone(),
                        crate::plugins::publication::inputs::RouteInput::from_http(Arc::clone(route)),
                    )
                })
                .collect(),
            self.publication_tables(),
        );
        drop(guard);
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

        let guard = self.lock_rebuild();
        {
            canonicalize_unchanged_routes(&self.route_cache, &mut add_or_update);

            // Compute affected listeners BEFORE updating cache.
            let mut affected_listeners: HashSet<ListenerKey> = HashSet::new();
            let mut changes = HttpRouteChanges::default();

            for (key, route) in &add_or_update {
                changes.record(key, route);
                let listeners = resolved_listeners_for_route(route); // &Arc coerces to &HTTPRoute
                if listeners.is_empty() {
                    tracing::warn!(route = %key, "HTTPRoute has no resolved_listeners during partial_update");
                } else {
                    for lk in listeners {
                        affected_listeners.insert(lk.clone());
                    }
                }
                if let Some(old) = self.route_cache.get(key) {
                    changes.record(key, old.value());
                    for lk in resolved_listeners_for_route(old.value()) {
                        affected_listeners.insert(lk.clone());
                    }
                }
            }
            for key in &remove {
                changes.resource_keys.insert(key.clone());
                if let Some(old) = self.route_cache.get(key) {
                    changes.record(key, old.value());
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

            self.rebuild_affected_listener_managers(&affected_listeners, &changes);
        }

        tracing::info!(
            component = "http_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "partial update done"
        );
        let policy_inputs = add_or_update
            .iter()
            .map(|(key, route)| {
                (
                    key.clone(),
                    crate::plugins::publication::inputs::RouteInput::from_http(Arc::clone(route)),
                )
            })
            .collect();
        crate::plugins::publication::global_publisher().update_http_routes(
            policy_inputs,
            &policy_removed,
            self.publication_tables(),
        );
        drop(guard);
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
    backends: Arc<[CompiledBackend]>,
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

/// Compile a match once, then share the same unit across declared hostnames.
fn append_match_to_domains(domain_rules: &mut DomainRouteRulesMap, hostnames: &[String], ctx: RouteUnitCtx<'_>) {
    let resource_key = ctx.resource_key;
    let regex = is_regex_path(ctx.match_item);
    let unit = if regex {
        match create_regex_route_unit(ctx) {
            Ok(unit) => Arc::new(unit),
            Err(e) => {
                tracing::warn!(route = %resource_key, err = %e, "failed to create regex route");
                return;
            }
        }
    } else {
        Arc::new(build_normal_route_unit(ctx))
    };
    for hostname in hostnames {
        let split = domain_rules.entry(hostname.clone()).or_default();
        if regex {
            split.1.push(Arc::clone(&unit));
        } else {
            split.0.push(Arc::clone(&unit));
        }
    }
}

/// Rule-owned immutable values shared across every hostname/match unit.
struct SharedHttpRule {
    rule_id: usize,
    rule: Arc<HTTPRouteRule>,
    runtime: HttpRuleRuntime,
    metric_handles: Arc<crate::observe::metrics::route_handles::RouteMetricHandles>,
}

/// Expand one route into its declared hostname groups, sharing units and selectors.
#[allow(clippy::too_many_arguments)] // route-build inputs have distinct lifetimes/ownership; grouping would obscure them
fn append_route_to_domains(
    domain_rules: &mut DomainRouteRulesMap,
    hostnames: &[String],
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
        path: Some(edgion_resources::default_path_match()),
        headers: None,
        query_params: None,
        method: None,
    }];

    for shared_rule in shared_rules {
        let rule_id = shared_rule.rule_id;
        let rule = &shared_rule.rule;
        // Compiled runtime is built ONCE per accepted rule;
        // the LB selector is built once per rule and shared across all of
        // this rule's match items and declared hostname groups.
        let rule_rt = &shared_rule.runtime;
        let backend_finder = HttpRouteRuleUnit::build_backend_selector(&rule_rt.backends);
        let matches = match &rule.matches {
            Some(m) if !m.is_empty() => m,
            _ => &default_matches,
        };
        for (match_id, match_item) in matches.iter().enumerate() {
            append_match_to_domains(
                domain_rules,
                hostnames,
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
        let Some(validated) = validate_http_route(route) else {
            skipped_routes += 1;
            continue;
        };
        let Some(accepted_refs) = filter_accepted_parent_refs(
            route.spec.parent_refs.as_deref(),
            route.spec.current_status.as_ref().map(|s| s.parents.as_slice()),
            Some(&validated.namespace),
            route.spec.resolved_status_controller.as_deref(),
        ) else {
            skipped_routes += 1;
            continue;
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
                if let Some(reason) = rule.request_mirror_validation_error() {
                    tracing::warn!(route = %resource_key, rule_id, %reason, "skipping invalid HTTPRoute rule");
                    return None;
                }
                let transform_violations = if route.spec.resolved_rules.is_some() {
                    edgion_resources::effective_request_transform_violations(rule)
                } else {
                    edgion_resources::authored_request_transform_violations(rule)
                };
                if !matches!(transform_violations, Ok(violations) if violations.is_empty()) {
                    return None;
                }
                // Share one runtime across every hostname and match of this rule.
                let runtime = HttpRouteRuleUnit::compile_prevalidated_http_rule(route, rule_id)?;
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
        append_route_to_domains(
            &mut domain_rules,
            &get_declared_hostnames(route),
            &validated,
            &resource_key,
            &resolved_parent_refs,
            creation_ts_ms,
            route_sv,
            &shared_rules,
            route,
        );
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
    edgion_resources::resources::refs::route_fields::HttpRouteBackendRefs::from_route(route).ok()?;
    // Controller status is advisory; malformed explicit lists never become catch-all.
    route.spec.validate_hostnames().ok()?;
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

    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default").to_owned();
    let route_name = route
        .metadata
        .name
        .clone()
        .expect("the actual Route source was validated");

    let (route_labels_meta, route_annotations_meta) = crate::ctx::extract_route_meta(&route.metadata);

    Some(ValidatedHttpRoute {
        rules,
        namespace: route_namespace,
        name: route_name,
        route_labels_meta,
        route_annotations_meta,
    })
}

/// Compile the path predicates for one touched hostname group.
fn build_route_rules(domain: &str, split: RouteRulesPair) -> Option<Arc<RouteRules>> {
    if split.0.is_empty() && split.1.is_empty() {
        return None;
    }
    let match_engine = if split.0.is_empty() {
        None
    } else {
        match RadixRouteMatchEngine::build(split.0.clone()) {
            Ok(engine) => Some(Arc::new(engine)),
            Err(e) => {
                tracing::error!(component="route_manager",domain=%domain,err=?e,"build failed");
                return None;
            }
        }
    };
    let regex_routes_engine = (!split.1.is_empty()).then(|| Arc::new(RegexRoutesEngine::build(split.1.clone())));
    let resource_keys = split
        .0
        .iter()
        .chain(&split.1)
        .map(|unit| unit.resource_key.clone())
        .collect();
    Some(Arc::new(RouteRules {
        resource_keys,
        route_rules_list: split.0,
        match_engine,
        regex_routes: split.1,
        regex_routes_engine,
    }))
}

/// Full payload replacement; an unchanged hostname set retains its tree.
pub(crate) fn build_domain_route_rules_from_routes(
    data: &HashMap<String, Arc<HTTPRoute>>,
    previous: &DomainRouteRules,
) -> DomainRouteRules {
    let mut groups = HashMap::new();
    let mut catch_all_routes = None;
    for (domain, split) in parse_http_routes_to_domain_rules(data) {
        let Some(rules) = build_route_rules(&domain, split) else {
            continue;
        };
        if domain == CATCH_ALL_HOSTNAME {
            catch_all_routes = Some(rules);
        } else if let Ok(pattern) = RadixHostnamePattern::parse(&domain) {
            groups.insert(pattern, rules);
        }
    }
    let pattern_count = groups.len();
    hostname_table_or_fail_closed(previous.hosts.with_groups(groups), catch_all_routes, pattern_count)
}

/// Recompile changed resources only, then replace only their old/new groups.
/// The caller's global cache scan discovers listener membership, not payloads.
pub(crate) fn patch_domain_route_rules(
    data: &HashMap<String, Arc<HTTPRoute>>,
    previous: &DomainRouteRules,
    changes: &HttpRouteChanges,
) -> DomainRouteRules {
    if previous.hostname_build_failed {
        return build_domain_route_rules_from_routes(data, previous);
    }
    let changed_routes = data
        .iter()
        .filter(|(key, _)| changes.resource_keys.contains(*key))
        .map(|(key, route)| (key.clone(), Arc::clone(route)))
        .collect();
    let mut additions = parse_http_routes_to_domain_rules(&changed_routes);
    let mut updates = Vec::new();
    let mut catch_all_routes = previous.catch_all_routes.clone();
    for domain in &changes.hostnames {
        let pattern = if domain == CATCH_ALL_HOSTNAME {
            None
        } else {
            let Ok(pattern) = RadixHostnamePattern::parse(domain) else {
                continue;
            };
            Some(pattern)
        };
        let prior = match &pattern {
            Some(pattern) => previous.hosts.get(pattern),
            None => previous.catch_all_routes.as_ref(),
        };
        let mut split = additions.remove(domain).unwrap_or_default();
        let removes_prior = prior.is_some_and(|rules| {
            rules
                .resource_keys
                .iter()
                .any(|key| changes.resource_keys.contains(key))
        });
        if !removes_prior && split.0.is_empty() && split.1.is_empty() {
            continue;
        }
        if let Some(prior) = prior {
            split.0.extend(
                prior
                    .route_rules_list
                    .iter()
                    .filter(|unit| !changes.resource_keys.contains(&unit.resource_key))
                    .cloned(),
            );
            split.1.extend(
                prior
                    .regex_routes
                    .iter()
                    .filter(|unit| !changes.resource_keys.contains(&unit.resource_key))
                    .cloned(),
            );
        }
        let rules = build_route_rules(domain, split);
        match pattern {
            Some(pattern) => updates.push(match rules {
                Some(rules) => RadixHostnameChange::Upsert(pattern, rules),
                None => RadixHostnameChange::Remove(pattern),
            }),
            None => catch_all_routes = rules,
        }
    }
    let pattern_count = previous.hosts.len();
    hostname_table_or_fail_closed(previous.hosts.with_changes(updates), catch_all_routes, pattern_count)
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
#[path = "../../../tests/unit/routes/http/conf_handler_impl_test.rs"]
mod tests;
