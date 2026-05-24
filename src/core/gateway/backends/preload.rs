//! LB preload module — warm `ServiceSelectorCell` entries at startup.
//!
//! Called after `wait_for_ready()` when all routes and EndpointSlices are
//! loaded. Pays the first-request hydration cost up front so that early
//! traffic does not see a cold-cell round trip.
//!
//! Only the EndpointSlice path is supported; endpoint discovery uses
//! `get_roundrobin_store` exclusively.

use crate::core::gateway::backends::discovery::get_roundrobin_store;
use crate::core::gateway::conf_sync::conf_client::ConfigClient;
use crate::core::gateway::lb::compiled_selector;
use std::collections::HashSet;
use std::sync::Arc;

/// Warm the `ServiceSelectorCell` for every service referenced by any route.
///
/// Called after `wait_for_ready()` when all routes and EndpointSlices are loaded.
pub fn preload_load_balancers(config_client: Arc<ConfigClient>) {
    let mut service_keys: HashSet<String> = HashSet::new();

    // Collect backend service keys from every route type. All five Gateway API
    // route kinds share the same `metadata.namespace` / `spec.rules` /
    // `rule.backend_refs` / `br.{namespace,name}` shape but are distinct concrete
    // types with no shared trait, so a local macro (mirroring `apply_change!` in
    // `config_client.rs`) is the idiomatic way to avoid five identical bodies.
    macro_rules! collect_routes {
        ($list_method:ident) => {
            for route in &config_client.$list_method().data {
                let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
                let Some(rules) = route.spec.rules.as_ref() else {
                    continue;
                };
                for rule in rules {
                    let Some(backend_refs) = rule.backend_refs.as_ref() else {
                        continue;
                    };
                    for br in backend_refs {
                        let ns = br.namespace.as_deref().unwrap_or(route_ns);
                        service_keys.insert(format!("{}/{}", ns, br.name));
                    }
                }
            }
        };
    }

    collect_routes!(list_routes);
    collect_routes!(list_grpc_routes);
    collect_routes!(list_tcp_routes);
    collect_routes!(list_udp_routes);
    collect_routes!(list_tls_routes);

    let total = service_keys.len();
    let mut success = 0;
    let mut skipped = 0;

    for service_key in &service_keys {
        if warm_cell(service_key) {
            success += 1;
        } else {
            skipped += 1;
        }
    }

    tracing::info!(
        component = "lb_preload",
        total,
        success,
        skipped,
        "LB preload completed"
    );
}

/// Hydrate the `ServiceSelectorCell` for `service_key` from the EndpointSlice store.
/// Returns true if data was available and the cell was warmed.
fn warm_cell(service_key: &str) -> bool {
    let backends = get_roundrobin_store().get_backends_for_service(service_key);

    if backends.is_empty() {
        tracing::debug!(
            component = "lb_preload",
            service_key,
            "Skipping: no backend data available"
        );
        return false;
    }

    // Seed the cell through `hydrate_if_empty` so the empty-check and the store
    // happen atomically under the cell's `build_lock` (shared with
    // `replace_backends`). A bare `load().is_empty()` + `replace_backends` would
    // be a TOCTOU: a concurrent EP-sync watch task could call `replace_backends`
    // with fresher data between the check and the write, and the stale snapshot
    // taken above would overwrite it. The provider re-reads the store inside the
    // lock so the freshest data is used. Mirrors the data-plane path in
    // `selector::hydrate_cell_if_empty`.
    let cell = compiled_selector::get_or_create_cell(service_key);
    cell.hydrate_if_empty(|| get_roundrobin_store().get_backends_for_service(service_key));

    tracing::trace!(component = "lb_preload", service_key, "Warmed ServiceSelectorCell");
    true
}
