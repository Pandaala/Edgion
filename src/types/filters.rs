use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

/// Filter configuration
/// Each stage uses a fixed array order as priority
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PluginConf {
    pub enable: bool,
    pub name: String,
    pub config: serde_json::Value,
}

/// Filter running stage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum PluginRunningStage {
    /// request_filter (async)
    Request,
    /// upstream_response_filter (sync)
    UpstreamResponseFilter,
    /// response_filter (async)
    UpstreamResponse,
}

/// Filter running result
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PluginRunningResult {
    /// Just nothing, continue to run other plugins
    Nothing,
    /// Filter running good, and should continue to run other plugins
    GoodNext,
    /// Filter judged that the request should be stopped here, no other plugins should run
    ErrTerminateRequest,
    /// Filter returns an error response with custom status and body
    ErrResponse {
        status: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<String>,
    },
}

/// Filter tags for categorization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum PluginTags {
    Auth,
    Security,
    Proxy,
    Traffic,
}
