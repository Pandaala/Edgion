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
use edgion_common::link_sys_core::outbound_tls::{GlobalOutboundTlsSnapshot, ValidatedLocalTlsSelection};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::ProcessingMode;
use edgion_resources::resources::refs::plugin_fields::BoundPluginEntry;
use edgion_resources::EdgionErrorCode;
use edgion_resources::EdgionPlugin;

const PLUGIN_NAME: &str = "ExtProc";

#[derive(Clone, Copy)]
enum BufferedRequestPhase {
    Headers,
    Body,
}

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
    pub(crate) fn validate_tls_entry<'a>(
        entry: &'a BoundPluginEntry<'_>,
        defaults: &GlobalOutboundTlsSnapshot,
    ) -> Result<Option<ValidatedLocalTlsSelection<'a>>, String> {
        ExtProcClient::validate_tls_entry(entry, defaults)
    }

    fn build_with_identity(
        entry: &BoundPluginEntry<'_>,
        defaults: &GlobalOutboundTlsSnapshot,
        identity: String,
    ) -> Option<Self> {
        let EdgionPlugin::ExtProc(config) = entry.config() else {
            return None;
        };
        match ExtProcClient::new(entry, defaults) {
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

    /// Compile one request entry into its request owner and internal response
    /// companion. Both carry the same immutable identity and target runtime.
    pub fn create_attached(
        entry: &BoundPluginEntry<'_>,
        defaults: &GlobalOutboundTlsSnapshot,
        identity: String,
    ) -> Option<(Box<dyn RequestFilter>, Box<dyn UpstreamResponse>)> {
        Self::build_with_identity(entry, defaults, identity).map(|plugin| {
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
            log.push_fmt("req hdr mut {}", la![count]);
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
        let Some(headers_end_stream) = session.request_headers_end_stream() else {
            return self.fail_request_owner(&owner, session, log, "streamed unsupported", None);
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
            log.push_fmt("resp hdr mut {}", la![count]);
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
    if !u16::try_from(status).is_ok_and(edgion_resources::http_status::is_valid_final_http_status) {
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
    log.push_fmt("ext_proc immediate {}", la![imm.status]);
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
        for phase in [BufferedRequestPhase::Headers, BufferedRequestPhase::Body] {
            let is_request_body = matches!(phase, BufferedRequestPhase::Body);
            if (is_request_body && !send_body) || (!is_request_body && !send_headers) {
                continue;
            }
            let effective_before = match owner.effective_mode() {
                Ok(mode) => mode,
                Err(detail) => return self.fail_request_owner(&owner, session, log, "bad verdict", Some(detail)),
            };
            if is_request_body && !effective_before.request_body_mode.is_buffered() {
                continue;
            }
            let future_response = has_future_response_phase(&effective_before);
            let last = (is_request_body || !send_body) && !future_response;
            let request = match phase {
                BufferedRequestPhase::Headers => ProcessingRequest {
                    request: Some(processing_request::Request::RequestHeaders(HttpHeaders {
                        headers: Some(mutation::collect_request_headers(session)),
                        end_of_stream: !send_body,
                    })),
                },
                BufferedRequestPhase::Body => {
                    // Keep the captured generation across the headers verdict;
                    // materialize its wire payload only after body admission.
                    let Some(body) = body.as_ref() else {
                        continue;
                    };
                    ProcessingRequest {
                        request: Some(processing_request::Request::RequestBody(HttpBody {
                            body: body.to_vec(),
                            end_of_stream: true,
                        })),
                    }
                }
            };
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
#[path = "../../../../tests/unit/plugins/http/ext_proc/plugin_test.rs"]
mod tests;
