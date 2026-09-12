//! ExtProc plugin — external processing over a bidirectional gRPC stream.
//!
//! Covers the request-header phase (`RequestFilter`) and response-header
//! phase (the async `UpstreamResponse` hook). The processor may mutate headers
//! or return an `ImmediateResponse` to short-circuit. The request phase also
//! supports a fully buffered body and body replacement. Response bodies use
//! the ordinary bounded streaming-processor seam and are never aggregated.
//!
//! One request entry constructs both wrappers around the same compiled owner.

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;

use super::attached::{AttachedExtProc, OverridePhase};
use super::client::ExtProcClient;
use super::mutation;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter, UpstreamResponse};
use edgion_common::ext_proc::proto::{
    body_mutation, common_response::ResponseStatus, processing_request, processing_response, CommonResponse, HttpBody,
    HttpHeaders, ImmediateResponse, ProcessingRequest,
};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::{ExtProcConfig, ProcessingMode};
use edgion_resources::EdgionErrorCode;

const PLUGIN_NAME: &str = "ExtProc";

fn has_future_response_phase(mode: &ProcessingMode) -> bool {
    mode.response_header_mode.is_send() || mode.response_body_mode.is_streamed() || mode.response_trailer_mode.is_send()
}

fn has_future_after_response_headers(mode: &ProcessingMode) -> bool {
    mode.response_body_mode.is_streamed() || mode.response_trailer_mode.is_send()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ResponseTerminalPlan {
    header_last: bool,
    install_body: bool,
    install_trailer: bool,
}

fn response_terminal_plan(mode: &ProcessingMode, header_end_of_stream: bool) -> ResponseTerminalPlan {
    ResponseTerminalPlan {
        header_last: header_end_of_stream || !has_future_after_response_headers(mode),
        install_body: !header_end_of_stream && mode.response_body_mode.is_streamed(),
        install_trailer: !header_end_of_stream && mode.response_trailer_mode.is_send(),
    }
}

pub(super) fn exchange_failure_reason(status: &tonic::Status) -> &'static str {
    if status.code() == tonic::Code::FailedPrecondition {
        "bad verdict"
    } else {
        "unreachable"
    }
}

#[derive(Clone)]
pub struct ExtProc {
    name: String,
    identity: String,
    client: ExtProcClient,
    mode: ProcessingMode,
    failure_mode_allow: bool,
    disable_immediate_response: bool,
    allow_mode_override: bool,
    allowed_override_modes: Arc<[ProcessingMode]>,
}

impl ExtProc {
    fn build_with_identity(config: &ExtProcConfig, identity: String) -> Option<Self> {
        match ExtProcClient::new(config) {
            Ok(client) => Some(ExtProc {
                name: PLUGIN_NAME.to_string(),
                identity,
                client,
                mode: config.processing_mode.clone(),
                failure_mode_allow: config.failure_mode_allow,
                disable_immediate_response: config.disable_immediate_response,
                allow_mode_override: config.allow_mode_override,
                allowed_override_modes: config
                    .allowed_override_modes
                    .iter()
                    .map(ProcessingMode::normalized_override)
                    .collect(),
            }),
            Err(e) => {
                // Validation already gated the config, so a build failure here is
                // unexpected; fail closed by declining to construct the plugin.
                tracing::error!(error = %e, "ext_proc client build failed; plugin disabled (fail-closed)");
                None
            }
        }
    }

    #[cfg(test)]
    fn build(config: &ExtProcConfig) -> Option<Self> {
        Self::build_with_identity(config, "test/requestPlugins/#0".to_string())
    }

    /// Compile one request entry into its request owner and internal response
    /// companion. Both carry the same immutable identity and target runtime.
    pub fn create_attached(
        config: &ExtProcConfig,
        identity: String,
    ) -> Option<(Box<dyn RequestFilter>, Box<dyn UpstreamResponse>)> {
        Self::build_with_identity(config, identity).map(|plugin| {
            let companion = plugin.clone();
            (
                Box::new(plugin) as Box<dyn RequestFilter>,
                Box::new(companion) as Box<dyn UpstreamResponse>,
            )
        })
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
                body: None,
            }
        }
    }

    fn fail_request_owner(
        &self,
        owner: &AttachedExtProc,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        reason: &str,
        detail: Option<&str>,
    ) -> PluginRunningResult {
        owner.mark_terminal(
            if self.failure_mode_allow {
                "fail-open"
            } else {
                "fail-close"
            },
            if reason == "bad verdict" {
                "bad-verdict"
            } else {
                "request-stage-failure"
            },
        );
        self.fail_mode(session, log, reason, detail)
    }

    async fn handle_request_immediate(
        &self,
        owner: &AttachedExtProc,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        imm: ImmediateResponse,
    ) -> PluginRunningResult {
        match immediate_action(imm, self.disable_immediate_response) {
            Ok(ImmediateAction::Suppress) => {
                if let Err(status) = owner.finish_success("immediate-disabled").await {
                    return self.fail_request_owner(
                        owner,
                        session,
                        log,
                        exchange_failure_reason(&status),
                        Some(status.code().description()),
                    );
                }
                owner.mark_terminal("suppressed", "immediate-disabled");
                PluginRunningResult::GoodNext
            }
            Ok(ImmediateAction::Reply(imm)) => {
                owner.mark_terminal("immediate-response", "request-immediate");
                immediate_reply(session, log, imm)
            }
            Err(detail) => self.fail_request_owner(owner, session, log, "bad verdict", Some(detail)),
        }
    }

    async fn handle_response_header_immediate(
        &self,
        owner: &AttachedExtProc,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        imm: ImmediateResponse,
    ) -> PluginRunningResult {
        match immediate_action(imm, self.disable_immediate_response) {
            Ok(ImmediateAction::Suppress) => {
                if let Err(status) = owner.finish_success("immediate-disabled").await {
                    owner.mark_terminal(
                        if self.failure_mode_allow {
                            "fail-open"
                        } else {
                            "fail-close"
                        },
                        "response-header-dependency",
                    );
                    return self.fail_mode(
                        session,
                        log,
                        exchange_failure_reason(&status),
                        Some(status.code().description()),
                    );
                }
                owner.mark_terminal("suppressed", "immediate-disabled");
                PluginRunningResult::GoodNext
            }
            Ok(ImmediateAction::Reply(imm)) => {
                owner.mark_terminal("immediate-response", "response-header-immediate");
                immediate_reply(session, log, imm)
            }
            Err(detail) => {
                owner.mark_terminal(
                    if self.failure_mode_allow {
                        "fail-open"
                    } else {
                        "fail-close"
                    },
                    "response-header-invalid-verdict",
                );
                self.fail_mode(session, log, "bad verdict", Some(detail))
            }
        }
    }

    fn fail_response_processor_install(
        &self,
        owner: &AttachedExtProc,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        owner.mark_terminal(
            if self.failure_mode_allow {
                "fail-open"
            } else {
                "fail-close"
            },
            "response-processor-install",
        );
        self.fail_mode(session, log, "late response processor", None)
    }

    async fn finish_request_owner_success(
        &self,
        owner: &AttachedExtProc,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        reason: &'static str,
    ) -> Result<(), PluginRunningResult> {
        owner.finish_success(reason).await.map_err(|status| {
            self.fail_request_owner(
                owner,
                session,
                log,
                exchange_failure_reason(&status),
                Some(status.code().description()),
            )
        })
    }

    async fn apply_request_common_response(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        owner: Option<&AttachedExtProc>,
        mut common: Option<CommonResponse>,
        apply_body_mutation: bool,
    ) -> Result<(), PluginRunningResult> {
        let Some(common) = common.as_mut() else {
            return Err(match owner {
                Some(owner) => {
                    self.fail_request_owner(owner, session, log, "bad verdict", Some("missing common response"))
                }
                None => self.fail_mode(session, log, "bad verdict", Some("missing common response")),
            });
        };
        if let Err(detail) = validate_request_common_response(common, apply_body_mutation) {
            return Err(match owner {
                Some(owner) => self.fail_request_owner(owner, session, log, "bad verdict", Some(detail)),
                None => self.fail_mode(session, log, "bad verdict", Some(detail)),
            });
        }
        if apply_body_mutation {
            if let Some(mutation) = common.body_mutation.take() {
                let body = match mutation.mutation {
                    Some(body_mutation::Mutation::Body(body)) => Some(Bytes::from(body)),
                    Some(body_mutation::Mutation::ClearBody(true)) => Some(Bytes::new()),
                    Some(body_mutation::Mutation::ClearBody(false)) | None => None,
                };
                if let Some(body) = body {
                    if session.overwrite_request_body(body).await.is_err() {
                        return Err(match owner {
                            Some(owner) => self.fail_request_owner(owner, session, log, "body mutation failed", None),
                            None => self.fail_mode(session, log, "body mutation failed", None),
                        });
                    }
                }
            }
        }
        if let Some(headers) = common.header_mutation.as_ref() {
            let count = mutation::apply_to_request(session, headers, log);
            log.push_fmt("req hdr mut {}", la![count.to_string().as_str()]);
        }
        Ok(())
    }

    /// `requestBodyMode: STREAMED` request phase: open the request-scoped
    /// stream, run the header phase on it, then install the per-chunk body
    /// handler. Installing a handler makes the request non-replayable and
    /// switches upstream framing to the streamed disposition.
    async fn run_request_streamed(
        &self,
        owner: Arc<AttachedExtProc>,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        // Gate on the latched transport fact, NOT `declares_body()`: the
        // latter infers from Content-Length, which an H2 client can
        // contradict (declare CL:0, still send DATA frames) to smuggle body
        // bytes past inspection. `None` = the session type cannot supply the
        // fact (custom-connector, subrequest) — the per-chunk capability is
        // absent there and the fail mode applies.
        let headers_end_stream = match session.request_headers_end_stream() {
            Some(fact) => fact,
            None => return self.fail_request_owner(&owner, session, log, "streamed unsupported", None),
        };

        // Transport-guaranteed bodyless request: nothing to stream. Degenerate
        // to the header-only exchange so retry/framing semantics stay
        // untouched.
        if headers_end_stream {
            if !self.mode.request_header_mode.is_send() {
                if !has_future_response_phase(&self.mode) {
                    owner.mark_terminal("success", "no-enabled-phases");
                }
                return PluginRunningResult::GoodNext;
            }
            let request = ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(HttpHeaders {
                    headers: Some(mutation::collect_request_headers(session)),
                    end_of_stream: true,
                })),
            };
            let last = !has_future_response_phase(&self.mode);
            let response = match owner
                .exchange_suppressible_immediate(request, last, !self.disable_immediate_response)
                .await
            {
                Ok(response) => response,
                Err(status) => {
                    let reason = exchange_failure_reason(&status);
                    return self.fail_request_owner(&owner, session, log, reason, Some(status.code().description()));
                }
            };
            let mode_override = response.mode_override;
            return match response.response {
                Some(processing_response::Response::RequestHeaders(hr)) => {
                    // Same rejection as the non-degenerate streamed branch:
                    // CONTINUE_AND_REPLACE has no buffered body to replace in
                    // streamed mode — never silently ignore it.
                    if hr.response.as_ref().is_some_and(|common| {
                        ResponseStatus::try_from(common.status).ok() == Some(ResponseStatus::ContinueAndReplace)
                    }) {
                        return self.fail_request_owner(
                            &owner,
                            session,
                            log,
                            "bad verdict",
                            Some("replace unsupported in streamed"),
                        );
                    }
                    let Some(common) = hr.response.as_ref() else {
                        return self.fail_request_owner(
                            &owner,
                            session,
                            log,
                            "bad verdict",
                            Some("missing common response"),
                        );
                    };
                    let effective = match validate_request_header_verdict(&owner, common, false, mode_override.as_ref())
                    {
                        Ok(mode) => mode,
                        Err(detail) => {
                            return self.fail_request_owner(&owner, session, log, "bad verdict", Some(detail));
                        }
                    };
                    match self
                        .apply_request_common_response(session, log, Some(owner.as_ref()), hr.response, false)
                        .await
                    {
                        Ok(()) => {
                            if !has_future_response_phase(&effective) {
                                if let Err(result) = self
                                    .finish_request_owner_success(&owner, session, log, "mode-override-eos")
                                    .await
                                {
                                    return result;
                                }
                            }
                            PluginRunningResult::GoodNext
                        }
                        Err(result) => result,
                    }
                }
                Some(processing_response::Response::ImmediateResponse(imm)) => {
                    self.handle_request_immediate(&owner, session, log, imm).await
                }
                _ => self.fail_request_owner(&owner, session, log, "bad verdict", None),
            };
        }

        if self.mode.request_header_mode.is_send() {
            let request = ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(HttpHeaders {
                    headers: Some(mutation::collect_request_headers(session)),
                    end_of_stream: false,
                })),
            };
            let response = match owner
                .exchange_suppressible_immediate(request, false, !self.disable_immediate_response)
                .await
            {
                Ok(response) => response,
                Err(status) => {
                    let reason = exchange_failure_reason(&status);
                    return self.fail_request_owner(&owner, session, log, reason, Some(status.code().description()));
                }
            };
            let mode_override = response.mode_override;
            match response.response {
                Some(processing_response::Response::RequestHeaders(hr)) => {
                    // CONTINUE_AND_REPLACE cannot apply in streamed mode: there
                    // is no cached body to replace and the chunk stream has not
                    // started yet.
                    if hr.response.as_ref().is_some_and(|common| {
                        ResponseStatus::try_from(common.status).ok() == Some(ResponseStatus::ContinueAndReplace)
                    }) {
                        return self.fail_request_owner(
                            &owner,
                            session,
                            log,
                            "bad verdict",
                            Some("replace unsupported in streamed"),
                        );
                    }
                    let Some(common) = hr.response.as_ref() else {
                        return self.fail_request_owner(
                            &owner,
                            session,
                            log,
                            "bad verdict",
                            Some("missing common response"),
                        );
                    };
                    if let Err(detail) = validate_request_header_verdict(&owner, common, false, mode_override.as_ref())
                    {
                        return self.fail_request_owner(&owner, session, log, "bad verdict", Some(detail));
                    }
                    if let Err(result) = self
                        .apply_request_common_response(session, log, Some(owner.as_ref()), hr.response, false)
                        .await
                    {
                        return result;
                    }
                }
                Some(processing_response::Response::ImmediateResponse(imm)) => {
                    return self.handle_request_immediate(&owner, session, log, imm).await;
                }
                _ => return self.fail_request_owner(&owner, session, log, "bad verdict", None),
            }
        }

        let effective = match owner.effective_mode() {
            Ok(mode) => mode,
            Err(detail) => return self.fail_request_owner(&owner, session, log, "bad verdict", Some(detail)),
        };
        if !effective.request_body_mode.is_streamed() {
            if !has_future_response_phase(&effective) {
                if let Err(result) = self
                    .finish_request_owner_success(&owner, session, log, "mode-override-eos")
                    .await
                {
                    return result;
                }
            }
            return PluginRunningResult::GoodNext;
        }

        let handler = super::streamed::ExtProcStreamedBodyHandler::new(
            Box::new(Arc::clone(&owner)),
            self.failure_mode_allow,
            self.disable_immediate_response,
            session.current_dye_plan(),
            !has_future_response_phase(&effective),
        );
        if !session.install_request_body_stream_handler(Box::new(handler)) {
            return self.fail_request_owner(&owner, session, log, "streamed unsupported", None);
        }
        log.push("ext_proc body streamed");
        PluginRunningResult::GoodNext
    }

    fn apply_response_headers_response(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        common: Option<CommonResponse>,
    ) -> PluginRunningResult {
        let Some(common) = common else {
            return self.fail_mode(session, log, "bad verdict", Some("missing common response"));
        };
        if let Err(detail) = validate_response_headers_common(&common) {
            return self.fail_mode(session, log, "bad verdict", Some(detail));
        }
        if let Some(mutation) = common.header_mutation.as_ref() {
            let count = mutation::apply_to_response(session, mutation, log);
            log.push_fmt("resp hdr mut {}", la![count.to_string().as_str()]);
        }
        PluginRunningResult::GoodNext
    }
}

fn validate_request_common_response(common: &CommonResponse, apply_body_mutation: bool) -> Result<(), &'static str> {
    let status = ResponseStatus::try_from(common.status).ok();
    if has_unsupported_common_fields(common) {
        return Err("unsupported common response fields");
    }
    if status != Some(ResponseStatus::Continue)
        && !(apply_body_mutation && status == Some(ResponseStatus::ContinueAndReplace))
    {
        return Err("invalid response status");
    }
    if common
        .body_mutation
        .as_ref()
        .is_some_and(|mutation| mutation.mutation.is_none())
    {
        return Err("malformed body mutation");
    }
    Ok(())
}

fn validate_request_header_verdict(
    owner: &AttachedExtProc,
    common: &CommonResponse,
    apply_body_mutation: bool,
    mode_override: Option<&edgion_common::ext_proc::proto::ProcessingMode>,
) -> Result<ProcessingMode, &'static str> {
    validate_request_common_response(common, apply_body_mutation)?;
    owner.apply_mode_override(OverridePhase::RequestHeaders, mode_override)
}

fn validate_response_header_verdict(
    owner: &AttachedExtProc,
    common: &CommonResponse,
    mode_override: Option<&edgion_common::ext_proc::proto::ProcessingMode>,
) -> Result<ProcessingMode, &'static str> {
    validate_response_headers_common(common)?;
    owner.apply_mode_override(OverridePhase::ResponseHeaders, mode_override)
}

fn validate_response_headers_common(common: &CommonResponse) -> Result<(), &'static str> {
    if has_unsupported_common_fields(common) {
        return Err("unsupported common response fields");
    }
    if ResponseStatus::try_from(common.status).ok() != Some(ResponseStatus::Continue) || common.body_mutation.is_some()
    {
        return Err("invalid response headers verdict");
    }
    Ok(())
}

pub(super) fn has_unsupported_common_fields(common: &CommonResponse) -> bool {
    common.clear_route_cache
        || common
            .trailers
            .as_ref()
            .is_some_and(|trailers| !trailers.headers.is_empty())
}

pub(super) struct ValidatedImmediateResponse {
    pub(super) status: u16,
    pub(super) grpc_status: Option<u16>,
    pub(super) headers: Option<edgion_common::ext_proc::proto::HeaderMutation>,
    pub(super) body: Option<Bytes>,
}

pub(super) enum ImmediateAction {
    Suppress,
    Reply(ValidatedImmediateResponse),
}

/// Validate before any mutation. Suppression deliberately wins over malformed
/// payloads because the disabled message has no semantic effect.
pub(super) fn immediate_action(
    imm: ImmediateResponse,
    disable_immediate_response: bool,
) -> Result<ImmediateAction, &'static str> {
    if disable_immediate_response {
        return Ok(ImmediateAction::Suppress);
    }
    let Some(status) = imm.status.as_ref().map(|status| status.code) else {
        return Err("missing immediate HTTP status");
    };
    if !(200..=599).contains(&status) {
        return Err("non-final immediate HTTP status");
    }
    let grpc_status = imm.grpc_status.map(|status| status.status);
    if grpc_status.is_some_and(|status| status > 16) {
        return Err("invalid immediate gRPC status");
    }
    Ok(ImmediateAction::Reply(ValidatedImmediateResponse {
        status: status as u16,
        grpc_status: grpc_status.map(|status| status as u16),
        headers: imm.headers,
        body: (!imm.body.is_empty()).then(|| Bytes::from(imm.body)),
    }))
}

fn immediate_reply(
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    imm: ValidatedImmediateResponse,
) -> PluginRunningResult {
    if let Some(headers) = imm.headers.as_ref() {
        mutation::queue_for_local_reply(session, headers, log);
    }
    log.push_fmt("ext_proc immediate {}", la![imm.status.to_string().as_str()]);
    if session.is_grpc_request() {
        if let Some(grpc_status) = imm.grpc_status {
            let message = imm.body.as_deref().and_then(|body| std::str::from_utf8(body).ok());
            if imm.body.is_some() && message.is_none() {
                session.record_error(EdgionErrorCode::PluginGrpcMessageBodyNotUtf8);
            }
            return PluginRunningResult::ErrGrpcError {
                grpc_status,
                message: message.map(str::to_owned),
            };
        }
    }
    PluginRunningResult::ErrResponse {
        status: imm.status,
        body: imm.body,
    }
}

#[async_trait]
impl RequestFilter for ExtProc {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let owner = Arc::new(AttachedExtProc::new(
            self.client.clone(),
            self.mode.clone(),
            self.allow_mode_override,
            Arc::clone(&self.allowed_override_modes),
        ));
        if !session.install_attached_ext_proc(
            self.identity.clone(),
            Arc::clone(&owner),
            self.mode.response_body_mode.is_streamed() || self.mode.response_trailer_mode.is_send(),
        ) {
            return self.fail_mode(session, log, "duplicate attachment", None);
        }
        log.set_internal_token(self.identity.clone());
        if self.mode.request_body_mode.is_streamed() {
            return self.run_request_streamed(owner, session, log).await;
        }
        let send_headers = self.mode.request_header_mode.is_send();
        let send_body = self.mode.request_body_mode.is_buffered();
        if !send_headers && !send_body {
            if !has_future_response_phase(&self.mode) {
                owner.mark_terminal("success", "no-enabled-phases");
            }
            return PluginRunningResult::GoodNext;
        }

        let body = if send_body {
            match session.read_request_body(None).await {
                Ok(body) => Some(body),
                Err(_) => return self.fail_request_owner(&owner, session, log, "body unavailable", None),
            }
        } else {
            None
        };
        let mut requests = Vec::with_capacity(2);
        if send_headers {
            requests.push(ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(HttpHeaders {
                    headers: Some(mutation::collect_request_headers(session)),
                    end_of_stream: !send_body,
                })),
            });
        }
        if let Some(body) = body {
            requests.push(ProcessingRequest {
                request: Some(processing_request::Request::RequestBody(HttpBody {
                    body: body.to_vec(),
                    end_of_stream: true,
                })),
            });
        }

        let request_count = requests.len();
        for (index, request) in requests.into_iter().enumerate() {
            let is_request_body = matches!(request.request, Some(processing_request::Request::RequestBody(_)));
            let effective_before = match owner.effective_mode() {
                Ok(mode) => mode,
                Err(detail) => return self.fail_request_owner(&owner, session, log, "bad verdict", Some(detail)),
            };
            if is_request_body && !effective_before.request_body_mode.is_buffered() {
                continue;
            }
            let future_response = has_future_response_phase(&effective_before);
            let last = index + 1 == request_count && !future_response;
            let response = match owner
                .exchange_suppressible_immediate(request, last, !self.disable_immediate_response)
                .await
            {
                Ok(response) => response,
                Err(status) => {
                    let reason = exchange_failure_reason(&status);
                    return self.fail_request_owner(&owner, session, log, reason, Some(status.code().description()));
                }
            };
            let mode_override = response.mode_override;
            match response.response {
                Some(processing_response::Response::RequestHeaders(hr)) => {
                    let stop_request_body = hr.response.as_ref().is_some_and(|common| {
                        ResponseStatus::try_from(common.status).ok() == Some(ResponseStatus::ContinueAndReplace)
                    });
                    let Some(common) = hr.response.as_ref() else {
                        return self.fail_request_owner(
                            &owner,
                            session,
                            log,
                            "bad verdict",
                            Some("missing common response"),
                        );
                    };
                    let effective = match validate_request_header_verdict(
                        &owner,
                        common,
                        stop_request_body,
                        mode_override.as_ref(),
                    ) {
                        Ok(mode) => mode,
                        Err(detail) => {
                            return self.fail_request_owner(&owner, session, log, "bad verdict", Some(detail));
                        }
                    };
                    if let Err(result) = self
                        .apply_request_common_response(
                            session,
                            log,
                            Some(owner.as_ref()),
                            hr.response,
                            stop_request_body,
                        )
                        .await
                    {
                        return result;
                    }
                    // Envoy's CONTINUE_AND_REPLACE at request headers replaces
                    // the buffered body and terminates the request-body
                    // direction. Do not send the original buffered body after
                    // the processor has explicitly stopped that direction.
                    if stop_request_body {
                        if !has_future_response_phase(&effective) {
                            if let Err(result) = self
                                .finish_request_owner_success(&owner, session, log, "request-direction-replaced")
                                .await
                            {
                                return result;
                            }
                        }
                        break;
                    }
                    if !effective.request_body_mode.is_buffered() && !has_future_response_phase(&effective) {
                        if let Err(result) = self
                            .finish_request_owner_success(&owner, session, log, "mode-override-eos")
                            .await
                        {
                            return result;
                        }
                        break;
                    }
                }
                Some(processing_response::Response::RequestBody(br)) => {
                    if let Err(result) = self
                        .apply_request_common_response(session, log, Some(owner.as_ref()), br.response, true)
                        .await
                    {
                        return result;
                    }
                }
                Some(processing_response::Response::ImmediateResponse(imm)) => {
                    return self.handle_request_immediate(&owner, session, log, imm).await;
                }
                _ => return self.fail_request_owner(&owner, session, log, "bad verdict", None),
            }
        }
        PluginRunningResult::GoodNext
    }
}

#[async_trait]
impl UpstreamResponse for ExtProc {
    fn name(&self) -> &str {
        &self.name
    }

    fn suppress_automatic_log(&self) -> bool {
        true
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let Some(owner) = session.attached_ext_proc(&self.identity) else {
            return PluginRunningResult::GoodNext;
        };
        // A request-stage ImmediateResponse deactivates this attachment even
        // when its local reply was disabled. Do not turn that deliberate
        // terminal state into a second exchange and a fail-close response.
        if owner.is_terminal() {
            return PluginRunningResult::GoodNext;
        }
        let effective = match owner.effective_mode() {
            Ok(mode) => mode,
            Err(detail) => {
                owner.mark_terminal(
                    if self.failure_mode_allow {
                        "fail-open"
                    } else {
                        "fail-close"
                    },
                    "mode-override-state",
                );
                return self.fail_mode(session, log, "bad verdict", Some(detail));
            }
        };
        let response_future_before = has_future_after_response_headers(&effective);
        let header_end_of_stream = session.upstream_response_headers_end_stream().unwrap_or(false);
        let initial_plan = response_terminal_plan(&effective, header_end_of_stream);

        if !effective.response_header_mode.is_send() {
            if header_end_of_stream {
                if let Err(failure) = self
                    .finish_request_owner_success(&owner, session, log, "response-header-eos")
                    .await
                {
                    return failure;
                }
                return PluginRunningResult::GoodNext;
            }
            if initial_plan.install_trailer
                && !session.install_upstream_response_trailer(Box::new(
                    super::response_trailer::ExtProcResponseTrailer::new(
                        Arc::clone(&owner),
                        self.failure_mode_allow,
                        self.disable_immediate_response,
                    ),
                ))
            {
                return self.fail_response_processor_install(&owner, session, log);
            }
            if initial_plan.install_body
                && !session.install_attached_response_processor(Box::new(
                    super::response_streamed::ExtProcResponseProcessor::new(
                        Arc::clone(&owner),
                        self.failure_mode_allow,
                        self.disable_immediate_response,
                        effective.response_trailer_mode.is_send(),
                    ),
                ))
            {
                return self.fail_response_processor_install(&owner, session, log);
            }
            return PluginRunningResult::GoodNext;
        }

        let request = ProcessingRequest {
            request: Some(processing_request::Request::ResponseHeaders(HttpHeaders {
                headers: Some(mutation::collect_response_headers(session)),
                end_of_stream: header_end_of_stream,
            })),
        };

        let resp = match owner
            .exchange_suppressible_immediate(request, initial_plan.header_last, !self.disable_immediate_response)
            .await
        {
            Ok(resp) => resp,
            Err(status) => {
                owner.mark_terminal(
                    if self.failure_mode_allow {
                        "fail-open"
                    } else {
                        "fail-close"
                    },
                    "response-header-dependency",
                );
                return self.fail_mode(
                    session,
                    log,
                    exchange_failure_reason(&status),
                    Some(status.code().description()),
                );
            }
        };

        let mode_override = resp.mode_override;
        match resp.response {
            Some(processing_response::Response::ResponseHeaders(hr)) => {
                let Some(common) = hr.response.as_ref() else {
                    owner.mark_terminal(
                        if self.failure_mode_allow {
                            "fail-open"
                        } else {
                            "fail-close"
                        },
                        "response-header-invalid-verdict",
                    );
                    return self.fail_mode(session, log, "bad verdict", Some("missing common response"));
                };
                let effective = match validate_response_header_verdict(&owner, common, mode_override.as_ref()) {
                    Ok(mode) => mode,
                    Err(detail) => {
                        owner.mark_terminal(
                            if self.failure_mode_allow {
                                "fail-open"
                            } else {
                                "fail-close"
                            },
                            "response-header-invalid-verdict",
                        );
                        return self.fail_mode(session, log, "bad verdict", Some(detail));
                    }
                };
                let plan = response_terminal_plan(&effective, header_end_of_stream);
                let result = self.apply_response_headers_response(session, log, hr.response);
                if !matches!(result, PluginRunningResult::GoodNext) {
                    owner.mark_terminal(
                        if self.failure_mode_allow {
                            "fail-open"
                        } else {
                            "fail-close"
                        },
                        "response-header-invalid-verdict",
                    );
                }
                if matches!(result, PluginRunningResult::GoodNext)
                    && plan.install_trailer
                    && !session.install_upstream_response_trailer(Box::new(
                        super::response_trailer::ExtProcResponseTrailer::new(
                            Arc::clone(&owner),
                            self.failure_mode_allow,
                            self.disable_immediate_response,
                        ),
                    ))
                {
                    return self.fail_response_processor_install(&owner, session, log);
                }
                if matches!(result, PluginRunningResult::GoodNext) && plan.install_body {
                    if !session.install_attached_response_processor(Box::new(
                        super::response_streamed::ExtProcResponseProcessor::new(
                            Arc::clone(&owner),
                            self.failure_mode_allow,
                            self.disable_immediate_response,
                            effective.response_trailer_mode.is_send(),
                        ),
                    )) {
                        return self.fail_response_processor_install(&owner, session, log);
                    }
                } else if response_future_before
                    && matches!(result, PluginRunningResult::GoodNext)
                    && (header_end_of_stream || !has_future_after_response_headers(&effective))
                {
                    if let Err(failure) = self
                        .finish_request_owner_success(&owner, session, log, "mode-override-eos")
                        .await
                    {
                        return failure;
                    }
                }
                result
            }
            Some(processing_response::Response::ImmediateResponse(imm)) => {
                self.handle_response_header_immediate(&owner, session, log, imm).await
            }
            _ => {
                owner.mark_terminal(
                    if self.failure_mode_allow {
                        "fail-open"
                    } else {
                        "fail-close"
                    },
                    "response-header-bad-verdict",
                );
                self.fail_mode(session, log, "bad verdict", None)
            }
        }
    }
}

// ========== Tests ==========

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_common::ext_proc::proto::external_processor_server::{ExternalProcessor, ExternalProcessorServer};
    use edgion_common::ext_proc::proto::{HeadersResponse, ProcessingMode as WireProcessingMode, ProcessingResponse};
    use edgion_resources::plugin_configs::{BodySendMode, GrpcServiceConfig, HeaderSendMode};
    use std::sync::Mutex;
    use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};

    #[test]
    fn response_terminal_plan_uses_real_header_eos_and_both_future_phases() {
        let mut mode = ProcessingMode::default();
        mode.response_body_mode = BodySendMode::Streamed;
        mode.response_trailer_mode = HeaderSendMode::Send;
        assert_eq!(
            response_terminal_plan(&mode, false),
            ResponseTerminalPlan {
                header_last: false,
                install_body: true,
                install_trailer: true,
            }
        );
        assert_eq!(
            response_terminal_plan(&mode, true),
            ResponseTerminalPlan {
                header_last: true,
                install_body: false,
                install_trailer: false,
            }
        );

        mode.response_body_mode = BodySendMode::None;
        assert_eq!(
            response_terminal_plan(&mode, false),
            ResponseTerminalPlan {
                header_last: false,
                install_body: false,
                install_trailer: true,
            }
        );
        mode.response_trailer_mode = HeaderSendMode::Skip;
        assert!(response_terminal_plan(&mode, false).header_last);
    }

    #[derive(Clone, Copy)]
    enum LiveOverrideOn {
        RequestHeaders,
        ResponseHeaders,
    }

    struct LiveHalfCloseProcessor {
        override_on: LiveOverrideOn,
        mode_override: WireProcessingMode,
        observed: Mutex<Option<tokio::sync::oneshot::Sender<(usize, bool, bool)>>>,
    }

    #[tonic::async_trait]
    impl ExternalProcessor for LiveHalfCloseProcessor {
        type ProcessStream = ReceiverStream<Result<ProcessingResponse, tonic::Status>>;

        async fn process(
            &self,
            request: tonic::Request<tonic::Streaming<ProcessingRequest>>,
        ) -> Result<tonic::Response<Self::ProcessStream>, tonic::Status> {
            let mut inbound = request.into_inner();
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            let observed = self.observed.lock().unwrap().take();
            let override_on = self.override_on;
            let mode_override = self.mode_override.clone();
            tokio::spawn(async move {
                let mut phases = 0;
                let observation = loop {
                    match inbound.message().await {
                        Ok(Some(message)) => {
                            phases += 1;
                            let (response, attach_override) = match message.request {
                                Some(processing_request::Request::RequestHeaders(_)) => (
                                    processing_response::Response::RequestHeaders(HeadersResponse {
                                        response: Some(CommonResponse::default()),
                                    }),
                                    matches!(override_on, LiveOverrideOn::RequestHeaders),
                                ),
                                Some(processing_request::Request::ResponseHeaders(_)) => (
                                    processing_response::Response::ResponseHeaders(HeadersResponse {
                                        response: Some(CommonResponse::default()),
                                    }),
                                    matches!(override_on, LiveOverrideOn::ResponseHeaders),
                                ),
                                _ => break (phases, false, false),
                            };
                            if tx
                                .send(Ok(ProcessingResponse {
                                    response: Some(response),
                                    mode_override: attach_override.then(|| mode_override.clone()),
                                }))
                                .await
                                .is_err()
                            {
                                break (phases, false, true);
                            }
                        }
                        Ok(None) => break (phases, true, false),
                        Err(_) => break (phases, false, true),
                    }
                };
                drop(tx);
                if let Some(observed) = observed {
                    let _ = observed.send(observation);
                }
            });
            Ok(tonic::Response::new(ReceiverStream::new(rx)))
        }
    }

    async fn spawn_live_half_close_processor(
        override_on: LiveOverrideOn,
        mode_override: WireProcessingMode,
    ) -> (
        std::net::SocketAddr,
        tokio::sync::oneshot::Receiver<(usize, bool, bool)>,
        tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(ExternalProcessorServer::new(LiveHalfCloseProcessor {
                    override_on,
                    mode_override,
                    observed: Mutex::new(Some(observed_tx)),
                }))
                .serve_with_incoming(TcpListenerStream::new(listener)),
        );
        (address, observed_rx, server)
    }

    struct LiveImmediateProcessor {
        observed: Mutex<Option<tokio::sync::oneshot::Sender<(usize, bool, bool)>>>,
    }

    #[tonic::async_trait]
    impl ExternalProcessor for LiveImmediateProcessor {
        type ProcessStream = ReceiverStream<Result<ProcessingResponse, tonic::Status>>;

        async fn process(
            &self,
            request: tonic::Request<tonic::Streaming<ProcessingRequest>>,
        ) -> Result<tonic::Response<Self::ProcessStream>, tonic::Status> {
            let mut inbound = request.into_inner();
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            let observed = self.observed.lock().unwrap().take();
            tokio::spawn(async move {
                let mut phases = 0;
                let mut clean_eof = false;
                let mut cancelled = false;
                while let Ok(Some(message)) = inbound.message().await {
                    phases += 1;
                    let response = match message.request {
                        Some(processing_request::Request::RequestHeaders(_)) => {
                            processing_response::Response::ImmediateResponse(ImmediateResponse {
                                status: Some(edgion_common::ext_proc::proto::HttpStatus { code: 403 }),
                                headers: Some(edgion_common::ext_proc::proto::HeaderMutation {
                                    set_headers: vec![edgion_common::ext_proc::proto::HeaderValueOption {
                                        header: Some(edgion_common::ext_proc::proto::HeaderValue {
                                            key: "x-ext-proc-denied".to_string(),
                                            value: String::new(),
                                            raw_value: b"1".to_vec(),
                                        }),
                                        append_action:
                                            edgion_common::ext_proc::proto::HeaderAppendAction::OverwriteIfExistsOrAdd
                                                as i32,
                                        keep_empty_value: false,
                                    }],
                                    remove_headers: Vec::new(),
                                }),
                                ..Default::default()
                            })
                        }
                        Some(processing_request::Request::ResponseHeaders(_)) => {
                            processing_response::Response::ResponseHeaders(HeadersResponse {
                                response: Some(CommonResponse::default()),
                            })
                        }
                        _ => break,
                    };
                    if tx
                        .send(Ok(ProcessingResponse {
                            response: Some(response),
                            mode_override: None,
                        }))
                        .await
                        .is_err()
                    {
                        cancelled = true;
                        break;
                    }
                }
                if !cancelled {
                    match inbound.message().await {
                        Ok(None) => clean_eof = true,
                        Ok(Some(_)) => phases += 1,
                        Err(_) => cancelled = true,
                    }
                }
                drop(tx);
                if let Some(observed) = observed {
                    let _ = observed.send((phases, clean_eof, cancelled));
                }
            });
            Ok(tonic::Response::new(ReceiverStream::new(rx)))
        }
    }

    async fn spawn_live_immediate_processor() -> (
        std::net::SocketAddr,
        tokio::sync::oneshot::Receiver<(usize, bool, bool)>,
        tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(ExternalProcessorServer::new(LiveImmediateProcessor {
                    observed: Mutex::new(Some(observed_tx)),
                }))
                .serve_with_incoming(TcpListenerStream::new(listener)),
        );
        (address, observed_rx, server)
    }

    fn terminal_reduction(base: &ProcessingMode) -> ProcessingMode {
        ProcessingMode {
            request_header_mode: HeaderSendMode::Skip,
            response_header_mode: base.response_header_mode,
            request_body_mode: BodySendMode::None,
            response_body_mode: BodySendMode::None,
            response_trailer_mode: HeaderSendMode::Skip,
        }
    }

    fn wire_terminal_reduction(response_header_mode: HeaderSendMode) -> WireProcessingMode {
        WireProcessingMode {
            request_header_mode: edgion_common::ext_proc::proto::processing_mode::HeaderSendMode::Skip as i32,
            response_header_mode: match response_header_mode {
                HeaderSendMode::Send => edgion_common::ext_proc::proto::processing_mode::HeaderSendMode::Send as i32,
                HeaderSendMode::Skip => edgion_common::ext_proc::proto::processing_mode::HeaderSendMode::Skip as i32,
            },
            request_body_mode: edgion_common::ext_proc::proto::processing_mode::BodySendMode::None as i32,
            response_body_mode: edgion_common::ext_proc::proto::processing_mode::BodySendMode::None as i32,
            request_trailer_mode: edgion_common::ext_proc::proto::processing_mode::HeaderSendMode::Skip as i32,
            response_trailer_mode: edgion_common::ext_proc::proto::processing_mode::HeaderSendMode::Skip as i32,
        }
    }

    fn expect_empty_request_headers(session: &mut MockPluginSession) {
        session.expect_get_method().return_const("POST".to_string());
        session.expect_get_uri().return_const("/half-close".to_string());
        session.expect_request_header_map().return_const(http::HeaderMap::new());
    }

    async fn assert_clean_half_close(
        path: &str,
        observed: tokio::sync::oneshot::Receiver<(usize, bool, bool)>,
        server: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    ) {
        let (phases, clean_eof, cancelled) = tokio::time::timeout(std::time::Duration::from_secs(2), observed)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(phases, 1, "{path} sent a phase after its dynamic terminal reduction");
        assert!(clean_eof, "{path} did not produce processor-side EOF");
        assert!(!cancelled, "{path} was observed as cancellation");
        server.abort();
    }

    fn streamed_config(failure_mode_allow: bool, header_send: bool) -> ExtProcConfig {
        use edgion_resources::plugin_configs::{BodySendMode, HeaderSendMode};
        let mut config = test_config(failure_mode_allow);
        config.processing_mode.request_body_mode = BodySendMode::Streamed;
        if !header_send {
            config.processing_mode.request_header_mode = HeaderSendMode::Skip;
        }
        config
    }

    /// STREAMED on a session that cannot supply the transport fact
    /// (custom-connector, subrequest, mocks) must fail per policy, before any
    /// network work.
    #[tokio::test]
    async fn streamed_without_transport_fact_fails_per_policy() {
        let ext_proc = ExtProc::build(&streamed_config(false, true)).expect("build must not dial");
        let mut session = MockPluginSession::new();
        session.expect_install_attached_ext_proc().times(1).return_const(true);
        session.expect_request_headers_end_stream().return_const(None);
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "streamed-unsupported")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.run_request(&mut session, &mut log).await;
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    /// SKIP headers + a body-bearing request: the handler installs without any
    /// network exchange, and the request-stage verdict is GoodNext.
    #[tokio::test]
    async fn streamed_skip_headers_installs_handler_without_network() {
        let ext_proc = ExtProc::build(&streamed_config(false, false)).expect("build must not dial");
        let mut session = MockPluginSession::new();
        session.expect_install_attached_ext_proc().times(1).return_const(true);
        session.expect_request_headers_end_stream().return_const(Some(false));
        session.expect_current_dye_plan().return_const(None);
        session
            .expect_install_request_body_stream_handler()
            .times(1)
            .return_const(true);
        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.run_request(&mut session, &mut log).await;
        assert!(matches!(result, PluginRunningResult::GoodNext));
    }

    #[tokio::test]
    async fn constructor_normalizes_allowed_override_modes() {
        let mut config = test_config(false);
        config.processing_mode.response_header_mode = HeaderSendMode::Send;
        config.processing_mode.response_trailer_mode = HeaderSendMode::Send;
        config.allow_mode_override = true;
        let configured = ProcessingMode {
            request_header_mode: HeaderSendMode::Send,
            response_header_mode: HeaderSendMode::Send,
            request_body_mode: BodySendMode::Skip,
            response_body_mode: BodySendMode::Skip,
            response_trailer_mode: HeaderSendMode::Send,
        };
        config.allowed_override_modes = vec![configured.clone()];
        let ext_proc = ExtProc::build(&config).expect("build must not dial");
        assert_eq!(
            ext_proc.allowed_override_modes.as_ref(),
            &[ProcessingMode {
                request_header_mode: HeaderSendMode::Skip,
                response_header_mode: HeaderSendMode::Send,
                request_body_mode: BodySendMode::None,
                response_body_mode: BodySendMode::None,
                response_trailer_mode: HeaderSendMode::Send,
            }]
        );
        assert_eq!(config.allowed_override_modes, vec![configured]);
        assert_eq!(ext_proc.mode, config.processing_mode);
        let companion = ext_proc.clone();
        assert!(Arc::ptr_eq(
            &ext_proc.allowed_override_modes,
            &companion.allowed_override_modes
        ));
    }

    #[tokio::test]
    async fn response_framing_reservation_remains_base_mode_derived_under_override() {
        let mut config = test_config(false);
        config.processing_mode.request_header_mode = HeaderSendMode::Skip;
        config.processing_mode.response_body_mode = BodySendMode::Streamed;
        config.allow_mode_override = true;
        let mut reduction = config.processing_mode.clone();
        reduction.response_body_mode = BodySendMode::None;
        config.allowed_override_modes = vec![reduction];
        let ext_proc = ExtProc::build(&config).expect("build must not dial");

        let mut session = MockPluginSession::new();
        session
            .expect_install_attached_ext_proc()
            .withf(|_, _, reserve_framing| *reserve_framing)
            .times(1)
            .return_const(true);
        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.run_request(&mut session, &mut log).await;

        assert!(matches!(result, PluginRunningResult::GoodNext));
    }

    #[tokio::test]
    async fn dynamic_request_header_reductions_cleanly_half_close_live_streams() {
        for (path, request_body_mode) in [
            ("streamed-request", BodySendMode::Streamed),
            ("buffered-request", BodySendMode::Buffered),
        ] {
            let base = ProcessingMode {
                request_header_mode: HeaderSendMode::Send,
                response_header_mode: HeaderSendMode::Skip,
                request_body_mode,
                response_body_mode: BodySendMode::None,
                response_trailer_mode: HeaderSendMode::Skip,
            };
            let reduction = terminal_reduction(&base);
            let (address, observed, server) = spawn_live_half_close_processor(
                LiveOverrideOn::RequestHeaders,
                wire_terminal_reduction(base.response_header_mode),
            )
            .await;
            let config = ExtProcConfig {
                grpc_service: GrpcServiceConfig {
                    target: address.to_string(),
                    timeout_ms: 2_000,
                    ..ExtProcConfig::default().grpc_service
                },
                processing_mode: base,
                allow_mode_override: true,
                allowed_override_modes: vec![reduction],
                ..Default::default()
            };
            let ext_proc = ExtProc::build(&config).unwrap();
            let mut session = MockPluginSession::new();
            session.expect_install_attached_ext_proc().return_const(true);
            expect_empty_request_headers(&mut session);
            if request_body_mode == BodySendMode::Streamed {
                session.expect_request_headers_end_stream().return_const(Some(false));
            } else {
                session
                    .expect_read_request_body()
                    .returning(|_| Ok(Bytes::from_static(b"prebuilt-buffered-body")));
            }

            let result = ext_proc.run_request(&mut session, &mut PluginLog::new("ExtProc")).await;
            assert!(matches!(result, PluginRunningResult::GoodNext), "{path} failed");
            assert_clean_half_close(path, observed, server).await;
        }
    }

    #[tokio::test]
    async fn dynamic_response_header_reduction_cleanly_half_closes_live_stream() {
        let base = ProcessingMode {
            request_header_mode: HeaderSendMode::Skip,
            response_header_mode: HeaderSendMode::Send,
            request_body_mode: BodySendMode::None,
            response_body_mode: BodySendMode::Streamed,
            response_trailer_mode: HeaderSendMode::Skip,
        };
        let reduction = terminal_reduction(&base);
        let (address, observed, server) = spawn_live_half_close_processor(
            LiveOverrideOn::ResponseHeaders,
            wire_terminal_reduction(base.response_header_mode),
        )
        .await;
        let config = ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target: address.to_string(),
                timeout_ms: 2_000,
                ..ExtProcConfig::default().grpc_service
            },
            processing_mode: base.clone(),
            allow_mode_override: true,
            allowed_override_modes: vec![reduction.clone()],
            ..Default::default()
        };
        let ext_proc = ExtProc::build(&config).unwrap();
        let owner = Arc::new(AttachedExtProc::new(
            ext_proc.client.clone(),
            base,
            true,
            Arc::clone(&ext_proc.allowed_override_modes),
        ));
        let mut session = MockPluginSession::new();
        session
            .expect_attached_ext_proc()
            .return_const(Some(Arc::clone(&owner)));
        session.expect_get_response_status().return_const(Some(200));
        session.expect_response_headers_bytes().return_const(Vec::new());
        session
            .expect_upstream_response_headers_end_stream()
            .return_const(Some(false));

        let result = ext_proc
            .run_upstream_response(&mut session, &mut PluginLog::new("ExtProc"))
            .await;
        assert!(matches!(result, PluginRunningResult::GoodNext));
        assert_clean_half_close("response-body", observed, server).await;
    }

    #[tokio::test]
    async fn disabled_request_header_immediate_deactivates_response_companion_with_clean_eof() {
        let (address, observed, server) = spawn_live_immediate_processor().await;
        let config = ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target: address.to_string(),
                timeout_ms: 2_000,
                ..ExtProcConfig::default().grpc_service
            },
            processing_mode: ProcessingMode {
                request_header_mode: HeaderSendMode::Send,
                response_header_mode: HeaderSendMode::Send,
                ..Default::default()
            },
            disable_immediate_response: true,
            failure_mode_allow: false,
            ..Default::default()
        };
        let ext_proc = ExtProc::build(&config).unwrap();
        let installed = Arc::new(Mutex::new(None));
        let installed_capture = Arc::clone(&installed);
        let mut request_session = MockPluginSession::new();
        request_session.expect_queue_local_reply_header_op().never();
        request_session
            .expect_install_attached_ext_proc()
            .times(1)
            .returning(move |_, owner, _| {
                *installed_capture.lock().unwrap() = Some(owner);
                true
            });
        expect_empty_request_headers(&mut request_session);

        let result = ext_proc
            .run_request(&mut request_session, &mut PluginLog::new("ExtProc"))
            .await;
        assert!(matches!(result, PluginRunningResult::GoodNext));
        let owner = installed.lock().unwrap().clone().expect("request owner installed");
        assert_eq!(owner.test_state(), ("suppressed", "immediate-disabled", true));

        let mut response_session = MockPluginSession::new();
        response_session
            .expect_attached_ext_proc()
            .times(1)
            .return_const(Some(owner));
        let response_result = ext_proc
            .run_upstream_response(&mut response_session, &mut PluginLog::new("ExtProc"))
            .await;
        assert!(matches!(response_result, PluginRunningResult::GoodNext));

        assert_clean_half_close("disabled-immediate", observed, server).await;
    }

    /// The install itself can refuse (capability re-check); the plugin must
    /// then apply its failure policy rather than continue silently.
    #[tokio::test]
    async fn streamed_install_refusal_fails_per_policy() {
        let ext_proc = ExtProc::build(&streamed_config(true, false)).expect("build must not dial");
        let mut session = MockPluginSession::new();
        session.expect_install_attached_ext_proc().times(1).return_const(true);
        session.expect_request_headers_end_stream().return_const(Some(false));
        session.expect_current_dye_plan().return_const(None);
        session
            .expect_install_request_body_stream_handler()
            .times(1)
            .return_const(false);
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "streamed-unsupported")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.run_request(&mut session, &mut log).await;
        assert!(matches!(result, PluginRunningResult::GoodNext));
    }

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

    #[test]
    fn immediate_validation_is_suppression_first_and_preserves_exact_zero() {
        use edgion_common::ext_proc::proto::{GrpcStatus, HttpStatus};

        assert!(matches!(
            immediate_action(ImmediateResponse::default(), true),
            Ok(ImmediateAction::Suppress)
        ));
        assert!(immediate_action(ImmediateResponse::default(), false).is_err());
        assert!(immediate_action(
            ImmediateResponse {
                status: Some(HttpStatus { code: 199 }),
                ..Default::default()
            },
            false,
        )
        .is_err());
        assert!(immediate_action(
            ImmediateResponse {
                status: Some(HttpStatus { code: 403 }),
                grpc_status: Some(GrpcStatus { status: 17 }),
                ..Default::default()
            },
            false,
        )
        .is_err());

        let Ok(ImmediateAction::Reply(valid)) = immediate_action(
            ImmediateResponse {
                status: Some(HttpStatus { code: 403 }),
                grpc_status: Some(GrpcStatus { status: 0 }),
                body: b"exact".to_vec(),
                ..Default::default()
            },
            false,
        ) else {
            panic!("valid exact status must produce a reply");
        };
        assert_eq!(valid.grpc_status, Some(0));
        assert_eq!(valid.body.as_deref(), Some(&b"exact"[..]));
    }

    #[test]
    fn immediate_reply_uses_exact_grpc_result_only_for_canonical_grpc() {
        let mut grpc_session = MockPluginSession::new();
        grpc_session.expect_is_grpc_request().return_const(true);
        let mut log = PluginLog::new("ExtProc");
        let result = immediate_reply(
            &mut grpc_session,
            &mut log,
            ValidatedImmediateResponse {
                status: 403,
                grpc_status: Some(8),
                headers: None,
                body: Some(Bytes::from_static(b"quota")),
            },
        );
        assert_eq!(
            result,
            PluginRunningResult::ErrGrpcError {
                grpc_status: 8,
                message: Some("quota".to_string())
            }
        );

        let mut http_session = MockPluginSession::new();
        http_session.expect_is_grpc_request().return_const(false);
        let mut log = PluginLog::new("ExtProc");
        let result = immediate_reply(
            &mut http_session,
            &mut log,
            ValidatedImmediateResponse {
                status: 403,
                grpc_status: Some(8),
                headers: None,
                body: Some(Bytes::from_static(b"denied")),
            },
        );
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: Some(Bytes::from_static(b"denied"))
            }
        );

        let mut grpc_without_exact = MockPluginSession::new();
        grpc_without_exact.expect_is_grpc_request().return_const(true);
        let mut log = PluginLog::new("ExtProc");
        let result = immediate_reply(
            &mut grpc_without_exact,
            &mut log,
            ValidatedImmediateResponse {
                status: 403,
                grpc_status: None,
                headers: None,
                body: Some(Bytes::from_static(b"mapped")),
            },
        );
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: Some(Bytes::from_static(b"mapped"))
            }
        );
    }

    #[test]
    fn immediate_body_preserves_binary_bytes_and_allocation() {
        let body = vec![0, 0xff, b'a', 0x80];
        let allocation = body.as_ptr();
        let Ok(ImmediateAction::Reply(valid)) = immediate_action(
            ImmediateResponse {
                status: Some(edgion_common::ext_proc::proto::HttpStatus { code: 403 }),
                body,
                ..Default::default()
            },
            false,
        ) else {
            panic!("binary HTTP body must be accepted");
        };
        let body = valid.body.unwrap();
        assert_eq!(body.as_ref(), &[0, 0xff, b'a', 0x80]);
        assert_eq!(body.as_ptr(), allocation);
    }

    #[test]
    fn immediate_reply_binary_http_body_is_unchanged() {
        for exact_status in [None, Some(0), Some(8)] {
            let mut session = MockPluginSession::new();
            session.expect_is_grpc_request().return_const(false);
            session.expect_record_error().never();
            let body = Bytes::from_static(&[0, 0xff, b'a', 0x80]);
            assert_eq!(
                immediate_reply(
                    &mut session,
                    &mut PluginLog::new("ExtProc"),
                    ValidatedImmediateResponse {
                        status: 403,
                        grpc_status: exact_status,
                        headers: None,
                        body: Some(body.clone()),
                    },
                ),
                PluginRunningResult::ErrResponse {
                    status: 403,
                    body: Some(body)
                }
            );
        }
    }

    #[test]
    fn immediate_reply_exact_grpc_invalid_body_records_error_without_message() {
        let mut session = MockPluginSession::new();
        session.expect_is_grpc_request().return_const(true);
        session
            .expect_record_error()
            .withf(|code| *code == EdgionErrorCode::PluginGrpcMessageBodyNotUtf8)
            .times(1)
            .return_const(());
        assert_eq!(
            immediate_reply(
                &mut session,
                &mut PluginLog::new("ExtProc"),
                ValidatedImmediateResponse {
                    status: 403,
                    grpc_status: Some(0),
                    headers: None,
                    body: Some(Bytes::from_static(&[0xff])),
                },
            ),
            PluginRunningResult::ErrGrpcError {
                grpc_status: 0,
                message: None
            }
        );
    }

    #[test]
    fn immediate_reply_exact_grpc_absent_body_preserves_zero() {
        let mut session = MockPluginSession::new();
        session.expect_is_grpc_request().return_const(true);
        session.expect_record_error().never();
        assert_eq!(
            immediate_reply(
                &mut session,
                &mut PluginLog::new("ExtProc"),
                ValidatedImmediateResponse {
                    status: 403,
                    grpc_status: Some(0),
                    headers: None,
                    body: None,
                },
            ),
            PluginRunningResult::ErrGrpcError {
                grpc_status: 0,
                message: None
            }
        );
    }

    #[test]
    fn immediate_reply_queues_mutations_for_fresh_local_reply() {
        use edgion_common::ext_proc::proto::{HeaderAppendAction, HeaderMutation, HeaderValue, HeaderValueOption};

        let mut session = MockPluginSession::new();
        session.expect_is_grpc_request().return_const(false);
        session.expect_set_response_header().never();
        session
            .expect_queue_local_reply_header_op()
            .withf(|op| {
                matches!(
                    op,
                    crate::PendingHeaderOp::Set { name, value }
                        if name == "x-local" && value == "reply"
                )
            })
            .times(1)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new("ExtProc");
        let result = immediate_reply(
            &mut session,
            &mut log,
            ValidatedImmediateResponse {
                status: 403,
                grpc_status: None,
                headers: Some(HeaderMutation {
                    set_headers: vec![HeaderValueOption {
                        header: Some(HeaderValue {
                            key: "x-local".to_string(),
                            value: "reply".to_string(),
                            raw_value: Vec::new(),
                        }),
                        append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
                        keep_empty_value: false,
                    }],
                    remove_headers: Vec::new(),
                }),
                body: None,
            },
        );
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 403, .. }));
    }

    #[tokio::test]
    async fn response_processor_install_failure_terminalizes_owner_per_policy() {
        for (failure_mode_allow, expected_outcome) in [(true, "fail-open"), (false, "fail-close")] {
            let ext_proc = ExtProc::build(&test_config(failure_mode_allow)).expect("build must not dial");
            let owner = AttachedExtProc::test_lazy();
            let mut session = MockPluginSession::new();
            session.expect_record_outcome().times(1).return_const(());
            let mut log = PluginLog::new("ExtProc");
            let result = ext_proc.fail_response_processor_install(&owner, &mut session, &mut log);
            assert_eq!(
                owner.test_state(),
                (expected_outcome, "response-processor-install", true)
            );
            assert_eq!(matches!(result, PluginRunningResult::GoodNext), failure_mode_allow);
        }
    }

    #[tokio::test]
    async fn disabled_immediate_handlers_close_owner_without_mutation_or_failure() {
        let mut config = test_config(false);
        config.disable_immediate_response = true;
        let ext_proc = ExtProc::build(&config).expect("client build must not touch the network");

        let request_owner = AttachedExtProc::test_lazy();
        let mut request_session = MockPluginSession::new();
        let mut request_log = PluginLog::new("ExtProc");
        assert_eq!(
            ext_proc
                .handle_request_immediate(
                    &request_owner,
                    &mut request_session,
                    &mut request_log,
                    ImmediateResponse::default(),
                )
                .await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(request_owner.test_state().0, "suppressed");

        let response_owner = AttachedExtProc::test_lazy();
        let mut response_session = MockPluginSession::new();
        let mut response_log = PluginLog::new("ExtProc");
        assert_eq!(
            ext_proc
                .handle_response_header_immediate(
                    &response_owner,
                    &mut response_session,
                    &mut response_log,
                    ImmediateResponse::default(),
                )
                .await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(response_owner.test_state().0, "suppressed");
    }

    #[tokio::test]
    async fn unsupported_common_fields_are_rejected_before_mutation() {
        use edgion_common::ext_proc::proto::{HeaderMap, HeaderValue};

        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session.expect_record_outcome().times(2).return_const(());
        session.expect_set_request_header_value().never();
        for common in [
            CommonResponse {
                clear_route_cache: true,
                ..Default::default()
            },
            CommonResponse {
                trailers: Some(HeaderMap {
                    headers: vec![HeaderValue {
                        key: "x-late".to_string(),
                        value: "value".to_string(),
                        raw_value: Vec::new(),
                    }],
                }),
                ..Default::default()
            },
        ] {
            let mut log = PluginLog::new("ExtProc");
            assert!(matches!(
                ext_proc
                    .apply_request_common_response(&mut session, &mut log, None, Some(common), true)
                    .await,
                Err(PluginRunningResult::ErrResponse { status: 500, .. })
            ));
        }
    }

    #[tokio::test]
    async fn malformed_header_override_is_rejected_before_header_mutation() {
        use edgion_common::ext_proc::proto::{HeaderMutation, ProcessingMode as WireProcessingMode};
        use edgion_resources::plugin_configs::{BodySendMode, HeaderSendMode};

        let mut config = test_config(false);
        config.processing_mode.request_body_mode = BodySendMode::Buffered;
        config.allow_mode_override = true;
        config.allowed_override_modes = vec![ProcessingMode {
            request_header_mode: HeaderSendMode::Skip,
            request_body_mode: BodySendMode::None,
            ..Default::default()
        }];
        let ext_proc = ExtProc::build(&config).expect("build must not dial");
        let owner = AttachedExtProc::new(
            ext_proc.client.clone(),
            config.processing_mode.clone(),
            true,
            Arc::clone(&ext_proc.allowed_override_modes),
        );
        let before = owner.effective_mode().unwrap();
        let common = CommonResponse {
            header_mutation: Some(HeaderMutation::default()),
            ..Default::default()
        };
        let malformed = WireProcessingMode {
            response_header_mode: 99,
            ..Default::default()
        };

        assert!(validate_request_header_verdict(&owner, &common, false, Some(&malformed)).is_err());
        assert_eq!(owner.effective_mode().unwrap(), before);
    }

    #[tokio::test]
    async fn response_header_common_unsupported_fields_are_rejected_before_mutation() {
        use edgion_common::ext_proc::proto::{HeaderMap, HeaderValue};

        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session.expect_record_outcome().times(2).return_const(());
        session.expect_set_response_header().never();
        for common in [
            CommonResponse {
                clear_route_cache: true,
                ..Default::default()
            },
            CommonResponse {
                trailers: Some(HeaderMap {
                    headers: vec![HeaderValue {
                        key: "x-late".to_string(),
                        value: "value".to_string(),
                        raw_value: Vec::new(),
                    }],
                }),
                ..Default::default()
            },
        ] {
            let mut log = PluginLog::new("ExtProc");
            assert!(matches!(
                ext_proc.apply_response_headers_response(&mut session, &mut log, Some(common)),
                PluginRunningResult::ErrResponse { status: 500, .. }
            ));
        }
    }

    #[tokio::test]
    async fn request_body_response_overwrites_buffered_body() {
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use edgion_common::ext_proc::proto::{body_mutation, BodyMutation, CommonResponse};

        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_overwrite_request_body()
            .withf(|body| body.as_ref() == b"processed")
            .returning(|_| Ok(()));
        let response = CommonResponse {
            body_mutation: Some(BodyMutation {
                mutation: Some(body_mutation::Mutation::Body(b"processed".to_vec())),
            }),
            ..Default::default()
        };
        let mut log = PluginLog::new("ExtProc");
        assert!(ext_proc
            .apply_request_common_response(&mut session, &mut log, None, Some(response), true)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn header_continue_ignores_header_phase_body_mutation() {
        use edgion_common::ext_proc::proto::{body_mutation, BodyMutation, CommonResponse};

        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session.expect_overwrite_request_body().never();
        let response = CommonResponse {
            status: ResponseStatus::Continue as i32,
            body_mutation: Some(BodyMutation {
                mutation: Some(body_mutation::Mutation::Body(b"must-not-apply".to_vec())),
            }),
            ..Default::default()
        };

        let mut log = PluginLog::new("ExtProc");
        assert!(ext_proc
            .apply_request_common_response(&mut session, &mut log, None, Some(response), false)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn request_body_response_applies_header_mutation() {
        use edgion_common::ext_proc::proto::{HeaderAppendAction, HeaderMutation, HeaderValue, HeaderValueOption};

        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "x-body-verdict" && value == "applied")
            .times(1)
            .returning(|_, _| Ok(()));
        let response = CommonResponse {
            header_mutation: Some(HeaderMutation {
                set_headers: vec![HeaderValueOption {
                    header: Some(HeaderValue {
                        key: "x-body-verdict".to_string(),
                        value: "applied".to_string(),
                        raw_value: Vec::new(),
                    }),
                    append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
                    keep_empty_value: false,
                }],
                remove_headers: Vec::new(),
            }),
            ..Default::default()
        };

        let mut log = PluginLog::new("ExtProc");
        assert!(ext_proc
            .apply_request_common_response(&mut session, &mut log, None, Some(response), true)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn request_header_and_buffered_body_reject_missing_unknown_and_malformed_common_verdicts() {
        use edgion_common::ext_proc::proto::BodyMutation;

        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session.expect_record_outcome().times(3).return_const(());
        let cases = [
            None,
            Some(CommonResponse {
                status: 99,
                ..Default::default()
            }),
            Some(CommonResponse {
                body_mutation: Some(BodyMutation { mutation: None }),
                ..Default::default()
            }),
        ];
        for (index, common) in cases.into_iter().enumerate() {
            let mut log = PluginLog::new("ExtProc");
            let result = ext_proc
                .apply_request_common_response(&mut session, &mut log, None, common, index != 0)
                .await;
            assert!(matches!(
                result,
                Err(PluginRunningResult::ErrResponse { status: 500, .. })
            ));
        }
    }

    #[tokio::test]
    async fn response_header_continue_and_replace_uses_failure_policy() {
        let ext_proc = ExtProc::build(&test_config(true)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "bad-verdict")
            .times(1)
            .return_const(());
        let response = CommonResponse {
            status: ResponseStatus::ContinueAndReplace as i32,
            ..Default::default()
        };

        let mut log = PluginLog::new("ExtProc");
        assert!(matches!(
            ext_proc.apply_response_headers_response(&mut session, &mut log, Some(response)),
            PluginRunningResult::GoodNext
        ));
    }

    #[tokio::test]
    async fn request_body_mutation_failure_replaces_final_success_summary_per_policy() {
        use crate::plugins::runtime::traits::session::BodyReadError;
        use edgion_common::ext_proc::proto::{body_mutation, BodyMutation, CommonResponse};

        for (failure_mode_allow, expected_outcome) in [(true, "fail-open"), (false, "fail-close")] {
            let ext_proc =
                ExtProc::build(&test_config(failure_mode_allow)).expect("client build must not touch the network");
            let owner = AttachedExtProc::test_lazy();
            owner.mark_terminal("success", "normal-eos");
            let mut session = MockPluginSession::new();
            session
                .expect_overwrite_request_body()
                .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
            session
                .expect_record_outcome()
                .withf(move |outcome, value| {
                    matches!(outcome, DyeOutcome::FailOpen) == failure_mode_allow && value == "body-mutation-failed"
                })
                .times(1)
                .return_const(());
            let response = CommonResponse {
                body_mutation: Some(BodyMutation {
                    mutation: Some(body_mutation::Mutation::Body(b"oversized".to_vec())),
                }),
                ..Default::default()
            };

            let result = ext_proc
                .apply_request_common_response(
                    &mut session,
                    &mut PluginLog::new("ExtProc"),
                    Some(&owner),
                    Some(response),
                    true,
                )
                .await;
            assert_eq!(owner.test_state(), (expected_outcome, "request-stage-failure", true));
            assert_eq!(
                matches!(&result, Err(PluginRunningResult::GoodNext)),
                failure_mode_allow
            );
            assert_eq!(
                matches!(
                    &result,
                    Err(PluginRunningResult::ErrResponse {
                        status: 500,
                        body: None
                    })
                ),
                !failure_mode_allow
            );
        }
    }

    #[tokio::test]
    async fn request_body_mutation_failure_deactivates_pending_response_per_policy() {
        use crate::plugins::runtime::traits::session::BodyReadError;
        use edgion_common::ext_proc::proto::{
            body_mutation, BodyMutation, CommonResponse, HeaderAppendAction, HeaderMutation, HeaderValue,
            HeaderValueOption,
        };

        for (failure_mode_allow, expected_outcome) in [(true, "fail-open"), (false, "fail-close")] {
            let mut config = test_config(failure_mode_allow);
            config.processing_mode.response_header_mode = HeaderSendMode::Send;
            let ext_proc = ExtProc::build(&config).expect("client build must not touch the network");
            let owner = Arc::new(AttachedExtProc::new(
                ext_proc.client.clone(),
                config.processing_mode.clone(),
                ext_proc.allow_mode_override,
                Arc::clone(&ext_proc.allowed_override_modes),
            ));
            let mut request_session = MockPluginSession::new();
            request_session
                .expect_overwrite_request_body()
                .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
            request_session.expect_set_request_header_value().never();
            request_session
                .expect_record_outcome()
                .withf(move |outcome, value| {
                    matches!(outcome, DyeOutcome::FailOpen) == failure_mode_allow && value == "body-mutation-failed"
                })
                .times(1)
                .return_const(());
            let response = CommonResponse {
                body_mutation: Some(BodyMutation {
                    mutation: Some(body_mutation::Mutation::Body(b"oversized".to_vec())),
                }),
                header_mutation: Some(HeaderMutation {
                    set_headers: vec![HeaderValueOption {
                        header: Some(HeaderValue {
                            key: "x-must-not-apply".to_string(),
                            value: "after-failed-body".to_string(),
                            raw_value: Vec::new(),
                        }),
                        append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
                        keep_empty_value: false,
                    }],
                    remove_headers: Vec::new(),
                }),
                ..Default::default()
            };

            let result = ext_proc
                .apply_request_common_response(
                    &mut request_session,
                    &mut PluginLog::new("ExtProc"),
                    Some(owner.as_ref()),
                    Some(response),
                    true,
                )
                .await;
            assert_eq!(owner.test_state(), (expected_outcome, "request-stage-failure", true));
            assert_eq!(
                matches!(&result, Err(PluginRunningResult::GoodNext)),
                failure_mode_allow
            );
            assert_eq!(
                matches!(
                    &result,
                    Err(PluginRunningResult::ErrResponse {
                        status: 500,
                        body: None
                    })
                ),
                !failure_mode_allow
            );

            let mut response_session = MockPluginSession::new();
            response_session
                .expect_attached_ext_proc()
                .times(1)
                .return_const(Some(Arc::clone(&owner)));
            let response_result = ext_proc
                .run_upstream_response(&mut response_session, &mut PluginLog::new("ExtProc"))
                .await;
            assert!(matches!(response_result, PluginRunningResult::GoodNext));
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
