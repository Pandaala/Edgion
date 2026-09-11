use crate::routes::common::{
    effective_hostnames, filter_accepted_parent_refs, gateways_from_parent_refs, resolve_parent_refs,
};
use crate::routes::grpc::match_unit::{GrpcMatchInfo, GrpcMatchSpecificity, GrpcRouteInfo, GrpcRuleRuntime};
use crate::routes::grpc::routes_mgr::{resolved_listeners_for_grpc_route, GlobalGrpcRouteManagers};
use crate::routes::grpc::{get_global_grpc_route_managers, GrpcRouteRuleUnit};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::{GRPCRoute, GRPCRouteRule, ListenerKey, ResourceMeta};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

impl ConfHandler<GRPCRoute> for GlobalGrpcRouteManagers {
    fn full_set(&self, data: &HashMap<String, Arc<GRPCRoute>>) {
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        let start_time = Instant::now();
        tracing::info!(component = "grpc_route_manager", count = data.len(), "full set start");

        // Step 1: Parse all routes and build units cache
        self.route_cache.clear();
        let mut new_units_cache: HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>> = HashMap::new();

        for (key, route) in data {
            let units = parse_route_to_units(key, route);
            new_units_cache.insert(key.clone(), units);
            self.route_cache.insert(key.clone(), route.clone()); // Arc clone
        }

        *self.route_units_cache.lock() = new_units_cache;

        // Step 2: Rebuild all per-listener managers (lock already held above).
        self.rebuild_all_listener_managers_locked();

        tracing::info!(
            component = "grpc_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "full set done"
        );
        drop(_guard);
        crate::plugins::publication::global_publisher().replace_routes(
            crate::plugins::publication::inputs::RouteKind::Grpc,
            data.iter()
                .map(|(key, route)| {
                    (
                        key.clone(),
                        crate::plugins::publication::inputs::RouteInput::from_grpc(route),
                    )
                })
                .collect(),
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<GRPCRoute>>,
        update: HashMap<String, Arc<GRPCRoute>>,
        remove: HashSet<String>,
    ) {
        let policy_inputs = add
            .iter()
            .chain(update.iter())
            .map(|(key, route)| {
                (
                    key.clone(),
                    crate::plugins::publication::inputs::RouteInput::from_grpc(route),
                )
            })
            .collect();
        let policy_removed = remove.clone();
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        let start_time = Instant::now();
        tracing::info!(
            component = "grpc_route_manager",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update start"
        );

        let mut add_or_update = add;
        add_or_update.extend(update);

        // Compute affected listeners BEFORE updating cache
        let mut affected_listeners: HashSet<ListenerKey> = HashSet::new();

        for (key, route) in &add_or_update {
            let listeners = resolved_listeners_for_grpc_route(route);
            if listeners.is_empty() {
                tracing::warn!(route = %key, "GRPCRoute has no resolved_listeners during partial_update");
            } else {
                for listener in listeners {
                    affected_listeners.insert(listener.clone());
                }
            }
            if let Some(old) = self.route_cache.get(key) {
                for listener in resolved_listeners_for_grpc_route(old.value()) {
                    affected_listeners.insert(listener.clone());
                }
            }
        }
        for key in &remove {
            if let Some(old) = self.route_cache.get(key) {
                for listener in resolved_listeners_for_grpc_route(old.value()) {
                    affected_listeners.insert(listener.clone());
                }
            }
        }

        // Update route cache and units cache
        {
            let mut units_cache = self.route_units_cache.lock();
            for (key, route) in &add_or_update {
                let units = parse_route_to_units(key, route);
                units_cache.insert(key.clone(), units);
                self.route_cache.insert(key.clone(), route.clone()); // Arc clone
            }
            for key in &remove {
                self.route_cache.remove(key);
                units_cache.remove(key);
            }
        }

        // Rebuild affected listener managers
        self.rebuild_affected_listener_managers(&affected_listeners);

        tracing::info!(
            component = "grpc_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "partial update done"
        );
        drop(_guard);
        crate::plugins::publication::global_publisher().update_routes(
            crate::plugins::publication::inputs::RouteKind::Grpc,
            policy_inputs,
            &policy_removed,
        );
    }
}

/// Create a GrpcRouteManager handler for registration with ConfigClient
pub fn create_grpc_route_handler() -> Box<dyn ConfHandler<GRPCRoute> + Send + Sync> {
    Box::new(get_global_grpc_route_managers())
}

/// Parse a single GRPCRoute into route rule units.
fn parse_route_to_units(resource_key: &str, route: &Arc<GRPCRoute>) -> Vec<Arc<GrpcRouteRuleUnit>> {
    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
    let route_name = route.metadata.name.as_deref().unwrap_or("");

    let accepted_refs = match filter_accepted_parent_refs(
        route.spec.parent_refs.as_deref(),
        route.status.as_ref().map(|s| s.parents.as_slice()),
        Some(route_namespace),
    ) {
        Some(refs) => refs,
        None => return Vec::new(),
    };

    // Derive attached gateway pairs before accepted_refs is moved into route_info.
    let attached_gateways = gateways_from_parent_refs(&accepted_refs, route_namespace);

    let route_sv = route.get_sync_version();
    let (route_labels_meta, route_annotations_meta) = crate::ctx::extract_route_meta(&route.metadata);
    let creation_ts_ms = route
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| t.0.timestamp_millis())
        .unwrap_or(0);
    let route_info = Arc::new(GrpcRouteInfo {
        parent_refs: Some(resolve_parent_refs(&accepted_refs, route_namespace)),
        effective_hostnames: effective_hostnames(
            route.spec.resolved_hostnames.as_deref(),
            route.spec.hostnames.as_deref(),
        ),
    });

    // Delegation: when the controller flattened a delegation tree into
    // resolved_rules, compile those INSTEAD of the operator-authored rules.
    // invalid_rule_indices refers to spec.rules; when resolved_rules is
    // present, flattening already dropped invalid rules and the indices do
    // not apply.
    let invalid_rule_indices = if route.spec.resolved_rules.is_some() {
        None
    } else {
        route.spec.invalid_rule_indices.as_ref()
    };

    let mut units = Vec::new();
    if let Some(rules) = route.effective_rules() {
        for (rule_id, rule) in rules.iter().enumerate() {
            // Skip rules marked invalid by Controller
            if invalid_rule_indices.is_some_and(|inv| inv.contains(&rule_id)) {
                continue;
            }
            let rule_arc = Arc::new(rule.clone());

            // Compile rule runtime ONCE per rule (plugin chains + compiled backends).
            // All match-item units of this rule share the same Arc fields so that
            // in-memory RateLimit CMS state is never split across units.
            let rule_runtime: GrpcRuleRuntime = GrpcRouteRuleUnit::compile_grpc_rule(rule, route_namespace);
            let rule_plugin_runtime = rule_runtime.plugin_runtime;
            let rule_backends = rule_runtime.backends;
            let backend_finder = GrpcRouteRuleUnit::build_backend_selector(&rule_backends);
            let metric_handles = Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::build(
                &attached_gateways,
                route_namespace,
                route_name,
                &grpc_backends_from_rule(rule, route_namespace),
            ));

            let effective_matches: &[edgion_resources::GRPCRouteMatch] = rule.matches.as_deref().unwrap_or(&[]);

            if effective_matches.is_empty() {
                // Per Gateway API spec, a rule with no matches (or an empty matches list)
                // matches all gRPC traffic.  Synthesise a single catch-all unit so that
                // `has_routes()` returns true for the listener and unmatched-hostname
                // requests are correctly answered with UNIMPLEMENTED(12).
                let specificity = GrpcMatchSpecificity {
                    header_count: 0,
                    creation_ts_ms,
                    route_key: resource_key.to_string(),
                    rule_index: rule_id as u32,
                    match_index: 0,
                };
                units.push(Arc::new(GrpcRouteRuleUnit {
                    resource_key: resource_key.to_string(),
                    matched_info: GrpcMatchInfo::new(
                        route_namespace.to_string(),
                        route_name.to_string(),
                        rule_id,
                        0,
                        edgion_resources::GRPCRouteMatch {
                            method: None,
                            headers: None,
                        },
                        route_sv,
                        rule.name.clone(),
                    )
                    .with_route_meta(route_labels_meta.clone(), route_annotations_meta.clone()),
                    rule: rule_arc.clone(),
                    route: Some(route.clone()),
                    route_info: route_info.clone(),
                    specificity,
                    metric_handles: metric_handles.clone(),
                    plugin_runtime: rule_plugin_runtime.clone(),
                    backend_finder: backend_finder.clone(),
                    backends: rule_backends.clone(),
                }));
            } else {
                for (match_id, match_item) in effective_matches.iter().enumerate() {
                    let header_count = match_item.headers.as_ref().map(|h| h.len() as u16).unwrap_or(0);
                    let specificity = GrpcMatchSpecificity {
                        header_count,
                        creation_ts_ms,
                        route_key: resource_key.to_string(),
                        rule_index: rule_id as u32,
                        match_index: match_id as u32,
                    };
                    units.push(Arc::new(GrpcRouteRuleUnit {
                        resource_key: resource_key.to_string(),
                        matched_info: GrpcMatchInfo::new(
                            route_namespace.to_string(),
                            route_name.to_string(),
                            rule_id,
                            match_id,
                            match_item.clone(),
                            route_sv,
                            rule.name.clone(),
                        )
                        .with_route_meta(route_labels_meta.clone(), route_annotations_meta.clone()),
                        rule: rule_arc.clone(),
                        route: Some(route.clone()),
                        route_info: route_info.clone(),
                        specificity,
                        metric_handles: metric_handles.clone(),
                        plugin_runtime: rule_plugin_runtime.clone(),
                        backend_finder: backend_finder.clone(),
                        backends: rule_backends.clone(),
                    }));
                }
            }
        }
    }
    units
}

/// Extract `(backend_ns, backend_name)` tuples in `backend_refs` order, with
/// fallback to `route_ns` when the backend ref omits its namespace.
fn grpc_backends_from_rule(rule: &GRPCRouteRule, route_ns: &str) -> Vec<(String, String)> {
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

    /// Build a minimal GRPCRoute suitable for unit tests.
    ///
    /// The route has a single parent ref (so `filter_accepted_parent_refs` returns
    /// `Some`) and one catch-all rule with a backend ref (so `parse_route_to_units`
    /// produces at least one unit).
    fn build_test_grpc_route(namespace: &str, name: &str) -> GRPCRoute {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "namespace": namespace, "name": name },
            "spec": {
                "parentRefs": [{ "name": "test-gw", "kind": "Gateway" }],
                "rules": [{
                    "backendRefs": [{ "name": "svc1", "port": 8080 }]
                }]
            }
        });
        let mut route: GRPCRoute = serde_json::from_value(json).expect("Failed to build GRPCRoute");
        route.pre_parse();
        route
    }

    /// Build a GRPCRoute with two `spec.rules`, each with a distinct backend
    /// name, so tests can tell which rule(s) actually compiled.
    fn build_grpc_route_with_two_rules(namespace: &str, name: &str) -> GRPCRoute {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "namespace": namespace, "name": name },
            "spec": {
                "parentRefs": [{ "name": "test-gw", "kind": "Gateway" }],
                "rules": [
                    { "matches": [{ "method": { "service": "svc.SpecA" } }],
                      "backendRefs": [{ "name": "spec-svc-a", "port": 8080 }] },
                    { "matches": [{ "method": { "service": "svc.SpecB" } }],
                      "backendRefs": [{ "name": "spec-svc-b", "port": 8080 }] }
                ]
            }
        });
        let mut route: GRPCRoute = serde_json::from_value(json).expect("Failed to build GRPCRoute");
        route.pre_parse();
        route
    }

    /// Extract backend Service names from every unit's compiled rule, so
    /// tests can assert exactly which backendRefs were compiled.
    fn unit_backend_names(units: &[Arc<GrpcRouteRuleUnit>]) -> Vec<String> {
        let mut names: Vec<String> = units
            .iter()
            .flat_map(|u| u.rule.backend_refs.iter().flatten())
            .map(|br| br.name.clone())
            .collect();
        names.sort();
        names
    }

    /// Build-site wiring, gRPC side. Mirrors the HTTP test in
    /// `routes::http::conf_handler_impl`: only this test would fail if
    /// `parse_route_to_units` ever handed `resolve_parent_refs` something other
    /// than the route namespace that the request path later passes as `route_ns`.
    ///
    /// gRPC uses its own `unwrap_or("default")` fallback, independent of HTTP's
    /// stricter "reject a route with no namespace" rule, so it needs its own guard.
    #[test]
    fn build_site_resolves_parent_ref_keys_with_the_route_namespace() {
        // The fixture declares `parentRefs: [{ name: "test-gw" }]` with no
        // namespace, so the resolved key must fall back to the route namespace.
        let route = build_grpc_route_with_two_rules("route-ns", "wiring");
        let units = parse_route_to_units("route-ns/wiring", &Arc::new(route));
        assert!(!units.is_empty(), "the fixture must compile to at least one unit");

        for unit in &units {
            let resolved = unit
                .route_info
                .parent_refs
                .as_ref()
                .expect("the build site always populates parent_refs");
            assert_eq!(resolved.len(), 1);
            assert_eq!(
                resolved[0].gateway_key, "route-ns/test-gw",
                "the omitted parentRef namespace must fall back to the ROUTE namespace"
            );
            assert_eq!(resolved[0].parent_ns, "route-ns");
            // The same namespace the request path passes as `route_ns`.
            assert_eq!(unit.matched_info.rsc.ns, "route-ns");
        }
    }

    #[test]
    fn test_grpc_resolved_rules_compiled_instead_of_spec_rules() {
        // Delegation contract: when resolved_rules is present the gateway
        // compiles it INSTEAD of spec.rules, including a resolved Service
        // that is absent from spec.rules entirely (the flattened child).
        let mut route = build_grpc_route_with_two_rules("test-grpc-deleg", "flat");
        route.spec.resolved_rules = Some(
            serde_json::from_value(serde_json::json!([
                { "matches": [{ "method": { "service": "svc.Resolved" } }],
                  "backendRefs": [{ "name": "resolved-svc", "namespace": "test-grpc-deleg", "port": 8080 }] }
            ]))
            .unwrap(),
        );
        route.pre_parse();

        let route = Arc::new(route);
        let units = parse_route_to_units("test-grpc-deleg/flat", &route);
        assert_eq!(
            unit_backend_names(&units),
            vec!["resolved-svc"],
            "resolved_rules must replace spec.rules, compiling the resolved service"
        );
    }

    #[test]
    fn test_grpc_invalid_rule_indices_ignored_when_resolved_rules_present() {
        // invalid_rule_indices refers to spec.rules; a flattened route must
        // not have its (pre-filtered) resolved_rules dropped by stale indices.
        let mut route = build_grpc_route_with_two_rules("test-grpc-deleg-inv", "flat");
        route.spec.invalid_rule_indices = Some(vec![0]);
        route.spec.resolved_rules = Some(
            serde_json::from_value(serde_json::json!([
                { "matches": [{ "method": { "service": "svc.RA" } }],
                  "backendRefs": [{ "name": "svc-r-a", "port": 8080 }] },
                { "matches": [{ "method": { "service": "svc.RB" } }],
                  "backendRefs": [{ "name": "svc-r-b", "port": 8080 }] }
            ]))
            .unwrap(),
        );
        route.pre_parse();

        let route = Arc::new(route);
        let units = parse_route_to_units("test-grpc-deleg-inv/flat", &route);
        assert_eq!(
            unit_backend_names(&units),
            vec!["svc-r-a", "svc-r-b"],
            "invalid_rule_indices (index 0) must not apply to resolved_rules"
        );

        // Control: without resolved_rules the same indices DO drop rule 0.
        let mut plain = build_grpc_route_with_two_rules("test-grpc-deleg-inv", "plain");
        plain.spec.invalid_rule_indices = Some(vec![0]);
        let plain = Arc::new(plain);
        let units = parse_route_to_units("test-grpc-deleg-inv/plain", &plain);
        assert_eq!(
            unit_backend_names(&units),
            vec!["spec-svc-b"],
            "indices must still apply to spec.rules"
        );
    }

    #[test]
    fn test_grpc_full_set_shares_arc_and_unit_carries_route() {
        let mgr = GlobalGrpcRouteManagers::new();
        let key = "test-grpc-arc/route1";
        let route = std::sync::Arc::new(build_test_grpc_route("test-grpc-arc", "route1"));
        let mut data = std::collections::HashMap::new();
        data.insert(key.to_string(), route.clone());
        mgr.full_set(&data);

        let cached = mgr.route_cache.get(key).expect("cached");
        assert!(std::sync::Arc::ptr_eq(cached.value(), &route), "cache shares Arc");

        let units = mgr.route_units_cache.lock();
        let unit = units.get(key).and_then(|v| v.first()).expect("a unit");
        assert!(
            std::sync::Arc::ptr_eq(unit.route.as_ref().expect("unit route"), &route),
            "unit shares the cache Arc"
        );
    }

    #[test]
    fn metric_handles_are_shared_within_each_rule_only() {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "namespace": "route-ns", "name": "metric-sharing" },
            "spec": {
                "parentRefs": [{ "name": "test-gw", "kind": "Gateway" }],
                "rules": [
                    {
                        "matches": [
                            { "method": { "service": "svc.RuleA", "method": "One" } },
                            { "method": { "service": "svc.RuleA", "method": "Two" } }
                        ],
                        "backendRefs": [
                            { "name": "rule-a-primary", "port": 8080 },
                            { "name": "rule-a-secondary", "port": 8081 }
                        ]
                    },
                    {
                        "matches": [{ "method": { "service": "svc.RuleB", "method": "Only" } }],
                        "backendRefs": [{ "name": "rule-b", "port": 9090 }]
                    }
                ]
            }
        });
        let mut route: GRPCRoute = serde_json::from_value(json).expect("Failed to build GRPCRoute");
        route.pre_parse();

        let units = parse_route_to_units("route-ns/metric-sharing", &Arc::new(route));
        assert_eq!(units.len(), 3);
        assert!(Arc::ptr_eq(&units[0].metric_handles, &units[1].metric_handles));
        assert!(!Arc::ptr_eq(&units[0].metric_handles, &units[2].metric_handles));

        assert_eq!((units[0].matched_info.rule_id, units[0].matched_info.match_id), (0, 0));
        assert_eq!((units[1].matched_info.rule_id, units[1].matched_info.match_id), (0, 1));
        assert_eq!((units[2].matched_info.rule_id, units[2].matched_info.match_id), (1, 0));
        let matched_methods: Vec<_> = units
            .iter()
            .map(|unit| {
                let method = unit.matched_info.matched.method.as_ref().expect("method match");
                (method.service.as_deref(), method.method.as_deref())
            })
            .collect();
        assert_eq!(
            matched_methods,
            vec![
                (Some("svc.RuleA"), Some("One")),
                (Some("svc.RuleA"), Some("Two")),
                (Some("svc.RuleB"), Some("Only")),
            ]
        );

        let first_rule_metrics = units[0]
            .metric_handles
            .lookup("route-ns", "test-gw")
            .expect("first rule gateway metrics");
        let second_rule_metrics = units[2]
            .metric_handles
            .lookup("route-ns", "test-gw")
            .expect("second rule gateway metrics");
        assert_eq!(first_rule_metrics.counters.len(), 2);
        assert_eq!(second_rule_metrics.counters.len(), 1);
    }
}
