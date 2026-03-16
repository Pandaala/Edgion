use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::gateway::routes::grpc::match_unit::{GrpcMatchInfo, GrpcRouteInfo, CATCH_ALL_HOSTNAME};
use crate::core::gateway::routes::grpc::routes_mgr::{DomainGrpcRouteRules, GrpcRouteRules};
use crate::core::gateway::routes::grpc::{
    get_global_grpc_route_manager, GrpcMatchEngine, GrpcRouteManager, GrpcRouteRuleUnit,
};
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

/// Implement ConfHandler for Arc<GrpcRouteManager> to allow using the global instance
impl ConfHandler<GRPCRoute> for Arc<GrpcRouteManager> {
    fn full_set(&self, data: &HashMap<String, GRPCRoute>) {
        (**self).full_set(data)
    }

    fn partial_update(
        &self,
        add: HashMap<String, GRPCRoute>,
        update: HashMap<String, GRPCRoute>,
        remove: HashSet<String>,
    ) {
        (**self).partial_update(add, update, remove)
    }
}

/// Create a GrpcRouteManager handler for registration with ConfigClient
/// Returns the global GrpcRouteManager instance
pub fn create_grpc_route_handler() -> Box<dyn ConfHandler<GRPCRoute> + Send + Sync> {
    Box::new(get_global_grpc_route_manager())
}

/// Private helper methods for GrpcRouteManager
impl GrpcRouteManager {
    /// Parse a single GRPCRoute into route rule units.
    /// Returns empty Vec if the route has no accepted parentRefs or no rules.
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

    /// Build GrpcRouteRules from a flat list of all route units.
    fn build_engine_from_all_units(
        all_units: &HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>,
    ) -> Arc<GrpcRouteRules> {
        let mut resource_keys = HashSet::new();
        let mut route_rules_list: Vec<Arc<GrpcRouteRuleUnit>> = Vec::new();

        for (key, units) in all_units {
            if !units.is_empty() {
                resource_keys.insert(key.clone());
                route_rules_list.extend(units.iter().cloned());
            }
        }

        let match_engine = if route_rules_list.is_empty() {
            None
        } else {
            Some(Arc::new(GrpcMatchEngine::new(route_rules_list.clone())))
        };

        Arc::new(GrpcRouteRules {
            resource_keys: std::sync::RwLock::new(resource_keys),
            route_rules_list: std::sync::RwLock::new(route_rules_list),
            match_engine,
        })
    }

    /// full_set implementation — builds a single global gRPC route table.
    fn full_set(&self, data: &HashMap<String, GRPCRoute>) {
        tracing::info!(
            component = "grpc_route_manager",
            count = data.len(),
            "Full set gRPC routes"
        );

        let mut parsed_routes = HashMap::new();
        let mut new_units_cache: HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>> = HashMap::new();

        for (key, mut route) in data.clone() {
            route.preparse();
            let units = Self::parse_route_to_units(&key, &route);
            new_units_cache.insert(key.clone(), units);
            parsed_routes.insert(key, route);
        }

        let new_route_rules = Self::build_engine_from_all_units(&new_units_cache);

        *self.grpc_routes.lock().unwrap() = parsed_routes;
        *self.route_units_cache.lock().unwrap() = new_units_cache;
        let new_domain = DomainGrpcRouteRules::new();
        new_domain.grpc_routes.store(new_route_rules);
        self.global_grpc_routes.store(Arc::new(new_domain));

        tracing::info!(component = "grpc_route_manager", "global gRPC routes updated");
    }

    /// partial_update — only re-parses changed routes, reuses cached units for unchanged ones.
    fn partial_update(
        &self,
        add: HashMap<String, GRPCRoute>,
        update: HashMap<String, GRPCRoute>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "grpc_route_manager",
            add_count = add.len(),
            update_count = update.len(),
            remove_count = remove.len(),
            "Partial update gRPC routes"
        );

        let mut parsed_add_or_update = HashMap::new();
        for (key, mut route) in add.into_iter().chain(update.into_iter()) {
            route.preparse();
            parsed_add_or_update.insert(key, route);
        }

        {
            let mut grpc_routes = self.grpc_routes.lock().unwrap();
            let mut units_cache = self.route_units_cache.lock().unwrap();

            for (key, route) in parsed_add_or_update.iter() {
                let units = Self::parse_route_to_units(key, route);
                units_cache.insert(key.clone(), units);
                grpc_routes.insert(key.clone(), route.clone());
            }

            for key in &remove {
                grpc_routes.remove(key);
                units_cache.remove(key);
            }

            let new_route_rules = Self::build_engine_from_all_units(&units_cache);
            let current = self.global_grpc_routes.load();
            current.grpc_routes.store(new_route_rules);
        }

        tracing::info!(component = "grpc_route_manager", "global gRPC routes updated");
    }
}
