//! Gateway Handler
//!
//! Handles Gateway resources with:
//! - Filter by gateway_class_name
//! - TLS Secret reference resolution
//! - SecretRefManager registration for cascading updates
//! - ListenerPortManager registration for port conflict detection
//! - Gateway API standard status management

use std::collections::HashSet;

use crate::core::conf_mgr::PROCESSOR_REGISTRY;
use crate::core::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_reasons, condition_true, condition_types, format_secret_key,
    get_listener_port_manager, get_secret, make_port_key, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::types::prelude_resources::{GRPCRoute, Gateway, HTTPRoute, TCPRoute, TLSRoute, UDPRoute};
use crate::types::resources::common::ParentReference;
use crate::types::resources::gateway::{GatewayStatus, ListenerStatus, RouteGroupKind};
use crate::types::ResourceKind;

/// Gateway handler
///
/// Features:
/// - filter: Filter by gateway_class_name (optional, None means no filter)
/// - parse: Parse TLS certificateRefs -> fill tls.secrets
/// - parse: Register Secret references to SecretRefManager
/// - parse: Register listeners to ListenerPortManager for conflict detection
/// - on_change: Requeue conflicting Gateways for bidirectional conflict marking
/// - on_delete: Clear SecretRefManager and ListenerPortManager references
/// - update_status: Set Conflicted/ListenersNotValid conditions based on port conflicts
pub struct GatewayHandler {
    /// If Some, only process Gateways with matching gatewayClassName
    /// If None, process all Gateways (used by FileSystem mode)
    gateway_class_name: Option<String>,
}

impl GatewayHandler {
    /// Create a new GatewayHandler
    ///
    /// - `gateway_class_name`: If Some, filter Gateways by this class name (K8s mode).
    ///   If None, process all Gateways (FileSystem mode).
    pub fn new(gateway_class_name: Option<String>) -> Self {
        Self { gateway_class_name }
    }
}

impl ProcessorHandler<Gateway> for GatewayHandler {
    fn filter(&self, g: &Gateway) -> bool {
        match &self.gateway_class_name {
            Some(class_name) => g.spec.gateway_class_name == *class_name,
            None => true, // No filter, process all Gateways
        }
    }

    fn parse(&self, mut g: Gateway, ctx: &HandlerContext) -> ProcessResult<Gateway> {
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        // Process all Listeners and resolve TLS certificates from global secret store
        if let Some(ref mut listeners) = g.spec.listeners {
            // Collect listener ports for ListenerPortManager registration
            let listener_ports: Vec<(String, String)> = listeners
                .iter()
                .map(|l| {
                    let port_key = make_port_key(l.port, &l.protocol, l.hostname.as_deref());
                    (l.name.clone(), port_key)
                })
                .collect();

            // Register to ListenerPortManager for port conflict detection
            get_listener_port_manager().register_gateway(&resource_ref.key(), &listener_ports);

            for listener in listeners.iter_mut() {
                let tls_config = match &mut listener.tls {
                    Some(tls) => tls,
                    None => continue,
                };

                if let Some(cert_refs) = &tls_config.certificate_refs {
                    if cert_refs.is_empty() {
                        continue;
                    }

                    let mut resolved_secrets = Vec::new();

                    for cert_ref in cert_refs {
                        let secret_ns = cert_ref.namespace.as_ref().or(g.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(secret_ns, &cert_ref.name);

                        // Register to SecretRefManager (critical for cascading updates)
                        // When this Secret arrives or updates, SecretHandler will trigger requeue for this Gateway
                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        // Try to resolve Secret from global store
                        if let Some(secret) = get_secret(secret_ns.map(|s| s.as_str()), &cert_ref.name) {
                            resolved_secrets.push(secret);
                            tracing::debug!(
                                gateway = %resource_ref.key(),
                                listener = %listener.name,
                                secret_key = %secret_key,
                                "Secret resolved and filled into Gateway TLS config"
                            );
                        } else {
                            // Secret not found yet - this is normal if Secret arrives after Gateway
                            // The SecretRefManager reference ensures we'll be requeued when Secret arrives
                            tracing::info!(
                                gateway = %resource_ref.key(),
                                listener = %listener.name,
                                secret_key = %secret_key,
                                "Secret not found yet, Gateway TLS will be updated when Secret arrives"
                            );
                        }
                    }

                    if !resolved_secrets.is_empty() {
                        tls_config.secrets = Some(resolved_secrets);
                    }
                }
            }
        }

        ProcessResult::Continue(g)
    }

    fn on_change(&self, gateway: &Gateway, ctx: &HandlerContext) {
        // Bidirectional conflict marking: when conflicts are detected, requeue all conflicting Gateways
        // This ensures all conflicting Listeners are marked as Conflicted (no winner picked)
        let gateway_key = format!(
            "{}/{}",
            gateway.metadata.namespace.as_deref().unwrap_or(""),
            gateway.metadata.name.as_deref().unwrap_or("")
        );

        let conflicting_gateways = get_listener_port_manager().get_conflicting_gateways(&gateway_key);

        let mut requeued = HashSet::new();
        for conflicting_gateway_key in conflicting_gateways {
            if !requeued.contains(&conflicting_gateway_key) {
                ctx.requeue("Gateway", conflicting_gateway_key.clone());
                requeued.insert(conflicting_gateway_key.clone());
                tracing::info!(
                    gateway = %gateway_key,
                    conflicting_gateway = %conflicting_gateway_key,
                    "Requeue conflicting Gateway for Conflicted status update"
                );
            }
        }
    }

    fn on_delete(&self, g: &Gateway, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );
        let gateway_key = resource_ref.key();

        // Clear SecretRefManager references
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        // Get conflicting gateways BEFORE unregistering (they need to be requeued)
        let conflicting_gateways = get_listener_port_manager().get_conflicting_gateways(&gateway_key);

        // Clear ListenerPortManager registration
        get_listener_port_manager().unregister_gateway(&gateway_key);

        // Requeue previously conflicting Gateways so they can update their Conflicted status
        // (change from Conflicted=True to Conflicted=False)
        for conflicting_gateway_key in conflicting_gateways {
            ctx.requeue("Gateway", conflicting_gateway_key.clone());
            tracing::info!(
                deleted_gateway = %gateway_key,
                conflicting_gateway = %conflicting_gateway_key,
                "Requeue previously conflicting Gateway to clear Conflicted status"
            );
        }

        tracing::debug!(
            gateway = %gateway_key,
            "Cleared secret and port manager references on Gateway delete"
        );
    }

    fn update_status(&self, gateway: &mut Gateway, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = gateway.metadata.generation;
        let gateway_ns = gateway.metadata.namespace.clone().unwrap_or_else(|| "default".to_string());
        let gateway_name = gateway.metadata.name.clone().unwrap_or_default();

        // Initialize status if not present
        let status = gateway.status.get_or_insert_with(GatewayStatus::default);

        // Initialize conditions if not present
        let conditions = status.conditions.get_or_insert_with(Vec::new);

        // Detect port conflicts from ListenerPortManager
        let gateway_key = format!(
            "{}/{}",
            gateway.metadata.namespace.as_deref().unwrap_or(""),
            gateway.metadata.name.as_deref().unwrap_or("")
        );
        let conflicts = get_listener_port_manager().detect_conflicts(&gateway_key);

        // Set Gateway-level conditions
        // Accepted: True if no validation errors
        if validation_errors.is_empty() {
            let cond = accepted_condition(generation);
            update_gateway_condition(conditions, cond);
        } else {
            let cond = condition_false(
                condition_types::ACCEPTED,
                "Invalid",
                validation_errors.join("; "),
                generation,
            );
            update_gateway_condition(conditions, cond);
        }

        // ListenersNotValid: True if any listener has port conflicts
        if !conflicts.is_empty() {
            let conflicting_names: Vec<&String> = conflicts.keys().collect();
            let cond = condition_true(
                condition_types::LISTENERS_NOT_VALID,
                condition_reasons::LISTENER_CONFLICT,
                format!("Listeners have port conflicts: {:?}", conflicting_names),
                generation,
            );
            update_gateway_condition(conditions, cond);
        } else {
            // Remove ListenersNotValid condition if no conflicts
            conditions.retain(|c| c.type_ != condition_types::LISTENERS_NOT_VALID);
        }

        // Programmed: Always True after parsing (configuration accepted)
        let programmed = condition_true(
            condition_types::PROGRAMMED,
            "Programmed",
            "Gateway configuration programmed",
            generation,
        );
        update_gateway_condition(conditions, programmed);

        // Ready: True (data plane ready)
        let ready = condition_true(condition_types::READY, "Ready", "Gateway is ready", generation);
        update_gateway_condition(conditions, ready);

        // Update listener statuses
        if let Some(listeners) = &gateway.spec.listeners {
            let listener_statuses = status.listeners.get_or_insert_with(Vec::new);

            for listener in listeners {
                // Find or create listener status
                let listener_status = listener_statuses.iter_mut().find(|ls| ls.name == listener.name);

                let ls = if let Some(ls) = listener_status {
                    ls
                } else {
                    // Create new listener status
                    let new_ls = ListenerStatus {
                        name: listener.name.clone(),
                        supported_kinds: get_supported_kinds_for_protocol(&listener.protocol),
                        attached_routes: 0,
                        conditions: Vec::new(),
                    };
                    listener_statuses.push(new_ls);
                    listener_statuses.last_mut().unwrap()
                };

                ls.attached_routes =
                    count_attached_routes_for_listener_by_key(&gateway_ns, &gateway_name, &listener.name);

                // Set Conflicted condition based on ListenerPortManager
                let is_conflicted = if let Some((reason, _)) = conflicts.get(&listener.name) {
                    let cond = condition_true(
                        condition_types::CONFLICTED,
                        condition_reasons::LISTENER_CONFLICT,
                        reason.clone(),
                        generation,
                    );
                    update_gateway_condition(&mut ls.conditions, cond);
                    true
                } else {
                    let cond = condition_false(
                        condition_types::CONFLICTED,
                        condition_reasons::NO_CONFLICTS,
                        "No port conflicts",
                        generation,
                    );
                    update_gateway_condition(&mut ls.conditions, cond);
                    false
                };

                // Update other listener conditions (pass conflict status for Programmed/Ready)
                update_listener_conditions(ls, validation_errors, generation, is_conflicted);
            }
        }
    }
}

fn count_attached_routes_for_listener_by_key(gateway_ns: &str, gateway_name: &str, listener_name: &str) -> i32 {
    if gateway_name.is_empty() {
        return 0;
    }

    let mut total = 0_i32;
    total += count_attached_routes_of_kind::<HTTPRoute, _>("HTTPRoute", gateway_ns, gateway_name, listener_name, |route| {
        (route.spec.parent_refs.as_ref(), route.metadata.namespace.as_deref())
    });
    total += count_attached_routes_of_kind::<GRPCRoute, _>("GRPCRoute", gateway_ns, gateway_name, listener_name, |route| {
            (route.spec.parent_refs.as_ref(), route.metadata.namespace.as_deref())
        });
    total += count_attached_routes_of_kind::<TCPRoute, _>("TCPRoute", gateway_ns, gateway_name, listener_name, |route| {
        (route.spec.parent_refs.as_ref(), route.metadata.namespace.as_deref())
    });
    total += count_attached_routes_of_kind::<TLSRoute, _>("TLSRoute", gateway_ns, gateway_name, listener_name, |route| {
        (route.spec.parent_refs.as_ref(), route.metadata.namespace.as_deref())
    });
    total += count_attached_routes_of_kind::<UDPRoute, _>("UDPRoute", gateway_ns, gateway_name, listener_name, |route| {
        (route.spec.parent_refs.as_ref(), route.metadata.namespace.as_deref())
    });
    total
}

fn count_attached_routes_of_kind<K, F>(
    kind: &str,
    gateway_ns: &str,
    gateway_name: &str,
    listener_name: &str,
    parent_refs_fn: F,
) -> i32
where
    K: serde::de::DeserializeOwned,
    F: Fn(&K) -> (Option<&Vec<ParentReference>>, Option<&str>),
{
    let Some(processor) = PROCESSOR_REGISTRY.get(kind) else {
        return 0;
    };

    let Ok((json, _)) = processor.as_watch_obj().list_json() else {
        tracing::warn!(kind = %kind, "Failed to list resources while calculating attachedRoutes");
        return 0;
    };

    let routes: Vec<K> = match serde_json::from_str(&json) {
        Ok(routes) => routes,
        Err(e) => {
            tracing::warn!(kind = %kind, error = %e, "Failed to decode resources while calculating attachedRoutes");
            return 0;
        }
    };

    routes
        .iter()
        .filter(|route| {
            let (parent_refs_opt, route_ns) = parent_refs_fn(route);
            let Some(parent_refs) = parent_refs_opt else {
                return false;
            };
            parent_refs.iter().any(|parent_ref| {
                parent_ref_matches_gateway_and_listener(parent_ref, route_ns, gateway_ns, gateway_name, listener_name)
            })
        })
        .count() as i32
}

fn parent_ref_matches_gateway_and_listener(
    parent_ref: &ParentReference,
    route_ns: Option<&str>,
    gateway_ns: &str,
    gateway_name: &str,
    listener_name: &str,
) -> bool {
    let parent_ns = parent_ref.namespace.as_deref().or(route_ns).unwrap_or("default");
    if parent_ns != gateway_ns || parent_ref.name != gateway_name {
        return false;
    }

    match parent_ref.section_name.as_deref() {
        Some(section_name) => section_name == listener_name,
        None => true,
    }
}

/// Update or insert a condition in Gateway conditions list
fn update_gateway_condition(
    conditions: &mut Vec<crate::types::resources::common::Condition>,
    new_condition: crate::types::resources::common::Condition,
) {
    if let Some(existing) = conditions.iter_mut().find(|c| c.type_ == new_condition.type_) {
        let status_changed = existing.status != new_condition.status;
        existing.status = new_condition.status;
        existing.reason = new_condition.reason;
        existing.message = new_condition.message;
        existing.observed_generation = new_condition.observed_generation;
        if status_changed {
            existing.last_transition_time = new_condition.last_transition_time;
        }
    } else {
        conditions.push(new_condition);
    }
}

/// Update listener conditions
///
/// # Arguments
/// * `ls` - Listener status to update
/// * `validation_errors` - Any validation errors from parsing
/// * `generation` - Resource generation for condition tracking
/// * `is_conflicted` - Whether this listener has port conflicts
fn update_listener_conditions(
    ls: &mut ListenerStatus,
    validation_errors: &[String],
    generation: Option<i64>,
    is_conflicted: bool,
) {
    // Accepted: True if no validation errors (conflict doesn't affect Accepted)
    if validation_errors.is_empty() {
        let cond = accepted_condition(generation);
        update_gateway_condition(&mut ls.conditions, cond);
    } else {
        let cond = condition_false(
            condition_types::ACCEPTED,
            "Invalid",
            validation_errors.join("; "),
            generation,
        );
        update_gateway_condition(&mut ls.conditions, cond);
    }

    // Programmed: False if conflicted (not actually bound to port)
    if is_conflicted {
        let programmed = condition_false(
            condition_types::PROGRAMMED,
            condition_reasons::LISTENER_CONFLICT,
            "Listener not programmed due to port conflict",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, programmed);
    } else {
        let programmed = condition_true(
            condition_types::PROGRAMMED,
            "Programmed",
            "Listener configuration programmed",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, programmed);
    }

    // ResolvedRefs: True (TLS secrets resolved in parse phase)
    let resolved = condition_true(
        condition_types::RESOLVED_REFS,
        "ResolvedRefs",
        "All references resolved",
        generation,
    );
    update_gateway_condition(&mut ls.conditions, resolved);

    // Ready: False if conflicted (not ready to serve traffic)
    if is_conflicted {
        let ready = condition_false(
            condition_types::READY,
            condition_reasons::LISTENER_CONFLICT,
            "Listener not ready due to port conflict",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, ready);
    } else {
        let ready = condition_true(condition_types::READY, "Ready", "Listener is ready", generation);
        update_gateway_condition(&mut ls.conditions, ready);
    }
}

/// Get supported route kinds for a protocol
fn get_supported_kinds_for_protocol(protocol: &str) -> Vec<RouteGroupKind> {
    match protocol.to_uppercase().as_str() {
        "HTTP" | "HTTPS" => vec![
            RouteGroupKind {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: "HTTPRoute".to_string(),
            },
            RouteGroupKind {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: "GRPCRoute".to_string(),
            },
        ],
        "TCP" => vec![RouteGroupKind {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: "TCPRoute".to_string(),
        }],
        "UDP" => vec![RouteGroupKind {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: "UDPRoute".to_string(),
        }],
        "TLS" => vec![RouteGroupKind {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: "TLSRoute".to_string(),
        }],
        _ => vec![],
    }
}
