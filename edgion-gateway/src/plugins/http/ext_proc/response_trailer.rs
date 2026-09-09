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

        let observed = match collect_trailers(trailers, session.is_grpc_request()) {
            Ok(observed) => observed,
            Err(_) => return self.finish_failure(session, log, "input-trailers"),
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
            self.owner.terminal("cancelled", reason);
        }
    }

    fn on_terminal_preempted(&mut self, reason: &'static str) {
        if !self.done {
            self.done = true;
            self.owner.terminal("superseded", reason);
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
/// platform trailer validator. The latter additionally forbids routing and
/// proxy-authentication fields even on native H2/custom transports.
fn is_forbidden_trailer_name(name: &str) -> bool {
    super::mutation::is_protected_header(name)
        || matches_ignore_ascii_case(name, &["host", "proxy-authenticate", "proxy-authorization"])
}

fn matches_ignore_ascii_case(value: &str, candidates: &[&str]) -> bool {
    candidates.iter().any(|candidate| value.eq_ignore_ascii_case(candidate))
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_common::ext_proc::proto::{
        HeaderValue as WireHeaderValue, HeaderValueOption, ImmediateResponse, TrailersResponse,
    };
    use std::sync::Mutex;

    struct Scripted {
        responses: Mutex<Vec<Result<ProcessingResponse, tonic::Status>>>,
        requests: Mutex<Vec<(ProcessingRequest, bool)>>,
        finishes: Mutex<Vec<&'static str>>,
        terminal: Mutex<Option<(&'static str, &'static str)>>,
        inactive: bool,
    }

    #[async_trait]
    impl TrailerExchange for Scripted {
        async fn exchange(&self, request: ProcessingRequest, last: bool) -> Result<ProcessingResponse, tonic::Status> {
            self.requests.lock().unwrap().push((request, last));
            self.responses.lock().unwrap().remove(0)
        }

        async fn finish_success(&self, reason: &'static str) -> Result<(), tonic::Status> {
            self.finishes.lock().unwrap().push(reason);
            Ok(())
        }

        fn terminal(&self, outcome: &'static str, reason: &'static str) {
            *self.terminal.lock().unwrap() = Some((outcome, reason));
        }

        fn release_inflight(&self) {}

        fn is_terminal(&self) -> bool {
            self.inactive
        }
    }

    fn response(mutation: Option<HeaderMutation>) -> ProcessingResponse {
        ProcessingResponse {
            response: Some(processing_response::Response::ResponseTrailers(TrailersResponse {
                header_mutation: mutation,
            })),
            mode_override: None,
        }
    }

    fn option(name: &str, value: &[u8], action: HeaderAppendAction) -> HeaderValueOption {
        HeaderValueOption {
            header: Some(WireHeaderValue {
                key: name.to_string(),
                value: String::new(),
                raw_value: value.to_vec(),
            }),
            append_action: action as i32,
            keep_empty_value: true,
        }
    }

    fn processor(
        response: Result<ProcessingResponse, tonic::Status>,
        fail_open: bool,
        inactive: bool,
    ) -> (ExtProcResponseTrailer, Arc<Scripted>) {
        let owner = Arc::new(Scripted {
            responses: Mutex::new(vec![response]),
            requests: Mutex::new(Vec::new()),
            finishes: Mutex::new(Vec::new()),
            terminal: Mutex::new(None),
            inactive,
        });
        (
            ExtProcResponseTrailer {
                owner: owner.clone(),
                failure_mode_allow: fail_open,
                disable_immediate_response: false,
                done: false,
            },
            owner,
        )
    }

    fn session(grpc: bool, representable: bool, allow_new: bool) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session.expect_is_grpc_request().return_const(grpc);
        session
            .expect_downstream_response_trailers_representable()
            .return_const(representable);
        session
            .expect_downstream_response_trailer_name_allowed()
            .return_const(allow_new);
        session
    }

    #[tokio::test]
    async fn real_trailers_preserve_duplicates_and_exchange_last() {
        let mutation = HeaderMutation {
            set_headers: vec![option("x-value", b"three", HeaderAppendAction::AppendIfExistsOrAdd)],
            remove_headers: Vec::new(),
        };
        let (mut processor, owner) = processor(Ok(response(Some(mutation))), false, false);
        let mut trailers = HeaderMap::new();
        trailers.append("x-value", HeaderValue::from_static("one"));
        trailers.append("x-value", HeaderValue::from_static("two"));
        let mut session = session(false, true, true);
        session.expect_record_outcome().times(0).return_const(());
        let mut log = PluginLog::new("ExtProc");

        assert_eq!(
            processor
                .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut log)
                .await,
            UpstreamTrailerResult::Continue
        );
        let requests = owner.requests.lock().unwrap();
        assert!(requests[0].1);
        let Some(processing_request::Request::ResponseTrailers(request)) = requests[0].0.request.as_ref() else {
            panic!("expected response trailer request")
        };
        let values: Vec<_> = request
            .trailers
            .as_ref()
            .unwrap()
            .headers
            .iter()
            .map(|header| header.raw_value.as_slice())
            .collect();
        assert_eq!(values, [b"one".as_slice(), b"two".as_slice()]);
        let output: Vec<_> = trailers
            .get_all("x-value")
            .iter()
            .map(|value| value.as_bytes())
            .collect();
        assert_eq!(output, [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()]);
    }

    #[tokio::test]
    async fn mutation_is_atomic_and_preserves_original_on_failure() {
        let mutation = HeaderMutation {
            set_headers: vec![
                option("x-ok", b"changed", HeaderAppendAction::OverwriteIfExistsOrAdd),
                option("content-length", b"4", HeaderAppendAction::OverwriteIfExistsOrAdd),
            ],
            remove_headers: Vec::new(),
        };
        let (mut processor, _) = processor(Ok(response(Some(mutation))), true, false);
        let mut trailers = HeaderMap::new();
        trailers.insert("x-ok", HeaderValue::from_static("original"));
        let original = trailers.clone();
        let mut session = session(false, true, true);
        session.expect_record_outcome().times(0).return_const(());
        let mut log = PluginLog::new("ExtProc");

        assert_eq!(
            processor
                .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut log)
                .await,
            UpstreamTrailerResult::Continue
        );
        assert_eq!(trailers, original);
    }

    #[tokio::test]
    async fn full_forbidden_union_rejects_input_set_and_remove_atomically() {
        for forbidden in [
            "connection",
            "keep-alive",
            "proxy-connection",
            "transfer-encoding",
            "content-length",
            "te",
            "upgrade",
            "trailer",
            "host",
            "proxy-authenticate",
            "proxy-authorization",
        ] {
            let (mut input_processor, owner) = processor(Ok(response(None)), true, false);
            let mut invalid_input = HeaderMap::new();
            invalid_input.insert("x-ok", HeaderValue::from_static("original"));
            invalid_input.insert(
                HeaderName::from_bytes(forbidden.as_bytes()).unwrap(),
                HeaderValue::from_static("blocked"),
            );
            let input_original = invalid_input.clone();
            let mut input_session = session(false, true, true);
            input_session.expect_record_outcome().times(0).return_const(());
            assert_eq!(
                input_processor
                    .run_upstream_response_trailer(
                        Some(&mut invalid_input),
                        &mut input_session,
                        &mut PluginLog::new("ExtProc"),
                    )
                    .await,
                UpstreamTrailerResult::Continue
            );
            assert_eq!(invalid_input, input_original, "forbidden input field {forbidden}");
            assert!(owner.requests.lock().unwrap().is_empty());

            for mutation in [
                HeaderMutation {
                    set_headers: vec![
                        option("x-ok", b"changed", HeaderAppendAction::OverwriteIfExistsOrAdd),
                        option(forbidden, b"blocked", HeaderAppendAction::OverwriteIfExistsOrAdd),
                    ],
                    remove_headers: Vec::new(),
                },
                HeaderMutation {
                    set_headers: vec![option("x-ok", b"changed", HeaderAppendAction::OverwriteIfExistsOrAdd)],
                    remove_headers: vec![forbidden.to_string()],
                },
            ] {
                let (mut processor, _) = processor(Ok(response(Some(mutation))), true, false);
                let mut trailers = HeaderMap::new();
                trailers.insert("x-ok", HeaderValue::from_static("original"));
                let original = trailers.clone();
                let mut session = session(false, true, true);
                session.expect_record_outcome().times(0).return_const(());
                assert_eq!(
                    processor
                        .run_upstream_response_trailer(
                            Some(&mut trailers),
                            &mut session,
                            &mut PluginLog::new("ExtProc"),
                        )
                        .await,
                    UpstreamTrailerResult::Continue
                );
                assert_eq!(trailers, original, "forbidden field {forbidden}");
            }
        }
    }

    #[tokio::test]
    async fn grpc_status_must_remain_exactly_one_valid_value() {
        for mutation in [
            HeaderMutation {
                set_headers: Vec::new(),
                remove_headers: vec!["grpc-status".to_string()],
            },
            HeaderMutation {
                set_headers: vec![option("grpc-status", b"17", HeaderAppendAction::OverwriteIfExistsOrAdd)],
                remove_headers: Vec::new(),
            },
            HeaderMutation {
                set_headers: vec![option("grpc-status", b"1", HeaderAppendAction::AppendIfExistsOrAdd)],
                remove_headers: Vec::new(),
            },
        ] {
            let (mut processor, _) = processor(Ok(response(Some(mutation))), true, false);
            let mut trailers = HeaderMap::new();
            trailers.insert("grpc-status", HeaderValue::from_static("0"));
            let original = trailers.clone();
            let mut session = session(true, true, true);
            session.expect_record_outcome().times(0).return_const(());
            let mut log = PluginLog::new("ExtProc");
            assert_eq!(
                processor
                    .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut log)
                    .await,
                UpstreamTrailerResult::Continue
            );
            assert_eq!(trailers, original);
        }
    }

    #[tokio::test]
    async fn new_name_requires_transport_admission_but_h2_style_admission_succeeds() {
        let mutation = HeaderMutation {
            set_headers: vec![option("x-new", b"value", HeaderAppendAction::OverwriteIfExistsOrAdd)],
            remove_headers: Vec::new(),
        };
        for (allowed, expected) in [(false, None), (true, Some("value"))] {
            let (mut processor, _) = processor(Ok(response(Some(mutation.clone()))), true, false);
            let mut trailers = HeaderMap::new();
            trailers.insert("x-old", HeaderValue::from_static("kept"));
            let mut session = session(false, true, allowed);
            session.expect_record_outcome().times(0).return_const(());
            let mut log = PluginLog::new("ExtProc");
            processor
                .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut log)
                .await;
            assert_eq!(trailers.get("x-new").and_then(|value| value.to_str().ok()), expected);
        }
    }

    #[tokio::test]
    async fn caps_are_inclusive_and_overflow_preserves_map() {
        let exact_value = vec![b'x'; TRAILER_BYTES_CAP - "x".len()];
        let mut exact = HeaderMap::new();
        exact.insert("x", HeaderValue::from_bytes(&exact_value).unwrap());
        assert!(checked_map_size(&exact).is_ok());

        let oversized_value = vec![b'x'; TRAILER_BYTES_CAP];
        let mut oversized = HeaderMap::new();
        oversized.insert("x", HeaderValue::from_bytes(&oversized_value).unwrap());
        let original = oversized.clone();
        let (mut input_processor, owner) = processor(Ok(response(None)), true, false);
        let mut input_session = session(false, true, true);
        input_session.expect_record_outcome().times(0).return_const(());
        let mut log = PluginLog::new("ExtProc");
        input_processor
            .run_upstream_response_trailer(Some(&mut oversized), &mut input_session, &mut log)
            .await;
        assert_eq!(oversized, original);
        assert!(owner.requests.lock().unwrap().is_empty());

        let mut too_many = HeaderMap::new();
        for index in 0..=TRAILER_FIELD_CAP {
            too_many.insert(
                HeaderName::from_bytes(format!("x-{index}").as_bytes()).unwrap(),
                HeaderValue::from_static("v"),
            );
        }
        assert!(checked_map_size(&too_many).is_err());

        let mutation = HeaderMutation {
            set_headers: vec![option(
                "x-large",
                &vec![b'y'; TRAILER_BYTES_CAP],
                HeaderAppendAction::OverwriteIfExistsOrAdd,
            )],
            remove_headers: Vec::new(),
        };
        let (mut processor, _) = processor(Ok(response(Some(mutation))), true, false);
        let mut original_map = HeaderMap::new();
        original_map.insert("x", HeaderValue::from_static("v"));
        let expected = original_map.clone();
        let mut session = session(false, true, true);
        session.expect_record_outcome().times(0).return_const(());
        processor
            .run_upstream_response_trailer(Some(&mut original_map), &mut session, &mut PluginLog::new("ExtProc"))
            .await;
        assert_eq!(original_map, expected);
    }

    #[tokio::test]
    async fn none_cleanly_half_closes_without_fabricating_a_phase() {
        let (mut processor, owner) = processor(Ok(response(None)), false, false);
        let mut session = session(false, true, true);
        assert_eq!(
            processor
                .run_upstream_response_trailer(None, &mut session, &mut PluginLog::new("ExtProc"))
                .await,
            UpstreamTrailerResult::Continue
        );
        assert!(owner.requests.lock().unwrap().is_empty());
        assert_eq!(*owner.finishes.lock().unwrap(), ["response-no-trailers"]);
    }

    #[tokio::test]
    async fn wrong_phase_and_enabled_immediate_follow_post_commit_failure_policy() {
        let wrong = ProcessingResponse {
            response: Some(processing_response::Response::ResponseBody(Default::default())),
            mode_override: None,
        };
        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(
                ImmediateResponse::default(),
            )),
            mode_override: None,
        };
        for verdict in [wrong, immediate] {
            let (mut processor, _) = processor(Ok(verdict), false, false);
            let mut trailers = HeaderMap::new();
            trailers.insert("x", HeaderValue::from_static("v"));
            let mut session = session(false, true, true);
            session.expect_record_outcome().times(0).return_const(());
            assert_eq!(
                processor
                    .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut PluginLog::new("ExtProc"),)
                    .await,
                UpstreamTrailerResult::FailClose
            );
        }
    }

    #[tokio::test]
    async fn disabled_immediate_and_prior_terminal_are_noop() {
        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(
                ImmediateResponse::default(),
            )),
            mode_override: None,
        };
        let (mut trailer_processor, owner) = processor(Ok(immediate), false, false);
        trailer_processor.disable_immediate_response = true;
        let mut trailers = HeaderMap::new();
        trailers.insert("x-value", HeaderValue::from_static("kept"));
        let mut session = session(false, true, true);
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(
            trailer_processor
                .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut log)
                .await,
            UpstreamTrailerResult::Continue
        );
        assert_eq!(
            *owner.terminal.lock().unwrap(),
            Some(("suppressed", "immediate-disabled"))
        );

        let (mut inactive, owner) = processor(Ok(response(None)), false, true);
        inactive
            .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut log)
            .await;
        assert!(owner.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unrepresentable_fail_open_keeps_map_for_later_companions() {
        let (mut processor, _) = processor(Ok(response(None)), true, false);
        let mut trailers = HeaderMap::new();
        trailers.insert("x-value", HeaderValue::from_static("kept"));
        let original = trailers.clone();
        let mut session = session(false, false, true);
        session.expect_record_outcome().times(0).return_const(());
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(
            processor
                .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut log)
                .await,
            UpstreamTrailerResult::Continue
        );
        assert_eq!(trailers, original);
    }
}
