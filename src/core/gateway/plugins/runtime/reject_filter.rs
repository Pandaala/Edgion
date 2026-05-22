//! RejectFilter — fail-closed placeholder for invalid plugin configs.
//!
//! When an `EdgionPlugins` resource contains a plugin whose config fails
//! `validation_error()`, the runtime builder skips constructing that plugin
//! (an invalid config must never run in the request path). To keep the route
//! fail-closed, `PluginRuntime::seal_fail_closed` prepends one of these filters
//! to the request stage: every request through the affected EdgionPlugins is
//! rejected with 500 before reaching upstream, instead of silently flowing
//! through as if the plugin were never configured.
//!
//! The CRD schema is the primary gate (invalid resources should be rejected at
//! the apiserver); this is the structural fallback for the cases the schema
//! cannot express, and for non-Kubernetes config sources.

use async_trait::async_trait;

use crate::core::gateway::plugins::runtime::log::PluginLog;
use crate::core::gateway::plugins::runtime::traits::{PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;

/// Response body returned to the client when a route is sealed fail-closed
/// because its EdgionPlugins carries an invalid plugin config. Intentionally
/// generic — the offending plugin names go to the access log, not the client.
const REJECT_BODY: &str = r#"{"message":"plugin configuration error"}"#;

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
            body: Some(REJECT_BODY.to_string()),
        }
    }
}
