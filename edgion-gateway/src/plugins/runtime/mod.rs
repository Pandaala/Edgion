//! Plugin runtime - manages filter execution

pub mod conditional_filter;
pub mod conditions;
pub mod data;
pub mod gateway_api_filters;
pub mod log;
mod plugin_runtime;
mod reject_filter;
pub mod session_adapter;
pub mod traits;

pub use conditional_filter::{
    ConditionalRequestFilter, ConditionalUpstreamResponse, ConditionalUpstreamResponseBodyFilter,
    ConditionalUpstreamResponseFilter,
};
pub use conditions::{Condition, EvaluationResult, PluginConditions};
pub use gateway_api_filters::RequestHeaderModifierFilter;
pub use log::{EdgionPluginsLog, PluginLog, StageLogs};
pub use plugin_runtime::PluginRuntime;
pub use session_adapter::PingoraSessionAdapter;
pub use traits::{
    PluginSession, PluginSessionError, PluginSessionResult, RequestFilter, UpstreamResponse,
    UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
