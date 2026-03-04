use crate::core::conf_sync::traits::ConfHandler;
use crate::core::routes::grpc_routes::match_unit::{GrpcMatchInfo, GrpcRouteInfo};
use crate::core::routes::grpc_routes::routes_mgr::{DomainGrpcRouteRules, GrpcRouteRules};
use crate::core::routes::grpc_routes::{
    get_global_grpc_route_manager, GrpcMatchEngine, GrpcRouteManager, GrpcRouteRuleUnit,
};
use crate::types::GRPCRoute;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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
    /// Build global GrpcRouteRules from all stored routes.
    fn build_global_routes(&self, all_routes: &HashMap<String, GRPCRoute>) -> Arc<GrpcRouteRules> {
        let mut resource_keys = HashSet::new();
        let mut route_rules_list: Vec<Arc<GrpcRouteRuleUnit>> = Vec::new();

        for (resource_key, route) in all_routes.iter() {
            resource_keys.insert(resource_key.clone());

            let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
            let route_name = route.metadata.name.as_deref().unwrap_or("");

            let route_info = Arc::new(GrpcRouteInfo {
                parent_refs: route.spec.parent_refs.clone(),
                hostnames: route.spec.hostnames.clone(),
            });

            if let Some(rules) = &route.spec.rules {
                for (rule_id, rule) in rules.iter().enumerate() {
                    let rule_arc = Arc::new(rule.clone());

                    if let Some(matches) = &rule.matches {
                        for (match_id, match_item) in matches.iter().enumerate() {
                            let unit = Arc::new(GrpcRouteRuleUnit {
                                resource_key: resource_key.clone(),
                                matched_info: GrpcMatchInfo::new(
                                    route_namespace.to_string(),
                                    route_name.to_string(),
                                    rule_id,
                                    match_id,
                                    match_item.clone(),
                                ),
                                rule: rule_arc.clone(),
                                route_info: route_info.clone(),
                            });

                            route_rules_list.push(unit);
                        }
                    }
                }
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
        for (key, mut route) in data.clone() {
            route.preparse();
            parsed_routes.insert(key, route);
        }

        *self.grpc_routes.lock().unwrap() = parsed_routes.clone();

        let new_route_rules = self.build_global_routes(&parsed_routes);
        let new_domain = DomainGrpcRouteRules::new();
        new_domain.grpc_routes.store(new_route_rules);
        self.global_grpc_routes.store(Arc::new(new_domain));

        tracing::info!(component = "grpc_route_manager", "global gRPC routes updated");
    }

    /// partial_update implementation — rebuilds the global gRPC route table.
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

        let all_routes = {
            let mut grpc_routes = self.grpc_routes.lock().unwrap();
            for (key, route) in parsed_add_or_update.iter() {
                grpc_routes.insert(key.clone(), route.clone());
            }
            for key in remove.iter() {
                grpc_routes.remove(key);
            }
            grpc_routes.clone()
        };

        let new_route_rules = self.build_global_routes(&all_routes);
        let current = self.global_grpc_routes.load();
        current.grpc_routes.store(new_route_rules);

        tracing::info!(component = "grpc_route_manager", "global gRPC routes updated");
    }
}
