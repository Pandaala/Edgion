use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::routes::grpc_routes::{
    GrpcRouteManager, GrpcRouteRuleUnit, get_global_grpc_route_manager, GrpcMatchEngine,
};
use crate::core::routes::grpc_routes::routes_mgr::{GrpcRouteRules, DomainGrpcRouteRules};
use crate::types::GRPCRoute;

type GatewayKey = String;
type DomainStr = String;

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
    /// Build gateway_hostnames map from add_or_update and remove sets
    /// Returns a map of gateway_key -> set of affected hostnames
    fn build_gateway_hostnames_map(
        &self,
        add_or_update: &HashMap<String, GRPCRoute>,
        remove: &HashSet<String>,
    ) -> HashMap<String, HashSet<String>> {
        let mut gateway_hostnames: HashMap<String, HashSet<String>> = HashMap::new();

        // Get grpc_routes lock once for efficiency
        let grpc_routes = self.grpc_routes.lock().unwrap();

        // Process add_or_update routes
        for (resource_key, route) in add_or_update.iter() {
            // Check if this is an update (route already exists)
            let old_route = grpc_routes.get(resource_key);

            // Collect hostnames from both old and new routes
            let mut all_hostnames = HashSet::new();

            // Add new hostnames
            if let Some(hostnames) = &route.spec.hostnames {
                for hostname in hostnames {
                    all_hostnames.insert(hostname.clone());
                }
            }

            // Add old hostnames (if this is an update)
            if let Some(old_route) = old_route {
                if let Some(old_hostnames) = &old_route.spec.hostnames {
                    for hostname in old_hostnames {
                        all_hostnames.insert(hostname.clone());
                    }
                }
            }

            // Process parent_refs and add all hostnames to gateway_hostnames
            if let Some(parent_refs) = &route.spec.parent_refs {
                for parent_ref in parent_refs {
                    let gateway_key = if let Some(ns) = &parent_ref.namespace {
                        format!("{}/{}", ns, parent_ref.name)
                    } else if let Some(ns) = &route.metadata.namespace {
                        format!("{}/{}", ns, parent_ref.name)
                    } else {
                        parent_ref.name.clone()
                    };

                    let hostname_set = gateway_hostnames
                        .entry(gateway_key)
                        .or_insert_with(HashSet::new);
                    for hostname in &all_hostnames {
                        hostname_set.insert(hostname.clone());
                    }
                }
            }
        }

        drop(grpc_routes); // Release lock before processing remove routes

        // Process remove routes
        let grpc_routes = self.grpc_routes.lock().unwrap();
        for resource_key in remove.iter() {
            if let Some(route) = grpc_routes.get(resource_key) {
                if let Some(hostnames) = &route.spec.hostnames {
                    if let Some(parent_refs) = &route.spec.parent_refs {
                        for parent_ref in parent_refs {
                            let gateway_key = if let Some(ns) = &parent_ref.namespace {
                                format!("{}/{}", ns, parent_ref.name)
                            } else if let Some(ns) = &route.metadata.namespace {
                                format!("{}/{}", ns, parent_ref.name)
                            } else {
                                parent_ref.name.clone()
                            };

                            let hostname_set = gateway_hostnames
                                .entry(gateway_key)
                                .or_insert_with(HashSet::new);
                            for hostname in hostnames {
                                hostname_set.insert(hostname.clone());
                            }
                        }
                    }
                }
            }
        }

        gateway_hostnames
    }

    /// Update a single hostname's GrpcRouteRules in the given HashMap
    fn update_single_hostname(
        &self,
        domain_hashmap: &mut HashMap<DomainStr, Arc<GrpcRouteRules>>,
        hostname: &str,
        add_or_update: &HashMap<String, GRPCRoute>,
        remove: &HashSet<String>,
    ) {
        // Get existing resource_keys or create empty set
        let mut resource_keys = domain_hashmap
            .get(hostname)
            .map(|rr| rr.resource_keys.read().unwrap().clone())
            .unwrap_or_else(HashSet::new);

        // Step 1: Remove resource keys
        for key in remove.iter() {
            if resource_keys.remove(key) {
                tracing::debug!(
                    component = "grpc_route_manager",
                    hostname = %hostname,
                    key = %key,
                    "Remove gRPC route key"
                );
            }
        }

        // Step 2: Add/update/remove resource keys from add_or_update
        for (resource_key, route) in add_or_update.iter() {
            // Check if this route applies to this hostname
            let applies = route
                .spec
                .hostnames
                .as_ref()
                .map(|hostnames| hostnames.contains(&hostname.to_string()))
                .unwrap_or(false);

            if applies {
                resource_keys.insert(resource_key.clone());
                tracing::debug!(
                    component = "grpc_route_manager",
                    hostname = %hostname,
                    key = %resource_key,
                    "Add/update gRPC route key"
                );
            } else {
                // Route no longer applies to this hostname
                if resource_keys.remove(resource_key) {
                    tracing::debug!(
                        component = "grpc_route_manager",
                        hostname = %hostname,
                        key = %resource_key,
                        "Remove gRPC route key (no longer applies)"
                    );
                }
            }
        }

        // Step 3: Rebuild route_rules_list and match_engine from resource_keys
        let mut route_rules_list: Vec<Arc<GrpcRouteRuleUnit>> = Vec::new();

        let grpc_routes = self.grpc_routes.lock().unwrap();
        for resource_key in resource_keys.iter() {
            if let Some(route) = grpc_routes.get(resource_key) {
                let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
                let route_name = route.metadata.name.as_deref().unwrap_or("");

                if let Some(rules) = &route.spec.rules {
                    for (rule_id, rule) in rules.iter().enumerate() {
                        let rule_arc = Arc::new(rule.clone());

                        // Each rule may have multiple matches
                        if let Some(matches) = &rule.matches {
                            for (match_id, match_item) in matches.iter().enumerate() {
                                // Extract service and method from match
                                let service = match_item
                                    .method
                                    .as_ref()
                                    .and_then(|m| m.service.clone());
                                let method = match_item
                                    .method
                                    .as_ref()
                                    .and_then(|m| m.method.clone());

                                // Create GrpcRouteRuleUnit
                                let unit = Arc::new(GrpcRouteRuleUnit::new(
                                    route_namespace.to_string(),
                                    route_name.to_string(),
                                    rule_id,
                                    match_id,
                                    resource_key.clone(),
                                    service,
                                    method,
                                    match_item.clone(),
                                    rule_arc.clone(),
                                ));

                                route_rules_list.push(unit);
                            }
                        }
                    }
                }
            }
        }

        // Step 4: Build match engine
        let match_engine = if route_rules_list.is_empty() {
            None
        } else {
            Some(Arc::new(GrpcMatchEngine::new(route_rules_list.clone())))
        };

        // Step 5: Create new GrpcRouteRules
        let new_route_rules = GrpcRouteRules {
            resource_keys: std::sync::RwLock::new(resource_keys),
            route_rules_list: std::sync::RwLock::new(route_rules_list),
            match_engine,
        };

        // Step 6: Update domain_hashmap
        domain_hashmap.insert(hostname.to_string(), Arc::new(new_route_rules));
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

        // Rebuild all gateway routes
        let all_add_or_update = parsed_routes;
        let empty_remove = HashSet::new();

        let gateway_hostnames = self.build_gateway_hostnames_map(&all_add_or_update, &empty_remove);

        // Update each gateway
        for (gateway_key, hostnames) in gateway_hostnames.iter() {
            let mut domain_hashmap = HashMap::new();

            for hostname in hostnames {
                self.update_single_hostname(
                    &mut domain_hashmap,
                    hostname,
                    &all_add_or_update,
                    &empty_remove,
                );
            }

            // Get or create DomainGrpcRouteRules for this gateway
            // Then update its internal ArcSwap (don't replace the entire instance)
            let domain_routes = self.gateway_routes_map
                .entry(gateway_key.clone())
                .or_insert_with(|| Arc::new(DomainGrpcRouteRules::new()))
                .value()
                .clone();
            
            // Update the internal ArcSwap with new domain_hashmap
            // Note: ArcSwap<Arc<T>> requires Arc<Arc<T>> for store() method
            domain_routes.domain_routes_map.store(Arc::new(Arc::new(domain_hashmap)));

            tracing::info!(
                component = "grpc_route_manager",
                gateway = %gateway_key,
                hostnames = ?hostnames,
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
        {
            let mut grpc_routes = self.grpc_routes.lock().unwrap();

            // Add/update routes
            for (key, route) in parsed_add_or_update.iter() {
                grpc_routes.insert(key.clone(), route.clone());
            }

            // Remove routes
            for key in remove.iter() {
                grpc_routes.remove(key);
            }
        }

        // Build gateway_hostnames map
        let gateway_hostnames =
            self.build_gateway_hostnames_map(&parsed_add_or_update, &remove);

        // Update each affected gateway
        for (gateway_key, hostnames) in gateway_hostnames.iter() {
            // Get or create DomainGrpcRouteRules for this gateway
            let domain_routes = self
                .gateway_routes_map
                .entry(gateway_key.clone())
                .or_insert_with(|| Arc::new(DomainGrpcRouteRules::new()))
                .clone();

            // Load current domain_routes_map
            let current_map = domain_routes.domain_routes_map.load();
            let current_hashmap: &HashMap<DomainStr, Arc<GrpcRouteRules>> = current_map.as_ref();
            let mut new_domain_hashmap: HashMap<DomainStr, Arc<GrpcRouteRules>> = 
                current_hashmap.clone();

            // Update each affected hostname
            for hostname in hostnames {
                self.update_single_hostname(
                    &mut new_domain_hashmap,
                    hostname,
                    &parsed_add_or_update,
                    &remove,
                );
            }

            // Store new domain_routes_map (RCU update)
            // Note: ArcSwap<Arc<T>> requires Arc<Arc<T>> for store() method
            domain_routes
                .domain_routes_map
                .store(Arc::new(Arc::new(new_domain_hashmap)));

            tracing::info!(
                component = "grpc_route_manager",
                gateway = %gateway_key,
                hostnames = ?hostnames,
                "Updated gRPC routes for gateway"
            );
        }
    }
}

