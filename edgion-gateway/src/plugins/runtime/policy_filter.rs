//! Enforcement filters for the captured GatewayClass plugin policy. Mirrors `reject_filter.rs`: the block gate is
//! an in-chain request-stage filter, so it executes wherever the chain
//! executes and needs no per-attachment-point wiring.

use async_trait::async_trait;

use crate::observe::metrics::registry::{record_plugin_policy_blocked, record_plugin_policy_bypassed};
use crate::plugins::runtime::log::{la, PluginLog};
use crate::plugins::runtime::traits::{PluginSession, RequestFilter};
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;

/// Body returned on a policy block. Intentionally generic: the plugin name
/// goes to the access log and metrics, never to the client.
const BLOCK_BODY: Bytes = Bytes::from_static(br#"{"message":"plugin disabled by policy"}"#);

/// Unconditionally rejects requests on a chain that contains a policy-blocked
/// plugin entry. Prepended AHEAD of the fail-closed RejectFilter, so the
/// policy status code wins when both gates exist.
pub struct PolicyBlockFilter {
    /// Qualified name (`http/Wasm`) of the first policy-blocked entry in spec
    /// order — its status code is the one returned.
    plugin: String,
    status: u16,
}

impl PolicyBlockFilter {
    pub fn new(plugin: String, status: u16) -> Self {
        Self { plugin, status }
    }
}

#[async_trait]
impl RequestFilter for PolicyBlockFilter {
    fn name(&self) -> &str {
        "PluginPolicyBlock"
    }

    async fn run_request(&self, _session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        log.push_err("blocked by plugin policy");
        // The qualified plugin name is dynamic (built per-runtime from the
        // restricted entry), so it cannot satisfy `push_err`'s `&'static str`
        // bound; `push_fmt` with a plain (non-sensitive) arg carries it
        // instead — it is operator config data, not user input.
        log.push_fmt("plugin: {}", la![self.plugin.as_str()]);
        record_plugin_policy_blocked(&self.plugin);
        PluginRunningResult::ErrResponse {
            status: self.status,
            body: Some(BLOCK_BODY),
        }
    }
}

/// No-op request filter that records policy-bypassed plugin names into the
/// access log (and metrics) on every request through the chain. Rides the
/// existing PluginLog plumbing instead of a new access-log field.
pub struct PolicyBypassNote {
    bypassed: Vec<String>,
}

impl PolicyBypassNote {
    pub fn new(bypassed: Vec<String>) -> Self {
        Self { bypassed }
    }
}

#[async_trait]
impl RequestFilter for PolicyBypassNote {
    fn name(&self) -> &str {
        "PluginPolicyBypass"
    }

    async fn run_request(&self, _session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        for name in &self.bypassed {
            log.push_fmt("bypassed: {}", la![name.as_str()]);
            record_plugin_policy_bypassed(name);
        }
        PluginRunningResult::Nothing
    }
}
