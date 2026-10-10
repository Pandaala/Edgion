//! Request-scoped ownership for one Envoy-compatible Process stream.

use std::sync::Arc;

use super::client::{ExtProcClient, ExtProcStream};
use edgion_common::ext_proc::proto::{processing_mode, processing_response, ProcessingRequest, ProcessingResponse};
use edgion_resources::plugin_configs::{BodySendMode, HeaderSendMode, ProcessingMode as ConfigProcessingMode};
use parking_lot::Mutex;

#[allow(clippy::large_enum_variant)]
enum StreamState {
    Lazy,
    Open(ExtProcStream),
    Finished,
    Failed,
}

/// One non-cloneable logical Process stream shared by request and response
/// phases. The `Arc` clones this owner, never the stream itself.
pub struct AttachedExtProc {
    client: ExtProcClient,
    state: Mutex<StreamState>,
    summary: Mutex<Summary>,
    phases: Mutex<PhaseTracker>,
    effective_mode: Mutex<ConfigProcessingMode>,
    allow_mode_override: bool,
    allowed_override_modes: Arc<[ConfigProcessingMode]>,
}

#[derive(Clone, Copy)]
pub(super) enum OverridePhase {
    RequestHeaders,
    ResponseHeaders,
}

#[derive(Default)]
struct PhaseTracker {
    last_rank: u8,
    request_headers_seen: bool,
    request_body_eos: bool,
    response_headers_seen: bool,
    response_body_eos: bool,
    response_trailers_seen: bool,
}

#[derive(Default)]
struct Summary {
    phases: u32,
    input_bytes: u64,
    output_bytes: u64,
    outcome: &'static str,
    reason: &'static str,
    emitted: bool,
}

impl AttachedExtProc {
    /// Retain the allowed override list already normalized by the plugin constructor.
    pub(super) fn new(
        client: ExtProcClient,
        base_mode: ConfigProcessingMode,
        allow_mode_override: bool,
        allowed_override_modes: Arc<[ConfigProcessingMode]>,
    ) -> Self {
        Self {
            client,
            state: Mutex::new(StreamState::Lazy),
            summary: Mutex::new(Summary {
                outcome: "pending",
                reason: "none",
                ..Summary::default()
            }),
            phases: Mutex::new(PhaseTracker::default()),
            effective_mode: Mutex::new(base_mode.normalized_override()),
            allow_mode_override,
            allowed_override_modes,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_state(&self) -> (&'static str, &'static str, bool) {
        let summary = self.summary.lock();
        let finished = matches!(*self.state.lock(), StreamState::Finished);
        (summary.outcome, summary.reason, finished)
    }

    /// Overrides can only reduce this capability, never grant it from NONE.
    pub(crate) fn consumes_response_body(&self) -> bool {
        self.effective_mode.lock().response_body_mode != BodySendMode::None
    }

    pub(super) fn effective_mode(&self) -> Result<ConfigProcessingMode, &'static str> {
        Ok(self.effective_mode.lock().clone())
    }

    pub(super) fn is_terminal(&self) -> bool {
        matches!(*self.state.lock(), StreamState::Finished | StreamState::Failed)
    }

    /// Apply a header-phase override atomically. Disabled, unlisted, late, or
    /// capability-expanding modes leave the effective mode unchanged. Unknown
    /// enum values are malformed and must run through the ordinary failure
    /// policy before any response mutation is applied.
    pub(super) fn apply_mode_override(
        &self,
        phase: OverridePhase,
        wire: Option<&edgion_common::ext_proc::proto::ProcessingMode>,
    ) -> Result<ConfigProcessingMode, &'static str> {
        let mut current = self.effective_mode.lock();
        let Some(wire) = wire else {
            return Ok(current.clone());
        };
        if !self.allow_mode_override {
            return Ok(current.clone());
        }

        let request_header = parse_header_mode(wire.request_header_mode)?;
        let response_header = parse_header_mode(wire.response_header_mode)?;
        let request_trailer = parse_header_mode(wire.request_trailer_mode)?;
        let response_trailer = parse_header_mode(wire.response_trailer_mode)?;
        let request_body = parse_body_mode(wire.request_body_mode)?;
        let response_body = parse_body_mode(wire.response_body_mode)?;

        // Parse every official field before deciding support. Request trailers
        // and unsupported official body modes are valid wire values but cannot
        // grant a capability in this slice, so they are safely ignored.
        let _ = request_header;
        if matches!(
            resolve_header(request_trailer, HeaderSendMode::Skip),
            HeaderSendMode::Send
        ) {
            return Ok(current.clone());
        }
        let Some(request_body) = supported_request_body(request_body) else {
            return Ok(current.clone());
        };
        let Some(response_body) = supported_response_body(response_body) else {
            return Ok(current.clone());
        };
        let requested = ConfigProcessingMode {
            request_header_mode: HeaderSendMode::Skip,
            response_header_mode: resolve_header(response_header, current.response_header_mode),
            request_body_mode: request_body,
            response_body_mode: response_body,
            response_trailer_mode: resolve_header(response_trailer, current.response_trailer_mode),
        }
        .normalized_override();

        if !requested.is_reduction_of(&current) || !self.allowed_override_modes.contains(&requested) {
            return Ok(current.clone());
        }
        match phase {
            OverridePhase::RequestHeaders => *current = requested,
            OverridePhase::ResponseHeaders => {
                current.response_body_mode = requested.response_body_mode;
                current.response_trailer_mode = requested.response_trailer_mode;
            }
        }
        Ok(current.clone())
    }

    pub(super) async fn exchange(
        &self,
        request: ProcessingRequest,
        last: bool,
    ) -> Result<ProcessingResponse, tonic::Status> {
        self.exchange_with_immediate_policy(request, last, true).await
    }

    /// Header phases may suppress an `ImmediateResponse` by configuration. In
    /// that case the caller needs to retain the stream long enough to perform
    /// a clean half-close instead of dropping it as a terminal cancellation.
    pub(super) async fn exchange_suppressible_immediate(
        &self,
        request: ProcessingRequest,
        last: bool,
        immediate_is_terminal: bool,
    ) -> Result<ProcessingResponse, tonic::Status> {
        self.exchange_with_immediate_policy(request, last, immediate_is_terminal)
            .await
    }

    async fn exchange_with_immediate_policy(
        &self,
        request: ProcessingRequest,
        last: bool,
        immediate_is_terminal: bool,
    ) -> Result<ProcessingResponse, tonic::Status> {
        let input_bytes = request_payload_len(&request);
        self.validate_phase(&request)?;
        // Taking the stream before awaiting is cancellation-safe: a cancelled
        // future drops the local stream and its target permit, while the owner
        // remains Failed and can never replay the phase.
        let prior = {
            let mut state = self.state.lock();
            std::mem::replace(&mut *state, StreamState::Failed)
        };
        let mut stream = match prior {
            StreamState::Lazy => self.client.open_stream().await?,
            StreamState::Open(stream) => stream,
            StreamState::Finished => return Err(tonic::Status::failed_precondition("ext_proc transaction finished")),
            StreamState::Failed => return Err(tonic::Status::aborted("ext_proc transaction unavailable")),
        };
        let result = stream.exchange(request, last).await;
        let immediate = result.as_ref().is_ok_and(|response| {
            matches!(
                response.response,
                Some(processing_response::Response::ImmediateResponse(_))
            )
        });
        {
            let mut summary = self.summary.lock();
            summary.phases = summary.phases.saturating_add(1);
            summary.input_bytes = summary.input_bytes.saturating_add(input_bytes as u64);
            if let Ok(response) = &result {
                summary.output_bytes = summary
                    .output_bytes
                    .saturating_add(response_payload_len(response) as u64);
            }
            summary.outcome = if result.is_err() {
                summary.reason = "dependency";
                "dependency-failure"
            } else if immediate {
                summary.reason = "immediate-response";
                "immediate-response"
            } else if last {
                summary.reason = "normal-eos";
                "success"
            } else {
                "pending"
            };
        }
        let mut state = self.state.lock();
        *state = if result.is_ok() && !last && (!immediate || !immediate_is_terminal) {
            StreamState::Open(stream)
        } else if result.is_ok() {
            StreamState::Finished
        } else {
            StreamState::Failed
        };
        result
    }

    /// Finish a successful dynamically shortened transaction with a real gRPC
    /// half-close. The stream is moved out before awaiting, so no synchronous
    /// mutex is held while the processor observes EOF and closes its side.
    pub(super) async fn finish_success(&self, reason: &'static str) -> Result<(), tonic::Status> {
        let prior = {
            let mut state = self.state.lock();
            std::mem::replace(&mut *state, StreamState::Failed)
        };
        let result = match prior {
            StreamState::Lazy | StreamState::Finished => Ok(()),
            StreamState::Open(mut stream) => stream.finish_success().await,
            StreamState::Failed => Err(tonic::Status::aborted("ext_proc transaction unavailable")),
        };
        *self.state.lock() = if result.is_ok() {
            StreamState::Finished
        } else {
            StreamState::Failed
        };
        {
            let mut summary = self.summary.lock();
            if result.is_ok() {
                summary.outcome = "success";
                summary.reason = reason;
            } else {
                summary.outcome = "dependency-failure";
                summary.reason = "half-close";
            }
        }
        result
    }

    pub(super) fn close(&self) {
        *self.state.lock() = StreamState::Finished;
        {
            let mut summary = self.summary.lock();
            if summary.outcome == "pending" {
                summary.outcome = "cancelled";
                summary.reason = "owner-closed";
            }
        }
    }

    pub(super) fn mark_terminal(&self, outcome: &'static str, reason: &'static str) {
        *self.state.lock() = StreamState::Finished;
        {
            let mut summary = self.summary.lock();
            summary.outcome = outcome;
            summary.reason = reason;
        }
    }

    pub(super) fn release_inflight(&self) {
        *self.state.lock() = StreamState::Finished;
    }

    pub(crate) fn terminalize_for_logging(&self) {
        let was_open = {
            let mut state = self.state.lock();
            if matches!(*state, StreamState::Lazy | StreamState::Open(_)) {
                *state = StreamState::Finished;
                true
            } else {
                false
            }
        };
        if was_open || self.summary.lock().outcome == "pending" {
            let mut summary = self.summary.lock();
            if summary.outcome == "pending" {
                summary.outcome = "cancelled";
                summary.reason = "logging-fallback";
            }
        }
    }

    #[allow(clippy::result_large_err)]
    fn validate_phase(&self, request: &ProcessingRequest) -> Result<(), tonic::Status> {
        use edgion_common::ext_proc::proto::processing_request::Request;
        let mut phases = self.phases.lock();
        let rank = match request.request.as_ref() {
            Some(Request::RequestHeaders(_)) => {
                if phases.request_headers_seen {
                    return Err(tonic::Status::failed_precondition("duplicate ext_proc request headers"));
                }
                phases.request_headers_seen = true;
                1
            }
            Some(Request::RequestBody(body)) => {
                if phases.request_body_eos {
                    return Err(tonic::Status::failed_precondition("ext_proc request body after EOS"));
                }
                phases.request_body_eos = body.end_of_stream;
                2
            }
            Some(Request::ResponseHeaders(_)) => {
                if phases.response_headers_seen {
                    return Err(tonic::Status::failed_precondition(
                        "duplicate ext_proc response headers",
                    ));
                }
                phases.response_headers_seen = true;
                3
            }
            Some(Request::ResponseBody(body)) => {
                if phases.response_body_eos || phases.response_trailers_seen {
                    return Err(tonic::Status::failed_precondition("ext_proc response body after EOS"));
                }
                phases.response_body_eos = body.end_of_stream;
                4
            }
            Some(Request::ResponseTrailers(_)) => {
                if phases.response_trailers_seen {
                    return Err(tonic::Status::failed_precondition(
                        "duplicate ext_proc response trailers",
                    ));
                }
                if phases.response_body_eos {
                    return Err(tonic::Status::failed_precondition(
                        "ext_proc response trailers after body EOS",
                    ));
                }
                phases.response_trailers_seen = true;
                5
            }
            _ => return Err(tonic::Status::invalid_argument("unsupported ext_proc attached phase")),
        };
        if rank < phases.last_rank {
            return Err(tonic::Status::failed_precondition("out-of-order ext_proc phase"));
        }
        phases.last_rank = rank;
        Ok(())
    }

    pub(crate) fn take_summary_line(&self) -> Option<String> {
        let mut summary = self.summary.lock();
        if summary.emitted {
            return None;
        }
        summary.emitted = true;
        Some(format!(
            "ext_proc attached phases={} input_bytes={} output_bytes={} outcome={} reason={}",
            summary.phases, summary.input_bytes, summary.output_bytes, summary.outcome, summary.reason
        ))
    }
}

fn parse_header_mode(value: i32) -> Result<processing_mode::HeaderSendMode, &'static str> {
    processing_mode::HeaderSendMode::try_from(value).map_err(|_| "invalid mode override header enum")
}

fn parse_body_mode(value: i32) -> Result<processing_mode::BodySendMode, &'static str> {
    processing_mode::BodySendMode::try_from(value).map_err(|_| "invalid mode override body enum")
}

fn resolve_header(mode: processing_mode::HeaderSendMode, inherited: HeaderSendMode) -> HeaderSendMode {
    match mode {
        processing_mode::HeaderSendMode::Default => inherited,
        processing_mode::HeaderSendMode::Send => HeaderSendMode::Send,
        processing_mode::HeaderSendMode::Skip => HeaderSendMode::Skip,
    }
}

fn supported_request_body(mode: processing_mode::BodySendMode) -> Option<BodySendMode> {
    match mode {
        processing_mode::BodySendMode::None => Some(BodySendMode::None),
        processing_mode::BodySendMode::Streamed => Some(BodySendMode::Streamed),
        processing_mode::BodySendMode::Buffered => Some(BodySendMode::Buffered),
        processing_mode::BodySendMode::BufferedPartial
        | processing_mode::BodySendMode::FullDuplexStreamed
        | processing_mode::BodySendMode::Grpc => None,
    }
}

fn supported_response_body(mode: processing_mode::BodySendMode) -> Option<BodySendMode> {
    match mode {
        processing_mode::BodySendMode::None => Some(BodySendMode::None),
        processing_mode::BodySendMode::Streamed => Some(BodySendMode::Streamed),
        processing_mode::BodySendMode::Buffered
        | processing_mode::BodySendMode::BufferedPartial
        | processing_mode::BodySendMode::FullDuplexStreamed
        | processing_mode::BodySendMode::Grpc => None,
    }
}

fn request_payload_len(request: &ProcessingRequest) -> usize {
    use edgion_common::ext_proc::proto::processing_request::Request;
    match request.request.as_ref() {
        Some(Request::RequestBody(body)) | Some(Request::ResponseBody(body)) => body.body.len(),
        Some(Request::RequestTrailers(trailers)) | Some(Request::ResponseTrailers(trailers)) => {
            trailers.trailers.as_ref().map_or(0, |map| {
                map.headers.iter().fold(0usize, |total, header| {
                    total
                        .saturating_add(header.key.len())
                        .saturating_add(if header.raw_value.is_empty() {
                            header.value.len()
                        } else {
                            header.raw_value.len()
                        })
                })
            })
        }
        _ => 0,
    }
}

fn response_payload_len(response: &ProcessingResponse) -> usize {
    use edgion_common::ext_proc::proto::{body_mutation::Mutation, processing_response::Response};
    let mutation = match response.response.as_ref() {
        Some(Response::RequestBody(body)) | Some(Response::ResponseBody(body)) => body
            .response
            .as_ref()
            .and_then(|common| common.body_mutation.as_ref())
            .and_then(|mutation| mutation.mutation.as_ref()),
        Some(Response::ImmediateResponse(response)) => return response.body.len(),
        Some(Response::RequestTrailers(response)) | Some(Response::ResponseTrailers(response)) => {
            return response.header_mutation.as_ref().map_or(0, |mutation| {
                let set = mutation.set_headers.iter().fold(0usize, |total, option| {
                    option.header.as_ref().map_or(total, |header| {
                        total
                            .saturating_add(header.key.len())
                            .saturating_add(if header.raw_value.is_empty() {
                                header.value.len()
                            } else {
                                header.raw_value.len()
                            })
                    })
                });
                mutation
                    .remove_headers
                    .iter()
                    .fold(set, |total, name| total.saturating_add(name.len()))
            });
        }
        _ => None,
    };
    match mutation {
        Some(Mutation::Body(body)) => body.len(),
        _ => 0,
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/ext_proc/attached_test.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::test_body_owner;
