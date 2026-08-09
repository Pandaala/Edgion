//! AI Gateway runtime configuration.

pub mod adaptive;
pub(crate) mod anthropic;
pub mod attempt;
pub mod attempt_plan;
pub mod attempt_runtime;
pub mod circuit_breaker;
pub mod credential_outlier;
pub mod dispatch;
pub mod error;
pub mod quota;
pub mod routing;
pub mod service_store;
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
    get_ai_credential_outlier_store, AiCredentialOutlierHandle, AiCredentialOutlierServiceHandle,
    AiCredentialOutlierStore,
};

pub use attempt_plan::{AiAttemptPlan, AiCandidateModel, AiPlanCandidate, AiPlanError, ReadyAiAttemptPlan};

pub use service_store::{
    count_ai_services, create_ai_service_handler, get_global_ai_service_store, global_ai_service_store,
    AiServiceSnapshot, AiServiceStore, ResolvedModel, RuntimeAiCredential, RuntimeAiCredentialPool, RuntimeAiEndpoint,
    RuntimeAiService,
};
pub use usage::{
    estimated_cost_usd, AiProviderLabel, AiRequestContext, AiUsageShared, GenAiAttempt, GenAiAttemptFailure,
    GenAiAttemptUsageSource, GenAiSummary, ReportedAiUsage, UsageExtractionProcessor,
};

#[cfg(test)]
pub use service_store::{lock_global_ai_service_store_for_test, GlobalAiServiceTestGuard};
