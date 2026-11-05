use crate::routes::common::{
    effective_hostnames, filter_accepted_parent_refs, gateways_from_parent_refs, resolve_parent_refs,
};
use crate::routes::delegation::DelegationSlotTable;
use crate::routes::grpc::delegation_build::is_grpc_delegation_child;
use crate::routes::grpc::match_unit::{GrpcMatchInfo, GrpcMatchSpecificity, GrpcRouteInfo};
use crate::routes::grpc::routes_mgr::{resolved_listeners_for_grpc_route, GlobalGrpcRouteManagers};
use crate::routes::grpc::{get_global_grpc_route_managers, GrpcMatchEngine, GrpcRouteRuleUnit};
use crate::routes::http::delegation_build::process_gateway_identities;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::route_delegation::{
    delegation_target_of_grpc_rule, key_for, standard_filter_mask_grpc, RouteKindName,
};
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
        let previous: Vec<Arc<GRPCRoute>> = self.route_cache.iter().map(|e| Arc::clone(e.value())).collect();
        self.route_cache.clear();
        let mut new_units_cache: HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>> = HashMap::new();

        for (key, route) in data {
            let units = parse_route_to_units(key, route, &self.delegation_slots);
            new_units_cache.insert(key.clone(), units);
            self.route_cache.insert(key.clone(), route.clone()); // Arc clone
        }

        *self.route_units_cache.lock() = new_units_cache;

        // Step 2: Rebuild all per-listener managers (lock already held above).
        self.rebuild_all_listener_managers_locked();

        // Step 3: Rebuild every delegation slot from scratch.
        let slot_garbage = self.rebuild_all_delegation_locked(previous.iter(), data, process_gateway_identities);

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
        // After the previous tables are released, so dropped parents no longer pin slots.
        drop(previous);
        self.collect_delegation_slots(&slot_garbage);
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
                if !is_grpc_delegation_child(route) {
                    tracing::warn!(route = %key, "GRPCRoute has no resolved_listeners during partial_update");
                }
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

        // Previous generations of every changed route, for delegation routing.
        let delegation_changes: Vec<(String, Option<Arc<GRPCRoute>>)> = add_or_update
            .keys()
            .chain(&remove)
            .map(|key| (key.clone(), self.route_cache.get(key).map(|e| Arc::clone(e.value()))))
            .collect();

        // Update route cache and units cache
        {
            let mut units_cache = self.route_units_cache.lock();
            for (key, route) in &add_or_update {
                let units = parse_route_to_units(key, route, &self.delegation_slots);
                units_cache.insert(key.clone(), units);
                self.route_cache.insert(key.clone(), route.clone()); // Arc clone
            }
            for key in &remove {
                self.route_cache.remove(key);
                units_cache.remove(key);
            }
        }

        // Rebuild affected listener managers. Child-only changes leave
        // `affected_listeners` empty: no listener rebuild.
        self.rebuild_affected_listener_managers(&affected_listeners);
        let slot_garbage = self.apply_delegation_changes_locked(&delegation_changes, process_gateway_identities);

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
        drop(delegation_changes);
        self.collect_delegation_slots(&slot_garbage);
    }
}

/// Create a GrpcRouteManager handler for registration with ConfigClient
pub fn create_grpc_route_handler() -> Box<dyn ConfHandler<GRPCRoute> + Send + Sync> {
    Box::new(get_global_grpc_route_managers())
}

#[cfg(test)]
include!("../../../tests/support/routes/grpc/conf_handler_impl_test/147_function_item.rs");

/// Parse a single root GRPCRoute into listener-bucketed rule units.
/// Delegation children yield no units here; they are compiled into slots.
fn parse_route_to_units(
    resource_key: &str,
    route: &Arc<GRPCRoute>,
    slots: &DelegationSlotTable<GrpcMatchEngine>,
) -> Vec<Arc<GrpcRouteRuleUnit>> {
    if !route_compiles(resource_key, route) || is_grpc_delegation_child(route) {
        return Vec::new();
    }
    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");

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
    let route_info = Arc::new(GrpcRouteInfo {
        parent_refs: Some(resolve_parent_refs(&accepted_refs, route_namespace)),
        effective_hostnames: effective_hostnames(
            route.spec.resolved_hostnames.as_deref(),
            route.spec.hostnames.as_deref(),
        ),
    });
    compile_route_units(resource_key, route, route_info, &attached_gateways, slots)
}

/// Compile one delegation child's units once. The units carry the child's own
/// namespace/identity and no hostname or Gateway eligibility (the root level
/// decides those); they are `Arc`-shared into every subtree whose key the
/// child holds. Leaf metric handles are prebuilt for every process Gateway.
pub(super) fn compile_child_units(
    route: &Arc<GRPCRoute>,
    slots: &DelegationSlotTable<GrpcMatchEngine>,
    process_gateways: &[(String, String)],
) -> Vec<Arc<GrpcRouteRuleUnit>> {
    let resource_key = route.key_name();
    if !route_compiles(&resource_key, route) {
        return Vec::new();
    }
    let route_info = Arc::new(GrpcRouteInfo {
        parent_refs: None,
        effective_hostnames: Vec::new(),
    });
    compile_route_units(&resource_key, route, route_info, process_gateways, slots)
}

/// Route-level checks shared by roots and children.
fn route_compiles(resource_key: &str, route: &GRPCRoute) -> bool {
    if edgion_resources::resources::refs::route_fields::GrpcRouteBackendRefs::from_route(route).is_err() {
        return false;
    }
    if let Err(error) = route.spec.validate_hostnames() {
        tracing::warn!(component = "grpc_route_manager", route = resource_key, %error,
            "Invalid GRPCRoute hostnames; skipping the complete route");
        return false;
    }
    true
}

/// Compile every accepted rule of one route into match units. A delegating
/// rule obtains its target slot here (compile time) and compiles no backends,
/// so the request path never looks a key up.
fn compile_route_units(
    resource_key: &str,
    route: &Arc<GRPCRoute>,
    route_info: Arc<GrpcRouteInfo>,
    metric_gateways: &[(String, String)],
    slots: &DelegationSlotTable<GrpcMatchEngine>,
) -> Vec<Arc<GrpcRouteRuleUnit>> {
    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
    let route_name = route.metadata.name.as_deref().unwrap_or("");
    let route_sv = route.get_sync_version();
    let (route_labels_meta, route_annotations_meta) = crate::ctx::extract_route_meta(&route.metadata);
    let creation_ts_ms = route
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| t.0.timestamp_millis())
        .unwrap_or(0);

    let invalid_rule_indices = route.spec.invalid_rule_indices.as_ref();

    let mut units = Vec::new();
    if let Some(rules) = route.spec.rules.as_ref() {
        for (rule_id, rule) in rules.iter().enumerate() {
            // Skip rules marked invalid by Controller
            if invalid_rule_indices.is_some_and(|inv| inv.contains(&rule_id)) {
                continue;
            }
            // Delegation shape is re-checked locally, not trusted from the Controller.
            let target = match delegation_target_of_grpc_rule(rule, route_namespace) {
                None => None,
                Some(Ok(target)) => Some(target),
                Some(Err(reason)) => {
                    tracing::warn!(route = %resource_key, rule_id, %reason,
                        "skipping malformed delegating GRPCRoute rule");
                    continue;
                }
            };
            // Compile matchers once, before allocating rule runtime state. An
            // invalid method predicate withdraws its entire rule on both planes,
            // including input whose Controller status is absent or stale.
            // A matchless rule is a catch-all; inside a delegation subtree that
            // is safe because the parent level already fixed the service.
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
            let Some(mut rule_runtime) = GrpcRouteRuleUnit::compile_grpc_rule(route, rule_id) else {
                continue;
            };
            let delegate = target.map(|target| {
                slots.get_or_create(&key_for(RouteKindName::GRPCRoute, route_namespace, route_name, &target))
            });
            // A delegating rule has no backends of its own (backends: leaf only).
            let metric_backends = if delegate.is_some() {
                rule_runtime.backends = Arc::from(Vec::new());
                Vec::new()
            } else {
                grpc_backends_from_rule(rule, route_namespace)
            };
            let filter_mask = standard_filter_mask_grpc(rule);
            let backend_finder = GrpcRouteRuleUnit::build_backend_selector(&rule_runtime.backends);
            let metric_handles = Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::build(
                metric_gateways,
                route_namespace,
                route_name,
                &metric_backends,
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
                    delegate: delegate.clone(),
                    filter_mask,
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
