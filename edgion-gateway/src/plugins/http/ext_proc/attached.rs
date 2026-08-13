//! Request-scoped ownership for one Envoy-compatible Process stream.

use std::sync::Mutex;

use super::client::{ExtProcClient, ExtProcStream};
use edgion_common::ext_proc::proto::{processing_response, ProcessingRequest, ProcessingResponse};

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
}

#[derive(Default)]
struct PhaseTracker {
    last_rank: u8,
    request_headers_seen: bool,
    request_body_eos: bool,
    response_headers_seen: bool,
    response_body_eos: bool,
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
    pub(super) fn new(client: ExtProcClient) -> Self {
        Self {
            client,
            state: Mutex::new(StreamState::Lazy),
            summary: Mutex::new(Summary {
                outcome: "pending",
                reason: "none",
                ..Summary::default()
            }),
            phases: Mutex::new(PhaseTracker::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_lazy() -> Self {
        use edgion_resources::plugin_configs::ExtProcConfig;

        let mut config = ExtProcConfig::default();
        config.grpc_service.target = "logging.example:9000".to_string();
        Self::new(ExtProcClient::new(&config).expect("test target must build lazily"))
    }

    #[cfg(test)]
    pub(crate) fn test_state(&self) -> (&'static str, &'static str, bool) {
        let summary = self.summary.lock().expect("test summary lock");
        let finished = self
            .state
            .lock()
            .is_ok_and(|state| matches!(*state, StreamState::Finished));
        (summary.outcome, summary.reason, finished)
    }

    pub(super) async fn exchange(
        &self,
        request: ProcessingRequest,
        last: bool,
    ) -> Result<ProcessingResponse, tonic::Status> {
        let input_bytes = request_payload_len(&request);
        self.validate_phase(&request)?;
        // Taking the stream before awaiting is cancellation-safe: a cancelled
        // future drops the local stream and its target permit, while the owner
        // remains Failed and can never replay the phase.
        let prior = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| tonic::Status::internal("ext_proc attached state lock poisoned"))?;
            std::mem::replace(&mut *state, StreamState::Failed)
        };
        let mut stream = match prior {
            StreamState::Lazy => self.client.open_stream().await?,
            StreamState::Open(stream) => stream,
            StreamState::Finished => return Err(tonic::Status::failed_precondition("ext_proc transaction finished")),
            StreamState::Failed => return Err(tonic::Status::aborted("ext_proc transaction unavailable")),
        };
        let result = stream.exchange(request, last).await;
        let terminal = result.as_ref().is_ok_and(|response| {
            matches!(
                response.response,
                Some(processing_response::Response::ImmediateResponse(_))
            )
        });
        if let Ok(mut summary) = self.summary.lock() {
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
            } else if terminal {
                summary.reason = "immediate-response";
                "immediate-response"
            } else if last {
                summary.reason = "normal-eos";
                "success"
            } else {
                "pending"
            };
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| tonic::Status::internal("ext_proc attached state lock poisoned"))?;
        *state = if result.is_ok() && !last && !terminal {
            StreamState::Open(stream)
        } else if result.is_ok() {
            StreamState::Finished
        } else {
            StreamState::Failed
        };
        result
    }

    pub(super) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = StreamState::Finished;
        }
        if let Ok(mut summary) = self.summary.lock() {
            if summary.outcome == "pending" {
                summary.outcome = "cancelled";
                summary.reason = "owner-closed";
            }
        }
    }

    pub(super) fn mark_terminal(&self, outcome: &'static str, reason: &'static str) {
        if let Ok(mut state) = self.state.lock() {
            *state = StreamState::Finished;
        }
        if let Ok(mut summary) = self.summary.lock() {
            summary.outcome = outcome;
            summary.reason = reason;
        }
    }

    pub(super) fn release_inflight(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = StreamState::Finished;
        }
    }

    pub(crate) fn terminalize_for_logging(&self) {
        let was_open = if let Ok(mut state) = self.state.lock() {
            if matches!(*state, StreamState::Lazy | StreamState::Open(_)) {
                *state = StreamState::Finished;
                true
            } else {
                false
            }
        } else {
            false
        };
        if was_open || self.summary.lock().is_ok_and(|summary| summary.outcome == "pending") {
            if let Ok(mut summary) = self.summary.lock() {
                if summary.outcome == "pending" {
                    summary.outcome = "cancelled";
                    summary.reason = "logging-fallback";
                }
            }
        }
    }

    fn validate_phase(&self, request: &ProcessingRequest) -> Result<(), tonic::Status> {
        use edgion_common::ext_proc::proto::processing_request::Request;
        let mut phases = self
            .phases
            .lock()
            .map_err(|_| tonic::Status::internal("ext_proc phase state lock poisoned"))?;
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
                if phases.response_body_eos {
                    return Err(tonic::Status::failed_precondition("ext_proc response body after EOS"));
                }
                phases.response_body_eos = body.end_of_stream;
                4
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
        let mut summary = self.summary.lock().ok()?;
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

fn request_payload_len(request: &ProcessingRequest) -> usize {
    use edgion_common::ext_proc::proto::processing_request::Request;
    match request.request.as_ref() {
        Some(Request::RequestBody(body)) | Some(Request::ResponseBody(body)) => body.body.len(),
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
        _ => None,
    };
    match mutation {
        Some(Mutation::Body(body)) => body.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_common::ext_proc::proto::{processing_request, HttpHeaders};
    use edgion_resources::plugin_configs::ExtProcConfig;

    #[tokio::test]
    async fn response_only_owner_stays_lazy_until_a_phase_exchange() {
        let mut config = ExtProcConfig::default();
        config.grpc_service.target = "lazy.example:9000".to_string();
        let owner = AttachedExtProc::new(ExtProcClient::new(&config).unwrap());
        assert!(matches!(*owner.state.lock().unwrap(), StreamState::Lazy));
        owner.terminalize_for_logging();
        let line = owner.take_summary_line().unwrap();
        assert!(line.contains("phases=0"));
        assert!(line.contains("reason=logging-fallback"));
        assert!(
            owner.take_summary_line().is_none(),
            "one owner emits one aggregate summary"
        );
    }

    #[tokio::test]
    async fn duplicate_and_regressed_phases_are_rejected_before_network_work() {
        let mut config = ExtProcConfig::default();
        config.grpc_service.target = "phase.example:9000".to_string();
        let owner = AttachedExtProc::new(ExtProcClient::new(&config).unwrap());
        let headers = ProcessingRequest {
            request: Some(processing_request::Request::ResponseHeaders(HttpHeaders::default())),
        };
        owner.validate_phase(&headers).unwrap();
        assert_eq!(
            owner.validate_phase(&headers).unwrap_err().code(),
            tonic::Code::FailedPrecondition
        );
        let request_headers = ProcessingRequest {
            request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
        };
        assert_eq!(
            owner.validate_phase(&request_headers).unwrap_err().code(),
            tonic::Code::FailedPrecondition
        );
    }
}
