//! Proof-carrying backend identities between route selection and transport.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use crate::ai::{AdaptiveTelemetryState, AiCredentialOutlierHandle, RuntimeAiBackend};
use crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy;
use crate::backends::policy::BackendTlsPolicyLookup;
use crate::routes::http::match_unit::{CompiledBackend, CompiledBackendTarget};
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};

/// One immutable, gateway-compiled HTTPRoute backend slot.
#[derive(Clone)]
pub struct LogicalBackendHandle {
    generation: Arc<Vec<CompiledBackend>>,
    index: usize,
}

impl LogicalBackendHandle {
    pub(crate) fn new(generation: Arc<Vec<CompiledBackend>>, index: usize) -> Option<Self> {
        if generation.get(index).is_some_and(|compiled| compiled.slot == index) {
            Some(Self { generation, index })
        } else {
            None
        }
    }

    #[inline]
    pub fn compiled(&self) -> &CompiledBackend {
        &self.generation[self.index]
    }

    #[inline]
    pub fn slot(&self) -> usize {
        self.compiled().slot
    }

    #[inline]
    pub fn identity(&self) -> &edgion_resources::LogicalBackendSlotIdentity {
        &self.compiled().identity
    }

    #[inline]
    pub fn target(&self) -> &CompiledBackendTarget {
        &self.compiled().target
    }

    #[inline]
    pub fn declared_weight(&self) -> i32 {
        self.compiled().schema.weight.unwrap_or(1)
    }

    #[inline]
    pub fn is_authorized(&self) -> bool {
        self.compiled().schema.ref_denied.is_none()
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
#[derive(Clone)]
pub struct SelectedServiceBackend {
    logical: LogicalBackendHandle,
    backend_tls_policy: BackendTlsPolicyLookup,
}

impl fmt::Debug for SelectedServiceBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SelectedServiceBackend")
            .field("logical", &self.logical)
            .field("backend_tls_policy", &self.backend_tls_policy.debug_label())
            .finish()
    }
}

impl SelectedServiceBackend {
    pub(crate) fn new(logical: LogicalBackendHandle, backend_tls_policy: BackendTlsPolicyLookup) -> Option<Self> {
        (logical.is_authorized() && matches!(logical.target(), CompiledBackendTarget::Service { .. })).then_some(Self {
            logical,
            backend_tls_policy,
        })
    }

    #[inline]
    pub fn logical(&self) -> &LogicalBackendHandle {
        &self.logical
    }

    #[inline]
    pub fn backend_tls_policy(&self) -> &BackendTlsPolicyLookup {
        &self.backend_tls_policy
    }
}

/// An AI backend and credential chosen from one request-stable plan generation.
#[derive(Clone)]
pub struct SelectedAiBackend {
    logical: LogicalBackendHandle,
    pub backend: Arc<RuntimeAiBackend>,
    pub effective_policy: Option<Arc<EffectiveBackendTrafficPolicy>>,
    /// Request-stable TLS policy selected for this logical backend.
    pub backend_tls_policy: BackendTlsPolicyLookup,
    pub adaptive: Option<Arc<AdaptiveTelemetryState>>,
    pub credential_outlier: Option<AiCredentialOutlierHandle>,
    pub credential_index: usize,
    pub credential_alias: String,
    pub effective_weight: i32,
}

impl SelectedAiBackend {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        logical: LogicalBackendHandle,
        backend: Arc<RuntimeAiBackend>,
        effective_policy: Option<Arc<EffectiveBackendTrafficPolicy>>,
        backend_tls_policy: BackendTlsPolicyLookup,
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
            .field("backend_tls_policy", &self.backend_tls_policy.debug_label())
            .field("adaptive", &self.adaptive.is_some())
            .field("credential_index", &self.credential_index)
            .field("effective_weight", &self.effective_weight)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum SelectedLogicalBackend {
    Service(SelectedServiceBackend),
    Ai(LogicalBackendHandle),
}

impl SelectedLogicalBackend {
    #[inline]
    pub fn logical(&self) -> &LogicalBackendHandle {
        match self {
            Self::Service(selected) => selected.logical(),
            Self::Ai(logical) => logical,
        }
    }

    #[inline]
    pub fn compiled(&self) -> &CompiledBackend {
        self.logical().compiled()
    }
}

/// The sole selected HTTP-backend fact stored in a request context.
pub enum SelectedHttpBackend {
    Service(SelectedServiceBackend),
    Ai(LogicalBackendHandle),
    Invalid(LogicalBackendHandle),
}

impl fmt::Debug for SelectedHttpBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Service(selected) => f.debug_tuple("Service").field(selected).finish(),
            Self::Ai(logical) => f.debug_tuple("Ai").field(logical).finish(),
            Self::Invalid(logical) => f
                .debug_struct("Invalid")
                .field("slot", &logical.slot())
                .field("identity", &logical.identity())
                .field("target", &logical.target())
                .finish(),
        }
    }
}

impl SelectedHttpBackend {
    #[inline]
    pub fn compiled(&self) -> &CompiledBackend {
        match self {
            Self::Service(selected) => selected.logical().compiled(),
            Self::Ai(logical) | Self::Invalid(logical) => logical.compiled(),
        }
    }

    #[inline]
    pub fn routed(&self) -> Option<SelectedLogicalBackend> {
        match self {
            Self::Service(selected) => Some(SelectedLogicalBackend::Service(selected.clone())),
            Self::Ai(logical) => Some(SelectedLogicalBackend::Ai(logical.clone())),
            Self::Invalid(_) => None,
        }
    }

    #[inline]
    pub fn is_ai(&self) -> bool {
        matches!(self, Self::Ai(_))
    }

    #[inline]
    pub fn plugin_runtime(&self) -> Arc<crate::plugins::PluginRuntime> {
        Arc::clone(&self.compiled().plugin_runtime)
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
    use crate::backends::policy::backend_tls::BackendTLSPolicyStore;
    use crate::plugins::PluginRuntime;
    use edgion_resources::resources::backend_tls_policy::{
        BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation,
    };
    use edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass;
    use edgion_resources::resources::BackendTLSPolicy;
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

    fn logical(compiled: CompiledBackend) -> LogicalBackendHandle {
        let generation = Arc::new(vec![compiled]);
        LogicalBackendHandle::new(generation, 0).expect("single-slot fixture must be valid")
    }

    fn tls_policy(name: &str, hostname: &str) -> Arc<BackendTLSPolicy> {
        Arc::new(BackendTLSPolicy {
            metadata: ObjectMeta {
                namespace: Some("backend-ns".to_string()),
                name: Some(name.to_string()),
                uid: Some(format!("uid-{name}")),
                ..Default::default()
            },
            spec: BackendTLSPolicySpec {
                target_refs: vec![BackendTLSPolicyTargetRef {
                    group: String::new(),
                    kind: "Service".to_string(),
                    name: "api".to_string(),
                    section_name: None,
                }],
                resolved_target_class: Some(ResolvedPolicyTargetClass::Service),
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: hostname.to_string(),
                    subject_alt_names: None,
                    well_known_ca_certificates: None,
                },
                options: None,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                use_system_ca: true,
            },
            status: None,
        })
    }

    #[test]
    fn logical_handle_preserves_slot_identity_target_and_declared_weight() {
        let generation = Arc::new(vec![compiled("api", 0, Some(0))]);
        let logical = LogicalBackendHandle::new(generation, 0).unwrap();
        assert_eq!(logical.slot(), 0);
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
        let mut backend = compiled("api", 0, None);
        backend.target = CompiledBackendTarget::Invalid {
            classified_kind: edgion_resources::HTTPBackendRefKind::Unsupported,
        };
        assert!(SelectedServiceBackend::new(logical(backend), BackendTlsPolicyLookup::Absent).is_none());
    }

    #[test]
    fn denied_service_cannot_materialize_selected_service() {
        let mut backend = compiled("api", 0, Some(1));
        backend.schema.ref_denied = Some(edgion_resources::resources::common::RefDenied {
            target_namespace: "backend-ns".to_string(),
            target_name: "api".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        });
        let logical = logical(backend);
        assert!(logical.policy_target_key().is_none());
        assert!(SelectedServiceBackend::new(logical, BackendTlsPolicyLookup::Absent).is_none());
    }

    #[test]
    fn logical_handle_rejects_invalid_generation_indexes() {
        let generation = Arc::new(vec![compiled("api", 0, None)]);
        assert!(LogicalBackendHandle::new(Arc::clone(&generation), 1).is_none());

        let mismatched = Arc::new(vec![compiled("api", 3, None)]);
        assert!(LogicalBackendHandle::new(mismatched, 0).is_none());
    }

    #[test]
    fn compact_backend_state_stays_within_hot_path_budget() {
        assert_eq!(std::mem::size_of::<LogicalBackendHandle>(), 16);
        assert_eq!(std::mem::size_of::<SelectedServiceBackend>(), 32);
        assert_eq!(std::mem::size_of::<Option<SelectedHttpBackend>>(), 32);
    }

    #[test]
    fn service_selection_and_runtime_projection_are_allocation_free() {
        let generation = Arc::new(vec![compiled("api", 0, None)]);
        let (selected, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            let logical = LogicalBackendHandle::new(Arc::clone(&generation), 0).unwrap();
            let selected = SelectedHttpBackend::Service(
                SelectedServiceBackend::new(logical, BackendTlsPolicyLookup::Absent).unwrap(),
            );
            let runtime = selected.plugin_runtime();
            drop(runtime);
            selected
        });

        assert_eq!(selected.slot, 0);
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }

    #[test]
    fn logical_handle_pins_generation_and_plugin_runtime() {
        let old_runtime = Arc::new(PluginRuntime::new());
        let old_generation = Arc::new(vec![CompiledBackend::compile(
            HTTPBackendRef {
                name: "old".to_string(),
                namespace: Some("backend-ns".to_string()),
                port: Some(8080),
                weight: Some(1),
                group: None,
                kind: Some("Service".to_string()),
                filters: None,
                backend_tls_policy: None,
                ref_denied: None,
            },
            "route-ns",
            0,
            Arc::clone(&old_runtime),
        )]);
        let logical = LogicalBackendHandle::new(Arc::clone(&old_generation), 0).unwrap();
        let selected =
            SelectedHttpBackend::Service(SelectedServiceBackend::new(logical, BackendTlsPolicyLookup::Absent).unwrap());

        let replacement = Arc::new(vec![compiled("new", 0, None)]);
        drop(old_generation);

        assert_eq!(selected.name, "old");
        assert!(!Arc::ptr_eq(
            &selected.routed().unwrap().logical().generation,
            &replacement
        ));
        assert!(Arc::ptr_eq(&selected.plugin_runtime(), &old_runtime));
    }

    #[test]
    fn selected_service_pins_tls_policy_without_mutating_compiled_schema() {
        let store = BackendTLSPolicyStore::new();
        let old_policy = tls_policy("old", "old.example.com");
        store.replace_all(HashMap::from([("backend-ns/old".to_string(), Arc::clone(&old_policy))]));

        let generation = Arc::new(vec![compiled("api", 0, None)]);
        let logical = LogicalBackendHandle::new(Arc::clone(&generation), 0).unwrap();
        let target = logical.policy_target_key().unwrap();
        let captured = store.snapshot().lookup(&target, None);
        let selected = SelectedServiceBackend::new(logical, captured).unwrap();

        let new_policy = tls_policy("new", "new.example.com");
        store.replace_all(HashMap::from([("backend-ns/new".to_string(), Arc::clone(&new_policy))]));
        let current = store
            .snapshot()
            .lookup(&target, None)
            .effective()
            .expect("replacement policy must stay effective")
            .clone();

        let retained = selected
            .backend_tls_policy()
            .effective()
            .expect("captured policy must stay effective");
        assert!(Arc::ptr_eq(&retained.policy, &old_policy));
        assert!(Arc::ptr_eq(&current.policy, &new_policy));
        assert_eq!(retained.policy.spec.validation.hostname, "old.example.com");
        assert_eq!(current.policy.spec.validation.hostname, "new.example.com");
        assert!(selected.logical().compiled().schema.backend_tls_policy.is_none());
    }

    #[test]
    fn selected_service_debug_does_not_expose_tls_policy_secrets() {
        let mut policy = (*tls_policy("private", "private.example.com")).clone();
        let secret = k8s_openapi::api::core::v1::Secret {
            string_data: Some(std::collections::BTreeMap::from([(
                "tls.key".to_string(),
                "PRIVATE-KEY-SENTINEL".to_string(),
            )])),
            ..Default::default()
        };
        policy.spec.resolved_ca_certificates = Some(vec![secret.clone()]);
        policy.spec.resolved_client_certificate = Some(secret);

        let selected = SelectedServiceBackend::new(
            logical(compiled("api", 0, None)),
            BackendTlsPolicyLookup::effective_from_policy(Arc::new(policy)),
        )
        .expect("authorized Service fixture must be selectable");
        let debug = format!("{:?}", SelectedHttpBackend::Service(selected));

        assert!(debug.contains("backend_tls_policy: \"effective\""));
        assert!(!debug.contains("PRIVATE-KEY-SENTINEL"));
        assert!(!debug.contains("resolved_ca_certificates"));
        assert!(!debug.contains("resolved_client_certificate"));
    }
}
