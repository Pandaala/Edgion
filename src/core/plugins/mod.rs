//! Filter system for request/response processing

pub mod gapi_filters;
pub mod plugin_runtime;
pub mod edgion_plugins;
pub mod edgion_stream_plugins;

pub use plugin_runtime::{Plugin, PluginLog, PluginRuntime, PluginSession, PluginSessionError, PluginSessionResult};
pub use gapi_filters::RequestHeaderModifierFilter;
pub use edgion_stream_plugins::{StreamPlugin, StreamPluginResult, StreamPluginRuntime, StreamContext};

