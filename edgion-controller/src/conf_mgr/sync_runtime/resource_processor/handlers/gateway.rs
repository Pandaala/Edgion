//! Gateway Handler
//!
//! Handles Gateway resources with:
//! - Filter by gateway_class_name
//! - TLS Secret reference resolution
//! - SecretRefManager registration for cascading updates
//! - ListenerPortManager registration for port conflict detection
//! - Gateway API standard status management

use crate::conf_mgr::sync_runtime::resource_processor::ref_grant::is_edgion_config_data_ref_allowed_from;
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, format_secret_key, make_conflict_key,
    ConflictKey, HandlerContext, ProcessResult, ProcessorHandler, ProxyProtocolListenerContribution,
    ProxyProtocolPolicyDeclaration, ResourceRef,
};
use crate::conf_mgr::ControllerRuntime;
use crate::config::is_reference_grant_validation_enabled;
use edgion_resources::constants::annotations;
use edgion_resources::constants::app::GATEWAY_NAME;
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::resources::common::is_core_api_group;
use edgion_resources::resources::gateway::{
    FrontendValidationMode, GatewayAddress, GatewayStatus, GatewayStatusAddress, GatewayTLSConfig, GatewayTlsMode,
    Listener as GatewayListener, ListenerStatus, RouteGroupKind,
};
use edgion_resources::resources::proxy_protocol::{
    BoundedProxyProtocolReferenceIdentity, InboundProxyProtocolAnnotation, ProxyProtocolConfigErrorCode,
    ProxyProtocolPolicySourceKind, ResolvedInboundProxyProtocol,
};
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::ByteString;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const GATEWAY_SYSTEM_NAMESPACE: &str = "edgion-system";
const GATEWAY_API_GROUP: &str = "gateway.networking.k8s.io";
const INVALID_PROXY_PROTOCOL_CONFIG: &str = "InvalidProxyProtocolConfig";
const INVALID_PROXY_PROTOCOL_LISTENER: &str = "InvalidProxyProtocolListener";
const INVALID_PROXY_PROTOCOL_REF: &str = "InvalidProxyProtocolRef";
const PROXY_PROTOCOL_POLICY_CONFLICT: &str = "ProxyProtocolPolicyConflict";

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
    protocol: String,
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

fn is_tcp_listener(listener: &GatewayListener) -> bool {
    matches!(
        listener.protocol.to_ascii_uppercase().as_str(),
        "HTTP" | "HTTPS" | "TCP" | "TLS"
    )
}

fn tcp_listener_names(gateway: &Gateway) -> Vec<String> {
    gateway
        .spec
        .listeners
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|listener| is_tcp_listener(listener))
        .map(|listener| listener.name.clone())
        .collect()
}

fn invalid_proxy_protocol_state(
    code: ProxyProtocolConfigErrorCode,
    listener_names: Vec<String>,
    reference: Option<BoundedProxyProtocolReferenceIdentity>,
) -> ResolvedInboundProxyProtocol {
    ResolvedInboundProxyProtocol::Invalid {
        code,
        listener_names,
        reference,
    }
}

fn resolve_proxy_protocol_listener_selection(
    gateway: &Gateway,
    requested: Option<&[String]>,
) -> Result<Vec<String>, (ProxyProtocolConfigErrorCode, Vec<String>)> {
    let listeners = gateway.spec.listeners.as_deref().unwrap_or_default();
    match requested {
        Some(requested) => {
            let selected: Vec<String> = requested
                .iter()
                .filter(|name| listeners.iter().any(|listener| listener.name.as_str() == name.as_str()))
                .cloned()
                .collect();
            let valid = selected.len() == requested.len()
                && requested.iter().all(|name| {
                    listeners
                        .iter()
                        .find(|listener| listener.name.as_str() == name.as_str())
                        .is_some_and(is_tcp_listener)
                });
            if valid {
                Ok(selected)
            } else {
                Err((ProxyProtocolConfigErrorCode::InvalidListenerSelection, selected))
            }
        }
        None => {
            let selected = tcp_listener_names(gateway);
            if selected.is_empty() {
                Err((ProxyProtocolConfigErrorCode::InvalidListenerSelection, Vec::new()))
            } else {
                Ok(selected)
            }
        }
    }
}

fn resolve_inbound_proxy_protocol(
    gateway: &Gateway,
    ctx: &HandlerContext,
    resource_ref: &ResourceRef,
) -> ResolvedInboundProxyProtocol {
    let Some(raw) = gateway
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(annotations::edgion::INBOUND_PROXY_PROTOCOL))
    else {
        return ResolvedInboundProxyProtocol::Disabled;
    };

    let annotation = match InboundProxyProtocolAnnotation::parse(raw) {
        Ok(annotation) => annotation,
        Err(code) => return invalid_proxy_protocol_state(code, tcp_listener_names(gateway), None),
    };
    let listener_names = match resolve_proxy_protocol_listener_selection(gateway, annotation.listeners()) {
        Ok(listener_names) => listener_names,
        Err((code, selected)) => return invalid_proxy_protocol_state(code, selected, None),
    };

    if let Some(policy) = annotation.policy() {
        return match policy.canonicalize() {
            Ok(policy) => ResolvedInboundProxyProtocol::Ready {
                listener_names,
                policy,
                source: ProxyProtocolPolicySourceKind::Inline,
            },
            Err(code) => invalid_proxy_protocol_state(code, listener_names, None),
        };
    }

    let Some(reference) = annotation.resource_ref() else {
        return invalid_proxy_protocol_state(ProxyProtocolConfigErrorCode::InternalInvariant, listener_names, None);
    };
    let gateway_namespace = gateway.metadata.namespace.as_deref().unwrap_or("default");
    let target_namespace = reference.namespace.as_deref().unwrap_or(gateway_namespace);
    let reference_identity =
        BoundedProxyProtocolReferenceIdentity::new(target_namespace.to_string(), reference.name.clone()).ok();
    let target_key = format!("{}/{}", target_namespace, reference.name);
    ctx.runtime()
        .edgion_config_data_refs
        .add_ref(target_key, format!("Gateway:{}", resource_ref.resource_key()));
    if target_namespace != gateway_namespace {
        ctx.runtime()
            .cross_ns_ref_mgr
            .add_ref(target_namespace.to_string(), resource_ref.clone());
    }

    let Some(target) = ctx
        .runtime()
        .edgion_config_data_store
        .get(Some(target_namespace), &reference.name)
    else {
        return invalid_proxy_protocol_state(
            ProxyProtocolConfigErrorCode::ReferenceNotFound,
            listener_names,
            reference_identity,
        );
    };
    if !is_edgion_config_data_ref_allowed_from(
        ctx.runtime(),
        gateway_namespace,
        GATEWAY_API_GROUP,
        "Gateway",
        target_namespace,
        &reference.name,
        target.spec.visibility,
    ) {
        return invalid_proxy_protocol_state(
            ProxyProtocolConfigErrorCode::ReferenceNotPermitted,
            listener_names,
            reference_identity,
        );
    }
    if !target.spec.enable {
        return invalid_proxy_protocol_state(
            ProxyProtocolConfigErrorCode::ReferenceDisabled,
            listener_names,
            reference_identity,
        );
    }
    if target.spec.active.is_some() {
        return invalid_proxy_protocol_state(
            ProxyProtocolConfigErrorCode::ReferenceActiveUnsupported,
            listener_names,
            reference_identity,
        );
    }
    let Some(policy) = target.spec.canonical_proxy_protocol_trust_policy() else {
        return invalid_proxy_protocol_state(
            ProxyProtocolConfigErrorCode::ReferenceWrongType,
            listener_names,
            reference_identity,
        );
    };
    match policy {
        Ok(policy) => ResolvedInboundProxyProtocol::Ready {
            listener_names,
            policy,
            source: ProxyProtocolPolicySourceKind::ResourceRef,
        },
        Err(_) => invalid_proxy_protocol_state(
            ProxyProtocolConfigErrorCode::ReferenceInvalidPolicy,
            listener_names,
            reference_identity,
        ),
    }
}

fn proxy_protocol_contributions(gateway: &Gateway) -> Vec<ProxyProtocolListenerContribution> {
    let selected: BTreeSet<&str> = match &gateway.spec.resolved_inbound_proxy_protocol {
        ResolvedInboundProxyProtocol::Ready { listener_names, .. }
        | ResolvedInboundProxyProtocol::Invalid { listener_names, .. } => {
            listener_names.iter().map(String::as_str).collect()
        }
        ResolvedInboundProxyProtocol::Disabled => BTreeSet::new(),
    };
    let ready_policy = match &gateway.spec.resolved_inbound_proxy_protocol {
        ResolvedInboundProxyProtocol::Ready { policy, .. } => Some(policy),
        _ => None,
    };

    gateway
        .spec
        .listeners
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|listener| is_tcp_listener(listener))
        .filter_map(|listener| {
            let declaration = if selected.contains(listener.name.as_str()) {
                ready_policy.map(ProxyProtocolPolicyDeclaration::from)?
            } else {
                ProxyProtocolPolicyDeclaration::Disabled
            };
            Some(ProxyProtocolListenerContribution {
                listener_name: listener.name.clone(),
                listener_key: ListenerKey::new(
                    resolve_listener_addr(gateway.spec.addresses.as_ref(), listener.port),
                    &listener.protocol,
                ),
                declaration,
            })
        })
        .collect()
}

fn is_proxy_protocol_reference_error(code: ProxyProtocolConfigErrorCode) -> bool {
    matches!(
        code,
        ProxyProtocolConfigErrorCode::ReferenceNotFound
            | ProxyProtocolConfigErrorCode::ReferenceNotPermitted
            | ProxyProtocolConfigErrorCode::ReferenceDisabled
            | ProxyProtocolConfigErrorCode::ReferenceActiveUnsupported
            | ProxyProtocolConfigErrorCode::ReferenceWrongType
            | ProxyProtocolConfigErrorCode::ReferenceInvalidPolicy
    )
}

fn is_proxy_protocol_shared_conflict(code: ProxyProtocolConfigErrorCode) -> bool {
    code == ProxyProtocolConfigErrorCode::SharedListenerConflict
}

fn proxy_protocol_error_message(
    code: ProxyProtocolConfigErrorCode,
    reference: Option<&BoundedProxyProtocolReferenceIdentity>,
    gateway_namespace: &str,
    gateway_name: &str,
) -> String {
    match reference {
        Some(reference) => format!(
            "Gateway '{gateway_namespace}/{gateway_name}' inbound PROXY protocol reference '{}/{}' is invalid ({code:?})",
            reference.namespace, reference.name,
        ),
        None => format!(
            "Gateway '{gateway_namespace}/{gateway_name}' annotation '{}' is invalid ({code:?})",
            annotations::edgion::INBOUND_PROXY_PROTOCOL,
        ),
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
                if let Ok(key) = resolve_gateway_stream_plugin_ref(raw, gateway_ns) {
                    ctx.runtime().stream_plugin_ref_mgr.add_ref(key, resource_ref.clone());
                }
            }
        }

        // Clear and re-register HTTP plugin annotation refs (edgion.io/edgion-plugins).
        ctx.runtime().plugin_ref_mgr.clear_resource_refs(&resource_ref);
        if let Some(v) = g
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(annotations::edgion::EDGION_PLUGINS))
        {
            let gateway_ns = g.metadata.namespace.as_deref().unwrap_or("default");
            for key in annotations::edgion::parse_plugin_ref_list(v, gateway_ns) {
                ctx.runtime().plugin_ref_mgr.add_ref(key, resource_ref.clone());
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

        // Clear the previous EdgionConfigData dependency before rebuilding the
        // annotation's complete reference set. Missing targets are registered
        // below as well, so their later arrival requeues this Gateway.
        let gateway_config_data_ref_key = format!("Gateway:{}", resource_ref.resource_key());
        ctx.runtime()
            .edgion_config_data_refs
            .clear_refs_from(&gateway_config_data_ref_key);

        // Track a cross-namespace `edgion.io/dns-resolver-ref` (Gateway → httpdns
        // LinkSys) so a ReferenceGrant add/remove requeues this Gateway. The grant
        // check + annotation strip happen in `update_status` (which reports the
        // denial via ResolvedRefs before stripping).
        if let Some(v) = g
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(annotations::edgion::DNS_RESOLVER_REF))
        {
            if let Some(key) = edgion_common::dns::resolver_select::LinkSysKey::parse(v) {
                let gateway_ns = g.metadata.namespace.as_deref().unwrap_or("default");
                if key.namespace != gateway_ns {
                    cross_ns_manager.add_ref(key.namespace, cross_ns_ref.clone());
                }
            }
        }

        g.spec.resolved_inbound_proxy_protocol = resolve_inbound_proxy_protocol(&g, ctx, &resource_ref);
        let gateway_key_for_policy = resource_ref.resource_key();
        let dirty_peers = ctx
            .runtime()
            .proxy_protocol_listener_policy
            .replace_gateway_contributions(&gateway_key_for_policy, proxy_protocol_contributions(&g));
        let shared_conflicts = ctx
            .runtime()
            .proxy_protocol_listener_policy
            .conflicting_listener_names(&gateway_key_for_policy);
        if !shared_conflicts.is_empty()
            && !matches!(
                g.spec.resolved_inbound_proxy_protocol,
                ResolvedInboundProxyProtocol::Invalid { .. }
            )
        {
            g.spec.resolved_inbound_proxy_protocol = invalid_proxy_protocol_state(
                ProxyProtocolConfigErrorCode::SharedListenerConflict,
                shared_conflicts.into_iter().collect(),
                None,
            );
        }
        for peer in dirty_peers {
            ctx.requeue("Gateway", peer).await;
        }

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

        // Resolve the Gateway-level default client identity used for upstream
        // mTLS when a BackendTLSPolicy does not provide its own certificate.
        if let Some(backend_tls) = g.spec.tls.as_mut().and_then(|tls| tls.backend.as_mut()) {
            backend_tls.resolved_client_certificate = None;

            if let Some(client_ref) = &backend_tls.client_certificate_ref {
                let gateway_ns = g.metadata.namespace.as_deref().unwrap_or("");
                let client_ns = client_ref.namespace.as_deref().unwrap_or(gateway_ns);
                let secret_key = format!("{client_ns}/{}", client_ref.name);

                ctx.secret_ref_manager()
                    .add_ref(secret_key.clone(), resource_ref.clone());

                if client_ns != gateway_ns {
                    cross_ns_manager.add_ref(client_ns.to_string(), cross_ns_ref.clone());
                }

                let valid_secret_ref = client_ref.kind.as_deref().unwrap_or("Secret") == "Secret"
                    && is_core_api_group(client_ref.group.as_deref().unwrap_or(""));
                let permitted = client_ns == gateway_ns
                    || !is_reference_grant_validation_enabled()
                    || ctx.runtime().ref_grant_store.check_reference_allowed(
                        gateway_ns,
                        "gateway.networking.k8s.io",
                        "Gateway",
                        client_ns,
                        "",
                        "Secret",
                        Some(&client_ref.name),
                    );

                if valid_secret_ref && permitted {
                    if let Some(secret) = ctx.runtime().secret_store.get(Some(client_ns), &client_ref.name) {
                        backend_tls.resolved_client_certificate = Some(secret);
                    } else {
                        tracing::info!(
                            gateway = %resource_ref.key(),
                            secret_key = %secret_key,
                            "Gateway backend client certificate Secret not found yet; it will be reprocessed when the Secret arrives"
                        );
                    }
                }
            }
        }

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
        ctx.runtime().plugin_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.runtime()
            .edgion_config_data_refs
            .clear_refs_from(&format!("Gateway:{}", resource_ref.resource_key()));

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
        let dirty_peers = ctx
            .runtime()
            .proxy_protocol_listener_policy
            .replace_gateway_contributions(&gateway_key, Vec::new());
        for peer in dirty_peers {
            ctx.requeue("Gateway", peer).await;
        }

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
                    protocol: l.protocol.clone(),
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
        let plugin_ref_error = validate_gateway_plugin_ref(gateway, _ctx.runtime());
        // Cross-namespace dns-resolver-ref grant denial (Gateway → httpdns LinkSys).
        let dns_resolver_ref_error = validate_gateway_dns_resolver_ref(gateway, _ctx.runtime());

        let (proxy_protocol_error, proxy_protocol_affected): (
            Option<(ProxyProtocolConfigErrorCode, String)>,
            BTreeSet<String>,
        ) = match &gateway.spec.resolved_inbound_proxy_protocol {
            ResolvedInboundProxyProtocol::Invalid {
                code,
                listener_names,
                reference,
            } => (
                Some((
                    *code,
                    proxy_protocol_error_message(*code, reference.as_ref(), &gateway_ns, &gateway_name),
                )),
                listener_names.iter().cloned().collect(),
            ),
            _ => (None, BTreeSet::new()),
        };
        let gateway_policy_key = format!("{}/{}", gateway_ns, gateway_name);
        let proxy_protocol_conflicts = _ctx
            .runtime()
            .proxy_protocol_listener_policy
            .conflicting_listener_names(&gateway_policy_key);

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

        let proxy_intrinsic_error = proxy_protocol_error
            .as_ref()
            .filter(|(code, _)| !is_proxy_protocol_reference_error(*code) && !is_proxy_protocol_shared_conflict(*code));
        let proxy_reference_error = proxy_protocol_error
            .as_ref()
            .filter(|(code, _)| is_proxy_protocol_reference_error(*code));

        // Set Gateway-level conditions. Intrinsic PROXY annotation errors are
        // admission errors; reference failures are merged into ResolvedRefs.
        if validation_errors.is_empty() && proxy_intrinsic_error.is_none() {
            let cond = condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Gateway accepted",
                generation,
            );
            update_gateway_condition(conditions, cond);
        } else if let Some((_, message)) = proxy_intrinsic_error {
            let mut messages = validation_errors.to_vec();
            messages.push(message.clone());
            let cond = condition_false(
                condition_types::ACCEPTED,
                INVALID_PROXY_PROTOCOL_CONFIG,
                messages.join("; "),
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

        // Gateway-level ResolvedRefs: merge stream-plugin, HTTP-plugin, and
        // dns-resolver ref errors into a single shared condition — never emit two
        // conditions of the same type. Reason precedence keeps the existing
        // plugin-ref reasons; a dns-resolver-ref denial alone uses the standard
        // RefNotPermitted reason.
        {
            let mut parts: Vec<String> = Vec::new();
            if let Some(s) = stream_plugin_ref_error.as_ref() {
                parts.push(s.clone());
            }
            if let Some(p) = plugin_ref_error.as_ref() {
                parts.push(p.clone());
            }
            if let Some(d) = dns_resolver_ref_error.as_ref() {
                parts.push(d.clone());
            }
            if let Some((_, message)) = proxy_reference_error {
                parts.push(message.clone());
            }
            let merged = if parts.is_empty() {
                None
            } else {
                let reason = if plugin_ref_error.is_some() {
                    condition_reasons::INVALID_PLUGINS_REF
                } else if stream_plugin_ref_error.is_some() {
                    condition_reasons::INVALID_STREAM_PLUGINS_REF
                } else if dns_resolver_ref_error.is_some()
                    || proxy_reference_error
                        .is_some_and(|(code, _)| *code == ProxyProtocolConfigErrorCode::ReferenceNotPermitted)
                {
                    condition_reasons::REF_NOT_PERMITTED
                } else {
                    INVALID_PROXY_PROTOCOL_REF
                };
                Some((reason, parts.join("; ")))
            };
            match merged {
                None => {
                    let cond = condition_true(
                        condition_types::RESOLVED_REFS,
                        condition_reasons::RESOLVED_REFS,
                        "All gateway-level references resolved",
                        generation,
                    );
                    update_gateway_condition(conditions, cond);
                }
                Some((reason, message)) => {
                    tracing::warn!(error = %message, "Gateway-level ref unresolved");
                    let cond = condition_false(condition_types::RESOLVED_REFS, reason, message, generation);
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
            if validation_errors.is_empty()
                && proxy_protocol_error
                    .as_ref()
                    .is_none_or(|(code, _)| is_proxy_protocol_shared_conflict(*code))
                && proxy_protocol_conflicts.is_empty()
            {
                let cond = condition_true(
                    condition_types::PROGRAMMED,
                    condition_reasons::PROGRAMMED,
                    "Gateway programmed",
                    generation,
                );
                update_gateway_condition(conditions, cond);
            } else {
                let message = proxy_protocol_error
                    .as_ref()
                    .filter(|(code, _)| !is_proxy_protocol_shared_conflict(*code))
                    .map(|(_, message)| message.clone())
                    .or_else(|| {
                        (!proxy_protocol_conflicts.is_empty())
                            .then(|| "Inbound PROXY protocol policies conflict on a shared listener".to_string())
                    })
                    .unwrap_or_else(|| validation_errors.join("; "));
                let cond = condition_false(
                    condition_types::PROGRAMMED,
                    condition_reasons::INVALID_LISTENER,
                    message,
                    generation,
                );
                update_gateway_condition(conditions, cond);
            }
        }

        // ListenersNotValid: shared PROXY policy conflict is process-fatal and
        // therefore also blocks the Gateway-level Programmed condition.
        if !proxy_protocol_conflicts.is_empty() {
            let mut message = format!(
                "Listeners have conflicting inbound PROXY protocol policies: {:?}",
                proxy_protocol_conflicts
            );
            if !conflicts.is_empty() {
                let conflicting_names: Vec<&String> = conflicts.keys().collect();
                message.push_str(&format!("; listeners also have port conflicts: {conflicting_names:?}"));
            }
            let cond = condition_true(
                condition_types::LISTENERS_NOT_VALID,
                PROXY_PROTOCOL_POLICY_CONFLICT,
                message,
                generation,
            );
            update_gateway_condition(conditions, cond);
        } else if !conflicts.is_empty() {
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
                protocol,
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

                let proxy_listener_error = proxy_protocol_error
                    .as_ref()
                    .filter(|_| proxy_protocol_affected.contains(&ls.name));
                let proxy_listener_conflict = proxy_protocol_conflicts.contains(&ls.name);

                // Set Conflicted condition: mixed TLS mode, port conflict, or
                // mismatched PROXY policy on one physical listener.
                let is_conflicted = if proxy_listener_conflict {
                    let mut message = "Inbound PROXY protocol policies conflict on this shared listener".to_string();
                    if mixed_tls_conflict {
                        message.push_str("; mixed TLS termination modes also conflict on this listener");
                    }
                    if let Some(reason) = conflicts.get(&ls.name) {
                        message.push_str(&format!("; port conflict: {reason}"));
                    }
                    let cond = condition_true(
                        condition_types::CONFLICTED,
                        PROXY_PROTOCOL_POLICY_CONFLICT,
                        message,
                        generation,
                    );
                    update_gateway_condition(&mut ls.conditions, cond);
                    true
                } else if mixed_tls_conflict {
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
                    proxy_listener_error,
                    protocol.eq_ignore_ascii_case("UDP"),
                    proxy_listener_conflict,
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

        // Strip a denied cross-namespace `dns-resolver-ref` after conditions are
        // reported, so the data plane never honors an unpermitted reference. Only
        // the shipped/cached copy is mutated (status write-back touches the status
        // subresource only), so the apiserver object keeps the annotation and a
        // later ReferenceGrant re-enables it on the next reconcile.
        if dns_resolver_ref_error.is_some() {
            super::dns_resolver_guard::strip_dns_resolver_ref(&mut gateway.metadata.annotations);
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
    cross_ns_manager: &crate::conf_mgr::sync_runtime::resource_processor::ref_grant::CrossNamespaceRefManager,
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
    proxy_protocol_error: Option<&(ProxyProtocolConfigErrorCode, String)>,
    is_udp_listener: bool,
    proxy_protocol_conflict: bool,
) {
    // Accepted: False if mixed TLS termination on the same port
    let proxy_intrinsic_error = proxy_protocol_error
        .filter(|(code, _)| !is_proxy_protocol_reference_error(*code) && !is_proxy_protocol_shared_conflict(*code));
    let proxy_reference_error = proxy_protocol_error.filter(|(code, _)| is_proxy_protocol_reference_error(*code));

    if let Some((code, message)) = proxy_intrinsic_error {
        let reason = if is_udp_listener && *code == ProxyProtocolConfigErrorCode::InvalidListenerSelection {
            INVALID_PROXY_PROTOCOL_LISTENER
        } else {
            INVALID_PROXY_PROTOCOL_CONFIG
        };
        let mut messages = validation_errors.to_vec();
        messages.push(message.clone());
        let cond = condition_false(condition_types::ACCEPTED, reason, messages.join("; "), generation);
        update_gateway_condition(&mut ls.conditions, cond);
    } else if mixed_tls_conflict {
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
    let has_unresolved_refs = !resolved_refs_errors.is_empty() || proxy_reference_error.is_some();
    if has_unresolved_refs {
        let reason = if !resolved_refs_errors.is_empty() {
            pick_gateway_resolved_refs_reason(resolved_refs_errors)
        } else if proxy_reference_error
            .is_some_and(|(code, _)| *code == ProxyProtocolConfigErrorCode::ReferenceNotPermitted)
        {
            condition_reasons::REF_NOT_PERMITTED
        } else {
            INVALID_PROXY_PROTOCOL_REF
        };
        let mut message_parts = resolved_refs_errors.iter().map(|e| e.message()).collect::<Vec<_>>();
        if let Some((_, message)) = proxy_reference_error {
            message_parts.push(message.clone());
        }
        let message = message_parts.join("; ");
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
    if proxy_protocol_conflict {
        let cond = condition_false(
            condition_types::PROGRAMMED,
            condition_reasons::INVALID_LISTENER,
            "Inbound PROXY protocol policies conflict on this shared listener",
            generation,
        );
        update_gateway_condition(&mut ls.conditions, cond);
    } else if is_conflicted {
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
        || proxy_intrinsic_error.is_some()
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
                                        edgion_common::tls_validation::check_key_matches_cert(cert_str, key_str)
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

/// Resolve a Gateway-level StreamPlugins annotation to its store key.
///
/// Gateway attachments are same-namespace only. Route-level StreamPlugins
/// references have a separate contract and continue to support qualified keys.
fn resolve_gateway_stream_plugin_ref(value: &str, gateway_ns: &str) -> Result<String, String> {
    if value.contains('/') {
        Err("Gateway edgion-stream-plugins reference must be an unqualified resource name".to_string())
    } else {
        Ok(format!("{}/{}", gateway_ns, value))
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
    let key = match resolve_gateway_stream_plugin_ref(annotation_value, gateway_ns) {
        Ok(key) => key,
        Err(error) => return Some(error),
    };
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

/// Returns `Some(error_message)` when the Gateway's `edgion.io/edgion-plugins` annotation
/// references one or more EdgionPlugins resources that do not exist. `None` when the
/// annotation is absent/empty or all referenced resources are found.
fn validate_gateway_plugin_ref(gateway: &Gateway, runtime: &ControllerRuntime) -> Option<String> {
    let value = gateway
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(annotations::edgion::EDGION_PLUGINS))?;
    let gateway_ns = gateway.metadata.namespace.as_deref().unwrap_or("default");
    let missing: Vec<String> = annotations::edgion::parse_plugin_ref_list(value, gateway_ns)
        .into_iter()
        .filter(|key| {
            runtime
                .processor_registry
                .get("EdgionPlugins")
                .and_then(|p| p.get_raw_any(key))
                .is_none()
        })
        .collect();
    if missing.is_empty() {
        None
    } else {
        Some(format!("EdgionPlugins not found: {}", missing.join(", ")))
    }
}

/// Returns `Some(error_message)` when the Gateway's `edgion.io/dns-resolver-ref`
/// annotation points at an httpdns LinkSys in another namespace with no matching
/// ReferenceGrant. `None` when the annotation is absent, same-namespace, or
/// permitted. Same-namespace refs and trusted config are never gated.
fn validate_gateway_dns_resolver_ref(gateway: &Gateway, runtime: &ControllerRuntime) -> Option<String> {
    use super::dns_resolver_guard::check_dns_resolver_ref_grant;
    let gateway_ns = gateway.metadata.namespace.as_deref().unwrap_or("default");
    let denial = check_dns_resolver_ref_grant(
        gateway_ns,
        "gateway.networking.k8s.io",
        "Gateway",
        gateway.metadata.annotations.as_ref(),
        runtime,
    )?;
    Some(format!(
        "dns-resolver-ref LinkSys '{}/{}' cross-namespace reference not permitted (no ReferenceGrant)",
        denial.ref_namespace, denial.name
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        gateway_service_lookup_candidates, has_all_ca_refs_invalid, resolve_gateway_stream_plugin_ref,
        resolve_inbound_proxy_protocol, validate_gateway_dns_resolver_ref, validate_gateway_plugin_ref,
        validate_gateway_stream_plugin_ref, GatewayHandler, GatewayResolvedRefsError,
    };
    use crate::conf_mgr::sync_runtime::resource_processor::{
        HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
    };
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::constants::app::GATEWAY_NAME;
    use edgion_resources::prelude_resources::{EdgionConfigData, Gateway};
    use edgion_resources::resources::common::SecretObjectReference;
    use edgion_resources::resources::gateway::{
        FrontendTLSValidation, GatewayBackendTLS, GatewaySpec, GatewaySpecTLS, GatewayTLSConfig, GatewayTlsMode,
        Listener as GatewayListener,
    };
    use edgion_resources::resources::proxy_protocol::{
        CanonicalProxyProtocolTrustPolicy, ProxyProtocolConfigErrorCode, ProxyProtocolPolicySourceKind,
        ResolvedInboundProxyProtocol,
    };
    use edgion_resources::ResourceKind;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::Arc;

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
                resolved_inbound_proxy_protocol: Default::default(),
            },
            status: None,
        }
    }

    fn proxy_protocol_test_context(runtime: Arc<ControllerRuntime>) -> HandlerContext {
        HandlerContext::new(None, None, Default::default(), 5, 20, runtime)
    }

    fn make_proxy_protocol_gateway(annotation: &str) -> Gateway {
        let mut gateway = make_test_gateway("default");
        gateway.metadata.name = Some("gateway".to_string());
        gateway.metadata.annotations = Some(BTreeMap::from([(
            "edgion.io/inbound-proxy-protocol".to_string(),
            annotation.to_string(),
        )]));
        gateway.spec.listeners = Some(vec![
            GatewayListener {
                name: "http".to_string(),
                hostname: None,
                port: 80,
                protocol: "HTTP".to_string(),
                tls: None,
                allowed_routes: None,
            },
            GatewayListener {
                name: "udp".to_string(),
                hostname: None,
                port: 53,
                protocol: "UDP".to_string(),
                tls: None,
                allowed_routes: None,
            },
        ]);
        gateway
    }

    #[test]
    fn proxy_protocol_omitted_listener_selection_ignores_udp() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = proxy_protocol_test_context(runtime);
        let gateway = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            Some("default".to_string()),
            "gateway".to_string(),
        );

        assert_eq!(
            resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref),
            ResolvedInboundProxyProtocol::Ready {
                listener_names: vec!["http".to_string()],
                policy: CanonicalProxyProtocolTrustPolicy::Required {},
                source: ProxyProtocolPolicySourceKind::Inline,
            }
        );
    }

    #[test]
    fn proxy_protocol_explicit_udp_selection_is_invalid_only_for_udp() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = proxy_protocol_test_context(runtime);
        let gateway =
            make_proxy_protocol_gateway(r#"{"source":"inline","listeners":["udp"],"config":{"mode":"required"}}"#);
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            Some("default".to_string()),
            "gateway".to_string(),
        );

        assert_eq!(
            resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref),
            ResolvedInboundProxyProtocol::Invalid {
                code: ProxyProtocolConfigErrorCode::InvalidListenerSelection,
                listener_names: vec!["udp".to_string()],
                reference: None,
            }
        );
    }

    #[test]
    fn proxy_protocol_missing_reference_is_invalid_and_registered() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = proxy_protocol_test_context(runtime.clone());
        let gateway = make_proxy_protocol_gateway(r#"{"source":"resourceRef","resourceRef":{"name":"missing"}}"#);
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            Some("default".to_string()),
            "gateway".to_string(),
        );

        assert!(matches!(
            resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref),
            ResolvedInboundProxyProtocol::Invalid {
                code: ProxyProtocolConfigErrorCode::ReferenceNotFound,
                ..
            }
        ));
        assert_eq!(
            runtime.edgion_config_data_refs.get_refs("default/missing"),
            vec!["Gateway:default/gateway".to_string()]
        );
    }

    #[test]
    fn proxy_protocol_same_namespace_reference_resolves_strict_variant() {
        let runtime = Arc::new(ControllerRuntime::default());
        let target: EdgionConfigData = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionConfigData",
            "metadata": {"namespace": "default", "name": "policy"},
            "spec": {
                "enable": true,
                "visibility": "Namespace",
                "data": {"type": "ProxyProtocolTrust", "config": {"mode": "required"}}
            }
        }))
        .unwrap();
        runtime
            .edgion_config_data_store
            .update(HashMap::from([("default/policy".to_string(), target)]), &HashSet::new());
        let ctx = proxy_protocol_test_context(runtime);
        let gateway = make_proxy_protocol_gateway(r#"{"source":"resourceRef","resourceRef":{"name":"policy"}}"#);
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            Some("default".to_string()),
            "gateway".to_string(),
        );

        assert!(matches!(
            resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref),
            ResolvedInboundProxyProtocol::Ready {
                source: ProxyProtocolPolicySourceKind::ResourceRef,
                ..
            }
        ));
    }

    #[test]
    fn proxy_protocol_reference_rejects_target_with_captured_unknown_field() {
        let runtime = Arc::new(ControllerRuntime::default());
        let mut target: EdgionConfigData = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionConfigData",
            "metadata": {"namespace": "default", "name": "policy"},
            "spec": {
                "data": {
                    "type": "ProxyProtocolTrust",
                    "config": {"mode": "required", "typo": true}
                }
            }
        }))
        .expect("target typo must remain deserializable");
        target.spec.unknown_fields = vec!["data.config.typo".to_string()];
        runtime
            .edgion_config_data_store
            .update(HashMap::from([("default/policy".to_string(), target)]), &HashSet::new());
        let ctx = proxy_protocol_test_context(runtime);
        let mut gateway = make_proxy_protocol_gateway(r#"{"source":"resourceRef","resourceRef":{"name":"policy"}}"#);
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            Some("default".to_string()),
            "gateway".to_string(),
        );

        gateway.spec.resolved_inbound_proxy_protocol = resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref);
        assert!(matches!(
            gateway.spec.resolved_inbound_proxy_protocol,
            ResolvedInboundProxyProtocol::Invalid {
                code: ProxyProtocolConfigErrorCode::ReferenceInvalidPolicy,
                ..
            }
        ));
        GatewayHandler::new(None, None).update_status(&mut gateway, &ctx, &[]);
        let resolved_refs = gateway
            .status
            .unwrap()
            .conditions
            .unwrap()
            .into_iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(resolved_refs.status, "False");
        assert_eq!(resolved_refs.reason, "InvalidProxyProtocolRef");
    }

    #[test]
    fn proxy_protocol_udp_status_does_not_pollute_unselected_tcp_listener() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = proxy_protocol_test_context(runtime);
        let mut gateway =
            make_proxy_protocol_gateway(r#"{"source":"inline","listeners":["udp"],"config":{"mode":"required"}}"#);
        gateway.spec.resolved_inbound_proxy_protocol = ResolvedInboundProxyProtocol::Invalid {
            code: ProxyProtocolConfigErrorCode::InvalidListenerSelection,
            listener_names: vec!["udp".to_string()],
            reference: None,
        };

        GatewayHandler::new(None, None).update_status(&mut gateway, &ctx, &[]);
        let statuses = gateway.status.unwrap().listeners.unwrap();
        let http = statuses.iter().find(|status| status.name == "http").unwrap();
        let udp = statuses.iter().find(|status| status.name == "udp").unwrap();
        assert_eq!(
            http.conditions
                .iter()
                .find(|condition| condition.type_ == "Accepted")
                .unwrap()
                .status,
            "True"
        );
        let udp_accepted = udp
            .conditions
            .iter()
            .find(|condition| condition.type_ == "Accepted")
            .unwrap();
        assert_eq!(udp_accepted.status, "False");
        assert_eq!(udp_accepted.reason, "InvalidProxyProtocolListener");
    }

    #[test]
    fn proxy_protocol_reference_error_merges_exact_conditions() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = proxy_protocol_test_context(runtime);
        let mut gateway = make_proxy_protocol_gateway(r#"{"source":"resourceRef","resourceRef":{"name":"missing"}}"#);
        gateway.spec.resolved_inbound_proxy_protocol = ResolvedInboundProxyProtocol::Invalid {
            code: ProxyProtocolConfigErrorCode::ReferenceNotFound,
            listener_names: vec!["http".to_string()],
            reference: Some(
                edgion_resources::resources::proxy_protocol::BoundedProxyProtocolReferenceIdentity::new(
                    "default".to_string(),
                    "missing".to_string(),
                )
                .unwrap(),
            ),
        };

        GatewayHandler::new(None, None).update_status(&mut gateway, &ctx, &[]);
        let status = gateway.status.unwrap();
        assert_eq!(
            status
                .conditions
                .as_ref()
                .unwrap()
                .iter()
                .filter(|condition| condition.type_ == "ResolvedRefs")
                .count(),
            1
        );
        let gateway_resolved = status
            .conditions
            .as_ref()
            .unwrap()
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(gateway_resolved.status, "False");
        assert_eq!(gateway_resolved.reason, "InvalidProxyProtocolRef");

        let listeners = status.listeners.unwrap();
        let http = listeners.iter().find(|listener| listener.name == "http").unwrap();
        let udp = listeners.iter().find(|listener| listener.name == "udp").unwrap();
        assert_eq!(
            http.conditions
                .iter()
                .find(|condition| condition.type_ == "ResolvedRefs")
                .unwrap()
                .reason,
            "InvalidProxyProtocolRef"
        );
        assert_eq!(
            udp.conditions
                .iter()
                .find(|condition| condition.type_ == "ResolvedRefs")
                .unwrap()
                .status,
            "True"
        );
    }

    #[tokio::test]
    async fn proxy_protocol_shared_listener_conflict_syncs_invalid_and_status() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = proxy_protocol_test_context(runtime);
        let handler = GatewayHandler::new(None, None);
        let first = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
        let ProcessResult::Continue(_first) = handler.parse(first, &ctx).await else {
            panic!("first Gateway should continue");
        };

        let mut second = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
        second.metadata.name = Some("second".to_string());
        second.metadata.annotations = None;
        let ProcessResult::Continue(mut second) = handler.parse(second, &ctx).await else {
            panic!("second Gateway should continue");
        };
        assert!(matches!(
            second.spec.resolved_inbound_proxy_protocol,
            ResolvedInboundProxyProtocol::Invalid {
                code: ProxyProtocolConfigErrorCode::SharedListenerConflict,
                ..
            }
        ));

        handler.update_status(&mut second, &ctx, &[]);
        let status = second.status.unwrap();
        let conditions = status.conditions.unwrap();
        assert_eq!(
            conditions
                .iter()
                .find(|condition| condition.type_ == "ListenersNotValid")
                .unwrap()
                .reason,
            "ProxyProtocolPolicyConflict"
        );
        assert_eq!(
            conditions
                .iter()
                .find(|condition| condition.type_ == "Accepted")
                .unwrap()
                .status,
            "True"
        );
        let http = status
            .listeners
            .unwrap()
            .into_iter()
            .find(|listener| listener.name == "http")
            .unwrap();
        let conflicted = http
            .conditions
            .iter()
            .find(|condition| condition.type_ == "Conflicted")
            .unwrap();
        assert_eq!(conflicted.status, "True");
        assert_eq!(conflicted.reason, "ProxyProtocolPolicyConflict");
    }

    #[tokio::test]
    async fn proxy_protocol_primary_invalid_survives_shared_conflict() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = proxy_protocol_test_context(runtime);
        let handler = GatewayHandler::new(None, None);
        let first = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
        let ProcessResult::Continue(_first) = handler.parse(first, &ctx).await else {
            panic!("first Gateway should continue");
        };

        let mut second =
            make_proxy_protocol_gateway(r#"{"source":"inline","listeners":["udp"],"config":{"mode":"required"}}"#);
        second.metadata.name = Some("second".to_string());
        let ProcessResult::Continue(mut second) = handler.parse(second, &ctx).await else {
            panic!("second Gateway should continue");
        };
        assert!(matches!(
            second.spec.resolved_inbound_proxy_protocol,
            ResolvedInboundProxyProtocol::Invalid {
                code: ProxyProtocolConfigErrorCode::InvalidListenerSelection,
                ..
            }
        ));

        handler.update_status(&mut second, &ctx, &[]);
        let status = second.status.unwrap();
        let conditions = status.conditions.unwrap();
        assert_eq!(
            conditions
                .iter()
                .find(|condition| condition.type_ == "Accepted")
                .unwrap()
                .reason,
            "InvalidProxyProtocolConfig"
        );
        assert_eq!(
            conditions
                .iter()
                .find(|condition| condition.type_ == "ListenersNotValid")
                .unwrap()
                .reason,
            "ProxyProtocolPolicyConflict"
        );
        let listeners = status.listeners.unwrap();
        assert_eq!(
            listeners
                .iter()
                .find(|listener| listener.name == "udp")
                .unwrap()
                .conditions
                .iter()
                .find(|condition| condition.type_ == "Accepted")
                .unwrap()
                .reason,
            "InvalidProxyProtocolListener"
        );
        assert_eq!(
            listeners
                .iter()
                .find(|listener| listener.name == "http")
                .unwrap()
                .conditions
                .iter()
                .find(|condition| condition.type_ == "Conflicted")
                .unwrap()
                .reason,
            "ProxyProtocolPolicyConflict"
        );
    }

    #[tokio::test]
    async fn proxy_protocol_conflict_precedes_mixed_tls_and_port_conflict() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = proxy_protocol_test_context(runtime);
        let handler = GatewayHandler::new(None, None);
        let tls = |name: &str, mode: GatewayTlsMode| GatewayListener {
            name: name.to_string(),
            hostname: None,
            port: 443,
            protocol: "TLS".to_string(),
            tls: Some(GatewayTLSConfig {
                mode: Some(mode),
                certificate_refs: None,
                options: None,
                secrets: None,
                frontend_validation: None,
                resolved_frontend_ca_secrets: None,
            }),
            allowed_routes: None,
        };

        let mut first = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
        first.spec.listeners = Some(vec![tls("tls", GatewayTlsMode::Passthrough)]);
        let ProcessResult::Continue(_first) = handler.parse(first, &ctx).await else {
            panic!("first Gateway should continue");
        };

        let mut second = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
        second.metadata.name = Some("second".to_string());
        second.metadata.annotations = None;
        second.spec.listeners = Some(vec![
            tls("terminate", GatewayTlsMode::Terminate),
            tls("passthrough", GatewayTlsMode::Passthrough),
        ]);
        let ProcessResult::Continue(mut second) = handler.parse(second, &ctx).await else {
            panic!("second Gateway should continue");
        };
        handler.update_status(&mut second, &ctx, &[]);

        let status = second.status.unwrap();
        let gateway_conflict = status
            .conditions
            .as_ref()
            .unwrap()
            .iter()
            .find(|condition| condition.type_ == "ListenersNotValid")
            .unwrap();
        assert_eq!(gateway_conflict.reason, "ProxyProtocolPolicyConflict");
        assert!(gateway_conflict.message.contains("port conflicts"));
        for listener in status.listeners.unwrap() {
            let conflicted = listener
                .conditions
                .iter()
                .find(|condition| condition.type_ == "Conflicted")
                .unwrap();
            assert_eq!(conflicted.reason, "ProxyProtocolPolicyConflict");
            assert!(conflicted.message.contains("mixed TLS"));
            assert!(conflicted.message.contains("port conflict"));
            assert_eq!(
                listener
                    .conditions
                    .iter()
                    .find(|condition| condition.type_ == "Accepted")
                    .unwrap()
                    .reason,
                "ProtocolConflict"
            );
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
        let gw = make_gateway_with_annotation("default", "my-stream");
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

    fn make_gateway_with_dns_resolver_ref(ns: &str, reference: &str) -> Gateway {
        let mut gw = make_test_gateway(ns);
        let mut annotations = BTreeMap::new();
        annotations.insert("edgion.io/dns-resolver-ref".to_string(), reference.to_string());
        gw.metadata.annotations = Some(annotations);
        gw
    }

    #[test]
    fn validate_dns_resolver_ref_no_annotation_returns_none() {
        let runtime = ControllerRuntime::default();
        let gw = make_test_gateway("default");
        assert!(validate_gateway_dns_resolver_ref(&gw, &runtime).is_none());
    }

    #[test]
    fn validate_dns_resolver_ref_same_namespace_is_allowed() {
        // A same-namespace ref is never grant-gated.
        let runtime = ControllerRuntime::default();
        let gw = make_gateway_with_dns_resolver_ref("app", "app/aliyun");
        assert!(validate_gateway_dns_resolver_ref(&gw, &runtime).is_none());
    }

    #[test]
    fn validate_dns_resolver_ref_cross_namespace_without_grant_is_denied() {
        // Empty ref_grant_store + default (enabled) validation → cross-ns denied.
        let runtime = ControllerRuntime::default();
        let gw = make_gateway_with_dns_resolver_ref("app", "dns/aliyun");
        let result = validate_gateway_dns_resolver_ref(&gw, &runtime);
        assert!(
            result.is_some(),
            "cross-namespace ref without a ReferenceGrant must be denied"
        );
        let msg = result.unwrap();
        assert!(msg.contains("dns/aliyun"), "message must name the ref, got: {msg}");
        assert!(
            msg.contains("not permitted"),
            "message must state the denial, got: {msg}"
        );
    }

    #[test]
    fn resolve_gateway_stream_plugin_ref_rejects_qualified_name() {
        let error = resolve_gateway_stream_plugin_ref("other-ns/my-plugin", "default").unwrap_err();
        assert!(error.contains("unqualified resource name"));
    }

    #[test]
    fn resolve_gateway_stream_plugin_ref_uses_gateway_ns() {
        assert_eq!(
            resolve_gateway_stream_plugin_ref("my-plugin", "default").unwrap(),
            "default/my-plugin"
        );
    }

    #[test]
    fn validate_stream_plugin_ref_rejects_qualified_name() {
        let runtime = ControllerRuntime::default();
        let gw = make_gateway_with_annotation("default", "other-ns/my-plugin");
        let error = validate_gateway_stream_plugin_ref(&gw, &runtime).unwrap();
        assert!(error.contains("unqualified resource name"));
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

    /// Verify that processing a Gateway with `edgion.io/edgion-plugins` annotation registers
    /// both the explicit-namespace ref ("ns1/p1") and the bare-name ref (expanded to the
    /// Gateway's namespace) into `plugin_ref_mgr`.
    #[tokio::test]
    async fn gateway_plugin_annotation_registers_refs() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());

        // Build a Gateway in "gw-ns" with two plugin refs: one fully-qualified, one bare.
        let mut gw = make_test_gateway("gw-ns");
        gw.metadata.name = Some("my-gateway".to_string());
        let mut annotations = BTreeMap::new();
        annotations.insert("edgion.io/edgion-plugins".to_string(), "ns1/p1, p2".to_string());
        gw.metadata.annotations = Some(annotations);

        let handler = GatewayHandler::new(None, None);
        handler.parse(gw, &ctx).await;

        // "ns1/p1" ref must be registered under the explicit namespace.
        let refs_p1 = runtime.plugin_ref_mgr.get_refs("ns1/p1");
        assert_eq!(refs_p1.len(), 1, "expected one Gateway ref for ns1/p1");
        assert_eq!(refs_p1[0].resource_key(), "gw-ns/my-gateway");

        // "p2" (bare name) must be expanded with the Gateway's namespace "gw-ns".
        let refs_p2 = runtime.plugin_ref_mgr.get_refs("gw-ns/p2");
        assert_eq!(refs_p2.len(), 1, "expected one Gateway ref for gw-ns/p2");
        assert_eq!(refs_p2[0].resource_key(), "gw-ns/my-gateway");
    }

    #[tokio::test]
    async fn gateway_backend_client_certificate_is_resolved_and_registered() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());

        let mut secret = Secret::default();
        secret.metadata.namespace = Some("gw-ns".to_string());
        secret.metadata.name = Some("client-cert".to_string());
        secret.data = Some(BTreeMap::from([
            ("tls.crt".to_string(), ByteString(b"certificate".to_vec())),
            ("tls.key".to_string(), ByteString(b"private-key".to_vec())),
        ]));
        runtime.secret_store.update(
            HashMap::from([("gw-ns/client-cert".to_string(), secret)]),
            &HashSet::new(),
        );

        let mut gw = make_test_gateway("gw-ns");
        gw.metadata.name = Some("my-gateway".to_string());
        gw.spec.tls = Some(GatewaySpecTLS {
            backend: Some(GatewayBackendTLS {
                client_certificate_ref: Some(SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "client-cert".to_string(),
                    namespace: None,
                }),
                resolved_client_certificate: None,
            }),
            frontend: None,
        });

        let handler = GatewayHandler::new(None, None);
        let ProcessResult::Continue(parsed) = handler.parse(gw, &ctx).await else {
            panic!("Gateway parse unexpectedly skipped the resource");
        };

        assert!(
            parsed
                .spec
                .tls
                .and_then(|tls| tls.backend)
                .and_then(|backend| backend.resolved_client_certificate)
                .is_some(),
            "Gateway backend client certificate must be embedded for conf_sync"
        );
        let refs = runtime.secret_ref_manager.get_refs("gw-ns/client-cert");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].resource_key(), "gw-ns/my-gateway");
    }

    fn make_gateway_with_plugin_annotation(ns: &str, annotation_value: &str) -> Gateway {
        let mut gw = make_test_gateway(ns);
        let mut annotations = BTreeMap::new();
        annotations.insert("edgion.io/edgion-plugins".to_string(), annotation_value.to_string());
        gw.metadata.annotations = Some(annotations);
        gw
    }

    #[test]
    fn validate_plugin_ref_no_annotation_returns_none() {
        let runtime = ControllerRuntime::default();
        let gw = make_test_gateway("default");
        assert!(validate_gateway_plugin_ref(&gw, &runtime).is_none());
    }

    #[test]
    fn validate_plugin_ref_empty_annotation_returns_none() {
        let runtime = ControllerRuntime::default();
        let gw = make_gateway_with_plugin_annotation("default", "  ");
        assert!(validate_gateway_plugin_ref(&gw, &runtime).is_none());
    }

    #[test]
    fn validate_plugin_ref_missing_resource_returns_error() {
        let runtime = ControllerRuntime::default();
        // "default/my-plugin" does not exist in the (empty) registry.
        let gw = make_gateway_with_plugin_annotation("default", "default/my-plugin");
        let result = validate_gateway_plugin_ref(&gw, &runtime);
        assert!(result.is_some());
        assert!(result.unwrap().contains("default/my-plugin"));
    }

    #[test]
    fn validate_plugin_ref_fallback_to_gateway_ns() {
        let runtime = ControllerRuntime::default();
        // Bare name "my-plugin" should be expanded to "my-ns/my-plugin".
        let gw = make_gateway_with_plugin_annotation("my-ns", "my-plugin");
        let result = validate_gateway_plugin_ref(&gw, &runtime);
        assert!(result.is_some());
        assert!(result.unwrap().contains("my-ns/my-plugin"));
    }
}
