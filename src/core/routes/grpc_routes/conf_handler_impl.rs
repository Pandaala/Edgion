use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::routes::grpc_routes::{
    GrpcRouteManager, GrpcRouteRuleUnit, get_global_grpc_route_manager, GrpcMatchEngine,
};
use crate::core::routes::grpc_routes::match_unit::GrpcRouteInfo;
use crate::core::routes::grpc_routes::routes_mgr::{GrpcRouteRules, DomainGrpcRouteRules};
use crate::types::GRPCRoute;

type GatewayKey = String;

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
    /// Build set of affected gateway keys from add_or_update and remove sets
    fn build_affected_gateways(
        &self,
        add_or_update: &HashMap<String, GRPCRoute>,
        remove: &HashSet<String>,
    ) -> HashSet<GatewayKey> {
        let mut affected_gateways = HashSet::new();

        // Process add_or_update routes
        for (_resource_key, route) in add_or_update.iter() {
            if let Some(parent_refs) = &route.spec.parent_refs {
                for parent_ref in parent_refs {
                    let gateway_key = if let Some(ns) = &parent_ref.namespace {
                        format!("{}/{}", ns, parent_ref.name)
                    } else if let Some(ns) = &route.metadata.namespace {
                        format!("{}/{}", ns, parent_ref.name)
                    } else {
                        parent_ref.name.clone()
                    };
                    affected_gateways.insert(gateway_key);
                }
            }
        }

        // Process remove routes
        let grpc_routes = self.grpc_routes.lock().unwrap();
        for resource_key in remove.iter() {
            if let Some(route) = grpc_routes.get(resource_key) {
                if let Some(parent_refs) = &route.spec.parent_refs {
                    for parent_ref in parent_refs {
                        let gateway_key = parent_ref.build_parent_key(route.metadata.namespace.as_deref());
                        affected_gateways.insert(gateway_key);
                    }
                }
            }
        }

        affected_gateways
    }

    /// Update gateway routes for a specific gateway
    fn update_gateway_routes(
        &self,
        gateway_key: &str,
        all_routes: &HashMap<String, GRPCRoute>,
    ) -> Arc<GrpcRouteRules> {
        let mut resource_keys = HashSet::new();
        let mut route_rules_list: Vec<Arc<GrpcRouteRuleUnit>> = Vec::new();

        // Collect all routes that belong to this gateway
        for (resource_key, route) in all_routes.iter() {
            // Check if this route applies to this gateway
            let applies_to_gateway = route
                .spec
                .parent_refs
                .as_ref()
                .map(|refs| {
                    refs.iter().any(|parent_ref| {
                        let gw_key = parent_ref.build_parent_key(route.metadata.namespace.as_deref());
                        gw_key == gateway_key
                    })
                })
                .unwrap_or(false);

            if !applies_to_gateway {
                continue;
            }

            resource_keys.insert(resource_key.clone());

            let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
            let route_name = route.metadata.name.as_deref().unwrap_or("");

            // Create shared route info for all rule units of this route
            let route_info = Arc::new(GrpcRouteInfo {
                parent_refs: route.spec.parent_refs.clone(),
                hostnames: route.spec.hostnames.clone(),
            });

            if let Some(rules) = &route.spec.rules {
                for (rule_id, rule) in rules.iter().enumerate() {
                    let rule_arc = Arc::new(rule.clone());

                    // Each rule may have multiple matches
                    if let Some(matches) = &rule.matches {
                        for (match_id, match_item) in matches.iter().enumerate() {
                            // Create GrpcRouteRuleUnit
                            let unit = Arc::new(GrpcRouteRuleUnit::new(
                                route_namespace.to_string(),
                                route_name.to_string(),
                                rule_id,
                                match_id,
                                resource_key.clone(),
                                match_item.clone(),
                                rule_arc.clone(),
                                route_info.clone(),
                            ));

                            route_rules_list.push(unit);
                        }
                    }
                }
            }
        }

        // Build match engine
        let match_engine = if route_rules_list.is_empty() {
            None
        } else {
            Some(Arc::new(GrpcMatchEngine::new(route_rules_list.clone())))
        };

        // Create new GrpcRouteRules
        Arc::new(GrpcRouteRules {
            resource_keys: std::sync::RwLock::new(resource_keys),
            route_rules_list: std::sync::RwLock::new(route_rules_list),
            match_engine,
        })
    }

    /// full_set implementation
    fn full_set(&self, data: &HashMap<String, GRPCRoute>) {
        tracing::info!(
            component = "grpc_route_manager",
            count = data.len(),
            "Full set gRPC routes"
        );

        // Parse hidden logic for all routes
        let mut parsed_routes = HashMap::new();
        for (key, mut route) in data.clone() {
            route.preparse();
            parsed_routes.insert(key, route);
        }

        // Update grpc_routes storage
        {
            let mut grpc_routes = self.grpc_routes.lock().unwrap();
            *grpc_routes = parsed_routes.clone();
        }

        // Find all affected gateways
        let empty_remove = HashSet::new();
        let affected_gateways = self.build_affected_gateways(&parsed_routes, &empty_remove);

        // Update each gateway
        for gateway_key in affected_gateways.iter() {
            let new_route_rules = self.update_gateway_routes(gateway_key, &parsed_routes);

            // Get or create DomainGrpcRouteRules for this gateway
            let domain_routes = self.gateway_routes_map
                .entry(gateway_key.clone())
                .or_insert_with(|| Arc::new(DomainGrpcRouteRules::new()))
                .value()
                .clone();
            
            // Update the internal ArcSwap with new route rules
            domain_routes.grpc_routes.store(new_route_rules);

            tracing::info!(
                component = "grpc_route_manager",
                gateway = %gateway_key,
                "Updated gRPC routes for gateway"
            );
        }
    }

    /// partial_update implementation
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

        // Parse hidden logic for add and update routes
        let mut parsed_add_or_update = HashMap::new();

        for (key, mut route) in add.into_iter().chain(update.into_iter()) {
            route.preparse();
            parsed_add_or_update.insert(key, route);
        }

        // Update grpc_routes storage
        let all_routes = {
            let mut grpc_routes = self.grpc_routes.lock().unwrap();

            // Add/update routes
            for (key, route) in parsed_add_or_update.iter() {
                grpc_routes.insert(key.clone(), route.clone());
            }

            // Remove routes
            for key in remove.iter() {
                grpc_routes.remove(key);
            }

            // Return a clone of all routes for rebuilding
            grpc_routes.clone()
        };

        // Find all affected gateways
        let affected_gateways = self.build_affected_gateways(&parsed_add_or_update, &remove);

        // Update each affected gateway
        for gateway_key in affected_gateways.iter() {
            let new_route_rules = self.update_gateway_routes(gateway_key, &all_routes);

            // Get or create DomainGrpcRouteRules for this gateway
            let domain_routes = self
                .gateway_routes_map
                .entry(gateway_key.clone())
                .or_insert_with(|| Arc::new(DomainGrpcRouteRules::new()))
                .clone();

            // Store new route rules (RCU update)
            domain_routes.grpc_routes.store(new_route_rules);

            tracing::info!(
                component = "grpc_route_manager",
                gateway = %gateway_key,
                "Updated gRPC routes for gateway"
            );
        }
    }
}

