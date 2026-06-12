//! Gateway Handler
//!
//! Handles Gateway resources with:
//! - Filter by gateway_class_name
//! - TLS Secret reference resolution
//! - SecretRefManager registration for cascading updates
//! - ListenerPortManager registration for port conflict detection
//! - Gateway API standard status management

use crate::core::common::config::is_reference_grant_validation_enabled;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, format_secret_key, make_conflict_key,
    ConflictKey, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::core::controller::conf_mgr::ControllerRuntime;
use edgion_resources::constants::annotations;
use edgion_resources::constants::app::GATEWAY_NAME;
use edgion_resources::listener_key::resolve_listener_addr;
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::resources::common::is_core_api_group;
use edgion_resources::resources::gateway::{
    FrontendValidationMode, GatewayAddress, GatewayStatus, GatewayStatusAddress, GatewayTLSConfig, GatewayTlsMode,
    Listener as GatewayListener, ListenerStatus, RouteGroupKind,
};
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;
use std::sync::Arc;

const GATEWAY_SYSTEM_NAMESPACE: &str = "edgion-system";

/// Gateway handler
///
/// Features:
/// - filter: Filter by gateway_class_name (optional, None means no filter)
/// - parse: Parse TLS certificateRefs -> fill tls.secrets
/// - parse: Register Secret references to SecretRefManager
/// - parse: Register listeners to ListenerPortManager for intra-gateway conflict detection
/// - on_change: Requeue referencing routes when listener hostnames/ports change
/// - on_delete: Clear SecretRefManager and ListenerPortManager references
/// - update_status: Set Conflicted/Programmed/ListenersNotValid conditions based on intra-gateway port conflicts
pub struct GatewayHandler {
    /// If Some, only process Gateways with matching gatewayClassName
    /// If None, process all Gateways (used by FileSystem mode)
    gateway_class_name: Option<String>,
    /// Static fallback address for Gateway status.addresses
    default_address: Option<String>,
}

/// Typed errors for Gateway listener ResolvedRefs condition.
///
/// Eliminates string-based reason inference in favour of compile-time
/// safe enum matching.
#[derive(Debug, Clone)]
enum GatewayResolvedRefsError {
    /// certificateRef kind is not "Secret"
    InvalidCertRef { kind: String },
    /// Secret referenced by certificateRef not found in the store
    SecretNotFound { namespace: String, name: String },
    /// Secret data is invalid (missing keys, empty, or invalid PEM)
    SecretInvalid {
        namespace: String,
        name: String,
        reason: String,
    },
    /// Cross-namespace Secret reference denied by ReferenceGrant.
    ///
    /// `from_namespace` is the Gateway's own namespace — recorded so the
    /// Condition `message` stays self-describing when consumed outside the
    /// Gateway's `metadata` context (events, log aggregation, dashboards).
    /// The referrer kind is always `Gateway` so it is not stored explicitly.
    RefNotPermitted {
        from_namespace: String,
        namespace: String,
        name: String,
    },
    /// Route kind is not supported for this listener's protocol
    InvalidRouteKind {
        group: String,
        kind: String,
        protocol: String,
    },
    /// CA certificate ref kind is invalid (not Secret or ConfigMap)
    InvalidCACertificateKind { kind: String },
    /// CA certificate ref is invalid (not found, missing key, etc.)
    InvalidCACertificateRef {
        namespace: String,
        name: String,
        reason: String,
    },
    /// CA certificate ref cross-namespace denied by ReferenceGrant.
    ///
    /// `from_namespace` is the Gateway's own namespace — see the note on
    /// `RefNotPermitted` above for rationale.
    CACertRefNotPermitted {
        from_namespace: String,
        namespace: String,
        name: String,
    },
}

impl GatewayResolvedRefsError {
    fn message(&self) -> String {
        match self {
            Self::InvalidCertRef { kind } => {
                format!("Invalid certificateRef kind '{}', must be 'Secret'", kind)
            }
            Self::SecretNotFound { namespace, name } => {
                format!("Secret '{}/{}' not found", namespace, name)
            }
            Self::SecretInvalid {
                namespace,
                name,
                reason,
            } => {
                format!("Secret '{}/{}' invalid: {}", namespace, name, reason)
            }
            Self::RefNotPermitted {
                from_namespace,
                namespace,
                name,
            } => {
                format!(
                    "Reference from {}/Gateway to Secret \"{}/{}\" is not permitted without a ReferenceGrant",
                    from_namespace, namespace, name
                )
            }
            Self::InvalidRouteKind { group, kind, protocol } => {
                format!(
                    "Route kind '{}/{}' is not supported for protocol '{}'",
                    group, kind, protocol
                )
            }
            Self::InvalidCACertificateKind { kind } => {
                format!(
                    "Invalid CA certificate ref kind '{}', must be 'Secret' or 'ConfigMap'",
                    kind
                )
            }
            Self::InvalidCACertificateRef {
                namespace,
                name,
                reason,
            } => {
                format!("CA certificate '{}/{}' invalid: {}", namespace, name, reason)
            }
            Self::CACertRefNotPermitted {
                from_namespace,
                namespace,
                name,
            } => {
                format!(
                    "Reference from {}/Gateway to CA certificate \"{}/{}\" is not permitted without a ReferenceGrant",
                    from_namespace, namespace, name
                )
            }
        }
    }
}

struct ListenerInfo {
    name: String,
    supported_kinds: Vec<RouteGroupKind>,
    route_count: i32,
    resolved_refs_errors: Vec<GatewayResolvedRefsError>,
    /// True when all frontendValidation CA refs are invalid (no valid CA certificate)
    no_valid_ca_certificate: bool,
    /// True when frontend validation uses AllowInsecureFallback mode
    insecure_fallback_mode: bool,
    /// True when listener TLS mode is not supported for its protocol/feature set.
    unsupported_tls_mode: bool,
    /// True when this TLS listener shares a port with another TLS listener
    /// that uses a different termination mode (e.g. Terminate + Passthrough).
    mixed_tls_conflict: bool,
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

#[async_trait::async_trait]
impl ProcessorHandler<Gateway> for GatewayHandler {
    fn validate(&self, g: &Gateway, _ctx: &HandlerContext) -> Vec<String> {
        let mut errors = Vec::new();
        for listener in g.spec.listeners.as_deref().unwrap_or_default() {
            if let Some(from) = listener
                .allowed_routes
                .as_ref()
                .and_then(|ar| ar.namespaces.as_ref())
                .and_then(|ns| ns.from.as_deref())
            {
                if from != "All" && from != "Same" && from != "Selector" {
                    tracing::warn!(
                        ns = g.metadata.namespace.as_deref().unwrap_or(""),
                        name = g.metadata.name.as_deref().unwrap_or(""),
                        kind = "Gateway",
                        field = "allowedRoutes.namespaces.from",
                        value = %from,
                        "Unsupported enum value, rejecting resource"
                    );
                    errors.push(format!(
                        "Unsupported allowedRoutes.namespaces.from value '{}' on listener '{}' (allowed: All, Same, Selector)",
                        from, listener.name
                    ));
                }
            }
        }
        errors
    }

    fn filter(&self, g: &Gateway) -> bool {
        match &self.gateway_class_name {
            Some(class_name) => g.spec.gateway_class_name == *class_name,
            None => true, // No filter, process all Gateways
        }
    }

    async fn parse(&self, mut g: Gateway, ctx: &HandlerContext) -> ProcessResult<Gateway> {
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        // Clear and re-register stream plugin annotation ref (for update/remove annotation scenario)
        ctx.runtime().stream_plugin_ref_mgr.clear_resource_refs(&resource_ref);
        if let Some(v) = g
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(annotations::edgion::EDGION_STREAM_PLUGINS))
        {
            let raw = v.trim();
            if !raw.is_empty() {
                let gateway_ns = g.metadata.namespace.as_deref().unwrap_or("default");
                let (ns, name) = parse_stream_plugin_ref(raw, gateway_ns);
                let key = format!("{}/{}", ns, name);
                ctx.runtime().stream_plugin_ref_mgr.add_ref(key, resource_ref.clone());
            }
        }

        // Track cross-namespace Secret references so ReferenceGrant changes requeue this Gateway
        let cross_ns_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );
        let cross_ns_manager = &ctx.runtime().cross_ns_ref_mgr;
        cross_ns_manager.clear_resource_refs(&cross_ns_ref);

        // Register listeners to ListenerPortManager (empty list clears stale registrations)
        let listener_ports: Vec<(String, ConflictKey, Option<String>)> = g
            .spec
            .listeners
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|l| {
                let addr = resolve_listener_addr(g.spec.addresses.as_ref(), l.port);
                let conflict_key = make_conflict_key(addr);
                (l.name.clone(), conflict_key, l.hostname.clone())
            })
            .collect();
        let gateway_key_for_port_mgr = format!(
            "{}/{}",
            g.metadata.namespace.as_deref().unwrap_or(""),
            g.metadata.name.as_deref().unwrap_or("")
        );
        ctx.runtime()
            .listener_port_manager
            .register_gateway(&gateway_key_for_port_mgr, &listener_ports);

        // Process all Listeners and resolve TLS certificates from global secret store
        if let Some(ref mut listeners) = g.spec.listeners {
            for listener in listeners.iter_mut() {
                let tls_config = match &mut listener.tls {
                    Some(tls) => tls,
                    None => continue,
                };

                // Only Terminate mode needs certificate resolution; Passthrough proxies raw TLS.
                if tls_config.mode.unwrap_or_default() != GatewayTlsMode::Terminate {
                    continue;
                }

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

                        // Try to resolve Secret from runtime store
                        if let Some(secret) = ctx
                            .runtime()
                            .secret_store
                            .get(secret_ns.map(|s| s.as_str()), &cert_ref.name)
                        {
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

                // Resolve frontend validation CA certificates
                let gateway_ns = g.metadata.namespace.as_deref().unwrap_or("");
                if let Some(ref fv) = tls_config.frontend_validation {
                    if let Some(ca_refs) = &fv.ca_certificate_refs {
                        let resolved_ca_secrets = resolve_frontend_ca_refs(
                            ca_refs,
                            &g.metadata.namespace,
                            gateway_ns,
                            &resource_ref,
                            &listener.name,
                            ctx,
                            &cross_ns_ref,
                            &cross_ns_manager,
                        );

                        if !resolved_ca_secrets.is_empty() {
                            tls_config.resolved_frontend_ca_secrets = Some(resolved_ca_secrets);
                        }
                    }
                }
            }
        }

        // Merge spec.tls.frontend into each listener's frontend_validation
        // spec.tls.frontend takes precedence over listener-level frontendValidation
        if let Some(ref spec_tls) = g.spec.tls {
            if let Some(ref frontend) = spec_tls.frontend {
                let gateway_ns = g.metadata.namespace.as_deref().unwrap_or("");

                if let Some(ref mut listeners) = g.spec.listeners {
                    for listener in listeners.iter_mut() {
                        // Only apply to HTTPS/TLS listeners
                        let protocol_upper = listener.protocol.to_uppercase();
                        if protocol_upper != "HTTPS" && protocol_upper != "TLS" {
                            continue;
                        }

                        // Find per-port override for this listener
                        let per_port_validation = frontend
                            .per_port
                            .as_ref()
                            .and_then(|pp| pp.iter().find(|p| p.port == listener.port))
                            .and_then(|p| p.tls.as_ref())
                            .and_then(|t| t.validation.as_ref());

                        // Use per-port if available, otherwise default
                        let merged_validation =
                            per_port_validation.or(frontend.default.as_ref().and_then(|d| d.validation.as_ref()));

                        if let Some(validation) = merged_validation {
                            // Ensure tls config exists
                            let tls_config = listener.tls.get_or_insert(GatewayTLSConfig {
                                mode: None,
                                certificate_refs: None,
                                options: None,
                                secrets: None,
                                frontend_validation: None,
                                resolved_frontend_ca_secrets: None,
                            });

                            // spec.tls.frontend takes precedence over listener-level
                            tls_config.frontend_validation = Some(validation.clone());

                            // Resolve CA refs from the merged validation
                            if let Some(ca_refs) = &validation.ca_certificate_refs {
                                let resolved_ca_secrets = resolve_frontend_ca_refs(
                                    ca_refs,
                                    &g.metadata.namespace,
                                    gateway_ns,
                                    &resource_ref,
                                    &listener.name,
                                    ctx,
                                    &cross_ns_ref,
                                    &cross_ns_manager,
                                );

                                if !resolved_ca_secrets.is_empty() {
                                    tls_config.resolved_frontend_ca_secrets = Some(resolved_ca_secrets);
                                } else {
                                    tls_config.resolved_frontend_ca_secrets = None;
                                }
                            }
                        }
                    }
                }
            }
        }

        ProcessResult::Continue(g)
    }

    async fn on_change(&self, gateway: &Gateway, ctx: &HandlerContext) {
        let gateway_key = format!(
            "{}/{}",
            gateway.metadata.namespace.as_deref().unwrap_or(""),
            gateway.metadata.name.as_deref().unwrap_or("")
        );

        // Requeue routes when listener hostnames or ports actually changed.
        // Cycle safety: route on_change only requeues Gateways when parentRef
        // attachments change; hostname/port-only changes don't trigger Gateway requeue.
        let route_index = &ctx.runtime().gateway_route_index;

        let listeners = gateway.spec.listeners.as_deref().unwrap_or_default();

        let current_hostnames: Vec<String> = listeners.iter().filter_map(|l| l.hostname.clone()).collect();
        let current_ports: Vec<i32> = listeners.iter().map(|l| l.port).collect();

        // CYCLE PREVENTION: Only requeue referencing routes when listener
        // hostnames or ports actually changed. All Route.on_change handlers
        // unconditionally requeue their parent Gateways, so removing this
        // condition would create Route→Gateway→Route infinite loops.
        //
        // Atomicity: change detection and route collection share one write
        // guard inside the index, so a concurrent `update_route` cannot insert
        // a new route between the two steps and slip past requeue.
        let listener_change =
            route_index.update_listener_config_and_collect_routes(&gateway_key, current_hostnames, current_ports);
        match listener_change {
            Some(change) if !change.routes.is_empty() => {
                tracing::info!(
                    gateway = %gateway_key,
                    route_count = change.routes.len(),
                    hostnames_changed = change.hostnames_changed,
                    ports_changed = change.ports_changed,
                    "Gateway listener config changed, requeue referencing routes"
                );
                for (route_kind, route_key) in change.routes {
                    ctx.requeue(route_kind.as_str(), route_key).await;
                }
            }
            Some(_) => {
                // Listener config changed but no routes reference this Gateway yet.
            }
            None => {
                tracing::trace!(
                    gateway = %gateway_key,
                    "Gateway listener hostnames/ports unchanged, skip requeue referencing routes"
                );
            }
        }
    }

    async fn on_delete(&self, g: &Gateway, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );
        // Clear SecretRefManager references
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        ctx.runtime().stream_plugin_ref_mgr.clear_resource_refs(&resource_ref);

        // Clear cross-namespace reference tracking
        let cross_ns_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&cross_ns_ref);

        // Clear ListenerPortManager registration (use ns/name format, same as parse/update_status)
        let gateway_key = format!(
            "{}/{}",
            g.metadata.namespace.as_deref().unwrap_or(""),
            g.metadata.name.as_deref().unwrap_or("")
        );
        ctx.runtime().listener_port_manager.unregister_gateway(&gateway_key);

        let route_index = &ctx.runtime().gateway_route_index;
        route_index.remove_gateway_hostnames(&gateway_key);
        route_index.remove_gateway_ports(&gateway_key);

        tracing::debug!(
            gateway = %gateway_key,
            "Cleared secret, port manager, hostname and port cache on Gateway delete"
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
        let tracker = &_ctx.runtime().attached_route_tracker;
        let gateway_key_for_tracker = format!("{}/{}", gateway_ns, gateway_name);

        // Pre-compute per-listener info while gateway is only immutably borrowed.
        let listeners_slice = gateway.spec.listeners.as_deref().unwrap_or_default();

        let mixed_tls_listeners = detect_mixed_tls_mode_listeners(listeners_slice, gateway.spec.addresses.as_ref());

        let listener_infos: Vec<ListenerInfo> = listeners_slice
            .iter()
            .map(|l| {
                let route_count = tracker.count_for_listener(&gateway_key_for_tracker, &l.name);
                let (supported_kinds, mut kind_errors) = compute_supported_kinds(l);
                let mut resolved_refs_errors = validate_listener_resolved_refs(gateway, l, _ctx.runtime());
                resolved_refs_errors.append(&mut kind_errors);

                let no_valid_ca_certificate = has_all_ca_refs_invalid(l, &resolved_refs_errors);

                let insecure_fallback_mode = l
                    .tls
                    .as_ref()
                    .and_then(|t| t.frontend_validation.as_ref())
                    .and_then(|fv| fv.mode.as_ref())
                    == Some(&FrontendValidationMode::AllowInsecureFallback);

                let unsupported_tls_mode = listener_has_unsupported_tls_mode(l);
                let mixed_tls_conflict = mixed_tls_listeners.contains(&l.name);

                let effective_kinds = if mixed_tls_conflict {
                    Vec::new()
                } else {
                    supported_kinds
                };

                ListenerInfo {
                    name: l.name.clone(),
                    supported_kinds: effective_kinds,
                    route_count,
                    resolved_refs_errors,
                    no_valid_ca_certificate,
                    insecure_fallback_mode,
                    unsupported_tls_mode,
                    mixed_tls_conflict,
                }
            })
            .collect();

        let derived_addresses = derive_gateway_addresses(gateway, self.default_address.as_deref(), _ctx.runtime());

        // Pre-compute stream plugin ref validation before taking a mutable borrow of gateway.
        let stream_plugin_ref_error = validate_gateway_stream_plugin_ref(gateway, _ctx.runtime());

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
        let conflicts = _ctx.runtime().listener_port_manager.detect_conflicts(&gateway_key);

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

        // Gateway-level ResolvedRefs: True unless edgion.io/edgion-stream-plugins
        // annotation references a missing EdgionStreamPlugins resource.
        {
            match stream_plugin_ref_error {
                None => {
                    let cond = condition_true(
                        condition_types::RESOLVED_REFS,
                        condition_reasons::RESOLVED_REFS,
                        "All gateway-level references resolved",
                        generation,
                    );
                    update_gateway_condition(conditions, cond);
                }
                Some(message) => {
                    tracing::warn!(
                        annotation = annotations::edgion::EDGION_STREAM_PLUGINS,
                        error = %message,
                        "Gateway-level stream plugin ref unresolved; stream plugin chain will be empty"
                    );
                    let cond = condition_false(
                        condition_types::RESOLVED_REFS,
                        condition_reasons::INVALID_STREAM_PLUGINS_REF,
                        message,
                        generation,
                    );
                    update_gateway_condition(conditions, cond);
                }
            }
        }

        // Gateway-level Programmed condition:
        // - True when gateway-level validation passes (Accepted=True), meaning
        //   the controller has fully processed this Gateway at the current generation.
        // - False only when there are gateway-level validation errors.
        //
        // Per Gateway API spec (GEP-1364): gateway-level Programmed indicates the
        // controller has processed the Gateway spec and sent configuration to the
        // data plane.  Listener-level issues (port conflicts, invalid TLS refs) are
        // reflected in per-listener Conflicted/Programmed conditions, not at the
        // gateway level.  This mirrors how reference implementations behave and is
        // required by the conformance test framework (NamespacesMustBeReady waits
        // for Programmed=True on ALL infrastructure gateways).
        {
            if validation_errors.is_empty() {
                let cond = condition_true(
                    condition_types::PROGRAMMED,
                    condition_reasons::PROGRAMMED,
                    "Gateway programmed",
                    generation,
                );
                update_gateway_condition(conditions, cond);
            } else {
                let cond = condition_false(
                    condition_types::PROGRAMMED,
                    condition_reasons::INVALID_LISTENER,
                    validation_errors.join("; "),
                    generation,
                );
                update_gateway_condition(conditions, cond);
            }
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

        // InsecureFrontendValidationMode: True if any listener uses AllowInsecureFallback
        let has_insecure_fallback = listener_infos.iter().any(|li| li.insecure_fallback_mode);
        if has_insecure_fallback {
            let cond = condition_true(
                "InsecureFrontendValidationMode",
                "ConfigurationChanged",
                "Frontend validation uses AllowInsecureFallback mode",
                generation,
            );
            update_gateway_condition(conditions, cond);
        } else {
            conditions.retain(|c| c.type_ != "InsecureFrontendValidationMode");
        }

        // Update listener statuses using pre-computed info
        if !listener_infos.is_empty() {
            let listener_statuses = status.listeners.get_or_insert_with(Vec::new);

            for ListenerInfo {
                name,
                supported_kinds,
                route_count,
                resolved_refs_errors,
                no_valid_ca_certificate,
                insecure_fallback_mode: _,
                unsupported_tls_mode,
                mixed_tls_conflict,
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
                    listener_statuses.last_mut().expect("just pushed; Vec is non-empty")
                };

                ls.supported_kinds = supported_kinds;
                ls.attached_routes = route_count;

                // Set Conflicted condition: mixed TLS mode or ListenerPortManager conflicts
                let is_conflicted = if mixed_tls_conflict {
                    let cond = condition_true(
                        condition_types::CONFLICTED,
                        condition_reasons::PROTOCOL_CONFLICT,
                        "Mixed TLS termination modes on the same port are not supported",
                        generation,
                    );
                    update_gateway_condition(&mut ls.conditions, cond);
                    true
                } else if let Some(reason) = conflicts.get(&ls.name) {
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

                update_listener_conditions(
                    ls,
                    validation_errors,
                    generation,
                    is_conflicted,
                    &resolved_refs_errors,
                    no_valid_ca_certificate,
                    unsupported_tls_mode,
                    mixed_tls_conflict,
                );
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

/// Resolve frontend CA certificate refs, supporting both Secret and ConfigMap kinds.
/// For ConfigMap refs, creates a synthetic Secret with the `ca.crt` data.
#[allow(clippy::too_many_arguments)]
fn resolve_frontend_ca_refs(
    ca_refs: &[edgion_resources::resources::gateway::SecretObjectReference],
    gateway_namespace: &Option<String>,
    gateway_ns: &str,
    resource_ref: &ResourceRef,
    listener_name: &str,
    ctx: &HandlerContext,
    cross_ns_ref: &ResourceRef,
    cross_ns_manager: &crate::core::controller::conf_mgr::sync_runtime::resource_processor::ref_grant::CrossNamespaceRefManager,
) -> Vec<k8s_openapi::api::core::v1::Secret> {
    let mut resolved_ca_secrets = Vec::new();

    for ca_ref in ca_refs {
        let ca_kind = ca_ref.kind.as_deref().unwrap_or("Secret");
        let ca_ns = ca_ref.namespace.as_ref().or(gateway_namespace.as_ref());
        let ca_ns_str = ca_ns.map(|s| s.as_str()).unwrap_or("");

        // Register cross-namespace refs
        if ca_ns_str != gateway_ns {
            cross_ns_manager.add_ref(ca_ns_str.to_string(), cross_ns_ref.clone());
        }

        match ca_kind {
            "Secret" => {
                let secret_key = format_secret_key(ca_ns, &ca_ref.name);
                ctx.secret_ref_manager()
                    .add_ref(secret_key.clone(), resource_ref.clone());

                if let Some(secret) = ctx.runtime().secret_store.get(ca_ns.map(|s| s.as_str()), &ca_ref.name) {
                    resolved_ca_secrets.push(secret);
                    tracing::debug!(
                        gateway = %resource_ref.key(),
                        listener = %listener_name,
                        secret_key = %secret_key,
                        "Frontend CA Secret resolved"
                    );
                } else {
                    tracing::info!(
                        gateway = %resource_ref.key(),
                        listener = %listener_name,
                        secret_key = %secret_key,
                        "Frontend CA Secret not found yet"
                    );
                }
            }
            "ConfigMap" => {
                if let Some(cm) = ctx.runtime().configmap_store.get(Some(ca_ns_str), &ca_ref.name) {
                    if let Some(data) = &cm.data {
                        if let Some(ca_pem) = data.get("ca.crt") {
                            // Create a synthetic Secret from ConfigMap data
                            let mut secret_data = BTreeMap::new();
                            secret_data.insert("ca.crt".to_string(), ByteString(ca_pem.as_bytes().to_vec()));
                            let fake_secret = k8s_openapi::api::core::v1::Secret {
                                data: Some(secret_data),
                                ..Default::default()
                            };
                            resolved_ca_secrets.push(fake_secret);
                            tracing::debug!(
                                gateway = %resource_ref.key(),
                                listener = %listener_name,
                                configmap = %ca_ref.name,
                                "Frontend CA ConfigMap resolved"
                            );
                        }
                    }
                } else {
                    tracing::info!(
                        gateway = %resource_ref.key(),
                        listener = %listener_name,
                        configmap = %ca_ref.name,
                        "Frontend CA ConfigMap not found yet"
                    );
                }
            }
            _ => {
                tracing::warn!(
                    gateway = %resource_ref.key(),
                    listener = %listener_name,
                    kind = %ca_kind,
                    "Unsupported CA certificate ref kind"
                );
            }
        }
    }

    resolved_ca_secrets
}

fn derive_gateway_addresses(
    gateway: &Gateway,
    default_address: Option<&str>,
    runtime: &ControllerRuntime,
) -> Vec<GatewayStatusAddress> {
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
    for (service_ns, service_name) in gateway_service_lookup_candidates(gateway_ns, gateway_name) {
        if let Some(cluster_ip) = lookup_service_cluster_ip(&service_ns, &service_name, runtime) {
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

fn gateway_service_lookup_candidates(gateway_ns: &str, gateway_name: &str) -> Vec<(String, String)> {
    let mut candidates = vec![(GATEWAY_SYSTEM_NAMESPACE.to_string(), GATEWAY_NAME.to_string())];

    if !gateway_name.is_empty() && (gateway_ns != GATEWAY_SYSTEM_NAMESPACE || gateway_name != GATEWAY_NAME) {
        candidates.push((gateway_ns.to_string(), gateway_name.to_string()));
    }

    candidates
}

fn lookup_service_cluster_ip(namespace: &str, name: &str, runtime: &ControllerRuntime) -> Option<String> {
    let processor = runtime.processor_registry.get("Service")?;
    let key = format!("{}/{}", namespace, name);
    let svc: Arc<Service> = processor.get_raw_any(&key)?.downcast::<Service>().ok()?;
    svc.spec
        .as_ref()
        .and_then(|spec| spec.cluster_ip.clone())
        .filter(|cluster_ip| !cluster_ip.is_empty() && cluster_ip != "None")
}

/// Update or insert a condition in Gateway conditions list
fn update_gateway_condition(
    conditions: &mut Vec<edgion_resources::resources::common::Condition>,
    new_condition: edgion_resources::resources::common::Condition,
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
    resolved_refs_errors: &[GatewayResolvedRefsError],
    no_valid_ca_certificate: bool,
    unsupported_tls_mode: bool,
    mixed_tls_conflict: bool,
) {
    // Accepted: False if mixed TLS termination on the same port
    if mixed_tls_conflict {
        let cond = condition_false(
            condition_types::ACCEPTED,
            condition_reasons::PROTOCOL_CONFLICT,
            "Mixed TLS termination modes on the same port are not supported",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, cond);
    } else if unsupported_tls_mode {
        let cond = condition_false(
            condition_types::ACCEPTED,
            condition_reasons::UNSUPPORTED_VALUE,
            "Listener TLS mode is not supported",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, cond);
    } else if no_valid_ca_certificate {
        let cond = condition_false(
            condition_types::ACCEPTED,
            condition_reasons::NO_VALID_CA_CERTIFICATE,
            "All frontend validation CA certificate references are invalid",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, cond);
    } else if validation_errors.is_empty() {
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
        let reason = pick_gateway_resolved_refs_reason(resolved_refs_errors);
        let message = resolved_refs_errors
            .iter()
            .map(|e| e.message())
            .collect::<Vec<_>>()
            .join("; ");
        let resolved = condition_false(condition_types::RESOLVED_REFS, reason, message, generation);
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

    // Programmed: True if listener is accepted, not conflicted, and all refs resolved; False otherwise.
    if is_conflicted {
        let cond = condition_false(
            condition_types::PROGRAMMED,
            condition_reasons::INVALID_LISTENER,
            "Listener has port conflicts",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, cond);
    } else if has_unresolved_refs
        || !validation_errors.is_empty()
        || no_valid_ca_certificate
        || unsupported_tls_mode
        || mixed_tls_conflict
    {
        let cond = condition_false(
            condition_types::PROGRAMMED,
            condition_reasons::INVALID_LISTENER,
            "Listener configuration is invalid",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, cond);
    } else {
        let cond = condition_true(
            condition_types::PROGRAMMED,
            condition_reasons::PROGRAMMED,
            "Listener programmed",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, cond);
    }
}

fn is_valid_pem(data: &[u8]) -> bool {
    let mut iter = x509_parser::pem::Pem::iter_from_buffer(data);
    let mut any = false;
    for r in iter.by_ref() {
        any = true;
        if r.is_err() {
            return false;
        }
    }
    any
}

fn pick_gateway_resolved_refs_reason(errors: &[GatewayResolvedRefsError]) -> &'static str {
    // Priority: InvalidRouteKind > InvalidCACertificateKind > RefNotPermitted/CACertRefNotPermitted > InvalidCACertificateRef > InvalidCertificateRef
    if errors
        .iter()
        .any(|e| matches!(e, GatewayResolvedRefsError::InvalidRouteKind { .. }))
    {
        return condition_reasons::INVALID_ROUTE_KIND;
    }
    if errors
        .iter()
        .any(|e| matches!(e, GatewayResolvedRefsError::InvalidCACertificateKind { .. }))
    {
        return condition_reasons::INVALID_CA_CERTIFICATE_KIND;
    }
    if errors.iter().any(|e| {
        matches!(
            e,
            GatewayResolvedRefsError::RefNotPermitted { .. } | GatewayResolvedRefsError::CACertRefNotPermitted { .. }
        )
    }) {
        return condition_reasons::REF_NOT_PERMITTED;
    }
    if errors
        .iter()
        .any(|e| matches!(e, GatewayResolvedRefsError::InvalidCACertificateRef { .. }))
    {
        return condition_reasons::INVALID_CA_CERTIFICATE_REF;
    }
    condition_reasons::INVALID_CERTIFICATE_REF
}

fn validate_listener_resolved_refs(
    gateway: &Gateway,
    listener: &GatewayListener,
    runtime: &ControllerRuntime,
) -> Vec<GatewayResolvedRefsError> {
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
            errors.push(GatewayResolvedRefsError::InvalidCertRef {
                kind: cert_kind.to_string(),
            });
            continue;
        }

        let cert_group = cert_ref.group.as_deref().unwrap_or("");
        if !is_core_api_group(cert_group) {
            errors.push(GatewayResolvedRefsError::InvalidCertRef {
                kind: format!("group:{}", cert_group),
            });
            continue;
        }

        let cert_ns = cert_ref
            .namespace
            .as_deref()
            .or(gateway.metadata.namespace.as_deref())
            .unwrap_or("default");

        match runtime.secret_store.get(Some(cert_ns), &cert_ref.name) {
            None => {
                errors.push(GatewayResolvedRefsError::SecretNotFound {
                    namespace: cert_ns.to_string(),
                    name: cert_ref.name.clone(),
                });
            }
            Some(secret) => {
                use edgion_resources::constants::secret_keys::tls;
                if let Some(data) = &secret.data {
                    if !data.contains_key(tls::CERT) || !data.contains_key(tls::KEY) {
                        errors.push(GatewayResolvedRefsError::SecretInvalid {
                            namespace: cert_ns.to_string(),
                            name: cert_ref.name.clone(),
                            reason: format!("missing required keys '{}' and/or '{}'", tls::CERT, tls::KEY),
                        });
                    } else {
                        let cert_bytes = data.get(tls::CERT).map(|b| &b.0);
                        let key_bytes = data.get(tls::KEY).map(|b| &b.0);
                        let cert_empty = cert_bytes.is_none_or(|b| b.is_empty());
                        let key_empty = key_bytes.is_none_or(|b| b.is_empty());
                        if cert_empty || key_empty {
                            errors.push(GatewayResolvedRefsError::SecretInvalid {
                                namespace: cert_ns.to_string(),
                                name: cert_ref.name.clone(),
                                reason: "empty certificate or key data".to_string(),
                            });
                        } else {
                            let cert_data =
                                cert_bytes.expect("cert_bytes is Some: contains_key and non-empty verified above");
                            let key_data =
                                key_bytes.expect("key_bytes is Some: contains_key and non-empty verified above");
                            let cert_pem_valid = is_valid_pem(cert_data);
                            let key_pem_valid = is_valid_pem(key_data);

                            if !cert_pem_valid {
                                errors.push(GatewayResolvedRefsError::SecretInvalid {
                                    namespace: cert_ns.to_string(),
                                    name: cert_ref.name.clone(),
                                    reason: "invalid certificate PEM data".to_string(),
                                });
                            }
                            if !key_pem_valid {
                                errors.push(GatewayResolvedRefsError::SecretInvalid {
                                    namespace: cert_ns.to_string(),
                                    name: cert_ref.name.clone(),
                                    reason: "invalid private key PEM data".to_string(),
                                });
                            }

                            // Check cert/key matching only when both PEMs are valid
                            if cert_pem_valid && key_pem_valid {
                                if let (Ok(cert_str), Ok(key_str)) =
                                    (std::str::from_utf8(cert_data), std::str::from_utf8(key_data))
                                {
                                    if let Err(e) =
                                        crate::core::common::tls_validation::check_key_matches_cert(cert_str, key_str)
                                    {
                                        errors.push(GatewayResolvedRefsError::SecretInvalid {
                                            namespace: cert_ns.to_string(),
                                            name: cert_ref.name.clone(),
                                            reason: e.to_string(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                } else {
                    errors.push(GatewayResolvedRefsError::SecretInvalid {
                        namespace: cert_ns.to_string(),
                        name: cert_ref.name.clone(),
                        reason: "no data".to_string(),
                    });
                }
            }
        }

        if gateway_ns != cert_ns && is_reference_grant_validation_enabled() {
            let allowed = runtime.ref_grant_store.check_reference_allowed(
                gateway_ns,
                "gateway.networking.k8s.io",
                "Gateway",
                cert_ns,
                "",
                "Secret",
                Some(&cert_ref.name),
            );
            if !allowed {
                errors.push(GatewayResolvedRefsError::RefNotPermitted {
                    from_namespace: gateway_ns.to_string(),
                    namespace: cert_ns.to_string(),
                    name: cert_ref.name.clone(),
                });
            }
        }
    }

    // Validate frontend validation CA certificate refs (supports both Secret and ConfigMap)
    if let Some(ref fv) = tls.frontend_validation {
        if let Some(ca_refs) = &fv.ca_certificate_refs {
            for ca_ref in ca_refs {
                let ca_kind = ca_ref.kind.as_deref().unwrap_or("Secret");

                let ca_group = ca_ref.group.as_deref().unwrap_or("");
                if !is_core_api_group(ca_group) {
                    errors.push(GatewayResolvedRefsError::InvalidCACertificateKind {
                        kind: format!("group:{}", ca_group),
                    });
                    continue;
                }

                let ca_ns = ca_ref
                    .namespace
                    .as_deref()
                    .or(gateway.metadata.namespace.as_deref())
                    .unwrap_or("default");

                match ca_kind {
                    "Secret" => {
                        match runtime.secret_store.get(Some(ca_ns), &ca_ref.name) {
                            None => {
                                errors.push(GatewayResolvedRefsError::InvalidCACertificateRef {
                                    namespace: ca_ns.to_string(),
                                    name: ca_ref.name.clone(),
                                    reason: "Secret not found".to_string(),
                                });
                            }
                            Some(secret) => {
                                use edgion_resources::constants::secret_keys::tls;
                                if let Some(data) = &secret.data {
                                    if !data.contains_key(tls::CA_CERT) {
                                        errors.push(GatewayResolvedRefsError::InvalidCACertificateRef {
                                            namespace: ca_ns.to_string(),
                                            name: ca_ref.name.clone(),
                                            reason: format!("missing required key '{}'", tls::CA_CERT),
                                        });
                                    }
                                } else {
                                    errors.push(GatewayResolvedRefsError::InvalidCACertificateRef {
                                        namespace: ca_ns.to_string(),
                                        name: ca_ref.name.clone(),
                                        reason: "Secret has no data".to_string(),
                                    });
                                }
                            }
                        }

                        // Cross-namespace ReferenceGrant check
                        if gateway_ns != ca_ns && is_reference_grant_validation_enabled() {
                            let allowed = runtime.ref_grant_store.check_reference_allowed(
                                gateway_ns,
                                "gateway.networking.k8s.io",
                                "Gateway",
                                ca_ns,
                                "",
                                "Secret",
                                Some(&ca_ref.name),
                            );
                            if !allowed {
                                errors.push(GatewayResolvedRefsError::CACertRefNotPermitted {
                                    from_namespace: gateway_ns.to_string(),
                                    namespace: ca_ns.to_string(),
                                    name: ca_ref.name.clone(),
                                });
                            }
                        }
                    }
                    "ConfigMap" => {
                        match runtime.configmap_store.get(Some(ca_ns), &ca_ref.name) {
                            None => {
                                errors.push(GatewayResolvedRefsError::InvalidCACertificateRef {
                                    namespace: ca_ns.to_string(),
                                    name: ca_ref.name.clone(),
                                    reason: "ConfigMap not found".to_string(),
                                });
                            }
                            Some(cm) => {
                                if let Some(data) = &cm.data {
                                    if !data.contains_key("ca.crt") {
                                        errors.push(GatewayResolvedRefsError::InvalidCACertificateRef {
                                            namespace: ca_ns.to_string(),
                                            name: ca_ref.name.clone(),
                                            reason: "missing required key 'ca.crt'".to_string(),
                                        });
                                    }
                                } else {
                                    errors.push(GatewayResolvedRefsError::InvalidCACertificateRef {
                                        namespace: ca_ns.to_string(),
                                        name: ca_ref.name.clone(),
                                        reason: "ConfigMap has no data".to_string(),
                                    });
                                }
                            }
                        }

                        // Cross-namespace ReferenceGrant check for ConfigMap
                        if gateway_ns != ca_ns && is_reference_grant_validation_enabled() {
                            let allowed = runtime.ref_grant_store.check_reference_allowed(
                                gateway_ns,
                                "gateway.networking.k8s.io",
                                "Gateway",
                                ca_ns,
                                "",
                                "ConfigMap",
                                Some(&ca_ref.name),
                            );
                            if !allowed {
                                errors.push(GatewayResolvedRefsError::CACertRefNotPermitted {
                                    from_namespace: gateway_ns.to_string(),
                                    namespace: ca_ns.to_string(),
                                    name: ca_ref.name.clone(),
                                });
                            }
                        }
                    }
                    _ => {
                        errors.push(GatewayResolvedRefsError::InvalidCACertificateKind {
                            kind: ca_kind.to_string(),
                        });
                    }
                }
            }
        }
    }

    errors
}

/// Check if a listener has frontend validation CA refs and ALL of them are invalid.
/// Returns true only when CA refs were configured but none are valid.
fn has_all_ca_refs_invalid(listener: &GatewayListener, errors: &[GatewayResolvedRefsError]) -> bool {
    let ca_ref_count = listener
        .tls
        .as_ref()
        .and_then(|t| t.frontend_validation.as_ref())
        .and_then(|fv| fv.ca_certificate_refs.as_ref())
        .map(|refs| refs.len())
        .unwrap_or(0);

    if ca_ref_count == 0 {
        return false;
    }

    // A single CA ref can produce multiple errors (e.g. both `InvalidCACertificateRef`
    // for missing key AND `CACertRefNotPermitted` for cross-ns without ReferenceGrant).
    // Count distinct invalid refs by (namespace, name) so a multi-error ref is not
    // double-counted as if it were multiple invalid refs.
    let mut invalid_refs: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
    let mut kind_error_count = 0usize;
    for e in errors {
        match e {
            GatewayResolvedRefsError::InvalidCACertificateRef { namespace, name, .. }
            | GatewayResolvedRefsError::CACertRefNotPermitted { namespace, name, .. } => {
                invalid_refs.insert((namespace.as_str(), name.as_str()));
            }
            // `InvalidCACertificateKind` carries no ref identity, but the producing
            // sites short-circuit further checks for that ref, so each occurrence
            // corresponds to exactly one distinct invalid ref.
            GatewayResolvedRefsError::InvalidCACertificateKind { .. } => kind_error_count += 1,
            _ => {}
        }
    }

    invalid_refs.len() + kind_error_count >= ca_ref_count
}

/// Returns the set of listener names that participate in a mixed TLS termination
/// conflict — i.e. TLS listeners on the same port with different modes
/// (e.g. one Terminate and one Passthrough).
fn detect_mixed_tls_mode_listeners(
    listeners: &[GatewayListener],
    addresses: Option<&Vec<GatewayAddress>>,
) -> std::collections::HashSet<String> {
    use std::collections::HashMap;
    let mut port_modes: HashMap<u16, Vec<(String, GatewayTlsMode)>> = HashMap::new();

    for l in listeners {
        if !l.protocol.eq_ignore_ascii_case("TLS") {
            continue;
        }
        let mode = l.tls.as_ref().and_then(|t| t.mode).unwrap_or_default();
        let addr = resolve_listener_addr(addresses, l.port);
        port_modes.entry(addr.port()).or_default().push((l.name.clone(), mode));
    }

    let mut result = std::collections::HashSet::new();
    for entries in port_modes.values() {
        if entries.len() < 2 {
            continue;
        }
        let first_mode = &entries[0].1;
        if entries.iter().any(|(_, m)| m != first_mode) {
            for (name, _) in entries {
                result.insert(name.clone());
            }
        }
    }
    result
}

fn listener_has_unsupported_tls_mode(listener: &GatewayListener) -> bool {
    // The CRD enum + serde rejects values other than Terminate/Passthrough,
    // so the only remaining semantic constraint is: HTTPS does not support
    // Passthrough (the listener must terminate TLS to expose HTTP routes).
    let Some(mode) = listener.tls.as_ref().and_then(|tls| tls.mode.as_ref()) else {
        return false;
    };
    listener.protocol.eq_ignore_ascii_case("HTTPS") && *mode == GatewayTlsMode::Passthrough
}

/// Compute supported kinds for a listener, honoring allowedRoutes.kinds when provided.
fn compute_supported_kinds(listener: &GatewayListener) -> (Vec<RouteGroupKind>, Vec<GatewayResolvedRefsError>) {
    if listener_has_unsupported_tls_mode(listener) {
        return (Vec::new(), Vec::new());
    }

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
            errors.push(GatewayResolvedRefsError::InvalidRouteKind {
                group: requested_group.to_string(),
                kind: requested.kind.clone(),
                protocol: listener.protocol.clone(),
            });
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

/// Parse `"namespace/name"` annotation value.
/// Falls back to `gateway_ns` when the value contains no slash.
/// Multi-slash values (e.g. `"a/b/c"`) yield `name = "b/c"`, which will never
/// match a real resource key — callers treat this as not found.
fn parse_stream_plugin_ref(value: &str, gateway_ns: &str) -> (String, String) {
    if let Some((ns, name)) = value.split_once('/') {
        (ns.to_string(), name.to_string())
    } else {
        (gateway_ns.to_string(), value.to_string())
    }
}

/// Returns `Some(error_message)` when the Gateway has the
/// `edgion.io/edgion-stream-plugins` annotation but the referenced
/// `EdgionStreamPlugins` resource does not exist in the processor registry.
/// Returns `None` when: annotation absent, annotation empty, or resource found.
fn validate_gateway_stream_plugin_ref(gateway: &Gateway, runtime: &ControllerRuntime) -> Option<String> {
    let annotation_value = gateway
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(annotations::edgion::EDGION_STREAM_PLUGINS))?;
    let annotation_value = annotation_value.trim();
    if annotation_value.is_empty() {
        return None;
    }
    let gateway_ns = gateway.metadata.namespace.as_deref().unwrap_or("default");
    let (ns, name) = parse_stream_plugin_ref(annotation_value, gateway_ns);
    let key = format!("{}/{}", ns, name);
    let exists = runtime
        .processor_registry
        .get("EdgionStreamPlugins")
        .and_then(|p| p.get_raw_any(&key))
        .is_some();
    if exists {
        None
    } else {
        Some(format!("EdgionStreamPlugins '{}' not found", key))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        gateway_service_lookup_candidates, has_all_ca_refs_invalid, parse_stream_plugin_ref,
        validate_gateway_stream_plugin_ref, GatewayResolvedRefsError,
    };
    use crate::core::controller::conf_mgr::ControllerRuntime;
    use edgion_resources::constants::app::GATEWAY_NAME;
    use edgion_resources::prelude_resources::Gateway;
    use edgion_resources::resources::common::SecretObjectReference;
    use edgion_resources::resources::gateway::{
        FrontendTLSValidation, GatewaySpec, GatewayTLSConfig, GatewayTlsMode, Listener as GatewayListener,
    };
    use std::collections::BTreeMap;

    fn make_test_gateway(ns: &str) -> Gateway {
        Gateway {
            metadata: kube::core::ObjectMeta {
                namespace: Some(ns.to_string()),
                ..Default::default()
            },
            spec: GatewaySpec {
                gateway_class_name: "test".to_string(),
                listeners: None,
                addresses: None,
                tls: None,
            },
            status: None,
        }
    }

    fn make_gateway_with_annotation(ns: &str, annotation_value: &str) -> Gateway {
        let mut gw = make_test_gateway(ns);
        let mut annotations = BTreeMap::new();
        annotations.insert(
            "edgion.io/edgion-stream-plugins".to_string(),
            annotation_value.to_string(),
        );
        gw.metadata.annotations = Some(annotations);
        gw
    }

    #[test]
    fn validate_stream_plugin_ref_no_annotation_returns_none() {
        let runtime = ControllerRuntime::default();
        let gw = make_test_gateway("default");
        assert!(validate_gateway_stream_plugin_ref(&gw, &runtime).is_none());
    }

    #[test]
    fn validate_stream_plugin_ref_empty_annotation_returns_none() {
        let runtime = ControllerRuntime::default();
        let gw = make_gateway_with_annotation("default", "  ");
        assert!(validate_gateway_stream_plugin_ref(&gw, &runtime).is_none());
    }

    #[test]
    fn validate_stream_plugin_ref_missing_resource_returns_error() {
        let runtime = ControllerRuntime::default();
        let gw = make_gateway_with_annotation("default", "default/my-stream");
        let result = validate_gateway_stream_plugin_ref(&gw, &runtime);
        assert!(result.is_some());
        assert!(result.unwrap().contains("default/my-stream"));
    }

    #[test]
    fn validate_stream_plugin_ref_fallback_to_gateway_ns() {
        let runtime = ControllerRuntime::default();
        let gw = make_gateway_with_annotation("my-ns", "my-stream");
        let result = validate_gateway_stream_plugin_ref(&gw, &runtime);
        assert!(result.is_some());
        assert!(result.unwrap().contains("my-ns/my-stream"));
    }

    #[test]
    fn parse_stream_plugin_ref_with_slash() {
        let (ns, name) = parse_stream_plugin_ref("other-ns/my-plugin", "default");
        assert_eq!(ns, "other-ns");
        assert_eq!(name, "my-plugin");
    }

    #[test]
    fn parse_stream_plugin_ref_without_slash_uses_gateway_ns() {
        let (ns, name) = parse_stream_plugin_ref("my-plugin", "default");
        assert_eq!(ns, "default");
        assert_eq!(name, "my-plugin");
    }

    #[test]
    fn parse_stream_plugin_ref_multi_slash_name_becomes_remainder() {
        let (ns, name) = parse_stream_plugin_ref("a/b/c", "default");
        assert_eq!(ns, "a");
        assert_eq!(name, "b/c");
    }

    fn listener_with_ca_refs(refs: Vec<(&str, &str)>) -> GatewayListener {
        let ca_refs = refs
            .into_iter()
            .map(|(ns, name)| SecretObjectReference {
                group: None,
                kind: Some("Secret".to_string()),
                name: name.to_string(),
                namespace: Some(ns.to_string()),
            })
            .collect();
        GatewayListener {
            name: "https".to_string(),
            hostname: None,
            port: 443,
            protocol: "HTTPS".to_string(),
            tls: Some(GatewayTLSConfig {
                mode: Some(GatewayTlsMode::Terminate),
                certificate_refs: None,
                options: None,
                secrets: None,
                frontend_validation: Some(FrontendTLSValidation {
                    ca_certificate_refs: Some(ca_refs),
                    mode: None,
                }),
                resolved_frontend_ca_secrets: None,
            }),
            allowed_routes: None,
        }
    }

    #[test]
    fn gateway_service_lookup_prefers_fixed_edgion_gateway_service() {
        let candidates = gateway_service_lookup_candidates("gateway-conformance-infra", "same-namespace");

        assert_eq!(
            candidates,
            vec![
                ("edgion-system".to_string(), GATEWAY_NAME.to_string()),
                ("gateway-conformance-infra".to_string(), "same-namespace".to_string()),
            ]
        );
    }

    #[test]
    fn gateway_service_lookup_avoids_duplicate_fixed_service_candidate() {
        let candidates = gateway_service_lookup_candidates("edgion-system", GATEWAY_NAME);

        assert_eq!(
            candidates,
            vec![("edgion-system".to_string(), GATEWAY_NAME.to_string())]
        );
    }

    #[test]
    fn has_all_ca_refs_invalid_returns_false_when_no_refs_configured() {
        let listener = listener_with_ca_refs(vec![]);
        assert!(!has_all_ca_refs_invalid(&listener, &[]));
    }

    #[test]
    fn has_all_ca_refs_invalid_does_not_double_count_multi_error_ref() {
        // refA produces both InvalidCACertificateRef AND CACertRefNotPermitted;
        // refB is fully valid (no errors). Old logic would count 2 errors >= 2 refs
        // and incorrectly return true.
        let listener = listener_with_ca_refs(vec![("ns-a", "ref-a"), ("ns-b", "ref-b")]);
        let errors = vec![
            GatewayResolvedRefsError::InvalidCACertificateRef {
                namespace: "ns-a".to_string(),
                name: "ref-a".to_string(),
                reason: "missing required key 'ca.crt'".to_string(),
            },
            GatewayResolvedRefsError::CACertRefNotPermitted {
                from_namespace: "gw-ns".to_string(),
                namespace: "ns-a".to_string(),
                name: "ref-a".to_string(),
            },
        ];
        assert!(!has_all_ca_refs_invalid(&listener, &errors));
    }

    #[test]
    fn has_all_ca_refs_invalid_returns_true_when_every_ref_invalid() {
        let listener = listener_with_ca_refs(vec![("ns-a", "ref-a"), ("ns-b", "ref-b")]);
        let errors = vec![
            GatewayResolvedRefsError::InvalidCACertificateRef {
                namespace: "ns-a".to_string(),
                name: "ref-a".to_string(),
                reason: "Secret not found".to_string(),
            },
            GatewayResolvedRefsError::CACertRefNotPermitted {
                from_namespace: "gw-ns".to_string(),
                namespace: "ns-b".to_string(),
                name: "ref-b".to_string(),
            },
        ];
        assert!(has_all_ca_refs_invalid(&listener, &errors));
    }

    #[test]
    fn has_all_ca_refs_invalid_counts_invalid_kind_as_one_distinct_ref() {
        let listener = listener_with_ca_refs(vec![("ns-a", "ref-a"), ("ns-b", "ref-b")]);
        let errors = vec![
            GatewayResolvedRefsError::InvalidCACertificateKind {
                kind: "BogusKind".to_string(),
            },
            GatewayResolvedRefsError::InvalidCACertificateRef {
                namespace: "ns-b".to_string(),
                name: "ref-b".to_string(),
                reason: "ConfigMap has no data".to_string(),
            },
        ];
        assert!(has_all_ca_refs_invalid(&listener, &errors));
    }
}
