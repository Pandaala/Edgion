use crate::routes::common::{effective_hostnames, filter_accepted_parent_refs, gateways_from_parent_refs};
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
            self.route_cache.insert(key.clone(), (**route).clone());
        }

        *self.route_units_cache.lock() = new_units_cache;

        // Step 2: Rebuild all per-listener managers (lock already held above).
        self.rebuild_all_listener_managers_locked();

        tracing::info!(
            component = "grpc_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "full set done"
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<GRPCRoute>>,
        update: HashMap<String, Arc<GRPCRoute>>,
        remove: HashSet<String>,
    ) {
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
                self.route_cache.insert(key.clone(), (**route).clone());
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
    }
}

/// Create a GrpcRouteManager handler for registration with ConfigClient
pub fn create_grpc_route_handler() -> Box<dyn ConfHandler<GRPCRoute> + Send + Sync> {
    Box::new(get_global_grpc_route_managers())
}

/// Parse a single GRPCRoute into route rule units.
fn parse_route_to_units(resource_key: &str, route: &GRPCRoute) -> Vec<Arc<GrpcRouteRuleUnit>> {
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
    let route_labels = crate::extract_log_labels(&route.metadata);
    let creation_ts_ms = route
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| t.0.timestamp_millis())
        .unwrap_or(0);
    let route_info = Arc::new(GrpcRouteInfo {
        parent_refs: Some(accepted_refs),
        effective_hostnames: effective_hostnames(
            route.spec.resolved_hostnames.as_deref(),
            route.spec.hostnames.as_deref(),
        ),
    });

    let mut units = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for (rule_id, rule) in rules.iter().enumerate() {
            // Skip rules marked invalid by Controller
            if route
                .spec
                .invalid_rule_indices
                .as_ref()
                .is_some_and(|inv| inv.contains(&rule_id))
            {
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
                        route_labels.clone(),
                    ),
                    rule: rule_arc.clone(),
                    route_info: route_info.clone(),
                    specificity,
                    metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::build(
                        &attached_gateways,
                        route_namespace,
                        route_name,
                        &grpc_backends_from_rule(rule, route_namespace),
                    )),
                    plugin_runtime: rule_plugin_runtime.clone(),
                    backend_finder: backend_finder.clone(),
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
                            route_labels.clone(),
                        ),
                        rule: rule_arc.clone(),
                        route_info: route_info.clone(),
                        specificity,
                        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::build(
                            &attached_gateways,
                            route_namespace,
                            route_name,
                            &grpc_backends_from_rule(rule, route_namespace),
                        )),
                        plugin_runtime: rule_plugin_runtime.clone(),
                        backend_finder: backend_finder.clone(),
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
