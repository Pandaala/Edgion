//! RejectFilter — fail-closed placeholder for invalid plugin configs.
//!
//! Controller validation is advisory: it reports invalid `EdgionPlugins`
//! configuration through resource status while lenient admission still allows
//! the resource to reach the Gateway. Runtime construction therefore provides
//! the serving gate. When it omits an invalid plugin,
//! `PluginRuntime::seal_fail_closed` prepends one of these filters so requests
//! are rejected with 500 before reaching upstream instead of silently flowing
//! through. Native HTTPRoute filter construction uses the same seal.
//!
//! `PluginStore`, not this filter, owns the narrow last-known-good retention
//! policy for selected invalid `EdgionPlugins` updates before HTTP and gRPC
//! routes resolve the published resource runtime.

use async_trait::async_trait;

use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, RequestFilter};
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;

/// Response body returned to the client when a route is sealed fail-closed
/// because its EdgionPlugins carries an invalid plugin config. Intentionally
/// generic — the offending plugin names go to the access log, not the client.
const REJECT_BODY: Bytes = Bytes::from_static(br#"{"message":"plugin configuration error"}"#);

/// Request-stage filter that unconditionally rejects the request with 500.
///
/// Holds the names of the invalid plugins purely for access-log diagnostics.
/// Plugin type names are `&'static str` (see `PluginRuntime::get_plugin_name`),
/// so no allocation is needed to carry them.
pub struct RejectFilter {
    invalid_plugins: Vec<&'static str>,
}

impl RejectFilter {
    pub fn new(invalid_plugins: Vec<&'static str>) -> Self {
        Self { invalid_plugins }
    }
}

#[async_trait]
impl RequestFilter for RejectFilter {
    fn name(&self) -> &str {
        "RejectInvalidConfig"
    }

    async fn run_request(&self, _session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        log.push_err("invalid plugin config");
        for &plugin in &self.invalid_plugins {
            log.push_err(plugin);
        }
        PluginRunningResult::ErrResponse {
            status: 500,
            body: Some(REJECT_BODY),
        }
    }
}
