//! Filter system for request/response processing

pub mod edgion_plugins;
pub mod edgion_stream_plugins;
pub mod gapi_filters;
pub mod plugin_runtime;
pub mod plugins_cond;

pub use edgion_stream_plugins::{StreamContext, StreamPlugin, StreamPluginResult, StreamPluginRuntime};
pub use gapi_filters::RequestHeaderModifierFilter;
pub use plugin_runtime::{
    EdgionPluginsLog, PluginLog, PluginRuntime, PluginSession, PluginSessionError, PluginSessionResult, RequestFilter,
    StageLogs, UpstreamResponse, UpstreamResponseFilter,
};
pub use plugins_cond::{Condition, ConditionContext, ConditionSource, EvaluationResult, PluginConditions};
