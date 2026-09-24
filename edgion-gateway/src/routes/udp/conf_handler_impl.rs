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
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        let start_time = Instant::now();
        tracing::info!(component = "udp_route_manager", cnt = data.len(), "full set start");

        self.clear_route_cache();

        for (key, route) in data {
            match self.initialize_route(Arc::clone(route)) {
                Ok(initialized) => {
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
        crate::plugins::publication::global_publisher().replace_routes(
            crate::plugins::publication::inputs::RouteKind::Udp,
            data.iter()
                .map(|(key, route)| {
                    (
                        key.clone(),
                        crate::plugins::publication::inputs::RouteInput::from_udp(route),
                    )
                })
                .collect(),
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<UDPRoute>>,
        update: HashMap<String, Arc<UDPRoute>>,
        remove: HashSet<String>,
    ) {
        let policy_inputs = add
            .iter()
            .chain(update.iter())
            .map(|(key, route)| {
                (
                    key.clone(),
                    crate::plugins::publication::inputs::RouteInput::from_udp(route),
                )
            })
            .collect();
        let policy_removed = remove.clone();
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
            |u| resolved_listeners_for_route(&u.route).to_vec(),
            |key| self.remove_route(key),
            |route| {
                self.initialize_route(route).map(|unit| {
                    let lks = resolved_listeners_for_route(&unit.route).to_vec();
                    self.insert_route(unit);
                    lks
                })
            },
            |affected| self.rebuild_affected_listener_managers(affected),
        );
        drop(_guard);
        crate::plugins::publication::global_publisher().update_routes(
            crate::plugins::publication::inputs::RouteKind::Udp,
            policy_inputs,
            &policy_removed,
        );
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
        Ok(Arc::new(UdpRouteUnit::from_route(route)))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/udp/conf_handler_impl_test.rs"]
mod tests;
