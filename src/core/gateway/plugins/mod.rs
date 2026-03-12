//! Filter system for request/response processing

pub mod http;
pub mod runtime;
pub mod stream;

pub use runtime::conditions::{Condition, EvaluationResult, PluginConditions};
pub use runtime::gateway_api_filters::RequestHeaderModifierFilter;
pub use runtime::{
    EdgionPluginsLog, PluginLog, PluginRuntime, PluginSession, PluginSessionError, PluginSessionResult, RequestFilter,
    StageLogs, UpstreamResponse, UpstreamResponseFilter,
};
pub use stream::{StreamContext, StreamPlugin, StreamPluginResult, StreamPluginRuntime};
