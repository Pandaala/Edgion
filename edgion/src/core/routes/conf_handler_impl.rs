use std::collections::{HashMap, HashSet};
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::routes::RouteManager;
use crate::types::HTTPRoute;

impl ConfHandler<HTTPRoute> for RouteManager {
    /// Full rebuild with a complete set of HTTPRoutes
    /// This is typically called during initial sync
    fn full_build(&mut self, data: &HashMap<String, HTTPRoute>) {
        tracing::info!(
            component = "route_manager",
            count = data.len(),
            "Full build with HTTPRoutes"
        );

        // Clear existing routes (if needed - currently RouteManager doesn't have a clear method)
        // For now, we'll just add/update all routes
        
        // Convert HashMap values to Vec and add them
        let routes: Vec<HTTPRoute> = data.values().cloned().collect();
        self.add_http_routes(routes);

        tracing::info!(
            component = "route_manager",
            count = data.len(),
            "Full build completed"
        );
    }

    /// Handle incremental configuration changes
    /// Processes additions, updates, and removals of HTTPRoutes
    fn conf_change(&mut self, add_or_update: HashMap<String, HTTPRoute>, remove: HashSet<String>) {
        tracing::info!(
            component = "route_manager",
            add_or_update_count = add_or_update.len(),
            remove_count = remove.len(),
            "Processing HTTPRoute changes"
        );

        // Process additions and updates
        for (key, route) in add_or_update {
            tracing::debug!(
                component = "route_manager",
                route_key = %key,
                "Adding/updating HTTPRoute"
            );
            self.add_http_route(route);
        }

        // Process removals
        for key in remove {
            tracing::debug!(
                component = "route_manager",
                route_key = %key,
                "Removing HTTPRoute (not yet implemented)"
            );
            // TODO: Implement route removal when RouteManager supports it
            // self.remove_http_route(&key);
        }

        tracing::info!(
            component = "route_manager",
            "HTTPRoute changes processed"
        );
    }

    /// Trigger a rebuild/refresh of the route configuration
    /// This could be used to optimize internal data structures or refresh caches
    fn update_rebuild(&mut self) {
        tracing::debug!(
            component = "route_manager",
            "Update rebuild triggered (no-op for now)"
        );
        
        // Currently no-op as RouteManager doesn't need periodic rebuilds
        // If needed in the future, we could:
        // - Rebuild internal match engines
        // - Optimize route lookup structures
        // - Refresh cached data
    }
}

