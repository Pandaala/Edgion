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
    condition_false, condition_reasons, condition_true, condition_types, format_secret_key,
    frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
    make_conflict_key, ConflictKey, HandlerContext, ProcessResult, ProcessorHandler, ProxyProtocolListenerContribution,
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
    FrontendCACertificateRef, FrontendCAResolutionError, FrontendCASourceIdentity, FrontendCASourceKind,
    FrontendMatcherEligibility, FrontendValidationMode, GatewayAddress, GatewayStatus, GatewayStatusAddress,
    GatewayTLSConfig, GatewayTlsMode, Listener as GatewayListener, ListenerStatus, ResolvedFrontendCAReference,
    ResolvedServingCertificateReference, RouteGroupKind, ServingCertificateResolutionError,
    ServingCertificateSourceIdentity,
};
use edgion_resources::resources::proxy_protocol::{
    BoundedProxyProtocolReferenceIdentity, InboundProxyProtocolAnnotation, ProxyProtocolConfigErrorCode,
    ProxyProtocolPolicySourceKind, ResolvedInboundProxyProtocol,
};
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::ByteString;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use super::GatewayClassOwnership;
use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::{
    gateway_publication_identity, ownership_dependency_kinds,
};

const GATEWAY_SYSTEM_NAMESPACE: &str = "edgion-system";
const GATEWAY_API_GROUP: &str = "gateway.networking.k8s.io";
const INVALID_PROXY_PROTOCOL_CONFIG: &str = "InvalidProxyProtocolConfig";
const INVALID_PROXY_PROTOCOL_LISTENER: &str = "InvalidProxyProtocolListener";
const INVALID_PROXY_PROTOCOL_REF: &str = "InvalidProxyProtocolRef";
const PROXY_PROTOCOL_POLICY_CONFLICT: &str = "ProxyProtocolPolicyConflict";

/// Gateway handler
///
/// Features:
/// - filter: Verify Kubernetes class ownership and its published incarnation
/// - parse: Resolve TLS certificateRefs into ordered typed outcomes
/// - parse: Register Secret references to SecretRefManager
/// - parse: Register listeners to ListenerPortManager for intra-gateway conflict detection
/// - after_publish: Requeue routes after listener attachment inputs change
/// - on_delete: Clear SecretRefManager and ListenerPortManager references
/// - update_status: Set Conflicted/Programmed/ListenersNotValid conditions based on intra-gateway port conflicts
pub struct GatewayHandler {
    /// Kubernetes deployment ownership. FS/etcd intentionally process all Gateways.
    ownership: Option<GatewayClassOwnership>,
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
    /// True when listener TLS mode is not supported for its protocol/feature set.
    unsupported_tls_mode: bool,
    /// True when this TLS listener shares a port with another TLS listener
    /// that uses a different termination mode (e.g. Terminate + Passthrough).
    mixed_tls_conflict: bool,
    frontend_matcher_eligibility: Option<FrontendMatcherEligibility>,
}

impl GatewayHandler {
    /// Create a new GatewayHandler
    ///
    /// `ownership` is supplied by Kubernetes; FS/etcd use `None`.
    pub fn new(ownership: Option<GatewayClassOwnership>, default_address: Option<String>) -> Self {
        Self {
            ownership,
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
        if g.spec
            .listeners
            .as_ref()
            .is_some_and(|listeners| listeners.len() > edgion_resources::resources::gateway::MAX_GATEWAY_LISTENERS)
        {
            return vec![format!(
                "spec.listeners must contain at most {} entries",
                edgion_resources::resources::gateway::MAX_GATEWAY_LISTENERS
            )];
        }
        let mut errors = Vec::new();
        if let Some(frontend) = g.spec.tls.as_ref().and_then(|tls| tls.frontend.as_ref()) {
            if let Some(per_port) = frontend.per_port.as_ref() {
                if per_port.len() > edgion_resources::resources::gateway::MAX_GATEWAY_FRONTEND_PER_PORT {
                    errors.push(format!(
                        "spec.tls.frontend.perPort must contain at most {} entries",
                        edgion_resources::resources::gateway::MAX_GATEWAY_FRONTEND_PER_PORT
                    ));
                }
                let mut ports = BTreeSet::new();
                for entry in per_port {
                    if !(1..=65535).contains(&entry.port) {
                        errors.push(format!(
                            "spec.tls.frontend.perPort port {} is outside 1..=65535",
                            entry.port
                        ));
                    }
                    if !ports.insert(entry.port) {
                        errors.push(format!(
                            "spec.tls.frontend.perPort contains duplicate port {}",
                            entry.port
                        ));
                    }
                }
            }
            for validation in std::iter::once(frontend.default.validation.as_ref())
                .chain(
                    frontend
                        .per_port
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(|entry| entry.tls.validation.as_ref()),
                )
                .flatten()
            {
                for reference in &validation.ca_certificate_refs {
                    validate_frontend_ca_reference(reference, &mut errors);
                }
            }
        }
        for listener in g.spec.listeners.as_deref().unwrap_or_default() {
            if listener
                .tls
                .as_ref()
                .and_then(|tls| tls.certificate_refs.as_ref())
                .is_some_and(|refs| refs.len() > edgion_common::tls_validation::MAX_SERVING_CERTIFICATE_REFS)
            {
                errors.push(format!(
                    "listener '{}' tls.certificateRefs must contain at most {} entries",
                    listener.name,
                    edgion_common::tls_validation::MAX_SERVING_CERTIFICATE_REFS
                ));
            }
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

    fn filter(&self, g: &Gateway, ctx: &HandlerContext) -> bool {
        let Some(ownership) = &self.ownership else {
            return true;
        };
        ownership.owns_gateway(g, ctx.runtime())
    }

    async fn parse(&self, mut g: Gateway, ctx: &HandlerContext) -> ProcessResult<Gateway> {
        g.spec.resolved_attachment_proof = None;
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        ctx.runtime().frontend_ca_ref_manager.clear_resource_refs(&resource_ref);

        // Kubernetes admission normally enforces this bound. A forged
        // conf-sync object or FS/etcd input must revoke the previous listener
        // projection without first traversing or cloning an attacker-sized
        // vector. The validation error was computed from the original object.
        if g.spec
            .listeners
            .as_ref()
            .is_some_and(|listeners| listeners.len() > edgion_resources::resources::gateway::MAX_GATEWAY_LISTENERS)
        {
            g.spec.listeners = None;
        }

        // Listener frontendValidation is a controller-private projection. Raw
        // objects from every config center can still deserialize this transport
        // field, so erase both it and its resolved material before any branch can
        // return or skip the listener. The only operator surface is
        // Gateway.spec.tls.frontend.
        if let Some(listeners) = g.spec.listeners.as_mut() {
            for listener in listeners {
                if let Some(tls) = listener.tls.as_mut() {
                    tls.frontend_validation = None;
                    tls.resolved_frontend_ca_refs = None;
                    tls.frontend_matcher_eligibility = None;
                    tls.resolved_certificate_refs = None;
                }
            }
        }
        let frontend_schema_errors = validate_standard_frontend_tls(&g);
        project_standard_frontend_validation(&mut g, !frontend_schema_errors.is_empty());

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
                let Some(tls_config) = &mut listener.tls else {
                    continue;
                };

                // Only Terminate mode needs certificate resolution; Passthrough proxies raw TLS.
                if tls_config.mode.unwrap_or_default() != GatewayTlsMode::Terminate {
                    continue;
                }

                if let Some(cert_refs) = &tls_config.certificate_refs {
                    if cert_refs.len() > edgion_common::tls_validation::MAX_SERVING_CERTIFICATE_REFS {
                        tls_config.frontend_matcher_eligibility = Some(FrontendMatcherEligibility::StrictReject);
                        continue;
                    }
                    let mut outcomes = Vec::with_capacity(cert_refs.len());
                    let gateway_ns = g.metadata.namespace.as_deref().unwrap_or("");

                    for (index, cert_ref) in cert_refs.iter().enumerate() {
                        let secret_ns = cert_ref.namespace.as_ref().or(g.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(secret_ns, &cert_ref.name);
                        let cert_ns_str = secret_ns.map(String::as_str).unwrap_or("");
                        let group = cert_ref.group.as_deref().unwrap_or("");
                        let kind = cert_ref.kind.as_deref().unwrap_or("Secret");
                        let mut outcome = ResolvedServingCertificateReference {
                            index: index as u16,
                            source: ServingCertificateSourceIdentity {
                                group: group.to_string(),
                                kind: kind.to_string(),
                                namespace: cert_ns_str.to_string(),
                                name: cert_ref.name.clone(),
                            },
                            secret: None,
                            error: None,
                        };

                        // Register to SecretRefManager (critical for cascading updates)
                        // When this Secret arrives or updates, SecretHandler will trigger requeue for this Gateway
                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        // Register cross-namespace cert refs so ReferenceGrant changes requeue this Gateway
                        if cert_ns_str != gateway_ns {
                            cross_ns_manager.add_ref(cert_ns_str.to_string(), cross_ns_ref.clone());
                        }

                        let valid_ref = kind == "Secret" && group.is_empty();
                        let permitted = cert_ns_str == gateway_ns
                            || !is_reference_grant_validation_enabled()
                            || ctx.runtime().ref_grant_store.check_reference_allowed(
                                gateway_ns,
                                GATEWAY_API_GROUP,
                                "Gateway",
                                cert_ns_str,
                                "",
                                "Secret",
                                Some(&cert_ref.name),
                            );
                        if !valid_ref || !permitted {
                            outcome.error = Some(if valid_ref {
                                ServingCertificateResolutionError::RefNotPermitted
                            } else {
                                ServingCertificateResolutionError::InvalidReference
                            });
                            outcomes.push(outcome);
                            continue;
                        }

                        // Try to resolve Secret from runtime store
                        if let Some(mut secret) = ctx
                            .runtime()
                            .secret_store
                            .get(secret_ns.map(|s| s.as_str()), &cert_ref.name)
                        {
                            secret.metadata.namespace = Some(cert_ns_str.to_string());
                            secret.metadata.name = Some(cert_ref.name.clone());
                            outcome.secret = Some(secret);
                            tracing::debug!(
                                gateway = %resource_ref.key(),
                                listener = %listener.name,
                                secret_key = %secret_key,
                                "Secret resolved and filled into Gateway TLS config"
                            );
                        } else {
                            outcome.error = Some(ServingCertificateResolutionError::NotFound);
                            // Secret not found yet - this is normal if Secret arrives after Gateway
                            // The SecretRefManager reference ensures we'll be requeued when Secret arrives
                            tracing::info!(
                                gateway = %resource_ref.key(),
                                listener = %listener.name,
                                secret_key = %secret_key,
                                "Secret not found yet, Gateway TLS will be updated when Secret arrives"
                            );
                        }
                        outcomes.push(outcome);
                    }
                    tls_config.resolved_certificate_refs = Some(outcomes);
                }

                // Resolve the controller-projected standard frontend validation.
                let gateway_ns = g.metadata.namespace.as_deref().unwrap_or("");
                if let Some(ref fv) = tls_config.frontend_validation {
                    if fv.ca_certificate_refs.len() > edgion_common::tls_validation::MAX_CA_REFERENCE_COUNT {
                        tls_config.frontend_matcher_eligibility = Some(FrontendMatcherEligibility::StrictReject);
                        tls_config.resolved_frontend_ca_refs = None;
                        continue;
                    }
                    let resolved_ca_refs = resolve_frontend_ca_refs(
                        &fv.ca_certificate_refs,
                        gateway_ns,
                        &resource_ref,
                        &listener.name,
                        ctx,
                        &cross_ns_ref,
                        cross_ns_manager,
                        fv.mode != Some(FrontendValidationMode::AllowInsecureFallback),
                    );

                    tls_config.resolved_frontend_ca_refs = Some(resolved_ca_refs);
                }
            }
        }

        ProcessResult::Continue(g)
    }

    fn finalize_transport(&self, gateway: &mut Gateway, ctx: &HandlerContext) {
        let epoch = ctx.runtime().next_gateway_attachment_epoch();
        gateway.spec.resolved_attachment_proof = Some(build_gateway_attachment_proof(
            gateway,
            ctx.runtime().gateway_attachment_boot_nonce(),
            epoch,
        ));
    }

    async fn after_publish(&self, gateway: &Gateway, ctx: &HandlerContext) {
        let gateway_key = format!(
            "{}/{}",
            gateway.metadata.namespace.as_deref().unwrap_or(""),
            gateway.metadata.name.as_deref().unwrap_or("")
        );
        // Route resolvers prefer the published Gateway cache. Enqueue only
        // after publication, with a config gate to break Route/Gateway cycles.
        let route_index = &ctx.runtime().gateway_route_index;
        if ctx.runtime().status_identity().is_some()
            && route_index
                .update_publication_ownership(&gateway_key, gateway_publication_identity(gateway, ctx.runtime()))
        {
            // Source events may reach dependents before this incarnation is
            // published. Replay once after publication, including uncached ACME
            // and transitive delegation references absent from the direct index.
            for kind in ownership_dependency_kinds("Gateway") {
                if let Some(processor) = ctx.runtime().processor_registry.get(kind) {
                    for key in processor.raw_keys() {
                        ctx.requeue(kind, key).await;
                    }
                }
            }
        }
        if let Some(routes) = route_index.update_listener_config_and_collect_routes(&gateway_key, gateway) {
            for (route_kind, route_key) in routes {
                if !matches!(route_kind, ResourceKind::EdgionTls | ResourceKind::EdgionAcme) {
                    ctx.requeue(route_kind.as_str(), route_key).await;
                }
            }
        }
        // EdgionTls and EdgionAcme consume the publication proof and must
        // refresh on every epoch, even when listener routing inputs did not
        // otherwise change.
        for (route_kind, route_key) in route_index.get_routes_for_gateway(&gateway_key) {
            if matches!(route_kind, ResourceKind::EdgionTls | ResourceKind::EdgionAcme) {
                ctx.requeue(route_kind.as_str(), route_key).await;
            }
        }
    }

    async fn after_unpublish(&self, gateway: &Gateway, ctx: &HandlerContext) {
        let gateway_key = format!(
            "{}/{}",
            gateway.metadata.namespace.as_deref().unwrap_or(""),
            gateway.metadata.name.as_deref().unwrap_or("")
        );
        let route_index = &ctx.runtime().gateway_route_index;
        for (route_kind, route_key) in route_index.remove_listener_config_and_collect_routes(&gateway_key) {
            ctx.requeue(route_kind.as_str(), route_key).await;
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
        ctx.runtime().frontend_ca_ref_manager.clear_resource_refs(&resource_ref);
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

        tracing::debug!(
            gateway = %gateway_key,
            "Cleared Gateway secret references and port manager registration"
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
        let gateway_key = format!(
            "{}/{}",
            gateway.metadata.namespace.as_deref().unwrap_or(""),
            gateway.metadata.name.as_deref().unwrap_or("")
        );
        let conflicts = _ctx.runtime().listener_port_manager.detect_conflicts(&gateway_key);
        let listener_infos: Vec<ListenerInfo> = listeners_slice
            .iter()
            .map(|l| {
                let route_count = tracker.count_for_listener(&gateway_key_for_tracker, &l.name);
                let (supported_kinds, mut kind_errors) = compute_supported_kinds(l);
                // Reject an oversized serving-ref list before any per-reference
                // validation, lookup, clone, error construction, or index cast.
                let too_many_serving_refs = l
                    .tls
                    .as_ref()
                    .and_then(|tls| tls.certificate_refs.as_ref())
                    .is_some_and(|refs| refs.len() > edgion_common::tls_validation::MAX_SERVING_CERTIFICATE_REFS);
                let mut resolved_refs_errors = if too_many_serving_refs {
                    Vec::new()
                } else {
                    validate_listener_resolved_refs(gateway, l, _ctx.runtime())
                };
                resolved_refs_errors.append(&mut kind_errors);

                let no_valid_ca_certificate = has_all_ca_refs_invalid(l, &gateway_ns);

                let unsupported_tls_mode = listener_has_unsupported_tls_mode(l);
                let mixed_tls_conflict = mixed_tls_listeners.contains(&l.name);
                let frontend_configured = l
                    .tls
                    .as_ref()
                    .is_some_and(|tls| tls.frontend_validation.is_some() || tls.frontend_matcher_eligibility.is_some());
                let has_edgion_tls_provider = l
                    .tls
                    .as_ref()
                    .and_then(|tls| tls.options.as_ref())
                    .and_then(|options| options.get("edgion.io/cert-provider"))
                    .is_some_and(|value| value.as_str() == Some("EdgionTls"));
                let declared_serving_refs = l
                    .tls
                    .as_ref()
                    .and_then(|tls| tls.certificate_refs.as_ref())
                    .is_some_and(|refs| !refs.is_empty());
                let unrelated_ref_error = resolved_refs_errors.iter().any(|error| {
                    !matches!(
                        error,
                        GatewayResolvedRefsError::InvalidCACertificateKind { .. }
                            | GatewayResolvedRefsError::InvalidCACertificateRef { .. }
                            | GatewayResolvedRefsError::CACertRefNotPermitted { .. }
                    )
                });
                let has_ca_error = resolved_refs_errors.iter().any(|error| {
                    matches!(
                        error,
                        GatewayResolvedRefsError::InvalidCACertificateKind { .. }
                            | GatewayResolvedRefsError::InvalidCACertificateRef { .. }
                            | GatewayResolvedRefsError::CACertRefNotPermitted { .. }
                    )
                });
                let frontend_matcher_eligibility = if l.tls.is_none()
                    || l.tls
                        .as_ref()
                        .is_some_and(|tls| tls.mode.unwrap_or_default() != GatewayTlsMode::Terminate)
                {
                    Some(FrontendMatcherEligibility::Omit)
                } else if l.tls.as_ref().is_some_and(|tls| {
                    tls.frontend_matcher_eligibility == Some(FrontendMatcherEligibility::StrictReject)
                }) || has_ca_error
                    || too_many_serving_refs
                    || (!validation_errors.is_empty() && has_edgion_tls_provider)
                    || ((unrelated_ref_error || unsupported_tls_mode || mixed_tls_conflict) && has_edgion_tls_provider)
                {
                    Some(FrontendMatcherEligibility::StrictReject)
                } else if unrelated_ref_error || unsupported_tls_mode || mixed_tls_conflict {
                    Some(FrontendMatcherEligibility::Omit)
                } else if declared_serving_refs {
                    Some(FrontendMatcherEligibility::Programmed)
                } else if frontend_configured && has_edgion_tls_provider {
                    Some(FrontendMatcherEligibility::RequireExactEdgionTlsCertificate)
                } else if frontend_configured {
                    Some(FrontendMatcherEligibility::StrictReject)
                } else {
                    Some(FrontendMatcherEligibility::Omit)
                };

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
                    unsupported_tls_mode,
                    mixed_tls_conflict,
                    frontend_matcher_eligibility,
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

        if let Some(listeners) = gateway.spec.listeners.as_mut() {
            for listener in listeners {
                let Some(tls) = listener.tls.as_mut() else {
                    continue;
                };
                let info = listener_infos.iter().find(|info| info.name == listener.name);
                let blocked_by_unrelated_listener_state = conflicts.contains_key(&listener.name)
                    || proxy_protocol_affected.contains(&listener.name)
                    || proxy_protocol_conflicts.contains(&listener.name);
                tls.frontend_matcher_eligibility = if blocked_by_unrelated_listener_state {
                    let has_edgion_tls_provider = tls
                        .options
                        .as_ref()
                        .and_then(|options| options.get("edgion.io/cert-provider"))
                        .is_some_and(|value| value.as_str() == Some("EdgionTls"));
                    Some(if has_edgion_tls_provider || tls.frontend_validation.is_some() {
                        FrontendMatcherEligibility::StrictReject
                    } else {
                        FrontendMatcherEligibility::Omit
                    })
                } else {
                    info.and_then(|info| info.frontend_matcher_eligibility)
                        .or(Some(FrontendMatcherEligibility::Omit))
                };
            }
        }

        // Initialize status if not present
        let status = gateway.status.get_or_insert_with(GatewayStatus::default);

        status.addresses = Some(derived_addresses);

        // Initialize conditions if not present
        let conditions = status.conditions.get_or_insert_with(Vec::new);

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
        let mut conflicting_names: Vec<&String> = conflicts.keys().collect();
        conflicting_names.sort_unstable();
        if !proxy_protocol_conflicts.is_empty() {
            let mut message = format!(
                "Listeners have conflicting inbound PROXY protocol policies: {:?}",
                proxy_protocol_conflicts
            );
            if !conflicts.is_empty() {
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
                unsupported_tls_mode,
                mixed_tls_conflict,
                frontend_matcher_eligibility: _,
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

/// Project the standard Gateway-level frontend TLS policy into HTTPS listeners.
///
/// A matching per-port entry is authoritative even when its nested policy is
/// absent at the wire level; serde rejects that malformed standard shape, and
/// this selection deliberately never falls back to `default` after a port match.
fn project_standard_frontend_validation(gateway: &mut Gateway, malformed: bool) {
    let frontend = gateway.spec.tls.as_ref().and_then(|tls| tls.frontend.as_ref()).cloned();
    let Some(frontend) = frontend else {
        return;
    };
    let Some(listeners) = gateway.spec.listeners.as_mut() else {
        return;
    };

    for listener in listeners {
        if !listener.protocol.eq_ignore_ascii_case("HTTPS") {
            continue;
        }
        if malformed {
            let tls = listener.tls.get_or_insert_with(empty_gateway_tls_config);
            tls.frontend_matcher_eligibility = Some(FrontendMatcherEligibility::StrictReject);
            continue;
        }
        let validation = match frontend
            .per_port
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|entry| entry.port == listener.port)
        {
            Some(entry) => entry.tls.validation.clone(),
            None => frontend.default.validation.clone(),
        };
        let Some(validation) = validation else {
            continue;
        };
        let tls = listener.tls.get_or_insert_with(empty_gateway_tls_config);
        tls.frontend_validation = Some(validation);
    }
}

fn empty_gateway_tls_config() -> GatewayTLSConfig {
    GatewayTLSConfig {
        mode: None,
        certificate_refs: None,
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: None,
    }
}

fn validate_standard_frontend_tls(gateway: &Gateway) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(frontend) = gateway.spec.tls.as_ref().and_then(|tls| tls.frontend.as_ref()) else {
        return errors;
    };
    if let Some(per_port) = frontend.per_port.as_ref() {
        if per_port.len() > edgion_resources::resources::gateway::MAX_GATEWAY_FRONTEND_PER_PORT {
            errors.push(format!(
                "spec.tls.frontend.perPort must contain at most {} entries",
                edgion_resources::resources::gateway::MAX_GATEWAY_FRONTEND_PER_PORT
            ));
        }
        let mut ports = BTreeSet::new();
        for entry in per_port {
            if !(1..=65535).contains(&entry.port) {
                errors.push(format!(
                    "spec.tls.frontend.perPort port {} is outside 1..=65535",
                    entry.port
                ));
            }
            if !ports.insert(entry.port) {
                errors.push(format!(
                    "spec.tls.frontend.perPort contains duplicate port {}",
                    entry.port
                ));
            }
        }
    }
    for validation in std::iter::once(frontend.default.validation.as_ref())
        .chain(
            frontend
                .per_port
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|entry| entry.tls.validation.as_ref()),
        )
        .flatten()
    {
        for reference in &validation.ca_certificate_refs {
            validate_frontend_ca_reference(reference, &mut errors);
        }
    }
    errors
}

fn validate_frontend_ca_reference(reference: &FrontendCACertificateRef, errors: &mut Vec<String>) {
    let dns_label = |value: &str| {
        !value.is_empty()
            && value.len() <= 63
            && value
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
            && value
                .chars()
                .last()
                .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
            && value
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    };
    let kind_valid = !reference.kind.is_empty()
        && reference.kind.len() <= 63
        && reference.kind.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic())
        && reference
            .kind
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && reference.kind.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-');
    if !reference.group.is_empty() {
        errors.push("frontend caCertificateRefs.group must be the exact empty core group".to_string());
    }
    if !kind_valid {
        errors.push(format!(
            "frontend caCertificateRefs kind '{}' is invalid",
            reference.kind
        ));
    }
    if reference.name.is_empty() || reference.name.len() > 253 {
        errors.push(format!(
            "frontend caCertificateRefs name '{}' is invalid",
            reference.name
        ));
    }
    if let Some(namespace) = reference.namespace.as_deref() {
        if !dns_label(namespace) {
            errors.push(format!(
                "frontend caCertificateRefs namespace '{}' is invalid",
                namespace
            ));
        }
    }
}

/// Resolve frontend CA certificate refs into a complete, ordered outcome set.
#[allow(clippy::too_many_arguments)]
fn resolve_frontend_ca_refs(
    ca_refs: &[FrontendCACertificateRef],
    gateway_ns: &str,
    resource_ref: &ResourceRef,
    listener_name: &str,
    ctx: &HandlerContext,
    cross_ns_ref: &ResourceRef,
    cross_ns_manager: &crate::conf_mgr::sync_runtime::resource_processor::ref_grant::CrossNamespaceRefManager,
    read_payload: bool,
) -> Vec<ResolvedFrontendCAReference> {
    let mut outcomes = Vec::with_capacity(ca_refs.len());
    let mut aggregate_bytes = 0usize;
    let mut unique_certificates = HashSet::new();

    for (index, ca_ref) in ca_refs.iter().enumerate() {
        let ca_kind = ca_ref.kind.as_str();
        let ca_ns_str = ca_ref.namespace.as_deref().unwrap_or(gateway_ns);
        let source_kind = match ca_kind {
            "Secret" => Some((FrontendCaSourceKind::Secret, FrontendCASourceKind::Secret)),
            "ConfigMap" => Some((FrontendCaSourceKind::ConfigMap, FrontendCASourceKind::ConfigMap)),
            _ => None,
        };
        let mut outcome = ResolvedFrontendCAReference {
            index: index as u8,
            source: FrontendCASourceIdentity {
                kind: source_kind
                    .map(|(_, transport)| transport)
                    .unwrap_or(FrontendCASourceKind::Secret),
                namespace: ca_ns_str.to_string(),
                name: ca_ref.name.clone(),
            },
            ca_certificate: None,
            error: None,
        };

        if ca_ref.group.is_empty() {
            if let Some((manager_kind, _)) = source_kind {
                ctx.runtime().frontend_ca_ref_manager.add_ref(
                    FrontendCaSourceKey::new(manager_kind, ca_ns_str, &ca_ref.name).index_key(),
                    resource_ref.clone(),
                );
            }
        }

        // Register cross-namespace refs
        if ca_ns_str != gateway_ns {
            cross_ns_manager.add_ref(ca_ns_str.to_string(), cross_ns_ref.clone());
        }

        if !ca_ref.group.is_empty() || source_kind.is_none() {
            outcome.error = Some(FrontendCAResolutionError::InvalidReference);
            outcomes.push(outcome);
            continue;
        }

        let permitted = ca_ns_str == gateway_ns
            || !is_reference_grant_validation_enabled()
            || ctx.runtime().ref_grant_store.check_reference_allowed(
                gateway_ns,
                GATEWAY_API_GROUP,
                "Gateway",
                ca_ns_str,
                "",
                ca_kind,
                Some(&ca_ref.name),
            );
        if !permitted {
            outcome.error = Some(FrontendCAResolutionError::RefNotPermitted);
            outcomes.push(outcome);
            continue;
        }

        if !read_payload {
            outcomes.push(outcome);
            continue;
        }

        match ca_kind {
            "Secret" => {
                outcome =
                    resolve_frontend_ca_secret(outcome, ctx.runtime().secret_store.get(Some(ca_ns_str), &ca_ref.name));
            }
            "ConfigMap" => {
                outcome = resolve_frontend_ca_configmap(
                    outcome,
                    ctx.runtime().configmap_store.get(Some(ca_ns_str), &ca_ref.name),
                );
            }
            _ => unreachable!("kind was validated above"),
        }
        if let Some(pem) = outcome.ca_certificate.as_ref() {
            let within_byte_limit = aggregate_bytes
                .checked_add(pem.0.len())
                .filter(|total| *total <= edgion_common::tls_validation::MAX_CA_BUNDLE_BYTES_AGGREGATE);
            let bundle = edgion_common::tls_validation::parse_canonical_ca_bundle(&pem.0).ok();
            let new_certificates = bundle
                .as_ref()
                .map(|bundle| {
                    bundle
                        .certificates_der
                        .iter()
                        .filter(|der| !unique_certificates.contains(*der))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let within_certificate_limit = unique_certificates
                .len()
                .checked_add(new_certificates.len())
                .is_some_and(|total| total <= edgion_common::tls_validation::MAX_CA_CERTIFICATES_AGGREGATE);
            if let (Some(total_bytes), true, Some(_)) = (within_byte_limit, within_certificate_limit, bundle) {
                aggregate_bytes = total_bytes;
                unique_certificates.extend(new_certificates);
            } else {
                outcome.ca_certificate = None;
                outcome.error = Some(FrontendCAResolutionError::InvalidCertificate);
            }
        }
        tracing::debug!(gateway = %resource_ref.key(), listener = %listener_name, index, "Frontend CA reference evaluated");
        outcomes.push(outcome);
    }

    outcomes
}

fn resolve_frontend_ca_secret(
    mut outcome: ResolvedFrontendCAReference,
    secret: Option<k8s_openapi::api::core::v1::Secret>,
) -> ResolvedFrontendCAReference {
    let Some(secret) = secret else {
        outcome.error = Some(FrontendCAResolutionError::NotFound);
        return outcome;
    };
    let value = secret.data.as_ref().and_then(|data| data.get("ca.crt"));
    resolve_frontend_ca_bytes(outcome, value.map(|value| value.0.clone()))
}

fn resolve_frontend_ca_configmap(
    mut outcome: ResolvedFrontendCAReference,
    configmap: Option<k8s_openapi::api::core::v1::ConfigMap>,
) -> ResolvedFrontendCAReference {
    let Some(configmap) = configmap else {
        outcome.error = Some(FrontendCAResolutionError::NotFound);
        return outcome;
    };
    let value = configmap
        .data
        .as_ref()
        .and_then(|data| data.get("ca.crt"))
        .map(|value| value.as_bytes().to_vec());
    resolve_frontend_ca_bytes(outcome, value)
}

fn resolve_frontend_ca_bytes(
    mut outcome: ResolvedFrontendCAReference,
    value: Option<Vec<u8>>,
) -> ResolvedFrontendCAReference {
    let Some(value) = value else {
        outcome.error = Some(FrontendCAResolutionError::MissingCertificate);
        return outcome;
    };
    if value.is_empty() {
        outcome.error = Some(FrontendCAResolutionError::EmptyCertificate);
    } else if !is_valid_ca_bundle(&value) {
        outcome.error = Some(FrontendCAResolutionError::InvalidCertificate);
    } else {
        outcome.ca_certificate = Some(ByteString(value));
    }
    outcome
}

fn build_gateway_attachment_proof(
    gateway: &Gateway,
    boot_nonce: &str,
    epoch: u64,
) -> edgion_resources::resources::edgion_tls::GatewayAttachmentProof {
    // Hash an explicit attachment-only projection. In particular, include
    // allowedRoutes because it controls whether an EdgionTls parent may attach,
    // while excluding Gateway backend TLS and every private-key byte.
    let listeners = gateway
        .spec
        .listeners
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|listener| {
            let tls = listener.tls.as_ref().map(|tls| {
                let serving = tls
                    .resolved_certificate_refs
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|outcome| {
                        let certificate_sha256 = outcome
                            .secret
                            .as_ref()
                            .and_then(|secret| secret.data.as_ref())
                            .and_then(|data| data.get(edgion_resources::constants::secret_keys::tls::CERT))
                            .map(|value| format!("{:x}", Sha256::digest(&value.0)));
                        serde_json::json!({
                            "index": outcome.index,
                            "source": outcome.source,
                            "certificateSha256": certificate_sha256,
                            "error": outcome.error,
                        })
                    })
                    .collect::<Vec<_>>();
                let frontend_ca = tls
                    .resolved_frontend_ca_refs
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|outcome| {
                        serde_json::json!({
                            "index": outcome.index,
                            "source": outcome.source,
                            "certificateSha256": outcome.ca_certificate.as_ref()
                                .map(|value| format!("{:x}", Sha256::digest(&value.0))),
                            "error": outcome.error,
                        })
                    })
                    .collect::<Vec<_>>();
                serde_json::json!({
                    "mode": tls.mode,
                    "certificateRefs": tls.certificate_refs,
                    "options": tls.options,
                    "servingOutcomes": serving,
                    "frontendValidation": tls.frontend_validation,
                    "frontendCaOutcomes": frontend_ca,
                    "frontendMatcherEligibility": tls.frontend_matcher_eligibility,
                })
            });
            serde_json::json!({
                "name": listener.name,
                "hostname": listener.hostname,
                "port": listener.port,
                "protocol": listener.protocol,
                "allowedRoutes": listener.allowed_routes,
                "tls": tls,
            })
        })
        .collect::<Vec<_>>();
    let mut digest = Sha256::new();
    digest.update(b"edgion.gateway-attachment-proof.v1\0");
    digest.update(serde_json::to_vec(&(gateway.spec.addresses.as_ref(), listeners)).unwrap_or_default());
    let attachment_digest = format!("{:x}", digest.finalize());
    let instance = gateway
        .metadata
        .uid
        .clone()
        .unwrap_or_else(|| format!("local-instance-{boot_nonce}-{epoch}"));
    edgion_resources::resources::edgion_tls::GatewayAttachmentProof {
        instance,
        generation: gateway.metadata.generation.unwrap_or_default(),
        epoch: format!("controller-{boot_nonce}-{epoch}"),
        attachment_digest,
    }
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
#[allow(clippy::too_many_arguments)]
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

fn is_valid_ca_bundle(data: &[u8]) -> bool {
    edgion_common::tls_validation::parse_canonical_ca_bundle(data).is_ok()
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
    _runtime: &ControllerRuntime,
) -> Vec<GatewayResolvedRefsError> {
    let gateway_ns = gateway.metadata.namespace.as_deref().unwrap_or("default");
    let mut errors = Vec::new();

    let Some(tls) = &listener.tls else {
        return errors;
    };
    if let Some(certificate_refs) = &tls.certificate_refs {
        let outcomes = tls.resolved_certificate_refs.as_deref().unwrap_or_default();
        for (index, cert_ref) in certificate_refs.iter().enumerate() {
            let cert_kind = cert_ref.kind.as_deref().unwrap_or("Secret");
            if cert_kind != "Secret" {
                errors.push(GatewayResolvedRefsError::InvalidCertRef {
                    kind: cert_kind.to_string(),
                });
                continue;
            }

            let cert_group = cert_ref.group.as_deref().unwrap_or("");
            if !cert_group.is_empty() {
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

            let outcome = outcomes.get(index).filter(|outcome| {
                outcome.index as usize == index
                    && outcome.source.group == cert_group
                    && outcome.source.kind == cert_kind
                    && outcome.source.namespace == cert_ns
                    && outcome.source.name == cert_ref.name
            });
            let Some(outcome) = outcome else {
                errors.push(GatewayResolvedRefsError::SecretInvalid {
                    namespace: cert_ns.to_string(),
                    name: cert_ref.name.clone(),
                    reason: "missing or mismatched controller resolution outcome".to_string(),
                });
                continue;
            };
            match outcome.error {
                Some(ServingCertificateResolutionError::InvalidReference) => {
                    errors.push(GatewayResolvedRefsError::InvalidCertRef {
                        kind: cert_kind.to_string(),
                    });
                    continue;
                }
                Some(ServingCertificateResolutionError::RefNotPermitted) => {
                    errors.push(GatewayResolvedRefsError::RefNotPermitted {
                        from_namespace: gateway_ns.to_string(),
                        namespace: cert_ns.to_string(),
                        name: cert_ref.name.clone(),
                    });
                    continue;
                }
                Some(ServingCertificateResolutionError::NotFound) => {
                    errors.push(GatewayResolvedRefsError::SecretNotFound {
                        namespace: cert_ns.to_string(),
                        name: cert_ref.name.clone(),
                    });
                    continue;
                }
                None => {}
            }

            match outcome.secret.as_ref() {
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
                                let cert_pem_valid =
                                    edgion_common::tls_validation::parse_strict_certificate_chain(cert_data).is_ok();
                                let key_pem_valid =
                                    edgion_common::tls_validation::parse_strict_private_key_pem(key_data).is_ok();

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
        }
    }

    // Status consumes the same complete, ordered resolution outcomes that are
    // transported to the Gateway. This avoids a second store read and ensures
    // denied references reveal nothing about target existence or content.
    if let Some(ref fv) = tls.frontend_validation {
        if fv.mode == Some(FrontendValidationMode::AllowInsecureFallback) {
            return errors;
        }
        let outcomes = tls.resolved_frontend_ca_refs.as_deref().unwrap_or_default();
        for (index, ca_ref) in fv.ca_certificate_refs.iter().enumerate() {
            let ca_ns = ca_ref.namespace.as_deref().unwrap_or(gateway_ns);
            let expected_kind = match ca_ref.kind.as_str() {
                "Secret" => Some(FrontendCASourceKind::Secret),
                "ConfigMap" => Some(FrontendCASourceKind::ConfigMap),
                _ => None,
            };
            let outcome = outcomes.get(index).filter(|outcome| {
                outcome.index as usize == index
                    && expected_kind == Some(outcome.source.kind)
                    && outcome.source.namespace == ca_ns
                    && outcome.source.name == ca_ref.name
            });
            let Some(outcome) = outcome else {
                errors.push(GatewayResolvedRefsError::InvalidCACertificateRef {
                    namespace: ca_ns.to_string(),
                    name: ca_ref.name.clone(),
                    reason: "missing or mismatched controller resolution outcome".to_string(),
                });
                continue;
            };
            match outcome.error {
                Some(FrontendCAResolutionError::InvalidReference) => {
                    errors.push(GatewayResolvedRefsError::InvalidCACertificateKind {
                        kind: ca_ref.kind.clone(),
                    });
                }
                Some(FrontendCAResolutionError::RefNotPermitted) => {
                    errors.push(GatewayResolvedRefsError::CACertRefNotPermitted {
                        from_namespace: gateway_ns.to_string(),
                        namespace: ca_ns.to_string(),
                        name: ca_ref.name.clone(),
                    });
                }
                Some(error) => errors.push(GatewayResolvedRefsError::InvalidCACertificateRef {
                    namespace: ca_ns.to_string(),
                    name: ca_ref.name.clone(),
                    reason: format!("{error:?}"),
                }),
                None if outcome.ca_certificate.is_none() => {
                    errors.push(GatewayResolvedRefsError::InvalidCACertificateRef {
                        namespace: ca_ns.to_string(),
                        name: ca_ref.name.clone(),
                        reason: "resolved outcome contains no CA certificate".to_string(),
                    });
                }
                None => {}
            }
        }
    }

    errors
}

/// Check if a listener has frontend validation CA refs and ALL of them are invalid.
/// Returns true only when CA refs were configured but none are valid.
fn has_all_ca_refs_invalid(listener: &GatewayListener, gateway_namespace: &str) -> bool {
    let Some(tls) = listener.tls.as_ref() else {
        return false;
    };
    let Some(validation) = tls.frontend_validation.as_ref() else {
        return false;
    };
    if validation.mode == Some(FrontendValidationMode::AllowInsecureFallback)
        || validation.ca_certificate_refs.is_empty()
    {
        return false;
    }
    let outcomes = tls.resolved_frontend_ca_refs.as_deref().unwrap_or_default();
    validation
        .ca_certificate_refs
        .iter()
        .enumerate()
        .all(|(index, reference)| {
            let namespace = reference.namespace.as_deref().unwrap_or(gateway_namespace);
            let expected_kind = match reference.kind.as_str() {
                "Secret" => Some(FrontendCASourceKind::Secret),
                "ConfigMap" => Some(FrontendCASourceKind::ConfigMap),
                _ => None,
            };
            outcomes.get(index).is_none_or(|outcome| {
                outcome.index as usize != index
                    || Some(outcome.source.kind) != expected_kind
                    || outcome.source.namespace != namespace
                    || outcome.source.name != reference.name
                    || outcome.error.is_some()
                    || outcome.ca_certificate.is_none()
            })
        })
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
        let is_valid_for_protocol =
            super::route_utils::protocol_supports_route_kind(&listener.protocol, requested_group, &requested.kind);

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
    super::route_utils::protocol_default_route_kinds(protocol)
        .iter()
        .map(|kind| RouteGroupKind {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: (*kind).to_string(),
        })
        .collect()
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
    let errors: Vec<String> = annotations::edgion::parse_plugin_ref_list(value, gateway_ns)
        .into_iter()
        .flat_map(|key| {
            let plugins = runtime
                .processor_registry
                .get("EdgionPlugins")
                .and_then(|p| p.get_raw_any(&key))
                .and_then(|raw| {
                    raw.downcast::<edgion_resources::prelude_resources::EdgionPlugins>()
                        .ok()
                });
            match plugins {
                Some(plugins) => plugins
                    .validate_request_id_early_attachment()
                    .into_iter()
                    .map(|error| format!("EdgionPlugins {key}: {error}"))
                    .collect::<Vec<_>>(),
                None => vec![format!("EdgionPlugins not found: {key}")],
            }
        })
        .collect();
    if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    }
}

/// Returns `Some(error_message)` when the Gateway's `edgion.io/dns-resolver-ref`
/// annotation points at an httpdns LinkSys in another namespace with no matching
/// ReferenceGrant. `None` when the annotation is absent, same-namespace, or
/// permitted. Same-namespace refs and trusted config are never gated.
fn validate_gateway_dns_resolver_ref(gateway: &Gateway, runtime: &ControllerRuntime) -> Option<String> {
    use super::dns_resolver_guard::check_dns_resolver_ref_grant;
    let gateway_ns = gateway.metadata.namespace.as_deref().unwrap_or("default");
    let owner = ResourceRef::new(
        ResourceKind::Gateway,
        gateway.metadata.namespace.clone(),
        gateway.metadata.name.clone().unwrap_or_default(),
    );
    let denial = check_dns_resolver_ref_grant(
        gateway_ns,
        "gateway.networking.k8s.io",
        "Gateway",
        gateway.metadata.annotations.as_ref(),
        &owner,
        runtime,
    )?;
    Some(format!(
        "dns-resolver-ref LinkSys '{}/{}' cross-namespace reference not permitted (no ReferenceGrant)",
        denial.ref_namespace, denial.name
    ))
}

#[cfg(test)]
#[path = "gateway_route_refresh_tests.rs"]
mod route_refresh_tests;

#[cfg(test)]
mod tests {
    use super::{
        build_gateway_attachment_proof, compute_supported_kinds, gateway_service_lookup_candidates,
        has_all_ca_refs_invalid, resolve_gateway_stream_plugin_ref, resolve_inbound_proxy_protocol,
        validate_gateway_dns_resolver_ref, validate_gateway_plugin_ref, validate_gateway_stream_plugin_ref,
        validate_listener_resolved_refs, GatewayHandler,
    };
    use crate::conf_mgr::sync_runtime::resource_processor::{
        frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
        HandlerContext, ProcessResult, ProcessorHandler, ResourceProcessor, ResourceRef,
    };
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::constants::app::GATEWAY_NAME;
    use edgion_resources::prelude_resources::{EdgionConfigData, Gateway};
    use edgion_resources::resources::common::{ParentReference, SecretObjectReference};
    use edgion_resources::resources::edgion_tls::{EdgionTls, EdgionTlsSpec};
    use edgion_resources::resources::gateway::{
        AllowedRoutes, FrontendCACertificateRef, FrontendCAResolutionError, FrontendCASourceIdentity,
        FrontendCASourceKind, FrontendMatcherEligibility, FrontendTLSValidation, FrontendValidationMode,
        GatewayBackendTLS, GatewayFrontendTLS, GatewayFrontendTLSDefault, GatewayFrontendTLSPerPort, GatewaySpec,
        GatewaySpecTLS, GatewayTLSConfig, GatewayTlsMode, Listener as GatewayListener, ResolvedFrontendCAReference,
        ResolvedServingCertificateReference, RouteGroupKind, RouteNamespaces, ServingCertificateSourceIdentity,
    };
    use edgion_resources::resources::proxy_protocol::{
        CanonicalProxyProtocolTrustPolicy, ProxyProtocolConfigErrorCode, ProxyProtocolPolicySourceKind,
        ResolvedInboundProxyProtocol,
    };
    use edgion_resources::ResourceKind;
    use k8s_openapi::api::core::v1::{ConfigMap, Secret};
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
                resolved_attachment_proof: None,
            },
            status: None,
        }
    }

    #[test]
    fn supported_kinds_intersect_explicit_kinds_with_listener_protocol() {
        let listener = GatewayListener {
            name: "http".to_string(),
            hostname: None,
            port: 80,
            protocol: "HTTP".to_string(),
            tls: None,
            allowed_routes: Some(AllowedRoutes {
                namespaces: None,
                kinds: Some(vec![
                    RouteGroupKind {
                        group: None,
                        kind: "HTTPRoute".to_string(),
                    },
                    RouteGroupKind {
                        group: None,
                        kind: "TLSRoute".to_string(),
                    },
                ]),
            }),
        };

        let (supported, errors) = compute_supported_kinds(&listener);
        assert_eq!(supported.len(), 1);
        assert_eq!(supported[0].kind, "HTTPRoute");
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            super::GatewayResolvedRefsError::InvalidRouteKind { kind, protocol, .. }
                if kind == "TLSRoute" && protocol == "HTTP"
        ));
    }

    fn proxy_protocol_test_context(runtime: Arc<ControllerRuntime>) -> HandlerContext {
        HandlerContext::new(None, None, Default::default(), 5, 20, runtime)
    }

    #[tokio::test]
    async fn conflict_status_messages_converge_across_repeated_reconciliation() {
        use crate::conf_mgr::sync_runtime::resource_processor::status_diff::status_semantically_changed;

        for with_proxy_conflict in [false, true] {
            let runtime = Arc::new(ControllerRuntime::default());
            let ctx = proxy_protocol_test_context(runtime);
            let handler = GatewayHandler::new(None, None);
            if with_proxy_conflict {
                let peer = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
                assert!(matches!(handler.parse(peer, &ctx).await, ProcessResult::Continue(_)));
            }
            // Distinct protocols keep each (port, protocol, hostname) tuple
            // unique, as required by Gateway API admission. The controller's
            // socket-level conflict detector still reports this combination.
            let mut gateway: Gateway = serde_json::from_value(serde_json::json!({
                "metadata": {"namespace": "default", "name": "conflicts", "generation": 1},
                "spec": {"gatewayClassName": "test", "listeners": [
                    {"name": "z-http", "port": 80, "protocol": "HTTP"},
                    {"name": "a-tcp", "port": 80, "protocol": "TCP"},
                    {"name": "m-tls", "port": 80, "protocol": "TLS", "tls": {"mode": "Passthrough"}}
                ]}
            }))
            .unwrap();
            let mut previous = None::<String>;
            for round in 0..12 {
                gateway.spec.listeners.as_mut().unwrap().rotate_left(1);
                let errors = handler.validate(&gateway, &ctx);
                assert!(errors.is_empty(), "{errors:?}");
                let ProcessResult::Continue(mut computed) = handler.parse(gateway, &ctx).await else {
                    panic!("conflicting Gateway must remain observable");
                };
                handler.update_status(&mut computed, &ctx, &errors);
                let status = computed.status.as_ref().unwrap();
                let aggregate = status
                    .conditions
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|condition| condition.type_ == "ListenersNotValid")
                    .unwrap();
                assert_eq!(
                    aggregate.reason,
                    if with_proxy_conflict {
                        "ProxyProtocolPolicyConflict"
                    } else {
                        "ListenerConflict"
                    }
                );
                let message = &aggregate.message;
                assert!(message.ends_with("[\"a-tcp\", \"m-tls\", \"z-http\"]"), "{message}");
                for listener in status.listeners.as_ref().unwrap() {
                    let others = ["a-tcp", "m-tls", "z-http"]
                        .into_iter()
                        .filter(|name| *name != listener.name)
                        .collect::<Vec<_>>()
                        .join(", ");
                    let conflict = listener
                        .conditions
                        .iter()
                        .find(|condition| condition.type_ == "Conflicted")
                        .unwrap();
                    assert!(
                        conflict.message.contains(&format!("conflicts with: {others}")),
                        "{}: {}",
                        listener.name,
                        conflict.message
                    );
                }
                let current = serde_json::to_string(status).unwrap();
                if let Some(previous) = &previous {
                    assert!(
                        !status_semantically_changed("Gateway", previous, &current, Some("default")),
                        "round {round}, proxy={with_proxy_conflict}"
                    );
                }
                previous = Some(current);
                gateway = computed;
            }
        }
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
                resolved_certificate_refs: None,
                frontend_validation: None,
                resolved_frontend_ca_refs: None,
                frontend_matcher_eligibility: None,
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
            .map(|(ns, name)| FrontendCACertificateRef {
                group: String::new(),
                kind: "Secret".to_string(),
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
                resolved_certificate_refs: None,
                frontend_validation: Some(FrontendTLSValidation {
                    ca_certificate_refs: ca_refs,
                    mode: None,
                }),
                resolved_frontend_ca_refs: None,
                frontend_matcher_eligibility: None,
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
        assert!(!has_all_ca_refs_invalid(&listener, "gw-ns"));
    }

    #[test]
    fn has_all_ca_refs_invalid_uses_each_declared_index_including_duplicates() {
        let mut listener = listener_with_ca_refs(vec![("ns-a", "same"), ("ns-a", "same")]);
        listener.tls.as_mut().unwrap().resolved_frontend_ca_refs = Some(vec![
            ResolvedFrontendCAReference {
                index: 0,
                source: FrontendCASourceIdentity {
                    kind: FrontendCASourceKind::Secret,
                    namespace: "ns-a".to_string(),
                    name: "same".to_string(),
                },
                ca_certificate: None,
                error: Some(FrontendCAResolutionError::NotFound),
            },
            ResolvedFrontendCAReference {
                index: 1,
                source: FrontendCASourceIdentity {
                    kind: FrontendCASourceKind::Secret,
                    namespace: "ns-a".to_string(),
                    name: "same".to_string(),
                },
                ca_certificate: None,
                error: Some(FrontendCAResolutionError::NotFound),
            },
        ]);
        assert!(has_all_ca_refs_invalid(&listener, "gw-ns"));
    }

    #[test]
    fn has_all_ca_refs_invalid_is_false_when_one_index_is_valid() {
        let mut listener = listener_with_ca_refs(vec![("ns-a", "ref-a"), ("ns-b", "ref-b")]);
        listener.tls.as_mut().unwrap().resolved_frontend_ca_refs = Some(vec![
            ResolvedFrontendCAReference {
                index: 0,
                source: FrontendCASourceIdentity {
                    kind: FrontendCASourceKind::Secret,
                    namespace: "ns-a".to_string(),
                    name: "ref-a".to_string(),
                },
                ca_certificate: None,
                error: Some(FrontendCAResolutionError::NotFound),
            },
            ResolvedFrontendCAReference {
                index: 1,
                source: FrontendCASourceIdentity {
                    kind: FrontendCASourceKind::Secret,
                    namespace: "ns-b".to_string(),
                    name: "ref-b".to_string(),
                },
                ca_certificate: Some(ByteString(b"certificate".to_vec())),
                error: None,
            },
        ]);
        assert!(!has_all_ca_refs_invalid(&listener, "gw-ns"));
    }

    #[test]
    fn frontend_ca_status_distinguishes_partial_from_fully_invalid_pem_sets() {
        let runtime = ControllerRuntime::default();
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["client.example.com".to_string()]).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let valid_pem = params.self_signed(&key).unwrap().pem();
        let make_secret = |name: &str, pem: &[u8]| Secret {
            metadata: kube::api::ObjectMeta {
                namespace: Some("gw-ns".to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            data: Some(BTreeMap::from([("ca.crt".to_string(), ByteString(pem.to_vec()))])),
            ..Default::default()
        };
        runtime.secret_store.update(
            HashMap::from([
                ("gw-ns/valid".to_string(), make_secret("valid", valid_pem.as_bytes())),
                (
                    "gw-ns/garbage".to_string(),
                    make_secret("garbage", b"not a certificate"),
                ),
            ]),
            &HashSet::new(),
        );
        let mut gateway = make_test_gateway("gw-ns");
        gateway.metadata.name = Some("gateway".to_string());

        let mut partial = listener_with_ca_refs(vec![("gw-ns", "valid"), ("gw-ns", "garbage")]);
        partial.tls.as_mut().unwrap().resolved_frontend_ca_refs = Some(vec![
            ResolvedFrontendCAReference {
                index: 0,
                source: FrontendCASourceIdentity {
                    kind: FrontendCASourceKind::Secret,
                    namespace: "gw-ns".to_string(),
                    name: "valid".to_string(),
                },
                ca_certificate: Some(ByteString(valid_pem.as_bytes().to_vec())),
                error: None,
            },
            ResolvedFrontendCAReference {
                index: 1,
                source: FrontendCASourceIdentity {
                    kind: FrontendCASourceKind::Secret,
                    namespace: "gw-ns".to_string(),
                    name: "garbage".to_string(),
                },
                ca_certificate: None,
                error: Some(FrontendCAResolutionError::InvalidCertificate),
            },
        ]);
        let partial_errors = validate_listener_resolved_refs(&gateway, &partial, &runtime);
        assert!(!partial_errors.is_empty());
        assert!(!has_all_ca_refs_invalid(&partial, "gw-ns"));

        let mut full = listener_with_ca_refs(vec![("gw-ns", "garbage")]);
        full.tls.as_mut().unwrap().resolved_frontend_ca_refs = Some(vec![ResolvedFrontendCAReference {
            index: 0,
            source: FrontendCASourceIdentity {
                kind: FrontendCASourceKind::Secret,
                namespace: "gw-ns".to_string(),
                name: "garbage".to_string(),
            },
            ca_certificate: None,
            error: Some(FrontendCAResolutionError::InvalidCertificate),
        }]);
        let _full_errors = validate_listener_resolved_refs(&gateway, &full, &runtime);
        assert!(has_all_ca_refs_invalid(&full, "gw-ns"));
    }

    #[test]
    fn insecure_frontend_missing_ca_does_not_create_status_error() {
        let runtime = ControllerRuntime::default();
        let gateway = make_test_gateway("gw-ns");
        let mut listener = listener_with_ca_refs(vec![("gw-ns", "missing")]);
        let tls = listener.tls.as_mut().unwrap();
        tls.frontend_validation.as_mut().unwrap().mode = Some(FrontendValidationMode::AllowInsecureFallback);
        tls.resolved_frontend_ca_refs = Some(vec![ResolvedFrontendCAReference {
            index: 0,
            source: FrontendCASourceIdentity {
                kind: FrontendCASourceKind::Secret,
                namespace: "gw-ns".to_string(),
                name: "missing".to_string(),
            },
            ca_certificate: None,
            error: Some(FrontendCAResolutionError::NotFound),
        }]);
        assert!(validate_listener_resolved_refs(&gateway, &listener, &runtime).is_empty());
        assert!(!has_all_ca_refs_invalid(&listener, "gw-ns"));
    }

    #[test]
    fn frontend_without_serving_refs_requires_exact_edgion_tls_opt_in() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
        let handler = GatewayHandler::new(None, None);

        for (provider, expected) in [
            (None, FrontendMatcherEligibility::StrictReject),
            (Some("edgiontls"), FrontendMatcherEligibility::StrictReject),
            (
                Some("EdgionTls"),
                FrontendMatcherEligibility::RequireExactEdgionTlsCertificate,
            ),
        ] {
            let mut gateway = make_test_gateway("gw-ns");
            gateway.metadata.name = Some("gateway".to_string());
            let mut listener = listener_with_ca_refs(vec![("gw-ns", "missing")]);
            let tls = listener.tls.as_mut().unwrap();
            tls.frontend_validation.as_mut().unwrap().mode = Some(FrontendValidationMode::AllowInsecureFallback);
            tls.certificate_refs = None;
            tls.options = provider.map(|value| serde_json::json!({"edgion.io/cert-provider": value}));
            tls.resolved_frontend_ca_refs = Some(vec![ResolvedFrontendCAReference {
                index: 0,
                source: FrontendCASourceIdentity {
                    kind: FrontendCASourceKind::Secret,
                    namespace: "gw-ns".to_string(),
                    name: "missing".to_string(),
                },
                ca_certificate: None,
                error: Some(FrontendCAResolutionError::NotFound),
            }]);
            gateway.spec.listeners = Some(vec![listener]);

            handler.update_status(&mut gateway, &ctx, &[]);
            assert_eq!(
                gateway.spec.listeners.as_ref().unwrap()[0]
                    .tls
                    .as_ref()
                    .unwrap()
                    .frontend_matcher_eligibility,
                Some(expected),
                "provider={provider:?}"
            );
        }
    }

    #[tokio::test]
    async fn real_serving_certificate_and_private_key_are_resolved_and_validated() {
        let runtime = Arc::new(ControllerRuntime::default());
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["server.example.com".to_string()]).unwrap();
        let cert = params.self_signed(&key).unwrap().pem();
        let secret = Secret {
            metadata: kube::api::ObjectMeta {
                namespace: Some("gw-ns".to_string()),
                name: Some("server".to_string()),
                ..Default::default()
            },
            data: Some(BTreeMap::from([
                ("tls.crt".to_string(), ByteString(cert.into_bytes())),
                ("tls.key".to_string(), ByteString(key.serialize_pem().into_bytes())),
            ])),
            ..Default::default()
        };
        runtime
            .secret_store
            .update(HashMap::from([("gw-ns/server".to_string(), secret)]), &HashSet::new());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
        let mut gateway = make_test_gateway("gw-ns");
        gateway.metadata.name = Some("gateway".to_string());
        gateway.spec.listeners = Some(vec![GatewayListener {
            name: "https".to_string(),
            hostname: Some("server.example.com".to_string()),
            port: 443,
            protocol: "HTTPS".to_string(),
            tls: Some(GatewayTLSConfig {
                mode: Some(GatewayTlsMode::Terminate),
                certificate_refs: Some(vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    namespace: None,
                    name: "server".to_string(),
                }]),
                options: None,
                resolved_certificate_refs: None,
                frontend_validation: None,
                resolved_frontend_ca_refs: None,
                frontend_matcher_eligibility: None,
            }),
            allowed_routes: None,
        }]);

        let handler = GatewayHandler::new(None, None);
        let parsed = match handler.parse(gateway, &ctx).await {
            ProcessResult::Continue(gateway) => gateway,
            ProcessResult::Skip { reason } => panic!("unexpected Skip: {reason}"),
        };
        let listener = &parsed.spec.listeners.as_ref().unwrap()[0];
        assert!(validate_listener_resolved_refs(&parsed, listener, &runtime).is_empty());
        let outcome = &listener
            .tls
            .as_ref()
            .unwrap()
            .resolved_certificate_refs
            .as_ref()
            .unwrap()[0];
        let payload = outcome.secret.as_ref().expect("real server Secret must resolve");
        assert_eq!(payload.metadata.name.as_deref(), Some("server"));
        assert_eq!(payload.metadata.namespace.as_deref(), Some("gw-ns"));
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

    fn frontend_validation(kind: &str, name: &str, mode: Option<FrontendValidationMode>) -> FrontendTLSValidation {
        FrontendTLSValidation {
            ca_certificate_refs: vec![FrontendCACertificateRef {
                group: String::new(),
                kind: kind.to_string(),
                name: name.to_string(),
                namespace: None,
            }],
            mode,
        }
    }

    #[tokio::test]
    async fn standard_frontend_projection_is_https_only_authoritative_and_resolves_all_kinds() {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.update(
            HashMap::from([(
                "gw-ns/default-ca".to_string(),
                Secret {
                    metadata: kube::api::ObjectMeta {
                        namespace: Some("gw-ns".to_string()),
                        name: Some("default-ca".to_string()),
                        ..Default::default()
                    },
                    data: Some(BTreeMap::from([(
                        "ca.crt".to_string(),
                        ByteString(b"secret-ca".to_vec()),
                    )])),
                    ..Default::default()
                },
            )]),
            &HashSet::new(),
        );
        runtime.configmap_store.update(
            HashMap::from([(
                "gw-ns/port-ca".to_string(),
                ConfigMap {
                    metadata: kube::api::ObjectMeta {
                        namespace: Some("gw-ns".to_string()),
                        name: Some("port-ca".to_string()),
                        ..Default::default()
                    },
                    data: Some(BTreeMap::from([("ca.crt".to_string(), "configmap-ca".to_string())])),
                    ..Default::default()
                },
            )]),
            &HashSet::new(),
        );
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
        let raw = frontend_validation("Secret", "raw-ca", None);
        let mut gateway = make_test_gateway("gw-ns");
        gateway.metadata.name = Some("gateway".to_string());
        gateway.spec.tls = Some(GatewaySpecTLS {
            backend: None,
            frontend: Some(GatewayFrontendTLS {
                default: GatewayFrontendTLSDefault {
                    validation: Some(frontend_validation("Secret", "default-ca", None)),
                },
                per_port: Some(vec![GatewayFrontendTLSPerPort {
                    port: 8443,
                    tls: GatewayFrontendTLSDefault {
                        validation: Some(frontend_validation(
                            "ConfigMap",
                            "port-ca",
                            Some(FrontendValidationMode::AllowInsecureFallback),
                        )),
                    },
                }]),
            }),
        });
        gateway.spec.listeners = Some(vec![
            GatewayListener {
                name: "https-default".to_string(),
                hostname: None,
                port: 443,
                protocol: "HTTPS".to_string(),
                tls: Some(GatewayTLSConfig {
                    mode: Some(GatewayTlsMode::Terminate),
                    certificate_refs: None,
                    options: None,
                    resolved_certificate_refs: None,
                    frontend_validation: Some(raw.clone()),
                    resolved_frontend_ca_refs: None,
                    frontend_matcher_eligibility: None,
                }),
                allowed_routes: None,
            },
            GatewayListener {
                name: "https-port".to_string(),
                hostname: None,
                port: 8443,
                protocol: "HTTPS".to_string(),
                tls: None,
                allowed_routes: None,
            },
            GatewayListener {
                name: "tls-ignored".to_string(),
                hostname: None,
                port: 9443,
                protocol: "TLS".to_string(),
                tls: Some(GatewayTLSConfig {
                    mode: Some(GatewayTlsMode::Terminate),
                    certificate_refs: None,
                    options: None,
                    resolved_certificate_refs: None,
                    frontend_validation: Some(raw),
                    resolved_frontend_ca_refs: None,
                    frontend_matcher_eligibility: None,
                }),
                allowed_routes: None,
            },
        ]);

        let ProcessResult::Continue(parsed) = GatewayHandler::new(None, None).parse(gateway, &ctx).await else {
            panic!("parse skipped");
        };
        let listeners = parsed.spec.listeners.unwrap();
        let default_tls = listeners[0].tls.as_ref().unwrap();
        assert_eq!(
            default_tls.frontend_validation.as_ref().unwrap().ca_certificate_refs[0].name,
            "default-ca"
        );
        assert_eq!(default_tls.resolved_frontend_ca_refs.as_ref().unwrap().len(), 1);
        let port_tls = listeners[1].tls.as_ref().unwrap();
        assert_eq!(
            port_tls.frontend_validation.as_ref().unwrap().ca_certificate_refs[0].name,
            "port-ca"
        );
        assert_eq!(port_tls.resolved_frontend_ca_refs.as_ref().unwrap().len(), 1);
        assert!(listeners[2].tls.as_ref().unwrap().frontend_validation.is_none());
        assert!(listeners[2].tls.as_ref().unwrap().resolved_frontend_ca_refs.is_none());
        assert_eq!(
            runtime
                .frontend_ca_ref_manager
                .get_refs(&FrontendCaSourceKey::new(FrontendCaSourceKind::Secret, "gw-ns", "default-ca").index_key())
                .len(),
            1
        );
        assert_eq!(
            runtime
                .frontend_ca_ref_manager
                .get_refs(&FrontendCaSourceKey::new(FrontendCaSourceKind::ConfigMap, "gw-ns", "port-ca").index_key())
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn per_port_empty_tls_explicitly_disables_default_without_fallback() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
        let mut gateway = make_test_gateway("gw-ns");
        gateway.spec.tls = Some(GatewaySpecTLS {
            backend: None,
            frontend: Some(GatewayFrontendTLS {
                default: GatewayFrontendTLSDefault {
                    validation: Some(frontend_validation("Secret", "default-ca", None)),
                },
                per_port: Some(vec![GatewayFrontendTLSPerPort {
                    port: 443,
                    tls: GatewayFrontendTLSDefault { validation: None },
                }]),
            }),
        });
        gateway.spec.listeners = Some(vec![GatewayListener {
            name: "https".to_string(),
            hostname: None,
            port: 443,
            protocol: "HTTPS".to_string(),
            tls: Some(GatewayTLSConfig {
                mode: Some(GatewayTlsMode::Terminate),
                certificate_refs: None,
                options: None,
                resolved_certificate_refs: None,
                frontend_validation: Some(frontend_validation("Secret", "forged", None)),
                resolved_frontend_ca_refs: None,
                frontend_matcher_eligibility: None,
            }),
            allowed_routes: None,
        }]);

        let ProcessResult::Continue(parsed) = GatewayHandler::new(None, None).parse(gateway, &ctx).await else {
            panic!("gateway parse unexpectedly stopped")
        };
        let tls = parsed.spec.listeners.unwrap()[0].tls.as_ref().unwrap().clone();
        assert!(tls.frontend_validation.is_none());
        assert!(tls.resolved_frontend_ca_refs.is_none());
    }

    #[test]
    fn frontend_runtime_validation_rejects_duplicate_range_and_nonempty_group() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
        let mut gateway = make_test_gateway("gw-ns");
        let invalid = FrontendTLSValidation {
            ca_certificate_refs: vec![FrontendCACertificateRef {
                group: "core".to_string(),
                kind: "Secret".to_string(),
                name: "ca".to_string(),
                namespace: None,
            }],
            mode: None,
        };
        gateway.spec.tls = Some(GatewaySpecTLS {
            backend: None,
            frontend: Some(GatewayFrontendTLS {
                default: GatewayFrontendTLSDefault {
                    validation: Some(invalid),
                },
                per_port: Some(vec![
                    GatewayFrontendTLSPerPort {
                        port: 0,
                        tls: GatewayFrontendTLSDefault { validation: None },
                    },
                    GatewayFrontendTLSPerPort {
                        port: 0,
                        tls: GatewayFrontendTLSDefault { validation: None },
                    },
                ]),
            }),
        });
        let errors = GatewayHandler::new(None, None).validate(&gateway, &ctx);
        assert!(errors.iter().any(|error| error.contains("outside 1..=65535")));
        assert!(errors.iter().any(|error| error.contains("duplicate port")));
        assert!(errors.iter().any(|error| error.contains("exact empty core group")));
    }

    #[test]
    fn frontend_namespace_validation_accepts_internal_consecutive_hyphens() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
        let mut gateway = make_test_gateway("gw-ns");
        gateway.spec.tls = Some(GatewaySpecTLS {
            backend: None,
            frontend: Some(GatewayFrontendTLS {
                default: GatewayFrontendTLSDefault {
                    validation: Some(FrontendTLSValidation {
                        ca_certificate_refs: vec![FrontendCACertificateRef {
                            group: String::new(),
                            kind: "Secret".to_string(),
                            name: "ca".to_string(),
                            namespace: Some("team--trust".to_string()),
                        }],
                        mode: None,
                    }),
                },
                per_port: None,
            }),
        });
        assert!(GatewayHandler::new(None, None).validate(&gateway, &ctx).is_empty());
    }

    #[tokio::test]
    async fn oversized_raw_listener_vector_is_revoked_before_traversal() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
        let handler = GatewayHandler::new(None, None);
        let mut gateway = make_test_gateway("gw-ns");
        gateway.metadata.name = Some("oversized".to_string());
        gateway.spec.listeners = Some(
            (0..=edgion_resources::resources::gateway::MAX_GATEWAY_LISTENERS)
                .map(|index| GatewayListener {
                    name: format!("listener-{index}"),
                    hostname: None,
                    port: 10_000 + index as i32,
                    protocol: "HTTPS".to_string(),
                    tls: None,
                    allowed_routes: None,
                })
                .collect(),
        );
        let errors = handler.validate(&gateway, &ctx);
        assert_eq!(errors.len(), 1);
        let parsed = match handler.parse(gateway, &ctx).await {
            ProcessResult::Continue(parsed) => parsed,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert!(
            parsed.spec.listeners.is_none(),
            "invalid update must revoke old listeners"
        );
    }

    #[test]
    fn attachment_proof_excludes_private_key_and_metadata_noise() {
        let mut gateway = make_test_gateway("gw-ns");
        gateway.metadata.name = Some("gateway".to_string());
        gateway.metadata.uid = Some("uid-1".to_string());
        gateway.metadata.generation = Some(7);
        gateway.metadata.resource_version = Some("11".to_string());
        gateway.spec.listeners = Some(vec![GatewayListener {
            name: "https".to_string(),
            hostname: Some("example.com".to_string()),
            port: 443,
            protocol: "HTTPS".to_string(),
            tls: Some(GatewayTLSConfig {
                mode: Some(GatewayTlsMode::Terminate),
                certificate_refs: Some(vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    namespace: None,
                    name: "server".to_string(),
                }]),
                options: None,
                resolved_certificate_refs: Some(vec![ResolvedServingCertificateReference {
                    index: 0,
                    source: ServingCertificateSourceIdentity {
                        group: String::new(),
                        kind: "Secret".to_string(),
                        namespace: "gw-ns".to_string(),
                        name: "server".to_string(),
                    },
                    secret: Some(Secret {
                        data: Some(BTreeMap::from([
                            ("tls.crt".to_string(), ByteString(b"public-cert".to_vec())),
                            ("tls.key".to_string(), ByteString(b"private-key-one".to_vec())),
                        ])),
                        ..Default::default()
                    }),
                    error: None,
                }]),
                frontend_validation: None,
                resolved_frontend_ca_refs: None,
                frontend_matcher_eligibility: None,
            }),
            allowed_routes: None,
        }]);

        let first = build_gateway_attachment_proof(&gateway, "boot-a", 10);
        gateway.spec.tls = Some(GatewaySpecTLS {
            backend: Some(GatewayBackendTLS {
                client_certificate_ref: Some(SecretObjectReference {
                    group: None,
                    kind: None,
                    namespace: None,
                    name: "backend-client".to_string(),
                }),
                resolved_client_certificate: Some(Secret {
                    data: Some(BTreeMap::from([(
                        "tls.key".to_string(),
                        ByteString(b"unrelated-backend-private-key".to_vec()),
                    )])),
                    ..Default::default()
                }),
            }),
            frontend: None,
        });
        let backend_tls_changed = build_gateway_attachment_proof(&gateway, "boot-a", 10);
        assert_eq!(
            first.attachment_digest, backend_tls_changed.attachment_digest,
            "unrelated Gateway backend TLS must not churn attachment proof"
        );
        let restarted = build_gateway_attachment_proof(&gateway, "boot-b", 10);
        assert_eq!(
            first.instance, restarted.instance,
            "Kubernetes UID remains authoritative"
        );
        assert_ne!(
            first.epoch, restarted.epoch,
            "controller restarts must create a new proof epoch"
        );
        assert_eq!(first.attachment_digest, restarted.attachment_digest);

        let mut local_gateway = gateway.clone();
        local_gateway.metadata.uid = None;
        let local_first = build_gateway_attachment_proof(&local_gateway, "boot-a", 10);
        let local_restarted = build_gateway_attachment_proof(&local_gateway, "boot-b", 10);
        assert_ne!(local_first.instance, local_restarted.instance);
        assert_ne!(local_first.epoch, local_restarted.epoch);
        assert_eq!(local_first.attachment_digest, local_restarted.attachment_digest);
        gateway.metadata.resource_version = Some("99".to_string());
        gateway.spec.listeners.as_mut().unwrap()[0]
            .tls
            .as_mut()
            .unwrap()
            .resolved_certificate_refs
            .as_mut()
            .unwrap()[0]
            .secret
            .as_mut()
            .unwrap()
            .data
            .as_mut()
            .unwrap()
            .insert("tls.key".to_string(), ByteString(b"private-key-two".to_vec()));
        let private_key_changed = build_gateway_attachment_proof(&gateway, "boot-a", 10);
        assert_eq!(first.attachment_digest, private_key_changed.attachment_digest);
        assert_eq!(first.instance, "uid-1");
        assert_eq!(first.generation, 7);

        gateway.spec.listeners.as_mut().unwrap()[0]
            .tls
            .as_mut()
            .unwrap()
            .resolved_certificate_refs
            .as_mut()
            .unwrap()[0]
            .secret
            .as_mut()
            .unwrap()
            .data
            .as_mut()
            .unwrap()
            .insert("tls.crt".to_string(), ByteString(b"new-public-cert".to_vec()));
        let public_cert_changed = build_gateway_attachment_proof(&gateway, "boot-a", 10);
        assert_ne!(first.attachment_digest, public_cert_changed.attachment_digest);

        gateway.spec.listeners.as_mut().unwrap()[0]
            .tls
            .as_mut()
            .unwrap()
            .frontend_matcher_eligibility = Some(FrontendMatcherEligibility::StrictReject);
        let final_eligibility_changed = build_gateway_attachment_proof(&gateway, "boot-a", 10);
        assert_ne!(
            public_cert_changed.attachment_digest, final_eligibility_changed.attachment_digest,
            "final derived admission/transport outcome must be proof-bound"
        );

        gateway.spec.listeners.as_mut().unwrap()[0].allowed_routes = Some(AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("Same".to_string()),
                selector: None,
            }),
            kinds: None,
        });
        let authorization_changed = build_gateway_attachment_proof(&gateway, "boot-a", 10);
        assert_ne!(
            final_eligibility_changed.attachment_digest, authorization_changed.attachment_digest,
            "allowedRoutes attachment authorization must be proof-bound"
        );

        gateway.spec.resolved_attachment_proof = Some(authorization_changed.clone());
        assert_eq!(
            edgion_resources::resources::gateway::gateway_attachment_proof(&gateway),
            authorization_changed,
            "the downstream projection must expose the exact finalized proof"
        );
    }

    #[test]
    fn delete_recreate_of_same_gateway_spec_receives_new_authoritative_epoch() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
        let handler = GatewayHandler::new(None, None);
        let mut gateway = make_test_gateway("gw-ns");
        gateway.metadata.name = Some("gateway".to_string());
        gateway.metadata.uid = Some("uid-1".to_string());
        gateway.metadata.generation = Some(1);

        handler.finalize_transport(&mut gateway, &ctx);
        let first = gateway.spec.resolved_attachment_proof.take().unwrap();
        handler.finalize_transport(&mut gateway, &ctx);
        let recreated = gateway.spec.resolved_attachment_proof.as_ref().unwrap();

        assert_eq!(first.instance, recreated.instance);
        assert_eq!(first.generation, recreated.generation);
        assert_eq!(first.attachment_digest, recreated.attachment_digest);
        assert_ne!(first.epoch, recreated.epoch);
    }

    #[tokio::test]
    async fn attachment_dependency_barrier_covers_publish_unpublish_and_same_key_recreate() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::edgion_tls::EdgionTlsHandler;

        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
        let gateway_handler = Arc::new(GatewayHandler::new(None, None));
        let gateway_processor = Arc::new(ResourceProcessor::<Gateway>::new(
            ResourceKind::Gateway.as_str(),
            16,
            gateway_handler.clone(),
            runtime.clone(),
        ));
        let tls_handler = Arc::new(EdgionTlsHandler::default());
        let tls_processor = Arc::new(ResourceProcessor::<EdgionTls>::new(
            ResourceKind::EdgionTls.as_str(),
            16,
            tls_handler.clone(),
            runtime.clone(),
        ));
        runtime.processor_registry.register(gateway_processor.clone());
        runtime.processor_registry.register(tls_processor.clone());

        let mut gateway = make_test_gateway("test-ns");
        gateway.metadata.name = Some("gateway".to_string());
        gateway.metadata.uid = Some("uid-1".to_string());
        gateway.metadata.generation = Some(1);
        gateway.spec.listeners = Some(vec![GatewayListener {
            name: "https".to_string(),
            hostname: Some("example.com".to_string()),
            port: 443,
            protocol: "HTTPS".to_string(),
            tls: None,
            allowed_routes: None,
        }]);
        gateway_handler.finalize_transport(&mut gateway, &ctx);
        let first_proof = gateway.spec.resolved_attachment_proof.clone().unwrap();
        gateway_processor.save(gateway.clone(), None);

        let tls = EdgionTls {
            metadata: kube::core::ObjectMeta {
                name: Some("tls".to_string()),
                namespace: Some("test-ns".to_string()),
                ..Default::default()
            },
            spec: EdgionTlsSpec {
                parent_refs: Some(vec![ParentReference {
                    group: None,
                    kind: None,
                    namespace: None,
                    name: "gateway".to_string(),
                    section_name: Some("https".to_string()),
                    port: None,
                }]),
                hosts: vec!["example.com".to_string()],
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    namespace: None,
                    name: "missing".to_string(),
                },
                client_auth: None,
                min_tls_version: None,
                ciphers: None,
                secret: None,
                resolved_listeners: None,
                resolved_listener_attachments: None,
                resolved_listener_attachment_index: Default::default(),
                resolved_log_labels: None,
            },
            status: None,
        };

        // Cache-before-edge: parse first installs the dependency, then reads
        // the already-published authoritative Gateway generation.
        let parsed = match tls_handler.parse(tls.clone(), &ctx).await {
            ProcessResult::Continue(parsed) => parsed,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert_eq!(
            parsed.spec.resolved_listener_attachments.as_ref().unwrap()[0].gateway_proof,
            first_proof
        );

        // Edge-before-hook: the post-publish hook reads the real reverse index
        // and queues the registered consumer only after cache publication.
        gateway_handler.after_publish(&gateway, &ctx).await;
        let queued = tokio::time::timeout(std::time::Duration::from_secs(1), tls_processor.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(queued.key, "test-ns/tls");
        tls_processor.workqueue().done(&queued.key);

        gateway_processor.remove("test-ns/gateway", None);
        gateway_handler.after_unpublish(&gateway, &ctx).await;
        let deleted = tokio::time::timeout(std::time::Duration::from_secs(1), tls_processor.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(deleted.key, "test-ns/tls");
        tls_processor.workqueue().done(&deleted.key);

        gateway_handler.finalize_transport(&mut gateway, &ctx);
        let recreated_proof = gateway.spec.resolved_attachment_proof.clone().unwrap();
        assert_ne!(first_proof.epoch, recreated_proof.epoch);
        gateway_processor.save(gateway.clone(), None);
        gateway_handler.after_publish(&gateway, &ctx).await;
        let recreated = tokio::time::timeout(std::time::Duration::from_secs(1), tls_processor.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        tls_processor.workqueue().done(&recreated.key);

        let reparsed = match tls_handler.parse(tls, &ctx).await {
            ProcessResult::Continue(parsed) => parsed,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert_eq!(
            reparsed.spec.resolved_listener_attachments.as_ref().unwrap()[0].gateway_proof,
            recreated_proof
        );
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
