//! ExtProc plugin — external processing over a bidirectional gRPC stream.
//!
//! Covers the request-header phase (`RequestFilter`) and response-header
//! phase (the async `UpstreamResponse` hook). The processor may mutate headers
//! or return an `ImmediateResponse` to short-circuit. Body phases are not
//! supported (rejected at config validation).
//!
//! A single plugin type implements both traits; the runtime constructs a
//! separate instance per stage from the same config.

use async_trait::async_trait;

use super::client::ExtProcClient;
use super::mutation;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter, UpstreamResponse};
use edgion_common::ext_proc::proto::{
    processing_request, processing_response, HeaderMutation, HttpHeaders, ImmediateResponse, ProcessingRequest,
};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::{ExtProcConfig, ProcessingMode};

const PLUGIN_NAME: &str = "ExtProc";

pub struct ExtProc {
    name: String,
    client: ExtProcClient,
    mode: ProcessingMode,
    failure_mode_allow: bool,
}

impl ExtProc {
    fn build(config: &ExtProcConfig) -> Option<Self> {
        match ExtProcClient::new(config) {
            Ok(client) => Some(ExtProc {
                name: PLUGIN_NAME.to_string(),
                client,
                mode: config.processing_mode.clone(),
                failure_mode_allow: config.failure_mode_allow,
            }),
            Err(e) => {
                // Validation already gated the config, so a build failure here is
                // unexpected; fail closed by declining to construct the plugin.
                tracing::error!(error = %e, "ext_proc client build failed; plugin disabled (fail-closed)");
                None
            }
        }
    }

    /// Construct the request-stage filter, or `None` to fail closed.
    pub fn create_request(config: &ExtProcConfig) -> Option<Box<dyn RequestFilter>> {
        Self::build(config).map(|p| Box::new(p) as Box<dyn RequestFilter>)
    }

    /// Construct the response-stage filter, or `None` to fail closed.
    pub fn create_response(config: &ExtProcConfig) -> Option<Box<dyn UpstreamResponse>> {
        Self::build(config).map(|p| Box::new(p) as Box<dyn UpstreamResponse>)
    }

    /// Decide a phase result when the processor could not be consulted or gave an
    /// unusable answer, honoring `failure_mode_allow`: fail-open continues the
    /// chain (`GoodNext`), fail-close short-circuits with a 500. Used for BOTH
    /// transport/gRPC errors AND malformed/wrong-phase verdicts — a buggy
    /// processor must not silently bypass a configured fail-close.
    ///
    /// `detail` carries an optional sub-tag (e.g. the gRPC status code's static
    /// description for transport failures) so the access-log PluginLog entry
    /// preserves the failure category without resorting to data-plane tracing.
    ///
    /// This is also the single choke point for the dye suite's dependency-
    /// failure recording: `record_outcome` is called here (before returning
    /// `PluginRunningResult`, so it is always ordered before the runtime's
    /// own response write) with `reason` kebab-cased as the cause.
    fn fail_mode(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        reason: &str,
        detail: Option<&str>,
    ) -> PluginRunningResult {
        match detail {
            Some(d) if self.failure_mode_allow => {
                log.push_fmt("ext_proc {}:{}:fail-open", la![reason, d]);
            }
            Some(d) => {
                log.push_fmt("ext_proc {}:{}:fail-close", la![reason, d]);
            }
            None if self.failure_mode_allow => {
                log.push_fmt("ext_proc {}:fail-open", la![reason]);
            }
            None => {
                log.push_fmt("ext_proc {}:fail-close", la![reason]);
            }
        }
        let cause = reason.replace(' ', "-");
        if self.failure_mode_allow {
            session.record_outcome(DyeOutcome::FailOpen, &cause);
            PluginRunningResult::GoodNext
        } else {
            session.record_outcome(DyeOutcome::FailClose, &cause);
            PluginRunningResult::ErrResponse {
                status: 500,
                body: Some("ext_proc unavailable".to_string()),
            }
        }
    }
}

/// Clamp a processor-supplied status code into a valid final-response range.
/// 1xx is not a valid final reply, so out-of-range codes map to 500.
fn sanitize_status(code: i32) -> u16 {
    if (200..=599).contains(&code) {
        code as u16
    } else {
        500
    }
}

/// Map a processor `ImmediateResponse` onto a short-circuit `ErrResponse`.
///
/// The runtime's `ErrResponse` handling is gRPC-route aware (emitting
/// grpc-status trailers when appropriate) in both stages, and the body is
/// carried as the reply body.
///
/// Header mutations (minus protected headers) are applied via the session. In
/// the **request** stage these are queued and drained into the local reply, so
/// they reach the client. In the **response** stage the runtime rebuilds a fresh
/// local reply, so a response-phase `ImmediateResponse`'s custom headers are NOT
/// preserved (only status, plus body on non-gRPC routes — gRPC routes terminate
/// status-only). Known limitations for the response-stage short-circuit.
fn immediate_reply(
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    imm: ImmediateResponse,
) -> PluginRunningResult {
    let status = imm.status.map(|s| sanitize_status(s.code)).unwrap_or(500);
    if let Some(headers) = imm.headers.as_ref() {
        mutation::apply_to_response(session, headers, log);
    }
    let body = (!imm.body.is_empty()).then(|| String::from_utf8_lossy(&imm.body).into_owned());
    log.push_fmt("ext_proc immediate {}", la![status.to_string().as_str()]);
    PluginRunningResult::ErrResponse { status, body }
}

/// Extract the `CommonResponse.header_mutation` from a headers verdict, if any.
fn headers_mutation(common: Option<&edgion_common::ext_proc::proto::CommonResponse>) -> Option<&HeaderMutation> {
    common.and_then(|c| c.header_mutation.as_ref())
}

#[async_trait]
impl RequestFilter for ExtProc {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if !self.mode.request_header_mode.is_send() {
            return PluginRunningResult::GoodNext;
        }

        let request = ProcessingRequest {
            request: Some(processing_request::Request::RequestHeaders(HttpHeaders {
                headers: Some(mutation::collect_request_headers(session)),
                end_of_stream: true,
            })),
        };

        let resp = match self.client.exchange(request).await {
            Ok(resp) => resp,
            Err(status) => {
                return self.fail_mode(session, log, "unreachable", Some(status.code().description()));
            }
        };

        match resp.response {
            Some(processing_response::Response::RequestHeaders(hr)) => {
                if let Some(m) = headers_mutation(hr.response.as_ref()) {
                    let n = mutation::apply_to_request(session, m, log);
                    log.push_fmt("req hdr mut {}", la![n.to_string().as_str()]);
                }
                PluginRunningResult::GoodNext
            }
            Some(processing_response::Response::ImmediateResponse(imm)) => immediate_reply(session, log, imm),
            // Wrong phase or empty verdict: a malformed answer must honor the
            // configured fail mode, not silently continue (which would defeat
            // fail-close).
            _ => self.fail_mode(session, log, "bad verdict", None),
        }
    }
}

#[async_trait]
impl UpstreamResponse for ExtProc {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if !self.mode.response_header_mode.is_send() {
            return PluginRunningResult::GoodNext;
        }

        let request = ProcessingRequest {
            request: Some(processing_request::Request::ResponseHeaders(HttpHeaders {
                headers: Some(mutation::collect_response_headers(session)),
                end_of_stream: true,
            })),
        };

        let resp = match self.client.exchange(request).await {
            Ok(resp) => resp,
            Err(status) => {
                return self.fail_mode(session, log, "unreachable", Some(status.code().description()));
            }
        };

        match resp.response {
            Some(processing_response::Response::ResponseHeaders(hr)) => {
                if let Some(m) = headers_mutation(hr.response.as_ref()) {
                    let n = mutation::apply_to_response(session, m, log);
                    log.push_fmt("resp hdr mut {}", la![n.to_string().as_str()]);
                }
                PluginRunningResult::GoodNext
            }
            Some(processing_response::Response::ImmediateResponse(imm)) => immediate_reply(session, log, imm),
            _ => self.fail_mode(session, log, "bad verdict", None),
        }
    }
}

// ========== Tests ==========

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::plugin_configs::GrpcServiceConfig;

    /// A structurally valid config (routable authority, never actually
    /// dialed by these tests — `fail_mode` is exercised directly).
    fn test_config(failure_mode_allow: bool) -> ExtProcConfig {
        ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target: "127.0.0.1:1".to_string(),
                ..ExtProcConfig::default().grpc_service
            },
            failure_mode_allow,
            ..Default::default()
        }
    }

    /// `fail_mode` is the single choke point both `run_request` and
    /// `run_upstream_response` route through on a transport error or a
    /// malformed/wrong-phase verdict. Fail-open arm: records `FailOpen` with
    /// the kebab-cased `reason` as cause, then continues (`GoodNext`).
    #[tokio::test]
    async fn fail_mode_fail_open_records_outcome() {
        let ext_proc = ExtProc::build(&test_config(true)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "unreachable")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.fail_mode(&mut session, &mut log, "unreachable", Some("deadline exceeded"));
        assert!(matches!(result, PluginRunningResult::GoodNext));
    }

    /// Fail-close arm: records `FailClose`, then returns `ErrResponse` (a
    /// value the runtime writes AFTER this call returns — record_outcome is
    /// therefore always ordered before that write by construction, since it
    /// executes before `fail_mode`'s own `return`).
    #[tokio::test]
    async fn fail_mode_fail_close_records_outcome() {
        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "bad-verdict")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.fail_mode(&mut session, &mut log, "bad verdict", None);
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }
}
