use crate::routes::common::{
    effective_hostnames, filter_accepted_parent_refs, gateways_from_parent_refs, resolve_parent_refs,
};
use crate::routes::grpc::match_unit::{GrpcMatchInfo, GrpcMatchSpecificity, GrpcRouteInfo};
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
        crate::plugins::publication::global_publisher().replace_grpc_routes(
            data.iter()
                .map(|(key, route)| {
                    (
                        key.clone(),
                        crate::plugins::publication::inputs::RouteInput::from_grpc(route),
                    )
                })
                .collect(),
            self.publication_tables(),
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
        crate::plugins::publication::global_publisher().update_grpc_routes(
            policy_inputs,
            &policy_removed,
            self.publication_tables(),
        );
    }
}

/// Create a GrpcRouteManager handler for registration with ConfigClient
pub fn create_grpc_route_handler() -> Box<dyn ConfHandler<GRPCRoute> + Send + Sync> {
    Box::new(get_global_grpc_route_managers())
}

#[cfg(test)]
include!("../../../tests/support/routes/grpc/conf_handler_impl_test/147_function_item.rs");

/// Parse a single GRPCRoute into route rule units.
fn parse_route_to_units(resource_key: &str, route: &Arc<GRPCRoute>) -> Vec<Arc<GrpcRouteRuleUnit>> {
    if edgion_resources::resources::refs::route_fields::GrpcRouteBackendRefs::from_route(route).is_err() {
        return Vec::new();
    }
    if let Err(error) = route.spec.validate_hostnames() {
        tracing::warn!(component = "grpc_route_manager", route = resource_key, %error,
            "Invalid GRPCRoute hostnames; skipping the complete route");
        return Vec::new();
    }
    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
    let route_name = route.metadata.name.as_deref().unwrap_or("");

    let Some(accepted_refs) = filter_accepted_parent_refs(
        route.spec.parent_refs.as_deref(),
        route.spec.current_status.as_ref().map(|s| s.parents.as_slice()),
        Some(route_namespace),
        route.spec.resolved_status_controller.as_deref(),
    ) else {
        return Vec::new();
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
            // Compile matchers once, before allocating rule runtime state. An
            // invalid method predicate withdraws its entire rule on both planes,
            // including input whose Controller status is absent or stale.
            let catch_all = edgion_resources::GRPCRouteMatch {
                method: None,
                headers: None,
            };
            let effective_matches = rule
                .matches
                .as_deref()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| std::slice::from_ref(&catch_all));
            let matched_infos: Vec<_> = effective_matches
                .iter()
                .enumerate()
                .map(|(match_id, matched)| {
                    GrpcMatchInfo::new(
                        route_namespace.to_string(),
                        route_name.to_string(),
                        rule_id,
                        match_id,
                        matched.clone(),
                        route_sv,
                        rule.name.clone(),
                    )
                    .with_route_meta(route_labels_meta.clone(), route_annotations_meta.clone())
                })
                .collect();
            if matched_infos.iter().any(|info| info.compiled_method_match.is_none()) {
                continue;
            }
            let rule_arc = Arc::new(rule.clone());
            let Some(rule_runtime) = GrpcRouteRuleUnit::compile_grpc_rule(route, rule_id) else {
                continue;
            };
            let backend_finder = GrpcRouteRuleUnit::build_backend_selector(&rule_runtime.backends);
            let metric_handles = Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::build(
                &attached_gateways,
                route_namespace,
                route_name,
                &grpc_backends_from_rule(rule, route_namespace),
            ));

            for matched_info in matched_infos {
                let specificity = GrpcMatchSpecificity {
                    header_count: matched_info
                        .matched
                        .headers
                        .as_ref()
                        .map_or(0, |headers| headers.len() as u16),
                    creation_ts_ms,
                    route_key: resource_key.to_string(),
                    rule_index: rule_id as u32,
                    match_index: matched_info.match_id as u32,
                };
                units.push(Arc::new(GrpcRouteRuleUnit {
                    session_persistence_ttl_secs: rule_runtime.session_persistence_ttl_secs,
                    resource_key: resource_key.to_string(),
                    matched_info,
                    rule: Arc::clone(&rule_arc),
                    route: Some(Arc::clone(route)),
                    route_info: Arc::clone(&route_info),
                    specificity,
                    metric_handles: Arc::clone(&metric_handles),
                    plugin_runtime: Arc::clone(&rule_runtime.plugin_runtime),
                    backend_finder: Arc::clone(&backend_finder),
                    backends: Arc::clone(&rule_runtime.backends),
                }));
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
#[path = "../../../tests/unit/routes/grpc/conf_handler_impl_test.rs"]
mod tests;
