//! Explicit request-owned response companions. These observe streaming bytes;
//! they do not grant whole-response buffering or response-body mutation.

use super::host_abi::WasmPhase;
use super::plugin::WasmPlugin;
use super::request::WasmRequestBinding;
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::{
    StreamingResponseEvent, StreamingResponseProcessor, UpstreamResponseTrailer, UpstreamTrailerResult,
};
use crate::plugins::runtime::{
    PluginLog, PluginSession, RequestFilter, ResponseStreamSession, ResponseTrailerSession, UpstreamResponse,
};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::plugin_configs::WasmResponseHooks;
use edgion_resources::{filters::PluginRunningResult, EdgionErrorCode};
use pingora_proxy::ResponseBodySink;
use std::collections::HashMap;
use std::sync::Arc;

#[async_trait]
impl RequestFilter for Arc<WasmPlugin> {
    fn name(&self) -> &str {
        "Wasm"
    }
    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        self.as_ref().run_request(session, log).await
    }
}

#[async_trait]
impl UpstreamResponse for Arc<WasmPlugin> {
    fn name(&self) -> &str {
        "Wasm"
    }
    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let Some(binding) = session.wasm_binding(&self.identity) else {
            return PluginRunningResult::GoodNext;
        };
        let Some(mut loan) = binding.take() else {
            // A failed/cancelled request hook never recreates a context here.
            return PluginRunningResult::GoodNext;
        };
        let hooks = self.runtime.config().response;
        let execution = loan.execution();
        if execution.response_headers_seen {
            loan.complete();
            return PluginRunningResult::GoodNext;
        }
        execution.response_headers_seen = true;
        let eos = session.upstream_response_headers_end_stream() == Some(true);
        let lease = &mut execution.lease;
        if hooks.headers {
            let headers = session.response_headers();
            let count = headers.len() as i32;
            lease.state_mut().begin_phase(
                WasmPhase::ResponseHeaders,
                session.request_headers(),
                headers,
                self.properties(session, WasmPhase::ResponseHeaders),
            );
            let Ok(action) = lease.call_bound("proxy_on_response_headers", count, eos) else {
                return self.fail_mode(session, log, "trap");
            };
            if action != 0 && lease.state().frame.local_response.is_none() {
                return self.fail_mode(session, log, "response pause unsupported");
            }
            let result = self.finish_callback(lease, session, log, action, false);
            if !matches!(result, PluginRunningResult::GoodNext) {
                loan.complete();
                return result;
            }
            execution.response_ended = eos;
        }
        if hooks.body
            && !session.install_attached_response_processor(Box::new(WasmResponseObserver {
                binding: binding.clone(),
                hooks,
            }))
        {
            return self.fail_mode(session, log, "response installation");
        }
        if (hooks.body || hooks.trailers)
            && !session.install_upstream_response_trailer(Box::new(WasmResponseObserver {
                binding: binding.clone(),
                hooks,
            }))
        {
            return self.fail_mode(session, log, "trailer installation");
        }
        loan.complete();
        PluginRunningResult::GoodNext
    }
}

struct WasmResponseObserver {
    binding: Arc<WasmRequestBinding>,
    hooks: WasmResponseHooks,
}

impl WasmResponseObserver {
    fn observe(&self, body: &Option<Bytes>, event: StreamingResponseEvent, session: &mut dyn ResponseStreamSession) {
        // A pre-trailer boundary does not end the response. Real trailers are
        // delivered by the companion; do not fabricate a terminal body callback.
        if matches!(event, StreamingResponseEvent::TerminalBeforeTrailers) && body.as_ref().is_none_or(Bytes::is_empty)
        {
            return;
        }
        let Some(mut loan) = self.binding.take() else {
            return;
        };
        let execution = loan.execution();
        if execution.response_ended {
            loan.complete();
            return;
        }
        let eos = event.closes_without_trailers();
        let lease = &mut execution.lease;
        lease
            .state_mut()
            .begin_phase(WasmPhase::ResponseBody, Vec::new(), Vec::new(), HashMap::new());
        lease.state_mut().frame.read_only = true;
        lease.state_mut().frame.body_chunk = body.clone().unwrap_or_default();
        let count = lease.state().frame.body_chunk.len() as i32;
        match lease.call_bound("proxy_on_response_body", count, eos) {
            Ok(0) => {}
            _ => {
                // Observation cannot replace committed headers or block the
                // stream. Disable this binding and emit one bounded diagnostic.
                session.record_error(EdgionErrorCode::WasmExecutionError);
                return;
            }
        }
        execution.response_ended = eos;
        loan.complete();
    }
}

#[async_trait]
impl StreamingResponseProcessor for WasmResponseObserver {
    fn release_inflight(&mut self) {
        self.binding.abort();
    }
    fn on_terminal_preempted(&mut self, _winner: crate::response::terminal::ResponseTerminalCause) {
        self.binding.abort();
    }
    async fn on_event(
        &mut self,
        chunk: &mut Option<Bytes>,
        event: StreamingResponseEvent,
        _kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
    ) {
        self.observe(chunk, event, session);
    }
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        eos: bool,
        _kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
    ) {
        self.observe(chunk, StreamingResponseEvent::Data { end_of_stream: eos }, session);
    }
}

#[async_trait]
impl UpstreamResponseTrailer for WasmResponseObserver {
    fn name(&self) -> &str {
        "Wasm"
    }
    fn release_inflight(&mut self) {
        self.binding.abort();
    }
    fn on_terminal_preempted(&mut self, _reason: &'static str) {
        self.binding.abort();
    }
    async fn run_upstream_response_trailer(
        &mut self,
        trailers: Option<&mut http::HeaderMap>,
        session: &mut dyn ResponseTrailerSession,
        _log: &mut PluginLog,
    ) -> UpstreamTrailerResult {
        let Some(mut loan) = self.binding.take() else {
            return UpstreamTrailerResult::Continue;
        };
        let execution = loan.execution();
        if !execution.response_ended {
            if let Some(trailers) = trailers.filter(|_| self.hooks.trailers) {
                // Transport limits header bytes; retain an independently bounded
                // callback snapshot before copying its values into host memory.
                let bytes = trailers.iter().try_fold(0usize, |n, (k, v)| {
                    n.checked_add(k.as_str().len())?.checked_add(v.len())
                });
                if bytes.is_none_or(|n| n > 64 * 1024) || trailers.len() > 1024 {
                    session.record_error(EdgionErrorCode::WasmExecutionError);
                    return UpstreamTrailerResult::Continue;
                }
                let lease = &mut execution.lease;
                lease
                    .state_mut()
                    .begin_phase(WasmPhase::ResponseTrailers, Vec::new(), Vec::new(), HashMap::new());
                lease.state_mut().frame.read_only = true;
                lease.state_mut().frame.response_trailers = trailers
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.as_str().to_owned(),
                            String::from_utf8_lossy(v.as_bytes()).into_owned(),
                        )
                    })
                    .collect();
                if !matches!(lease.call_bound_trailers(trailers.len() as i32), Ok(0)) {
                    session.record_error(EdgionErrorCode::WasmExecutionError);
                    return UpstreamTrailerResult::Continue;
                }
            }
            execution.response_ended = true;
        }
        loan.complete();
        UpstreamTrailerResult::Continue
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/wasm/attached_test.rs"]
mod tests;
