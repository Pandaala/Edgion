//! Request-stable proof-carrying AI backend and credential selection.

use crate::ai::{AdaptiveTelemetryState, AiCredentialOutlierHandle, RuntimeAiBackend};
use crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy;
use crate::backends::policy::BackendTlsPolicyLookup;
use crate::backends::runtime::CompiledBackendTarget;
use crate::backends::LogicalBackendHandle;
use std::fmt;
use std::sync::Arc;

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
        (logical.is_authorized() && matches!(logical.target(), CompiledBackendTarget::EdgionBackend(_))).then_some(
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
