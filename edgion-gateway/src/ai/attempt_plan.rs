//! Immutable request plan shared by all AI upstream attempts.

use std::fmt;
use std::sync::Arc;

use bytes::Bytes;
use edgion_resources::resources::edgion_backend::ParsedAiModelPricing;
use edgion_resources::resources::edgion_plugins::AiBackendSelection;
use edgion_resources::resources::BackendTLSPolicy;

use crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy;
use crate::backends::LogicalBackendHandle;

use super::attempt::AttemptScheduler;
use super::quota::AiQuotaRequest;
use super::{AdaptiveTelemetryState, AiBackendSnapshot, AiCredentialOutlierHandle, RuntimeAiBackend};

/// A fixed failure sampled before AI request processing begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiPlanError {
    InvalidBackend,
    InvalidRouteConfiguration,
}

/// Candidate-local model resolution from one backend-store generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiCandidateModel {
    pub provider_model: String,
    pub pricing: Option<ParsedAiModelPricing>,
}

/// One route backend slot retained for later attempt selection.
#[derive(Clone)]
pub struct AiPlanCandidate {
    /// Complete compiled logical-slot proof captured with this request plan.
    pub logical: LogicalBackendHandle,
    pub slot: usize,
    pub order: u8,
    pub weight: i32,
    pub backend_key: String,
    pub backend: Option<Arc<RuntimeAiBackend>>,
    pub model: Option<AiCandidateModel>,
    pub effective_policy: Option<Arc<EffectiveBackendTrafficPolicy>>,
    /// TLS policy winner captured with this candidate's request-stable plan.
    pub backend_tls_policy: Option<Arc<BackendTLSPolicy>>,
    /// Route/rule/target/model telemetry generation captured with this plan.
    pub adaptive: Option<Arc<AdaptiveTelemetryState>>,
    /// Credential state captured from the candidate's exact EBTP policy
    /// generation. Indices align with `backend.credential_pool.credentials`.
    pub credential_outliers: Vec<Option<AiCredentialOutlierHandle>>,
}

impl fmt::Debug for AiPlanCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiPlanCandidate")
            .field("logical", &self.logical)
            .field("slot", &self.slot)
            .field("order", &self.order)
            .field("weight", &self.weight)
            .field("backend_key", &self.backend_key)
            .field("backend_available", &self.backend.is_some())
            .field("model", &self.model)
            .field("effective_policy", &self.effective_policy.is_some())
            .field("backend_tls_policy", &self.backend_tls_policy.is_some())
            .field("adaptive", &self.adaptive.is_some())
            .field(
                "credential_outliers_available",
                &self
                    .credential_outliers
                    .iter()
                    .filter(|handle| handle.is_some())
                    .count(),
            )
            .finish()
    }
}

/// Request data that is immutable across provider and credential retries.
pub struct ReadyAiAttemptPlan {
    pub canonical_openai_body: Bytes,
    pub canonical_path: String,
    pub canonical_query: Option<String>,
    pub logical_model: String,
    pub requested_streaming: bool,
    pub inject_usage_tracking: bool,
    pub input_chars: u64,
    pub selection: AiBackendSelection,
    pub selection_sequence: u8,
    pub candidates: Vec<AiPlanCandidate>,
    pub scheduler: AttemptScheduler,
    pub retry_cap_initialized: bool,
    /// Keeps the complete captured generation alive for request stability.
    pub backend_snapshot: AiBackendSnapshot,
    pub quota: Option<AiQuotaRequest>,
    pub quota_status: Option<&'static str>,
    /// Resolved from the compiled `AiTokenQuotaConfig.onExceeded`: `true`
    /// only for `Reject`. `Throttle`/`Downgrade` are an operator declaring
    /// "do not hard-fail this user" -- honoring that at admission time only
    /// and then hard-cutting an in-flight stream anyway would silently
    /// override a configured choice, so the mid-stream response processors
    /// gate their over-quota termination on this flag too.
    pub terminate_on_over_limit: bool,
}

impl fmt::Debug for ReadyAiAttemptPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadyAiAttemptPlan")
            .field("canonical_openai_body_bytes", &self.canonical_openai_body.len())
            .field("canonical_path", &self.canonical_path)
            .field("logical_model", &self.logical_model)
            .field("requested_streaming", &self.requested_streaming)
            .field("inject_usage_tracking", &self.inject_usage_tracking)
            .field("input_chars", &self.input_chars)
            .field("selection", &self.selection)
            .field("candidates", &self.candidates)
            .field("scheduler_dispatches", &self.scheduler.dispatch_count())
            .field("retry_cap_initialized", &self.retry_cap_initialized)
            .field("quota_configured", &self.quota.is_some())
            .field("quota_status", &self.quota_status)
            .field("terminate_on_over_limit", &self.terminate_on_over_limit)
            .finish()
    }
}

/// The single request-wide AI forwarding decision.
pub enum AiAttemptPlan {
    Rejected(AiPlanError),
    Ready(ReadyAiAttemptPlan),
}

impl fmt::Debug for AiAttemptPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(error) => f.debug_tuple("Rejected").field(error).finish(),
            Self::Ready(plan) => plan.fmt(f),
        }
    }
}
