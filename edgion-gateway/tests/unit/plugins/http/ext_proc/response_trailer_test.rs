use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_common::ext_proc::proto::{
    HeaderValue as WireHeaderValue, HeaderValueOption, ImmediateResponse, TrailersResponse,
};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Scripted {
    responses: Mutex<Vec<Result<ProcessingResponse, tonic::Status>>>,
    requests: Mutex<Vec<(ProcessingRequest, bool)>>,
    finishes: Mutex<Vec<&'static str>>,
    terminal: Mutex<Option<(&'static str, &'static str)>>,
    inactive: AtomicBool,
    terminal_writes: AtomicUsize,
    releases: AtomicUsize,
}

#[async_trait]
impl TrailerExchange for Scripted {
    async fn exchange(&self, request: ProcessingRequest, last: bool) -> Result<ProcessingResponse, tonic::Status> {
        self.requests.lock().push((request, last));
        let response = self.responses.lock().remove(0);
        if last && response.is_ok() {
            self.inactive.store(true, Ordering::SeqCst);
            *self.terminal.lock() = Some(("success", "normal-eos"));
        }
        response
    }

    async fn finish_success(&self, reason: &'static str) -> Result<(), tonic::Status> {
        self.finishes.lock().push(reason);
        Ok(())
    }

    fn terminal(&self, outcome: &'static str, reason: &'static str) {
        self.terminal_writes.fetch_add(1, Ordering::SeqCst);
        self.inactive.store(true, Ordering::SeqCst);
        *self.terminal.lock() = Some((outcome, reason));
    }

    fn release_inflight(&self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
        self.inactive.store(true, Ordering::SeqCst);
    }

    fn is_terminal(&self) -> bool {
        self.inactive.load(Ordering::SeqCst)
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
        inactive: AtomicBool::new(inactive),
        terminal_writes: AtomicUsize::new(0),
        releases: AtomicUsize::new(0),
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

fn cleanup(processor: &mut ExtProcResponseTrailer, preempted: bool) {
    if preempted {
        processor.on_terminal_preempted("earlier-trailer-fail-close");
    } else {
        processor.finalize_incomplete("logging-fallback");
    }
}

#[test]
fn cleanup_hooks_preserve_settled_owner_outcomes() {
    for summary in [
        ("fail-close", "dependency"),
        ("superseded", "winner"),
        ("fail-open", "dependency"),
        ("suppressed", "immediate-disabled"),
        ("success", "response-trailers"),
    ] {
        for preempted in [false, true] {
            let (mut processor, owner) = processor(Ok(response(None)), false, true);
            *owner.terminal.lock() = Some(summary);
            cleanup(&mut processor, preempted);
            assert!(processor.done);
            cleanup(&mut processor, preempted);
            cleanup(&mut processor, !preempted);
            processor.release_inflight();
            assert_eq!(*owner.terminal.lock(), Some(summary));
            assert_eq!(owner.terminal_writes.load(Ordering::SeqCst), 0);
            assert_eq!(owner.releases.load(Ordering::SeqCst), 0);
        }
    }
}

#[test]
fn cleanup_hooks_terminalize_active_owner_once() {
    for (preempted, expected) in [
        (false, ("cancelled", "logging-fallback")),
        (true, ("superseded", "earlier-trailer-fail-close")),
    ] {
        let (mut processor, owner) = processor(Ok(response(None)), false, false);
        cleanup(&mut processor, preempted);
        assert!(processor.done);
        assert!(owner.is_terminal());
        cleanup(&mut processor, preempted);
        cleanup(&mut processor, !preempted);
        processor.release_inflight();
        assert_eq!(*owner.terminal.lock(), Some(expected));
        assert_eq!(owner.terminal_writes.load(Ordering::SeqCst), 1);
        assert_eq!(owner.releases.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn cleanup_hooks_preserve_release_only_pending_summary() {
    for preempted in [false, true] {
        let (mut processor, owner) = processor(Ok(response(None)), false, false);
        *owner.terminal.lock() = Some(("pending", "pending"));
        // Another companion releases the owner without finalizing its summary.
        owner.release_inflight();
        cleanup(&mut processor, preempted);
        assert!(processor.done);
        cleanup(&mut processor, !preempted);
        processor.release_inflight();
        assert_eq!(*owner.terminal.lock(), Some(("pending", "pending")));
        assert_eq!(owner.terminal_writes.load(Ordering::SeqCst), 0);
        assert_eq!(owner.releases.load(Ordering::SeqCst), 1);
    }

    let (mut processor, owner) = processor(Ok(response(None)), false, false);
    *owner.terminal.lock() = Some(("pending", "pending"));
    processor.release_inflight();
    processor.release_inflight();
    cleanup(&mut processor, false);
    cleanup(&mut processor, true);
    assert!(processor.done);
    assert!(owner.is_terminal());
    assert_eq!(*owner.terminal.lock(), Some(("pending", "pending")));
    assert_eq!(owner.terminal_writes.load(Ordering::SeqCst), 0);
    assert_eq!(owner.releases.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalid_mutation_corrects_transport_success_for_both_failure_policies() {
    for fail_open in [false, true] {
        let mutation = HeaderMutation {
            set_headers: vec![
                option("x-value", b"changed", HeaderAppendAction::OverwriteIfExistsOrAdd),
                option("content-length", b"4", HeaderAppendAction::OverwriteIfExistsOrAdd),
            ],
            remove_headers: Vec::new(),
        };
        let (mut processor, owner) = processor(Ok(response(Some(mutation))), fail_open, false);
        let mut trailers = HeaderMap::new();
        trailers.insert("x-value", HeaderValue::from_static("original"));
        let original = trailers.clone();
        let mut session = session(false, true, true);
        session.expect_record_outcome().times(0).return_const(());
        let result = processor
            .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut PluginLog::new("ExtProc"))
            .await;
        assert_eq!(
            result,
            if fail_open {
                UpstreamTrailerResult::Continue
            } else {
                UpstreamTrailerResult::FailClose
            }
        );
        assert_eq!(trailers, original);
        assert!(processor.done);
        assert!(owner.is_terminal());
        let requests = owner.requests.lock();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].1);
        drop(requests);
        cleanup(&mut processor, false);
        cleanup(&mut processor, true);
        assert_eq!(
            *owner.terminal.lock(),
            Some((if fail_open { "fail-open" } else { "fail-close" }, "bad-verdict"))
        );
        assert_eq!(owner.terminal_writes.load(Ordering::SeqCst), 1);
    }
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
    let requests = owner.requests.lock();
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
async fn no_mutation_preserves_trailer_storage_and_sensitive_opaque_values() {
    for grpc in [false, true] {
        for fail_open in [false, true] {
            let (mut processor, owner) = processor(Ok(response(None)), fail_open, false);
            let mut trailers = HeaderMap::new();
            let mut opaque = HeaderValue::from_bytes(b"opaque-\x80").unwrap();
            opaque.set_sensitive(true);
            trailers.append("x-value", opaque);
            trailers.append("x-value", HeaderValue::from_static("second"));
            if grpc {
                trailers.insert("grpc-status", HeaderValue::from_static("0"));
            }
            let original = trailers.clone();
            let original_value = std::ptr::from_ref(trailers.get("x-value").unwrap());
            let mut session = session(grpc, true, true);
            session.expect_record_outcome().times(0).return_const(());
            let mut log = PluginLog::new("ExtProc");
            assert_eq!(
                processor
                    .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut log)
                    .await,
                UpstreamTrailerResult::Continue
            );
            assert_eq!(trailers, original);
            assert_eq!(std::ptr::from_ref(trailers.get("x-value").unwrap()), original_value);
            assert!(trailers.get("x-value").unwrap().is_sensitive());
            assert!(processor.done);
            assert_eq!(*owner.terminal.lock(), Some(("success", "response-trailers")));
            let requests = owner.requests.lock();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].1);
            let Some(processing_request::Request::ResponseTrailers(request)) = requests[0].0.request.as_ref() else {
                panic!("expected response trailer request");
            };
            let values: Vec<_> = request
                .trailers
                .as_ref()
                .unwrap()
                .headers
                .iter()
                .filter(|header| header.key == "x-value")
                .map(|header| header.raw_value.as_slice())
                .collect();
            assert_eq!(values, [b"opaque-\x80".as_slice(), b"second".as_slice()]);
        }
    }
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
        assert!(owner.requests.lock().is_empty());

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
                    .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut PluginLog::new("ExtProc"),)
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
    assert!(owner.requests.lock().is_empty());

    let mut too_many = HeaderMap::new();
    for index in 0..=TRAILER_FIELD_CAP {
        too_many.insert(
            HeaderName::from_bytes(format!("x-{index}").as_bytes()).unwrap(),
            HeaderValue::from_static("v"),
        );
    }
    assert!(checked_map_size(&too_many).is_err());

    for field_overflow in [false, true] {
        for fail_open in [false, true] {
            let set_headers = if field_overflow {
                (0..TRAILER_FIELD_CAP)
                    .map(|index| option(&format!("x-{index}"), b"v", HeaderAppendAction::OverwriteIfExistsOrAdd))
                    .collect()
            } else {
                vec![option(
                    "x-large",
                    &vec![b'y'; TRAILER_BYTES_CAP],
                    HeaderAppendAction::OverwriteIfExistsOrAdd,
                )]
            };
            let mutation = HeaderMutation {
                set_headers,
                remove_headers: Vec::new(),
            };
            let (mut processor, owner) = processor(Ok(response(Some(mutation))), fail_open, false);
            let mut original_map = HeaderMap::new();
            original_map.insert("x", HeaderValue::from_static("v"));
            let expected = original_map.clone();
            let mut session = session(false, true, true);
            session.expect_record_outcome().times(0).return_const(());
            let result = processor
                .run_upstream_response_trailer(Some(&mut original_map), &mut session, &mut PluginLog::new("ExtProc"))
                .await;
            assert_eq!(original_map, expected);
            assert_eq!(
                result,
                if fail_open {
                    UpstreamTrailerResult::Continue
                } else {
                    UpstreamTrailerResult::FailClose
                }
            );
            assert_eq!(
                *owner.terminal.lock(),
                Some((if fail_open { "fail-open" } else { "fail-close" }, "bad-verdict"))
            );
            assert!(processor.done);
        }
    }
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
    assert!(owner.requests.lock().is_empty());
    assert_eq!(*owner.finishes.lock(), ["response-no-trailers"]);
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
    assert_eq!(*owner.terminal.lock(), Some(("suppressed", "immediate-disabled")));

    let (mut inactive, owner) = processor(Ok(response(None)), false, true);
    inactive
        .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut log)
        .await;
    assert!(owner.requests.lock().is_empty());
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

#[test]
fn trailer_status_validation_preserves_cardinality_and_numeric_rules() {
    for grpc in [false, true] {
        for (values, valid) in [
            (vec![], !grpc),
            (vec![b"0".as_slice()], true),
            (vec![b"16".as_slice()], true),
            (vec![b"+0".as_slice()], true),
            (vec![b"00".as_slice()], true),
            (vec![b"17".as_slice()], false),
            (vec![b"65536".as_slice()], false),
            (vec![b"-1".as_slice()], false),
            (vec![b" 0".as_slice()], false),
            (vec![b"".as_slice()], false),
            (vec![b"\xff".as_slice()], false),
            (vec![b"0".as_slice(), b"0".as_slice()], false),
            (vec![b"bad".as_slice(), b"0".as_slice()], false),
            (vec![b"0".as_slice(), b"bad".as_slice()], false),
        ] {
            let mut map = HeaderMap::new();
            for value in values {
                map.append("grpc-status", HeaderValue::from_bytes(value).unwrap());
            }
            let (result, allocations) =
                crate::test_allocation_counter::count_allocations(|| validate_trailer_map(&map, grpc));
            assert_eq!(result.is_ok(), valid);
            assert_eq!(allocations, 0);
        }
    }
}

#[test]
fn trailer_status_validation_retains_whole_map_gates() {
    let mut map = HeaderMap::new();
    map.insert("grpc-status", HeaderValue::from_static("0"));
    for index in 1..TRAILER_FIELD_CAP {
        map.insert(
            HeaderName::from_bytes(format!("x-{index}").as_bytes()).unwrap(),
            HeaderValue::from_static("value"),
        );
    }
    assert!(validate_trailer_map(&map, true).is_ok());
    map.insert("x-over-cap", HeaderValue::from_static("value"));
    assert!(validate_trailer_map(&map, true).is_err());
    map.remove("x-over-cap");
    map.remove("x-1");
    map.insert("host", HeaderValue::from_static("example.test"));
    assert!(validate_trailer_map(&map, true).is_err());
}
