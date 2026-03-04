//! Gateway Handler
//!
//! Handles Gateway resources with:
//! - Filter by gateway_class_name
//! - TLS Secret reference resolution
//! - SecretRefManager registration for cascading updates
//! - ListenerPortManager registration for port conflict detection
//! - Gateway API standard status management

use std::collections::HashSet;

use crate::core::cli::config::is_reference_grant_validation_enabled;
use crate::core::conf_mgr::sync_runtime::resource_processor::get_attached_route_tracker;
use crate::core::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, format_secret_key,
    get_global_cross_ns_ref_manager, get_global_reference_grant_store, get_listener_port_manager, get_secret,
    make_port_key, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::core::conf_mgr::PROCESSOR_REGISTRY;
use crate::types::prelude_resources::Gateway;
use crate::types::resources::gateway::{
    GatewayStatus, GatewayStatusAddress, Listener as GatewayListener, ListenerStatus, RouteGroupKind,
};
use crate::types::ResourceKind;
use k8s_openapi::api::core::v1::Service;

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
    /// Static fallback address for Gateway status.addresses
    default_address: Option<String>,
}

struct ListenerInfo {
    name: String,
    supported_kinds: Vec<RouteGroupKind>,
    route_count: i32,
    resolved_refs_errors: Vec<String>,
}

impl GatewayHandler {
    /// Create a new GatewayHandler
    ///
    /// - `gateway_class_name`: If Some, filter Gateways by this class name (K8s mode).
    ///   If None, process all Gateways (FileSystem mode).
    pub fn new(gateway_class_name: Option<String>, default_address: Option<String>) -> Self {
        Self {
            gateway_class_name,
            default_address,
        }
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

        // Track cross-namespace Secret references so ReferenceGrant changes requeue this Gateway
        let cross_ns_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );
        let cross_ns_manager = get_global_cross_ns_ref_manager();
        cross_ns_manager.clear_resource_refs(&cross_ns_ref);

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
                    let gateway_ns = g.metadata.namespace.as_deref().unwrap_or("");

                    for cert_ref in cert_refs {
                        let secret_ns = cert_ref.namespace.as_ref().or(g.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(secret_ns, &cert_ref.name);

                        // Register to SecretRefManager (critical for cascading updates)
                        // When this Secret arrives or updates, SecretHandler will trigger requeue for this Gateway
                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        // Register cross-namespace cert refs so ReferenceGrant changes requeue this Gateway
                        let cert_ns_str = secret_ns.map(|s| s.as_str()).unwrap_or("");
                        if cert_ns_str != gateway_ns {
                            cross_ns_manager.add_ref(cert_ns_str.to_string(), cross_ns_ref.clone());
                        }

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

        // Clear cross-namespace reference tracking
        let cross_ns_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );
        get_global_cross_ns_ref_manager().clear_resource_refs(&cross_ns_ref);

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
        let gateway_ns = gateway
            .metadata
            .namespace
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let gateway_name = gateway.metadata.name.clone().unwrap_or_default();
        let tracker = get_attached_route_tracker();
        let gateway_key_for_tracker = format!("{}/{}", gateway_ns, gateway_name);

        // Pre-compute per-listener info while gateway is only immutably borrowed.
        let listener_infos: Vec<ListenerInfo> = gateway
            .spec
            .listeners
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|l| {
                let route_count = tracker.count_for_listener(&gateway_key_for_tracker, &l.name);
                let (supported_kinds, mut kind_errors) = compute_supported_kinds(l);
                let mut resolved_refs_errors = validate_listener_resolved_refs(gateway, l);
                resolved_refs_errors.append(&mut kind_errors);

                ListenerInfo {
                    name: l.name.clone(),
                    supported_kinds,
                    route_count,
                    resolved_refs_errors,
                }
            })
            .collect();

        let derived_addresses = derive_gateway_addresses(gateway, self.default_address.as_deref());

        // Initialize status if not present
        let status = gateway.status.get_or_insert_with(GatewayStatus::default);

        status.addresses = Some(derived_addresses);

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
            let cond = condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Gateway accepted",
                generation,
            );
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

        // Update listener statuses using pre-computed info
        if !listener_infos.is_empty() {
            let listener_statuses = status.listeners.get_or_insert_with(Vec::new);

            for ListenerInfo {
                name,
                supported_kinds,
                route_count,
                resolved_refs_errors,
            } in listener_infos
            {
                let listener_status = listener_statuses.iter_mut().find(|ls| ls.name == name);

                let ls = if let Some(ls) = listener_status {
                    ls
                } else {
                    let new_ls = ListenerStatus {
                        name,
                        supported_kinds: supported_kinds.clone(),
                        attached_routes: 0,
                        conditions: Vec::new(),
                    };
                    listener_statuses.push(new_ls);
                    listener_statuses.last_mut().unwrap()
                };

                ls.supported_kinds = supported_kinds;
                ls.attached_routes = route_count;

                // Set Conflicted condition based on ListenerPortManager
                let is_conflicted = if let Some((reason, _)) = conflicts.get(&ls.name) {
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
                update_listener_conditions(ls, validation_errors, generation, is_conflicted, &resolved_refs_errors);
            }

            // Remove stale listener statuses for listeners no longer in the spec
            let current_listener_names: Vec<&str> = gateway
                .spec
                .listeners
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|l| l.name.as_str())
                .collect();
            listener_statuses.retain(|ls| current_listener_names.contains(&ls.name.as_str()));
        } else {
            // All listeners removed from spec: clear status listeners
            status.listeners = Some(Vec::new());
        }
    }
}

fn derive_gateway_addresses(gateway: &Gateway, default_address: Option<&str>) -> Vec<GatewayStatusAddress> {
    if let Some(spec_addresses) = gateway.spec.addresses.as_ref().filter(|a| !a.is_empty()) {
        return spec_addresses
            .iter()
            .map(|address| GatewayStatusAddress {
                address_type: address.address_type.clone(),
                value: address.value.clone(),
            })
            .collect();
    }

    let gateway_ns = gateway.metadata.namespace.as_deref().unwrap_or("default");
    let gateway_name = gateway.metadata.name.as_deref().unwrap_or("");
    if !gateway_name.is_empty() {
        if let Some(cluster_ip) = lookup_service_cluster_ip(gateway_ns, gateway_name) {
            return vec![GatewayStatusAddress {
                address_type: Some("IPAddress".to_string()),
                value: cluster_ip,
            }];
        }
    }

    if let Some(addr) = default_address {
        return vec![GatewayStatusAddress {
            address_type: Some("IPAddress".to_string()),
            value: addr.to_string(),
        }];
    }

    vec![GatewayStatusAddress {
        address_type: Some("IPAddress".to_string()),
        value: "0.0.0.0".to_string(),
    }]
}

fn lookup_service_cluster_ip(namespace: &str, name: &str) -> Option<String> {
    let processor = PROCESSOR_REGISTRY.get("Service")?;
    let (json, _) = processor.as_watch_obj().list_json().ok()?;
    let services: Vec<Service> = serde_json::from_str(&json).ok()?;

    services.into_iter().find_map(|svc| {
        let svc_ns = svc.metadata.namespace.as_deref().unwrap_or("default");
        let svc_name = svc.metadata.name.as_deref().unwrap_or("");
        if svc_ns != namespace || svc_name != name {
            return None;
        }

        svc.spec
            .and_then(|spec| spec.cluster_ip)
            .filter(|cluster_ip| !cluster_ip.is_empty() && cluster_ip != "None")
    })
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
    resolved_refs_errors: &[String],
) {
    // Accepted: True if no validation errors (conflict doesn't affect Accepted)
    if validation_errors.is_empty() {
        let cond = condition_true(
            condition_types::ACCEPTED,
            condition_reasons::ACCEPTED,
            "Listener accepted",
            generation,
        );
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

    // ResolvedRefs: True only when all references are valid and resolvable.
    let has_unresolved_refs = !resolved_refs_errors.is_empty();
    if has_unresolved_refs {
        let resolved = condition_false(
            condition_types::RESOLVED_REFS,
            resolved_refs_condition_reason(resolved_refs_errors),
            resolved_refs_errors.join("; "),
            generation,
        );
        update_gateway_condition(&mut ls.conditions, resolved);
    } else {
        let resolved = condition_true(
            condition_types::RESOLVED_REFS,
            condition_reasons::RESOLVED_REFS,
            "All references resolved",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, resolved);
    }

    // Programmed: False if conflicted or has unresolved refs
    if is_conflicted {
        let programmed = condition_false(
            condition_types::PROGRAMMED,
            condition_reasons::LISTENER_CONFLICT,
            "Listener not programmed due to port conflict",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, programmed);
    } else if has_unresolved_refs {
        let programmed = condition_false(
            condition_types::PROGRAMMED,
            condition_reasons::INVALID,
            "Listener not programmed due to unresolved references",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, programmed);
    } else {
        let programmed = condition_true(
            condition_types::PROGRAMMED,
            condition_reasons::PROGRAMMED,
            "Listener configuration programmed",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, programmed);
    }

    // Ready: False if conflicted or has unresolved refs
    if is_conflicted || has_unresolved_refs {
        let reason = if is_conflicted {
            condition_reasons::LISTENER_CONFLICT
        } else {
            condition_reasons::PENDING
        };
        let msg = if is_conflicted {
            "Listener not ready due to port conflict".to_string()
        } else {
            "Listener not ready due to unresolved references".to_string()
        };
        let ready = condition_false(condition_types::READY, reason, msg, generation);
        update_gateway_condition(&mut ls.conditions, ready);
    } else {
        let ready = condition_true(
            condition_types::READY,
            condition_reasons::READY,
            "Listener is ready",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, ready);
    }
}

fn is_valid_pem(data: &[u8]) -> bool {
    x509_parser::pem::Pem::iter_from_buffer(data)
        .next()
        .is_some_and(|r| r.is_ok())
}

fn resolved_refs_condition_reason(resolved_refs_errors: &[String]) -> &'static str {
    if resolved_refs_errors
        .iter()
        .any(|error| error.contains("not allowed by ReferenceGrant"))
    {
        condition_reasons::REF_NOT_PERMITTED
    } else if resolved_refs_errors.iter().any(|error| error.contains("Route kind")) {
        condition_reasons::INVALID_ROUTE_KIND
    } else {
        "InvalidCertificateRef"
    }
}

fn validate_listener_resolved_refs(gateway: &Gateway, listener: &GatewayListener) -> Vec<String> {
    let gateway_ns = gateway.metadata.namespace.as_deref().unwrap_or("default");
    let mut errors = Vec::new();

    let Some(tls) = &listener.tls else {
        return errors;
    };
    let Some(certificate_refs) = &tls.certificate_refs else {
        return errors;
    };

    for cert_ref in certificate_refs {
        let cert_kind = cert_ref.kind.as_deref().unwrap_or("Secret");
        if cert_kind != "Secret" {
            errors.push(format!("Invalid certificateRef kind '{}', must be 'Secret'", cert_kind));
            continue;
        }

        let cert_group = cert_ref.group.as_deref().unwrap_or("");
        if !cert_group.is_empty() && cert_group != "core" {
            errors.push(format!(
                "Invalid certificateRef group '{}', must be empty/core for Secret",
                cert_group
            ));
            continue;
        }

        let cert_ns = cert_ref
            .namespace
            .as_deref()
            .or(gateway.metadata.namespace.as_deref())
            .unwrap_or("default");

        match get_secret(Some(cert_ns), &cert_ref.name) {
            None => {
                errors.push(format!(
                    "Secret '{}/{}' not found for listener '{}'",
                    cert_ns, cert_ref.name, listener.name
                ));
            }
            Some(secret) => {
                use crate::types::constants::secret_keys::tls;
                if let Some(data) = &secret.data {
                    if !data.contains_key(tls::CERT) || !data.contains_key(tls::KEY) {
                        errors.push(format!(
                            "Secret '{}/{}' missing required keys '{}' and/or '{}'",
                            cert_ns,
                            cert_ref.name,
                            tls::CERT,
                            tls::KEY
                        ));
                    } else {
                        let cert_bytes = data.get(tls::CERT).map(|b| &b.0);
                        let key_bytes = data.get(tls::KEY).map(|b| &b.0);
                        let cert_empty = cert_bytes.is_none_or(|b| b.is_empty());
                        let key_empty = key_bytes.is_none_or(|b| b.is_empty());
                        if cert_empty || key_empty {
                            errors.push(format!(
                                "Secret '{}/{}' has empty certificate or key data",
                                cert_ns, cert_ref.name
                            ));
                        } else {
                            if !is_valid_pem(cert_bytes.unwrap()) {
                                errors.push(format!(
                                    "Secret '{}/{}' has invalid certificate PEM data",
                                    cert_ns, cert_ref.name
                                ));
                            }
                            if !is_valid_pem(key_bytes.unwrap()) {
                                errors.push(format!(
                                    "Secret '{}/{}' has invalid private key PEM data",
                                    cert_ns, cert_ref.name
                                ));
                            }
                        }
                    }
                } else {
                    errors.push(format!("Secret '{}/{}' has no data", cert_ns, cert_ref.name));
                }
            }
        }

        if gateway_ns != cert_ns && is_reference_grant_validation_enabled() {
            let allowed = get_global_reference_grant_store().check_reference_allowed(
                gateway_ns,
                "gateway.networking.k8s.io",
                "Gateway",
                cert_ns,
                "",
                "Secret",
                Some(&cert_ref.name),
            );
            if !allowed {
                errors.push(format!(
                    "Cross-namespace reference to Secret '{}/{}' not allowed by ReferenceGrant",
                    cert_ns, cert_ref.name
                ));
            }
        }
    }

    errors
}

/// Compute supported kinds for a listener, honoring allowedRoutes.kinds when provided.
fn compute_supported_kinds(listener: &GatewayListener) -> (Vec<RouteGroupKind>, Vec<String>) {
    let protocol_kinds = get_supported_kinds_for_protocol(&listener.protocol);
    let requested_kinds = listener
        .allowed_routes
        .as_ref()
        .and_then(|allowed_routes| allowed_routes.kinds.as_ref())
        .filter(|kinds| !kinds.is_empty());

    let Some(requested_kinds) = requested_kinds else {
        return (protocol_kinds, Vec::new());
    };

    let mut supported = Vec::new();
    let mut errors = Vec::new();

    for requested in requested_kinds {
        let requested_group = requested.group.as_deref().unwrap_or("gateway.networking.k8s.io");
        let is_valid_for_protocol = protocol_kinds.iter().any(|protocol_kind| {
            let protocol_group = protocol_kind.group.as_deref().unwrap_or("gateway.networking.k8s.io");
            protocol_group == requested_group && protocol_kind.kind == requested.kind
        });

        if is_valid_for_protocol {
            supported.push(requested.clone());
        } else {
            errors.push(format!(
                "Route kind '{}/{}' is not supported for protocol '{}'",
                requested_group, requested.kind, listener.protocol
            ));
        }
    }

    (supported, errors)
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
