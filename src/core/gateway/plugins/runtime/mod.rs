//! Plugin runtime - manages filter execution

pub mod conditions;
pub mod conditional_filter;
pub mod gateway_api_filters;
pub mod log;
pub mod runtime;
pub mod session_adapter;
pub mod traits;

pub use conditions::{Condition, EvaluationResult, PluginConditions};
pub use conditional_filter::{
    ConditionalRequestFilter, ConditionalUpstreamResponse, ConditionalUpstreamResponseBodyFilter,
    ConditionalUpstreamResponseFilter,
};
pub use gateway_api_filters::RequestHeaderModifierFilter;
pub use log::{EdgionPluginsLog, PluginLog, StageLogs};
pub use runtime::PluginRuntime;
pub use session_adapter::PingoraSessionAdapter;
pub use traits::{
    PluginSession, PluginSessionError, PluginSessionResult, RequestFilter, UpstreamResponse,
    UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
