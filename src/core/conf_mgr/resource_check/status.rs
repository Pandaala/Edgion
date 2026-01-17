//! Status Generation (Event-Driven)
//!
//! Generates expected status for K8s resources.
//! Called when resource changes are detected (event-driven, not polling).
//!
//! Current implementation returns fixed "healthy" status.
//! Future enhancements can add real validation logic here.

use crate::types::resources::common::Condition;
use crate::types::resources::gateway::{Gateway, GatewayStatus, GatewayStatusAddress, ListenerStatus};
use crate::types::resources::http_route::{HTTPRoute, HTTPRouteStatus, RouteParentStatus};
use chrono::Utc;

/// Generate expected Gateway status
///
/// # Arguments
/// * `gateway` - The Gateway resource
/// * `gateway_class_name` - The gateway class name this controller manages
///
/// # Returns
/// `Some(GatewayStatus)` if this gateway should be managed, `None` otherwise
pub fn generate_gateway_status(gateway: &Gateway, gateway_class_name: &str) -> Option<GatewayStatus> {
    // Only process gateways that match our gateway class
    if gateway.spec.gateway_class_name != gateway_class_name {
        return None;
    }

    let now = Utc::now().to_rfc3339();
    let generation = gateway.metadata.generation;

    let mut status = GatewayStatus {
        addresses: Some(vec![GatewayStatusAddress {
            address_type: Some("IPAddress".to_string()),
            value: "0.0.0.0".to_string(), // Placeholder: in real world this should watch LoadBalancer Service
        }]),
        conditions: Some(vec![
            Condition {
                type_: "Programmed".to_string(),
                status: "True".to_string(),
                reason: "Programmed".to_string(),
                message: "Gateway programmed by Edgion controller".to_string(),
                last_transition_time: now.clone(),
                observed_generation: generation,
            },
            Condition {
                type_: "Accepted".to_string(),
                status: "True".to_string(),
                reason: "Accepted".to_string(),
                message: "Gateway accepted by Edgion controller".to_string(),
                last_transition_time: now.clone(),
                observed_generation: generation,
            },
        ]),
        listeners: Some(vec![]),
    };

    // Generate listener statuses
    if let Some(listeners) = &gateway.spec.listeners {
        let mut listener_statuses = Vec::new();
        for listener in listeners {
            listener_statuses.push(ListenerStatus {
                name: listener.name.clone(),
                supported_kinds: vec![], // TODO: populate supported kinds based on protocols
                attached_routes: 0,      // TODO: calculate attached routes
                conditions: vec![
                    Condition {
                        type_: "Accepted".to_string(),
                        status: "True".to_string(),
                        reason: "Accepted".to_string(),
                        message: "Listener accepted".to_string(),
                        last_transition_time: now.clone(),
                        observed_generation: generation,
                    },
                    Condition {
                        type_: "Programmed".to_string(),
                        status: "True".to_string(),
                        reason: "Programmed".to_string(),
                        message: "Listener programmed".to_string(),
                        last_transition_time: now.clone(),
                        observed_generation: generation,
                    },
                ],
            });
        }
        status.listeners = Some(listener_statuses);
    }

    Some(status)
}

/// Generate expected HTTPRoute status
///
/// # Arguments
/// * `route` - The HTTPRoute resource
///
/// # Returns
/// `Some(HTTPRouteStatus)` if the route has parent refs, `None` otherwise
pub fn generate_http_route_status(route: &HTTPRoute) -> Option<HTTPRouteStatus> {
    let now = Utc::now().to_rfc3339();
    let generation = route.metadata.generation;

    let mut parents_status = Vec::new();

    if let Some(parent_refs) = &route.spec.parent_refs {
        for parent_ref in parent_refs {
            let kind = parent_ref.kind.as_deref().unwrap_or("Gateway");
            let group = parent_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");

            // Only report status for Gateway parents
            if kind == "Gateway" && group == "gateway.networking.k8s.io" {
                parents_status.push(RouteParentStatus {
                    parent_ref: parent_ref.clone(),
                    controller_name: "edgion.io/gateway-controller".to_string(),
                    conditions: vec![
                        Condition {
                            type_: "Accepted".to_string(),
                            status: "True".to_string(),
                            reason: "Accepted".to_string(),
                            message: "Route accepted".to_string(),
                            last_transition_time: now.clone(),
                            observed_generation: generation,
                        },
                        Condition {
                            type_: "ResolvedRefs".to_string(),
                            status: "True".to_string(),
                            reason: "ResolvedRefs".to_string(),
                            message: "All references resolved".to_string(),
                            last_transition_time: now.clone(),
                            observed_generation: generation,
                        },
                    ],
                });
            }
        }
    }

    if parents_status.is_empty() {
        return None;
    }

    Some(HTTPRouteStatus {
        parents: parents_status,
    })
}

/// Compare two sets of conditions for equality (ignoring last_transition_time)
///
/// This is used to determine if status needs to be updated.
/// We ignore last_transition_time because it changes every time we generate status.
///
/// # Arguments
/// * `current` - Current conditions from K8s
/// * `expected` - Expected conditions we generated
///
/// # Returns
/// `true` if the conditions are semantically equal
pub fn status_conditions_equal(current: &Option<Vec<Condition>>, expected: &Option<Vec<Condition>>) -> bool {
    match (current, expected) {
        (None, None) => true,
        (Some(c), Some(e)) => {
            if c.len() != e.len() {
                return false;
            }
            // Compare each condition ignoring last_transition_time
            for (cc, ec) in c.iter().zip(e.iter()) {
                if cc.type_ != ec.type_
                    || cc.status != ec.status
                    || cc.reason != ec.reason
                    || cc.message != ec.message
                    || cc.observed_generation != ec.observed_generation
                {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

/// Check if Gateway status needs to be updated
///
/// Compares current status with expected status, ignoring last_transition_time
pub fn gateway_status_needs_update(current: &Option<GatewayStatus>, expected: &GatewayStatus) -> bool {
    match current {
        None => true, // No current status, need to update
        Some(curr) => {
            // Compare conditions
            if !status_conditions_equal(&curr.conditions, &expected.conditions) {
                return true;
            }
            // Compare addresses
            if curr.addresses != expected.addresses {
                return true;
            }
            // Compare listeners (simplified - just check count and names)
            match (&curr.listeners, &expected.listeners) {
                (Some(c), Some(e)) => {
                    if c.len() != e.len() {
                        return true;
                    }
                    for (cl, el) in c.iter().zip(e.iter()) {
                        if cl.name != el.name
                            || !status_conditions_equal(&Some(cl.conditions.clone()), &Some(el.conditions.clone()))
                        {
                            return true;
                        }
                    }
                }
                (None, None) => {}
                _ => return true,
            }
            false
        }
    }
}

/// Check if HTTPRoute status needs to be updated
///
/// Compares current status with expected status, ignoring last_transition_time
pub fn http_route_status_needs_update(current: &Option<HTTPRouteStatus>, expected: &HTTPRouteStatus) -> bool {
    match current {
        None => true, // No current status, need to update
        Some(curr) => {
            // Compare parents count
            if curr.parents.len() != expected.parents.len() {
                return true;
            }
            // Compare each parent status
            for (cp, ep) in curr.parents.iter().zip(expected.parents.iter()) {
                if cp.controller_name != ep.controller_name {
                    return true;
                }
                // Compare parent_ref (simplified)
                if cp.parent_ref.name != ep.parent_ref.name || cp.parent_ref.namespace != ep.parent_ref.namespace {
                    return true;
                }
                // Compare conditions
                if !status_conditions_equal(&Some(cp.conditions.clone()), &Some(ep.conditions.clone())) {
                    return true;
                }
            }
            false
        }
    }
}
