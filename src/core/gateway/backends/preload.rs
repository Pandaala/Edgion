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

    collect_http_routes(&config_client, &mut service_keys);
    collect_grpc_routes(&config_client, &mut service_keys);
    collect_tcp_routes(&config_client, &mut service_keys);
    collect_udp_routes(&config_client, &mut service_keys);
    collect_tls_routes(&config_client, &mut service_keys);

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

fn collect_http_routes(config_client: &ConfigClient, keys: &mut HashSet<String>) {
    for route in &config_client.list_routes().data {
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
                keys.insert(format!("{}/{}", ns, br.name));
            }
        }
    }
}

fn collect_grpc_routes(config_client: &ConfigClient, keys: &mut HashSet<String>) {
    for route in &config_client.list_grpc_routes().data {
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
                keys.insert(format!("{}/{}", ns, br.name));
            }
        }
    }
}

fn collect_tcp_routes(config_client: &ConfigClient, keys: &mut HashSet<String>) {
    for route in &config_client.list_tcp_routes().data {
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
                keys.insert(format!("{}/{}", ns, br.name));
            }
        }
    }
}

fn collect_udp_routes(config_client: &ConfigClient, keys: &mut HashSet<String>) {
    for route in &config_client.list_udp_routes().data {
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
                keys.insert(format!("{}/{}", ns, br.name));
            }
        }
    }
}

fn collect_tls_routes(config_client: &ConfigClient, keys: &mut HashSet<String>) {
    for route in &config_client.list_tls_routes().data {
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
                keys.insert(format!("{}/{}", ns, br.name));
            }
        }
    }
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

    let cell = compiled_selector::get_or_create_cell(service_key);
    if cell.backends.load().is_empty() {
        cell.replace_backends(backends);
    }

    tracing::trace!(component = "lb_preload", service_key, "Warmed ServiceSelectorCell");
    true
}
