use crate::plugins::publication::{
    global_publisher,
    inputs::{RouteInput, RouteKind},
    PluginPublisher,
};
use crate::routes::l4_partial_update::apply_l4_partial_update;
use crate::routes::udp::route_unit::UdpRouteUnit;
use crate::routes::udp::routes_mgr::{
    get_global_udp_route_managers, resolved_listeners_for_route, GlobalUdpRouteManagers,
};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::UDPRoute;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Create a handler for registration with ConfigClient.
pub fn create_udp_route_handler() -> Box<dyn ConfHandler<UDPRoute> + Send + Sync> {
    Box::new(get_global_udp_route_managers())
}

impl ConfHandler<UDPRoute> for GlobalUdpRouteManagers {
    fn full_set(&self, data: &HashMap<String, Arc<UDPRoute>>) {
        self.full_set_with_publisher(data, global_publisher());
    }
    fn partial_update(
        &self,
        add: HashMap<String, Arc<UDPRoute>>,
        update: HashMap<String, Arc<UDPRoute>>,
        remove: HashSet<String>,
    ) {
        self.partial_update_with_publisher(add, update, remove, global_publisher());
    }
}

impl GlobalUdpRouteManagers {
    fn full_set_with_publisher(&self, data: &HashMap<String, Arc<UDPRoute>>, publisher: &PluginPublisher) {
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        let start_time = Instant::now();
        tracing::info!(component = "udp_route_manager", cnt = data.len(), "full set start");

        self.clear_route_cache();
        let mut policy_inputs = HashMap::new();

        for (key, route) in data {
            match self.initialize_route(Arc::clone(route)) {
                Ok(initialized) => {
                    policy_inputs.insert(key.clone(), RouteInput::from_udp(initialized.route()));
                    self.insert_route(initialized);
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to initialize UDPRoute during full_set"
                    );
                }
            }
        }

        self.rebuild_all_listener_managers_locked();
        tracing::info!(
            component = "udp_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "full set done"
        );
        drop(_guard);
        publisher.replace_routes(RouteKind::Udp, policy_inputs);
    }

    fn partial_update_with_publisher(
        &self,
        add: HashMap<String, Arc<UDPRoute>>,
        update: HashMap<String, Arc<UDPRoute>>,
        remove: HashSet<String>,
        publisher: &PluginPublisher,
    ) {
        let policy_removed: HashSet<_> = remove.iter().chain(update.keys()).chain(add.keys()).cloned().collect();
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        tracing::info!(
            component = "udp_route_manager",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        apply_l4_partial_update(
            "UDPRoute",
            add,
            update,
            remove,
            |key| self.get_route(key),
            |u| resolved_listeners_for_route(u.route()).to_vec(),
            |key| self.remove_route(key),
            |route| {
                self.initialize_route(route).map(|unit| {
                    let lks = resolved_listeners_for_route(unit.route()).to_vec();
                    self.insert_route(unit);
                    lks
                })
            },
            |affected| self.rebuild_affected_listener_managers(affected),
        );
        // Publish only the final surviving units, respecting remove/update/add order.
        let policy_inputs = policy_removed
            .iter()
            .filter_map(|key| {
                self.get_route(key)
                    .map(|unit| (key.clone(), RouteInput::from_udp(unit.route())))
            })
            .collect();
        drop(_guard);
        publisher.update_routes(RouteKind::Udp, policy_inputs, &policy_removed);
    }
}

impl GlobalUdpRouteManagers {
    /// Compile a raw UDPRoute schema into its gateway-side [`UdpRouteUnit`].
    ///
    /// Built ONCE per route here (in the conf-handler), then cached and reused
    /// across listener rebuilds, so a sibling route change on the same listener
    /// never resets this route's `backend_finder` WRR cursor. The compiled
    /// runtime (BackendSelector, stream-plugin store key) lives on the unit,
    /// not on the schema — see [`UdpRouteUnit::from_route`].
    fn initialize_route(&self, route: Arc<UDPRoute>) -> Result<Arc<UdpRouteUnit>, String> {
        UdpRouteUnit::from_route(route)
            .map(Arc::new)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/udp/conf_handler_impl_test.rs"]
mod tests;
