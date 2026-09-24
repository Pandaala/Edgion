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
    generation: Arc<[CompiledBackend]>,
    index: usize,
}

impl LogicalBackendHandle {
    pub(crate) fn new(generation: Arc<[CompiledBackend]>, index: usize) -> Option<Self> {
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
#[path = "../../tests/unit/backends/runtime_test.rs"]
mod tests;
