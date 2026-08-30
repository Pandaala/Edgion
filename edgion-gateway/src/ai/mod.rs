//! AI Gateway runtime configuration.

pub mod adaptive;
pub(crate) mod anthropic;
pub mod attempt;
pub mod attempt_plan;
pub mod attempt_runtime;
pub mod backend_store;
pub(crate) mod canonical_sse;
pub mod circuit_breaker;
pub mod credential_outlier;
pub mod dispatch;
pub mod error;
pub(crate) mod json_span;
pub mod model_router;
pub mod quota;
pub mod routing;
pub(crate) mod terminal;
pub mod usage;

pub(crate) use error::render_openai_error;
pub use error::{openai_error_response, AiProxyError};

pub use adaptive::{
    get_adaptive_telemetry_store, run_adaptive_telemetry_ticker, AdaptiveTelemetryKey, AdaptiveTelemetryState,
    AdaptiveTelemetryStore,
};
pub(crate) use adaptive::{
    reconcile_adaptive_plugin_keys, reconcile_adaptive_targets, reconcile_all_adaptive_plugin_dependents,
    replace_adaptive_route_contributions, update_adaptive_route_contributions,
};

pub use credential_outlier::{
    get_ai_credential_outlier_store, AiCredentialOutlierBackendHandle, AiCredentialOutlierHandle,
    AiCredentialOutlierStore,
};

pub use attempt_plan::{AiAttemptPlan, AiCandidateModel, AiPlanCandidate, AiPlanError, ReadyAiAttemptPlan};

pub use backend_store::{
    count_ai_backends, create_edgion_backend_handler, get_global_ai_backend_store, get_global_edgion_backend_store,
    global_ai_backend_store, global_edgion_backend_store, AiBackendSnapshot, AiBackendStore, EdgionBackendStore,
    ResolvedModel, RuntimeAiBackend, RuntimeAiCredential, RuntimeAiCredentialPool, RuntimeAiEndpoint,
};
pub use usage::{
    estimated_cost_usd, AiProviderLabel, AiRequestContext, AiUsageShared, GenAiAttempt, GenAiAttemptFailure,
    GenAiAttemptUsageSource, GenAiSummary, ReportedAiUsage, UsageExtractionProcessor,
};

#[cfg(test)]
pub use backend_store::{lock_global_ai_backend_store_for_test, GlobalAiBackendTestGuard};
