//! Configuration for the TraceContext (W3C trace-context propagation) plugin.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::PluginValidation;

/// W3C Trace Context propagation plugin configuration (propagation-only).
///
/// The gateway reads/forwards the W3C `traceparent` header so downstream
/// OpenTelemetry-instrumented services correlate with gateway requests. It does
/// not export spans to a collector.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TraceContextConfig {
    /// Generate a fresh root `traceparent` when the request carries none (or
    /// `trustInbound` is false). Default: true.
    #[serde(default = "default_true")]
    pub generate_when_missing: bool,

    /// Also write `traceparent` onto the client-facing response. Default: false.
    #[serde(default)]
    pub echo_to_client: bool,

    /// Honor a well-formed client-supplied `traceparent` (pass it through). Set
    /// false at an internet edge to always originate a fresh root and ignore
    /// (untrusted) inbound trace context. Default: true.
    #[serde(default = "default_true")]
    pub trust_inbound: bool,

    /// trace-flags used when generating a root: false => `00` (defer sampling to
    /// downstream), true => `01` (force sampled). Default: false.
    #[serde(default)]
    pub default_sampled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for TraceContextConfig {
    fn default() -> Self {
        Self {
            generate_when_missing: true,
            echo_to_client: false,
            trust_inbound: true,
            default_sampled: false,
        }
    }
}

impl PluginValidation for TraceContextConfig {
    fn validation_error(&self) -> Option<String> {
        // No invalid combination — all fields are independent booleans.
        None
    }
}
