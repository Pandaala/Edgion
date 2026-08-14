//! Proof-carrying backend identities between route selection and transport.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use crate::ai::{
    AdaptiveTelemetryState, AiCredentialOutlierHandle, EdgionBackendSnapshot, RuntimeA2aBackend, RuntimeAiBackend,
    RuntimeMcpBackend, RuntimeMcpTarget,
};
use crate::backends::policy::backend_tls::BackendTLSPolicySnapshot;
use crate::backends::policy::backend_traffic::{BackendTrafficPolicySnapshot, EffectiveBackendTrafficPolicy};
use crate::routes::http::match_unit::{CompiledBackend, CompiledBackendTarget};
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::BackendTLSPolicy;

/// One immutable, gateway-compiled HTTPRoute backend slot.
#[derive(Clone)]
pub struct LogicalBackendHandle {
    compiled: CompiledBackend,
}

impl LogicalBackendHandle {
    pub(crate) fn new(compiled: CompiledBackend) -> Self {
        Self { compiled }
    }

    #[inline]
    pub fn compiled(&self) -> &CompiledBackend {
        &self.compiled
    }

    #[inline]
    pub fn slot(&self) -> usize {
        self.compiled.slot
    }

    #[inline]
    pub fn identity(&self) -> &edgion_resources::LogicalBackendSlotIdentity {
        &self.compiled.identity
    }

    #[inline]
    pub fn target(&self) -> &CompiledBackendTarget {
        &self.compiled.target
    }

    #[inline]
    pub fn declared_weight(&self) -> i32 {
        self.compiled.schema.weight.unwrap_or(1)
    }

    #[inline]
    pub fn is_authorized(&self) -> bool {
        self.compiled.schema.ref_denied.is_none()
    }

    /// Canonical resource identity used by backend-attached policy stores.
    ///
    /// Slot occurrence deliberately does not participate: two route slots that
    /// refer to the same backend resource share the same attached policy.
    pub fn policy_target_key(&self) -> Option<PolicyTargetKey> {
        if !self.is_authorized() {
            return None;
        }
        let (target_kind, namespace, name) = match self.target() {
            CompiledBackendTarget::Service { namespace, name } => (PolicyTargetKind::Service, namespace, name),
            CompiledBackendTarget::EdgionBackend { namespace, name } => {
                (PolicyTargetKind::EdgionBackend, namespace, name)
            }
            CompiledBackendTarget::Invalid { .. } => return None,
        };
        Some(PolicyTargetKey {
            target_kind,
            namespace: namespace.clone(),
            name: name.clone(),
        })
    }
}

impl fmt::Debug for LogicalBackendHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogicalBackendHandle")
            .field("slot", &self.slot())
            .field("identity", &self.identity())
            .field("target", &self.target())
            .field("declared_weight", &self.declared_weight())
            .field("authorized", &self.is_authorized())
            .finish()
    }
}

/// A Service slot after sticky/Internal/WRR logical selection and TLS-policy capture.
#[derive(Clone, Debug)]
pub struct SelectedServiceBackend {
    logical: LogicalBackendHandle,
}

impl SelectedServiceBackend {
    pub(crate) fn new(logical: LogicalBackendHandle) -> Option<Self> {
        (logical.is_authorized() && matches!(logical.target(), CompiledBackendTarget::Service { .. }))
            .then_some(Self { logical })
    }

    #[inline]
    pub fn logical(&self) -> &LogicalBackendHandle {
        &self.logical
    }
}

/// An AI backend and credential chosen from one request-stable plan generation.
#[derive(Clone)]
pub struct SelectedAiBackend {
    logical: LogicalBackendHandle,
    pub backend: Arc<RuntimeAiBackend>,
    pub effective_policy: Option<Arc<EffectiveBackendTrafficPolicy>>,
    /// Request-stable TLS policy selected for this logical backend.
    pub backend_tls_policy: Option<Arc<BackendTLSPolicy>>,
    pub adaptive: Option<Arc<AdaptiveTelemetryState>>,
    pub credential_outlier: Option<AiCredentialOutlierHandle>,
    pub credential_index: usize,
    pub credential_alias: String,
    pub effective_weight: i32,
}

/// An A2A backend selected from the same immutable EdgionBackend generation.
#[derive(Clone)]
pub struct SelectedA2aBackend {
    logical: LogicalBackendHandle,
    pub backend: Arc<RuntimeA2aBackend>,
    pub effective_policy: Option<Arc<EffectiveBackendTrafficPolicy>>,
    pub backend_tls_policy: Option<Arc<BackendTLSPolicy>>,
}

impl SelectedA2aBackend {
    pub(crate) fn new(
        logical: LogicalBackendHandle,
        backend: Arc<RuntimeA2aBackend>,
        effective_policy: Option<Arc<EffectiveBackendTrafficPolicy>>,
        backend_tls_policy: Option<Arc<BackendTLSPolicy>>,
    ) -> Option<Self> {
        (logical.is_authorized() && matches!(logical.target(), CompiledBackendTarget::EdgionBackend { .. })).then_some(
            Self {
                logical,
                backend,
                effective_policy,
                backend_tls_policy,
            },
        )
    }

    pub fn logical(&self) -> &LogicalBackendHandle {
        &self.logical
    }
}

/// WP10 handoff proof for an already-selected MCP logical backend.
///
/// WP9 does not place this type in [`SelectedLogicalBackend`] or invoke it from
/// ordinary HTTP proxy dispatch. Construction resolves the runtime from one
/// captured backend snapshot and fails closed on stale TLS capability markers.
#[derive(Clone)]
pub struct SelectedMcpBackend {
    logical: LogicalBackendHandle,
    pub backend: Arc<RuntimeMcpBackend>,
    pub backend_snapshot_generation: u64,
    pub effective_policy: Option<Arc<EffectiveBackendTrafficPolicy>>,
    pub backend_tls_policy: Option<Arc<BackendTLSPolicy>>,
}

impl SelectedMcpBackend {
    pub fn from_snapshot(
        logical: LogicalBackendHandle,
        backend_snapshot: &EdgionBackendSnapshot,
        traffic_policy_snapshot: &BackendTrafficPolicySnapshot,
        tls_policy_snapshot: &BackendTLSPolicySnapshot,
    ) -> Option<Self> {
        if !logical.is_authorized() {
            return None;
        }
        let CompiledBackendTarget::EdgionBackend { namespace, name } = logical.target() else {
            return None;
        };
        let key = format!("{namespace}/{name}");
        let backend = backend_snapshot.get_mcp_by_key(&key)?;
        let policy_target = logical.policy_target_key()?;
        let expected_policy_class = match &backend.target {
            RuntimeMcpTarget::Service { .. } => {
                edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::McpService
            }
            RuntimeMcpTarget::External(_) => {
                edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::McpExternal
            }
        };
        let effective_policy = traffic_policy_snapshot.get_effective(&policy_target);
        if effective_policy
            .as_ref()
            .is_some_and(|policy| policy.resolved_target_class != expected_policy_class)
        {
            return None;
        }
        let backend_tls_policy = tls_policy_snapshot.get_policy(&policy_target, None);
        if backend_tls_policy.is_none() && tls_policy_snapshot.has_direct_candidate(&policy_target) {
            return None;
        }
        let expected_tls_class = match &backend.target {
            RuntimeMcpTarget::Service { .. } => {
                Some(edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::McpService)
            }
            RuntimeMcpTarget::External(endpoint) if endpoint.use_tls => {
                Some(edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::McpExternal)
            }
            RuntimeMcpTarget::External(_) => None,
        };
        match (&backend_tls_policy, expected_tls_class) {
            (Some(policy), Some(expected)) if policy.spec.resolved_target_class == Some(expected) => {}
            (None, _) => {}
            _ => return None,
        }
        Some(Self {
            logical,
            backend,
            backend_snapshot_generation: backend_snapshot.generation(),
            effective_policy,
            backend_tls_policy,
        })
    }

    pub fn logical(&self) -> &LogicalBackendHandle {
        &self.logical
    }
}

impl fmt::Debug for SelectedMcpBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SelectedMcpBackend")
            .field("logical", &self.logical)
            .field("backend_snapshot_generation", &self.backend_snapshot_generation)
            .field("effective_policy", &self.effective_policy.is_some())
            .field("backend_tls_policy", &self.backend_tls_policy.is_some())
            .finish()
    }
}

impl fmt::Debug for SelectedA2aBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SelectedA2aBackend")
            .field("logical", &self.logical)
            .field("effective_policy", &self.effective_policy.is_some())
            .field("backend_tls_policy", &self.backend_tls_policy.is_some())
            .finish()
    }
}

impl SelectedAiBackend {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        logical: LogicalBackendHandle,
        backend: Arc<RuntimeAiBackend>,
        effective_policy: Option<Arc<EffectiveBackendTrafficPolicy>>,
        backend_tls_policy: Option<Arc<BackendTLSPolicy>>,
        adaptive: Option<Arc<AdaptiveTelemetryState>>,
        credential_outlier: Option<AiCredentialOutlierHandle>,
        credential_index: usize,
        credential_alias: String,
        effective_weight: i32,
    ) -> Option<Self> {
        (logical.is_authorized() && matches!(logical.target(), CompiledBackendTarget::EdgionBackend { .. })).then_some(
            Self {
                logical,
                backend,
                effective_policy,
                backend_tls_policy,
                adaptive,
                credential_outlier,
                credential_index,
                credential_alias,
                effective_weight,
            },
        )
    }

    #[inline]
    pub fn logical(&self) -> &LogicalBackendHandle {
        &self.logical
    }
}

impl fmt::Debug for SelectedAiBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SelectedAiBackend")
            .field("logical", &self.logical)
            .field("runtime_available", &true)
            .field("effective_policy", &self.effective_policy.is_some())
            .field("backend_tls_policy", &self.backend_tls_policy.is_some())
            .field("adaptive", &self.adaptive.is_some())
            .field("credential_index", &self.credential_index)
            .field("effective_weight", &self.effective_weight)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum SelectedLogicalBackend {
    Service(SelectedServiceBackend),
    Ai(SelectedAiBackend),
    A2a(SelectedA2aBackend),
}

impl SelectedLogicalBackend {
    #[inline]
    pub fn logical(&self) -> &LogicalBackendHandle {
        match self {
            Self::Service(selected) => selected.logical(),
            Self::Ai(selected) => selected.logical(),
            Self::A2a(selected) => selected.logical(),
        }
    }

    #[inline]
    pub fn compiled(&self) -> &CompiledBackend {
        self.logical().compiled()
    }
}

/// The sole selected HTTP-backend fact stored in a request context.
#[derive(Clone)]
pub enum SelectedHttpBackend {
    Routed(SelectedLogicalBackend),
    Invalid(CompiledBackend),
    Direct(CompiledBackend),
}

impl fmt::Debug for SelectedHttpBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Routed(selected) => f.debug_tuple("Routed").field(selected).finish(),
            Self::Invalid(compiled) => f
                .debug_struct("Invalid")
                .field("slot", &compiled.slot)
                .field("identity", &compiled.identity)
                .field("target", &compiled.target)
                .finish(),
            Self::Direct(compiled) => f
                .debug_struct("Direct")
                .field("slot", &compiled.slot)
                .field("identity", &compiled.identity)
                .field("target", &compiled.target)
                .finish(),
        }
    }
}

impl SelectedHttpBackend {
    #[inline]
    pub fn compiled(&self) -> &CompiledBackend {
        match self {
            Self::Routed(selected) => selected.compiled(),
            Self::Invalid(compiled) | Self::Direct(compiled) => compiled,
        }
    }

    #[inline]
    pub fn routed(&self) -> Option<&SelectedLogicalBackend> {
        match self {
            Self::Routed(selected) => Some(selected),
            Self::Invalid(_) | Self::Direct(_) => None,
        }
    }
}

impl Deref for SelectedHttpBackend {
    type Target = CompiledBackend;

    fn deref(&self) -> &Self::Target {
        self.compiled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::PluginRuntime;
    use edgion_common::conf_sync::traits::ConfHandler;
    use edgion_resources::resources::edgion_backend::{
        EdgionBackend, EdgionBackendSpec, McpBackendSpec, McpBackendTarget, McpTransport, McpTransportType,
        ResolvedMcpBackend, ResolvedMcpTarget,
    };
    use edgion_resources::resources::{
        BackendTLSPolicy, BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation,
        ResolvedPolicyTargetClass, WellKnownCACertificates,
    };
    use edgion_resources::HTTPBackendRef;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::HashMap;

    fn compiled(name: &str, slot: usize, weight: Option<i32>) -> CompiledBackend {
        let schema = HTTPBackendRef {
            name: name.to_string(),
            namespace: Some("backend-ns".to_string()),
            port: Some(8080),
            weight,
            group: None,
            kind: Some("Service".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        };
        CompiledBackend::compile(schema, "route-ns", slot, Arc::new(PluginRuntime::new()))
    }

    fn compiled_edgion_backend(name: &str, slot: usize, weight: Option<i32>) -> CompiledBackend {
        let schema = HTTPBackendRef {
            name: name.to_string(),
            namespace: Some("backend-ns".to_string()),
            port: None,
            weight,
            group: Some("edgion.io".to_string()),
            kind: Some("EdgionBackend".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        };
        CompiledBackend::compile(schema, "route-ns", slot, Arc::new(PluginRuntime::new()))
    }

    #[test]
    fn logical_handle_preserves_slot_identity_target_and_declared_weight() {
        let logical = LogicalBackendHandle::new(compiled("api", 7, Some(0)));
        assert_eq!(logical.slot(), 7);
        assert_eq!(logical.declared_weight(), 0);
        assert!(logical.is_authorized());
        assert!(matches!(
            logical.target(),
            CompiledBackendTarget::Service { namespace, name }
                if namespace == "backend-ns" && name == "api"
        ));
        assert_eq!(logical.identity().namespace, "backend-ns");
        assert_eq!(logical.identity().name, "api");
        assert_eq!(
            logical.policy_target_key(),
            Some(PolicyTargetKey {
                target_kind: PolicyTargetKind::Service,
                namespace: "backend-ns".to_string(),
                name: "api".to_string(),
            })
        );
    }

    #[test]
    fn invalid_slot_cannot_materialize_selected_service() {
        let mut backend = compiled("api", 2, None);
        backend.target = CompiledBackendTarget::Invalid {
            classified_kind: edgion_resources::HTTPBackendRefKind::Unsupported,
        };
        assert!(SelectedServiceBackend::new(LogicalBackendHandle::new(backend)).is_none());
    }

    #[test]
    fn denied_service_cannot_materialize_selected_service() {
        let mut backend = compiled("api", 2, Some(1));
        backend.schema.ref_denied = Some(edgion_resources::resources::common::RefDenied {
            target_namespace: "backend-ns".to_string(),
            target_name: "api".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        });
        let logical = LogicalBackendHandle::new(backend);
        assert!(logical.policy_target_key().is_none());
        assert!(SelectedServiceBackend::new(logical).is_none());
    }

    #[test]
    fn selected_mcp_proof_is_bound_to_the_selected_slot_and_snapshot_generation() {
        let store = Arc::new(crate::ai::EdgionBackendStore::new());
        let mut backend = EdgionBackend::new(
            "tools",
            EdgionBackendSpec {
                ai: None,
                a2a: None,
                mcp: Some(McpBackendSpec {
                    target: McpBackendTarget {
                        service_ref: None,
                        endpoint: Some("https://tools.example.test/mcp".to_string()),
                        path: None,
                    },
                    allow_insecure_http: false,
                    transport: McpTransport {
                        r#type: McpTransportType::StreamableHTTP,
                    },
                    authentication: None,
                }),
                resolved_a2a: None,
                resolved_mcp: Some(ResolvedMcpBackend {
                    target: ResolvedMcpTarget::External {
                        normalized_endpoint: "https://tools.example.test/mcp".to_string(),
                    },
                }),
                reconcile_outcome: None,
            },
        );
        backend.metadata.namespace = Some("backend-ns".to_string());
        let https_spec = backend.spec.clone();
        let mut other = EdgionBackend::new("other-tools", backend.spec.clone());
        other.metadata.namespace = Some("backend-ns".to_string());
        store.full_set(&HashMap::from([
            ("backend-ns/tools".to_string(), Arc::new(backend)),
            ("backend-ns/other-tools".to_string(), Arc::new(other)),
        ]));
        let snapshot = store.edgion_backend_snapshot();
        let traffic_store =
            crate::backends::policy::backend_traffic::BackendTrafficPolicyStore::with_backend_store(Arc::clone(&store));
        let tls_store =
            crate::backends::policy::backend_tls::BackendTLSPolicyStore::with_backend_store(Arc::clone(&store));
        let unrelated_tls = Arc::new(BackendTLSPolicy {
            metadata: ObjectMeta {
                namespace: Some("backend-ns".to_string()),
                name: Some("other-tools-tls".to_string()),
                ..Default::default()
            },
            spec: BackendTLSPolicySpec {
                target_refs: vec![BackendTLSPolicyTargetRef {
                    group: "edgion.io".to_string(),
                    kind: "EdgionBackend".to_string(),
                    name: "other-tools".to_string(),
                    section_name: None,
                }],
                resolved_target_class: Some(ResolvedPolicyTargetClass::McpExternal),
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: "tools.example.test".to_string(),
                    subject_alt_names: None,
                    well_known_ca_certificates: Some(WellKnownCACertificates::System),
                },
                options: None,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                use_system_ca: true,
            },
            status: None,
        });
        tls_store.replace_all(HashMap::from([(
            "backend-ns/other-tools-tls".to_string(),
            unrelated_tls,
        )]));
        let selected = SelectedMcpBackend::from_snapshot(
            LogicalBackendHandle::new(compiled_edgion_backend("tools", 4, Some(7))),
            &snapshot,
            &traffic_store.snapshot(),
            &tls_store.snapshot(),
        )
        .expect("selected MCP proof");
        assert_eq!(selected.logical().slot(), 4);
        assert_eq!(selected.logical().declared_weight(), 7);
        assert_eq!(selected.backend_snapshot_generation, snapshot.generation());
        assert_eq!(
            selected.logical().policy_target_key(),
            Some(PolicyTargetKey {
                target_kind: PolicyTargetKind::EdgionBackend,
                namespace: "backend-ns".to_string(),
                name: "tools".to_string(),
            })
        );
        assert!(
            selected.backend_tls_policy.is_none(),
            "same-class TLS policy for another EdgionBackend must not attach"
        );

        let mut http_spec = https_spec.clone();
        let http_mcp = http_spec.mcp.as_mut().expect("MCP spec");
        http_mcp.target.endpoint = Some("http://tools.example.test/mcp".to_string());
        http_mcp.allow_insecure_http = true;
        http_spec.resolved_mcp = Some(ResolvedMcpBackend {
            target: ResolvedMcpTarget::External {
                normalized_endpoint: "http://tools.example.test/mcp".to_string(),
            },
        });
        let mut http_backend = EdgionBackend::new("tools", http_spec);
        http_backend.metadata.namespace = Some("backend-ns".to_string());
        store.partial_update(
            HashMap::new(),
            HashMap::from([("backend-ns/tools".to_string(), Arc::new(http_backend))]),
            std::collections::HashSet::new(),
        );

        let direct_tls = Arc::new(BackendTLSPolicy {
            metadata: ObjectMeta {
                namespace: Some("backend-ns".to_string()),
                name: Some("tools-tls".to_string()),
                ..Default::default()
            },
            spec: BackendTLSPolicySpec {
                target_refs: vec![BackendTLSPolicyTargetRef {
                    group: "edgion.io".to_string(),
                    kind: "EdgionBackend".to_string(),
                    name: "tools".to_string(),
                    section_name: None,
                }],
                resolved_target_class: Some(ResolvedPolicyTargetClass::McpExternal),
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: "tools.example.test".to_string(),
                    subject_alt_names: None,
                    well_known_ca_certificates: Some(WellKnownCACertificates::System),
                },
                options: None,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                use_system_ca: true,
            },
            status: None,
        });
        tls_store.replace_all(HashMap::from([("backend-ns/tools-tls".to_string(), direct_tls)]));
        let stale_tls_snapshot = tls_store.snapshot();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "backend-ns".to_string(),
            name: "tools".to_string(),
        };
        assert!(stale_tls_snapshot.has_direct_candidate(&target));
        assert!(stale_tls_snapshot.get_policy(&target, None).is_none());

        let mut https_backend = EdgionBackend::new("tools", https_spec);
        https_backend.metadata.namespace = Some("backend-ns".to_string());
        store.partial_update(
            HashMap::new(),
            HashMap::from([("backend-ns/tools".to_string(), Arc::new(https_backend))]),
            std::collections::HashSet::new(),
        );
        let https_snapshot = store.edgion_backend_snapshot();
        assert!(SelectedMcpBackend::from_snapshot(
            LogicalBackendHandle::new(compiled_edgion_backend("tools", 4, Some(7))),
            &https_snapshot,
            &traffic_store.snapshot(),
            &stale_tls_snapshot,
        )
        .is_none());
    }
}
