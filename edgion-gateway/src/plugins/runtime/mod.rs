//! Plugin runtime - manages filter execution

pub mod body_gate;
pub mod conditional_filter;
pub mod conditions;
pub mod dye_plan;
pub mod edgion_config_data;
pub(crate) mod entry_bundle;
pub mod gateway_api_filters;
pub mod log;
mod plugin_runtime;
pub mod policy_filter;
mod reject_filter;
pub mod session_adapter;
pub mod traits;

pub use body_gate::{BodyGatedRequestFilter, BodyGatedUpstreamResponse};
pub use conditional_filter::{
    ConditionalRequestFilter, ConditionalUpstreamResponse, ConditionalUpstreamResponseBodyFilter,
    ConditionalUpstreamResponseFilter,
};
pub use conditions::{Condition, EvaluationResult, PluginConditions};
pub use dye_plan::{CompiledDyePlan, CompiledDyeRule};
pub use gateway_api_filters::RequestHeaderModifierFilter;
pub use log::{EdgionPluginsLog, PluginLog, StageLogs};
pub(crate) use plugin_runtime::InvalidUpdatePolicy;
pub use plugin_runtime::PluginRuntime;
pub use session_adapter::PingoraSessionAdapter;
pub use traits::{
    ChunkVerdict, DyeOutcome, PluginSession, PluginSessionError, PluginSessionHeaderExt, PluginSessionResult,
    RequestBodyStreamHandler, RequestFilter, StreamingResponseEvent, UpstreamResponse, UpstreamResponseBodyFilter,
    UpstreamResponseFilter, UpstreamResponseTrailer, UpstreamTrailerResult,
};

#[cfg(test)]
mod rate_limit_overflow_tests;
