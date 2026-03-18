use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::gateway::routes::grpc::match_unit::{GrpcMatchInfo, GrpcRouteInfo, CATCH_ALL_HOSTNAME};
use crate::core::gateway::routes::grpc::routes_mgr::{resolved_ports_for_grpc_route, GlobalGrpcRouteManagers};
use crate::core::gateway::routes::grpc::{get_global_grpc_route_managers, GrpcRouteRuleUnit};
use crate::core::gateway::routes::http::conf_handler_impl::filter_accepted_parent_refs;
use crate::types::{GRPCRoute, ResourceMeta};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn get_effective_hostnames(route: &GRPCRoute) -> Vec<String> {
    if let Some(resolved) = &route.spec.resolved_hostnames {
        if !resolved.is_empty() {
            return resolved.clone();
        }
    }
    if let Some(hostnames) = &route.spec.hostnames {
        if !hostnames.is_empty() {
            return hostnames.clone();
        }
    }
    vec![CATCH_ALL_HOSTNAME.to_string()]
}

/// Implement ConfHandler for &'static GlobalGrpcRouteManagers
impl ConfHandler<GRPCRoute> for &'static GlobalGrpcRouteManagers {
    fn full_set(&self, data: &HashMap<String, GRPCRoute>) {
        (**self).full_set(data);
    }

    fn partial_update(
        &self,
        add: HashMap<String, GRPCRoute>,
        update: HashMap<String, GRPCRoute>,
        remove: HashSet<String>,
    ) {
        (**self).partial_update(add, update, remove);
    }
}

impl ConfHandler<GRPCRoute> for GlobalGrpcRouteManagers {
    fn full_set(&self, data: &HashMap<String, GRPCRoute>) {
        tracing::info!(
            component = "grpc_route_manager",
            count = data.len(),
            "Full set gRPC routes"
        );

        // Step 1: Parse all routes and build units cache
        self.route_cache.clear();
        let mut new_units_cache: HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>> = HashMap::new();

        for (key, route) in data {
            let mut route = route.clone();
            route.preparse();
            let units = parse_route_to_units(key, &route);
            new_units_cache.insert(key.clone(), units);
            self.route_cache.insert(key.clone(), route);
        }

        *self.route_units_cache.lock().unwrap() = new_units_cache;

        // Step 2: Rebuild all per-port managers
        self.rebuild_all_port_managers();

        tracing::info!(component = "grpc_route_manager", "full set done");
    }

    fn partial_update(
        &self,
        add: HashMap<String, GRPCRoute>,
        update: HashMap<String, GRPCRoute>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "grpc_route_manager",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update start"
        );

        let mut add_or_update = HashMap::new();
        for (key, mut route) in add.into_iter().chain(update.into_iter()) {
            route.preparse();
            add_or_update.insert(key, route);
        }

        // Compute affected ports BEFORE updating cache
        let mut affected_ports = HashSet::new();
        let mut needs_all_ports = false;

        for (key, route) in &add_or_update {
            let ports = resolved_ports_for_grpc_route(route);
            if ports.is_empty() {
                if let Some(parent_refs) = &route.spec.parent_refs {
                    let any_port = parent_refs.iter().any(|pr| pr.port.is_some());
                    if any_port {
                        for pr in parent_refs {
                            if let Some(p) = pr.port {
                                affected_ports.insert(p as u16);
                            }
                        }
                    } else {
                        needs_all_ports = true;
                    }
                } else {
                    needs_all_ports = true;
                }
            } else {
                for &port in ports {
                    affected_ports.insert(port);
                }
            }
            if let Some(old) = self.route_cache.get(key) {
                for &port in resolved_ports_for_grpc_route(old.value()) {
                    affected_ports.insert(port);
                }
            }
        }
        for key in &remove {
            if let Some(old) = self.route_cache.get(key) {
                let ports = resolved_ports_for_grpc_route(old.value());
                if ports.is_empty() {
                    needs_all_ports = true;
                } else {
                    for &port in ports {
                        affected_ports.insert(port);
                    }
                }
            }
        }
        if needs_all_ports {
            for entry in self.by_port.iter() {
                affected_ports.insert(*entry.key());
            }
        }

        // Update route cache and units cache
        {
            let mut units_cache = self.route_units_cache.lock().unwrap();
            for (key, route) in &add_or_update {
                let units = parse_route_to_units(key, route);
                units_cache.insert(key.clone(), units);
                self.route_cache.insert(key.clone(), route.clone());
            }
            for key in &remove {
                self.route_cache.remove(key);
                units_cache.remove(key);
            }
        }

        // Rebuild affected port managers
        self.rebuild_affected_port_managers(&affected_ports);

        tracing::info!(component = "grpc_route_manager", "partial update done");
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
        route.spec.parent_refs.as_ref(),
        route.status.as_ref().map(|s| s.parents.as_slice()),
        Some(route_namespace),
    ) {
        Some(refs) => refs,
        None => return Vec::new(),
    };

    let route_sv = route.get_sync_version();
    let route_info = Arc::new(GrpcRouteInfo {
        parent_refs: Some(accepted_refs),
        effective_hostnames: get_effective_hostnames(route),
    });

    let mut units = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for (rule_id, rule) in rules.iter().enumerate() {
            let rule_arc = Arc::new(rule.clone());

            if let Some(matches) = &rule.matches {
                for (match_id, match_item) in matches.iter().enumerate() {
                    units.push(Arc::new(GrpcRouteRuleUnit {
                        resource_key: resource_key.to_string(),
                        matched_info: GrpcMatchInfo::new(
                            route_namespace.to_string(),
                            route_name.to_string(),
                            rule_id,
                            match_id,
                            match_item.clone(),
                            route_sv,
                        ),
                        rule: rule_arc.clone(),
                        route_info: route_info.clone(),
                    }));
                }
            }
        }
    }
    units
}
