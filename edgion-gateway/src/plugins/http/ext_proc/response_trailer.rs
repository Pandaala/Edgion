//! Atomic ExtProc response-trailer exchange and mutation.
//! Trailer outcomes cannot mutate committed response headers and do not record dye.

use std::sync::Arc;

use async_trait::async_trait;
use http::{HeaderMap, HeaderName, HeaderValue};

use super::attached::AttachedExtProc;
use super::plugin::exchange_failure_reason;
use crate::plugins::runtime::{PluginLog, PluginSession, UpstreamResponseTrailer, UpstreamTrailerResult};
use edgion_common::ext_proc::proto::{
    processing_request, processing_response, HeaderAppendAction, HeaderMutation, HttpTrailers, ProcessingRequest,
    ProcessingResponse,
};

pub(super) const TRAILER_FIELD_CAP: usize = 256;
pub(super) const TRAILER_BYTES_CAP: usize = 64 * 1024;

#[async_trait]
trait TrailerExchange: Send + Sync {
    async fn exchange(&self, request: ProcessingRequest, last: bool) -> Result<ProcessingResponse, tonic::Status>;
    async fn finish_success(&self, reason: &'static str) -> Result<(), tonic::Status>;
    fn terminal(&self, outcome: &'static str, reason: &'static str);
    fn release_inflight(&self);
    fn is_terminal(&self) -> bool;
}

#[async_trait]
impl TrailerExchange for AttachedExtProc {
    async fn exchange(&self, request: ProcessingRequest, last: bool) -> Result<ProcessingResponse, tonic::Status> {
        AttachedExtProc::exchange(self, request, last).await
    }

    async fn finish_success(&self, reason: &'static str) -> Result<(), tonic::Status> {
        AttachedExtProc::finish_success(self, reason).await
    }

    fn terminal(&self, outcome: &'static str, reason: &'static str) {
        self.mark_terminal(outcome, reason);
    }

    fn release_inflight(&self) {
        AttachedExtProc::release_inflight(self);
    }

    fn is_terminal(&self) -> bool {
        AttachedExtProc::is_terminal(self)
    }
}

pub(super) struct ExtProcResponseTrailer {
    owner: Arc<dyn TrailerExchange>,
    failure_mode_allow: bool,
    disable_immediate_response: bool,
    done: bool,
}

impl ExtProcResponseTrailer {
    pub(super) fn new(owner: Arc<AttachedExtProc>, failure_mode_allow: bool, disable_immediate_response: bool) -> Self {
        Self {
            owner,
            failure_mode_allow,
            disable_immediate_response,
            done: false,
        }
    }

    fn finish_failure(
        &mut self,
        _session: &mut dyn PluginSession,
        log: &mut PluginLog,
        reason: &'static str,
    ) -> UpstreamTrailerResult {
        self.done = true;
        if self.failure_mode_allow {
            self.owner.terminal("fail-open", reason);
            log.push("ext_proc trailer fail-open");
            UpstreamTrailerResult::Continue
        } else {
            self.owner.terminal("fail-close", reason);
            log.push("ext_proc trailer fail-close");
            UpstreamTrailerResult::FailClose
        }
    }

    async fn finish_without_trailers(
        &mut self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> UpstreamTrailerResult {
        match self.owner.finish_success("response-no-trailers").await {
            Ok(()) => {
                self.done = true;
                UpstreamTrailerResult::Continue
            }
            Err(status) => {
                let _reason = exchange_failure_reason(&status);
                self.finish_failure(session, log, "dependency")
            }
        }
    }
}

#[async_trait]
impl UpstreamResponseTrailer for ExtProcResponseTrailer {
    fn name(&self) -> &str {
        "ExtProc"
    }

    async fn run_upstream_response_trailer(
        &mut self,
        trailers: Option<&mut HeaderMap>,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> UpstreamTrailerResult {
        if self.done {
            return UpstreamTrailerResult::Continue;
        }
        // A body/header companion may already have deactivated this shared
        // Process owner (fail-open or disabled ImmediateResponse). Its outcome
        // is final for this processor, so the trailer companion must not turn
        // it into a second dependency failure.
        if self.owner.is_terminal() {
            self.done = true;
            return UpstreamTrailerResult::Continue;
        }
        let Some(trailers) = trailers else {
            return self.finish_without_trailers(session, log).await;
        };

        let Ok(observed) = collect_trailers(trailers, session.is_grpc_request()) else {
            return self.finish_failure(session, log, "input-trailers");
        };
        let request = ProcessingRequest {
            request: Some(processing_request::Request::ResponseTrailers(HttpTrailers {
                trailers: Some(observed),
            })),
        };
        let response = match self.owner.exchange(request, true).await {
            Ok(response) => response,
            Err(status) => {
                let _reason = exchange_failure_reason(&status);
                return self.finish_failure(session, log, "dependency");
            }
        };

        if matches!(
            response.response,
            Some(processing_response::Response::ImmediateResponse(_))
        ) && self.disable_immediate_response
        {
            self.done = true;
            self.owner.terminal("suppressed", "immediate-disabled");
            return UpstreamTrailerResult::Continue;
        }

        let Some(processing_response::Response::ResponseTrailers(response)) = response.response else {
            let reason = if matches!(
                response.response,
                Some(processing_response::Response::ImmediateResponse(_))
            ) {
                "post-commit-immediate"
            } else {
                "wrong-verdict"
            };
            return self.finish_failure(session, log, reason);
        };

        let mut candidate = trailers.clone();
        if let Some(mutation) = response.header_mutation.as_ref() {
            if apply_mutation(&mut candidate, trailers, mutation, session).is_err() {
                return self.finish_failure(session, log, "bad-verdict");
            }
        }
        if validate_trailer_map(&candidate, session.is_grpc_request()).is_err() {
            return self.finish_failure(session, log, "bad-verdict");
        }

        if !session.downstream_response_trailers_representable() {
            return self.finish_failure(session, log, "unrepresentable");
        }

        *trailers = candidate;
        self.done = true;
        self.owner.terminal("success", "response-trailers");
        UpstreamTrailerResult::Continue
    }

    fn finalize_incomplete(&mut self, reason: &'static str) {
        if !self.done {
            self.done = true;
            // Cleanup must preserve a decision already settled by another companion.
            if !self.owner.is_terminal() {
                self.owner.terminal("cancelled", reason);
            }
        }
    }

    fn on_terminal_preempted(&mut self, reason: &'static str) {
        if !self.done {
            self.done = true;
            if !self.owner.is_terminal() {
                self.owner.terminal("superseded", reason);
            }
        }
    }

    fn release_inflight(&mut self) {
        if !self.done {
            self.done = true;
            self.owner.release_inflight();
        }
    }
}

fn checked_map_size(map: &HeaderMap) -> Result<usize, ()> {
    let mut count = 0usize;
    let mut bytes = 0usize;
    for (name, value) in map.iter() {
        count = count.checked_add(1).ok_or(())?;
        bytes = bytes.checked_add(name.as_str().len()).ok_or(())?;
        bytes = bytes.checked_add(value.as_bytes().len()).ok_or(())?;
        if count > TRAILER_FIELD_CAP || bytes > TRAILER_BYTES_CAP {
            return Err(());
        }
    }
    Ok(bytes)
}

fn collect_trailers(map: &HeaderMap, grpc: bool) -> Result<edgion_common::ext_proc::proto::HeaderMap, ()> {
    validate_trailer_map(map, grpc)?;
    let headers = map
        .iter()
        .map(|(name, value)| edgion_common::ext_proc::proto::HeaderValue {
            key: name.as_str().to_string(),
            value: String::new(),
            raw_value: value.as_bytes().to_vec(),
        })
        .collect();
    Ok(edgion_common::ext_proc::proto::HeaderMap { headers })
}

fn validate_trailer_map(map: &HeaderMap, grpc: bool) -> Result<(), ()> {
    checked_map_size(map)?;
    for name in map.keys() {
        if is_forbidden_trailer_name(name.as_str()) {
            return Err(());
        }
    }
    let statuses: Vec<_> = map.get_all("grpc-status").iter().collect();
    if statuses.is_empty() {
        return if grpc { Err(()) } else { Ok(()) };
    }
    if statuses.len() != 1 {
        return Err(());
    }
    let status = statuses[0].to_str().map_err(|_| ())?.parse::<u16>().map_err(|_| ())?;
    (status <= 16).then_some(()).ok_or(())
}

/// Trailer-specific union of the ordinary proxy-managed header set and the
/// platform trailer validator. The latter additionally forbids the routing
/// field even on native H2/custom transports.
fn is_forbidden_trailer_name(name: &str) -> bool {
    super::mutation::is_protected_header(name) || name.eq_ignore_ascii_case("host")
}

fn apply_mutation(
    candidate: &mut HeaderMap,
    original: &HeaderMap,
    mutation: &HeaderMutation,
    session: &dyn PluginSession,
) -> Result<(), ()> {
    for option in &mutation.set_headers {
        let header = option.header.as_ref().ok_or(())?;
        let name = HeaderName::from_bytes(header.key.as_bytes()).map_err(|_| ())?;
        if is_forbidden_trailer_name(name.as_str()) {
            return Err(());
        }
        let raw = if !header.raw_value.is_empty() {
            header.raw_value.as_slice()
        } else {
            header.value.as_bytes()
        };
        if raw.is_empty() && !option.keep_empty_value {
            continue;
        }
        let value = HeaderValue::from_bytes(raw).map_err(|_| ())?;
        let action = HeaderAppendAction::try_from(option.append_action).map_err(|_| ())?;
        let exists = candidate.contains_key(&name);
        let introduces_name = !original.contains_key(&name) && !exists;
        if introduces_name && !session.downstream_response_trailer_name_allowed(&name) {
            return Err(());
        }
        match action {
            HeaderAppendAction::AppendIfExistsOrAdd => {
                candidate.append(name, value);
            }
            HeaderAppendAction::OverwriteIfExistsOrAdd => {
                candidate.insert(name, value);
            }
            HeaderAppendAction::AddIfAbsent if !exists => {
                candidate.insert(name, value);
            }
            HeaderAppendAction::OverwriteIfExists if exists => {
                candidate.insert(name, value);
            }
            HeaderAppendAction::AddIfAbsent | HeaderAppendAction::OverwriteIfExists => {}
        }
    }
    for raw_name in &mutation.remove_headers {
        let name = HeaderName::from_bytes(raw_name.as_bytes()).map_err(|_| ())?;
        if is_forbidden_trailer_name(name.as_str()) {
            return Err(());
        }
        candidate.remove(name);
    }
    checked_map_size(candidate)?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/ext_proc/response_trailer_test.rs"]
mod tests;
