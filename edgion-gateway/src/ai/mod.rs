//! AI Gateway runtime, routing, protocols, and accounting.

pub mod accounting;
pub mod config;
pub(crate) mod connectors;
pub(crate) mod context;
pub(crate) mod conversion;
pub(crate) mod guards;
pub(crate) mod http;
pub mod protocols;
pub mod quota;
pub mod routing;
pub mod runtime;

pub(crate) use protocols::openai::error::render_openai_error;
pub use protocols::openai::error::{openai_error_response, AiProxyError};

pub use routing::adaptive::{
    get_adaptive_telemetry_store, run_adaptive_telemetry_ticker, AdaptiveTelemetryKey, AdaptiveTelemetryState,
    AdaptiveTelemetryStore,
};
pub(crate) use routing::adaptive::{
    reconcile_adaptive_plugin_keys, reconcile_adaptive_targets, reconcile_all_adaptive_plugin_dependents,
    replace_adaptive_route_contributions, update_adaptive_route_contributions,
};

pub use runtime::credential_outlier::{
    get_ai_credential_outlier_store, AiCredentialOutlierBackendHandle, AiCredentialOutlierHandle,
    AiCredentialOutlierStore,
};

pub use runtime::attempt_plan::{AiAttemptPlan, AiCandidateModel, AiPlanCandidate, AiPlanError, ReadyAiAttemptPlan};

pub use accounting::usage::{
    estimated_cost_usd, AiProviderLabel, AiRequestContext, AiUsageShared, GenAiAttempt, GenAiAttemptFailure,
    GenAiAttemptUsageSource, GenAiSummary, ReportedAiUsage, UsageExtractionProcessor,
};
pub use runtime::backend_store::{
    create_edgion_backend_handler, get_global_ai_backend_store, get_global_edgion_backend_store,
    global_ai_backend_store, global_edgion_backend_store, AiBackendSnapshot, AiBackendStore, EdgionBackendStore,
    ResolvedModel, RuntimeAiBackend, RuntimeAiCredential, RuntimeAiCredentialPool, RuntimeAiEndpoint,
};

#[cfg(test)]
pub use runtime::backend_store::{lock_global_ai_backend_store_for_test, GlobalAiBackendTestGuard};

pub use runtime::selected::SelectedAiBackend;
