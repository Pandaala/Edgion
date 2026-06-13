//! TraceContext plugin — W3C trace-context propagation (propagation-only).
//!
//! - `trustInbound` false: ignore inbound trace context, always originate a root.
//! - well-formed inbound `traceparent`: pass it through unchanged, align x-trace-id.
//! - absent/malformed: strip partial/garbage headers, generate a root (if enabled).

use async_trait::async_trait;

use super::w3c;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::TraceContextConfig;

const TRACEPARENT: &str = "traceparent";
const TRACESTATE: &str = "tracestate";
/// W3C-recommended upper bound on `tracestate`; oversized => dropped.
const MAX_TRACESTATE_BYTES: usize = 512;

pub struct TraceContext {
    name: String,
    config: TraceContextConfig,
}

impl TraceContext {
    pub fn create(config: &TraceContextConfig) -> Box<dyn RequestFilter> {
        Box::new(TraceContext {
            name: "TraceContext".to_string(),
            config: config.clone(),
        })
    }

    /// Drop any inbound trace headers, then (if enabled) generate a fresh root,
    /// inject it upstream, align x-trace-id, and optionally echo to the client.
    fn reset_and_maybe_generate(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        let _ = session.remove_request_header(TRACEPARENT);
        let _ = session.remove_request_header(TRACESTATE);
        if !self.config.generate_when_missing {
            plugin_log.push("no valid traceparent; generation disabled");
            return;
        }
        let root = w3c::generate_root(self.config.default_sampled);
        let _ = session.set_request_header(TRACEPARENT, &root.traceparent);
        let _ = session.set_trace_id(&root.trace_id);
        if self.config.echo_to_client {
            let _ = session.set_response_header(TRACEPARENT, &root.traceparent);
        }
        plugin_log.push("generated root traceparent");
    }
}

#[async_trait]
impl RequestFilter for TraceContext {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Edge posture: ignore (untrusted) inbound trace context entirely.
        if !self.config.trust_inbound {
            self.reset_and_maybe_generate(session, plugin_log);
            return PluginRunningResult::GoodNext;
        }

        let inbound = session.header_value(TRACEPARENT);
        match inbound.as_deref().and_then(w3c::parse_trace_id) {
            Some(trace_id) => {
                // Pass-through: the original traceparent is already on the request.
                let _ = session.set_trace_id(&trace_id);
                // Forward tracestate unchanged, but enforce a size cap.
                if let Some(ts) = session.header_value(TRACESTATE) {
                    if ts.len() > MAX_TRACESTATE_BYTES {
                        let _ = session.remove_request_header(TRACESTATE);
                        plugin_log.push("tracestate dropped (oversized)");
                    }
                }
                if self.config.echo_to_client {
                    if let Some(tp) = inbound {
                        let _ = session.set_response_header(TRACEPARENT, &tp);
                    }
                }
                plugin_log.push("passthrough traceparent");
            }
            None => self.reset_and_maybe_generate(session, plugin_log),
        }

        PluginRunningResult::GoodNext
    }
}
