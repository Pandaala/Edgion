//! LB preload module for warming up load balancers at startup
//!
//! This module preloads LoadBalancers for all routes to reduce first-request latency.
//! It is called after `wait_for_ready()` when all routes and EndpointSlices are loaded.

use crate::core::backends::{get_global_endpoint_mode, EndpointMode};
use crate::core::conf_sync::conf_client::ConfigClient;
use crate::types::resources::ParsedLBPolicy;
use std::collections::HashSet;
use std::sync::Arc;

/// Key for deduplication: (service_key, lb_policy_type)
/// Same service with different LB policies need separate LB instances
type PreloadKey = (String, Option<LbPolicyType>);

/// Simplified LB policy type for deduplication (ignores hash source details)
#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
enum LbPolicyType {
    ConsistentHash,
    LeastConn,
    Ewma,
}

impl From<&ParsedLBPolicy> for LbPolicyType {
    fn from(policy: &ParsedLBPolicy) -> Self {
        match policy {
            ParsedLBPolicy::ConsistentHash(_) => LbPolicyType::ConsistentHash,
            ParsedLBPolicy::LeastConn => LbPolicyType::LeastConn,
            ParsedLBPolicy::Ewma => LbPolicyType::Ewma,
        }
    }
}

/// Preload LBs for all routes to reduce first-request latency.
///
/// Called after `wait_for_ready()` when all routes and EndpointSlices are loaded.
/// This function is synchronous as all store operations are sync.
pub fn preload_load_balancers(config_client: Arc<ConfigClient>) {
    let mode = get_global_endpoint_mode();
    let mut keys: HashSet<PreloadKey> = HashSet::new();

    // Collect from all route types
    collect_http_routes(&config_client, &mut keys);
    collect_grpc_routes(&config_client, &mut keys);
    collect_tcp_routes(&config_client, &mut keys);
    collect_udp_routes(&config_client, &mut keys);
    collect_tls_routes(&config_client, &mut keys);

    let total = keys.len();
    let mut success = 0;
    let mut skipped = 0;

    // Preload each unique (service_key, lb_policy)
    for (service_key, lb_type) in &keys {
        if preload_lb(service_key, lb_type, mode) {
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
        mode = ?mode,
        "LB preload completed"
    );
}

fn collect_http_routes(config_client: &ConfigClient, keys: &mut HashSet<PreloadKey>) {
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
                let key = format!("{}/{}", ns, br.name);
                let lb_type = br.extension_info.lb_policy.as_ref().map(LbPolicyType::from);
                keys.insert((key, lb_type));
            }
        }
    }
}

fn collect_grpc_routes(config_client: &ConfigClient, keys: &mut HashSet<PreloadKey>) {
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
                let key = format!("{}/{}", ns, br.name);
                let lb_type = br.extension_info.lb_policy.as_ref().map(LbPolicyType::from);
                keys.insert((key, lb_type));
            }
        }
    }
}

fn collect_tcp_routes(config_client: &ConfigClient, keys: &mut HashSet<PreloadKey>) {
    // TCPRoute has no extension_info, always uses RoundRobin
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
                let key = format!("{}/{}", ns, br.name);
                keys.insert((key, None)); // No LB policy
            }
        }
    }
}

fn collect_udp_routes(config_client: &ConfigClient, keys: &mut HashSet<PreloadKey>) {
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
                let key = format!("{}/{}", ns, br.name);
                let lb_type = br.extension_info.lb_policy.as_ref().map(LbPolicyType::from);
                keys.insert((key, lb_type));
            }
        }
    }
}

fn collect_tls_routes(config_client: &ConfigClient, keys: &mut HashSet<PreloadKey>) {
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
                let key = format!("{}/{}", ns, br.name);
                let lb_type = br.extension_info.lb_policy.as_ref().map(LbPolicyType::from);
                keys.insert((key, lb_type));
            }
        }
    }
}

fn preload_lb(service_key: &str, lb_type: &Option<LbPolicyType>, mode: EndpointMode) -> bool {
    match mode {
        EndpointMode::EndpointSlice | EndpointMode::Both | EndpointMode::Auto => preload_eps_lb(service_key, lb_type),
        EndpointMode::Endpoint => preload_endpoint_lb(service_key, lb_type),
    }
}

fn preload_eps_lb(service_key: &str, lb_type: &Option<LbPolicyType>) -> bool {
    use crate::core::backends::endpoint_slice::*;

    let rr_store = get_roundrobin_store();

    // Always preload RoundRobin first (data layer)
    let rr_lb = rr_store.get_or_create(service_key);
    if rr_lb.is_none() {
        tracing::debug!(
            component = "lb_preload",
            service_key,
            "Skipping: no EndpointSlice data available"
        );
        return false;
    }

    // Additionally preload specific LB if configured
    match lb_type {
        Some(LbPolicyType::ConsistentHash) => {
            get_consistent_store().get_or_create_with_provider(service_key, |key| rr_store.get_slices_for_service(key));
        }
        Some(LbPolicyType::LeastConn) => {
            get_leastconn_store().get_or_create_with_provider(service_key, |key| rr_store.get_slices_for_service(key));
        }
        Some(LbPolicyType::Ewma) => {
            get_ewma_store().get_or_create_with_provider(service_key, |key| rr_store.get_slices_for_service(key));
        }
        None => {} // RoundRobin already preloaded
    }

    tracing::trace!(
        component = "lb_preload",
        service_key,
        lb_type = ?lb_type,
        "Preloaded LB (EndpointSlice)"
    );
    true
}

fn preload_endpoint_lb(service_key: &str, lb_type: &Option<LbPolicyType>) -> bool {
    use crate::core::backends::endpoint::*;

    let rr_store = get_endpoint_roundrobin_store();

    // Always preload RoundRobin first (data layer)
    let rr_lb = rr_store.get_or_create(service_key);
    if rr_lb.is_none() {
        tracing::debug!(
            component = "lb_preload",
            service_key,
            "Skipping: no Endpoint data available"
        );
        return false;
    }

    // Additionally preload specific LB if configured
    match lb_type {
        Some(LbPolicyType::ConsistentHash) => {
            get_endpoint_consistent_store()
                .get_or_create_with_provider(service_key, |key| rr_store.get_endpoint_for_service(key));
        }
        Some(LbPolicyType::LeastConn) => {
            get_endpoint_leastconn_store()
                .get_or_create_with_provider(service_key, |key| rr_store.get_endpoint_for_service(key));
        }
        Some(LbPolicyType::Ewma) => {
            get_endpoint_ewma_store()
                .get_or_create_with_provider(service_key, |key| rr_store.get_endpoint_for_service(key));
        }
        None => {} // RoundRobin already preloaded
    }

    tracing::trace!(
        component = "lb_preload",
        service_key,
        lb_type = ?lb_type,
        "Preloaded LB (Endpoint)"
    );
    true
}
