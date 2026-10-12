use super::{
    ambiguous_decided_h2_retry_allowed, decide_retry, is_backend_request_timeout_retry_candidate,
    pingora_decided_retry_exhausted, should_record_local_origin_failure, MAX_PINGORA_DECIDED_RETRIES,
};
use crate::EdgionHttpContext;
use crate::PeerTimeoutSource;
use pingora_core::prelude::HttpPeer;
use pingora_core::{ErrorSource, ErrorType};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use tokio::io::AsyncWriteExt;

async fn live_h1_session() -> (Session, tokio::io::DuplexStream) {
    live_h1_session_for(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await
}

async fn live_h1_session_for(request: &'static [u8]) -> (Session, tokio::io::DuplexStream) {
    let (mut client, server) = tokio::io::duplex(4096);
    client.write_all(request).await.unwrap();
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    session.read_request().await.unwrap();
    (session, client)
}

#[tokio::test]
async fn ambiguous_decided_h2_errors_do_not_replay_post_by_default() {
    let peer = HttpPeer::new("127.0.0.1:30001", false, String::new());
    for client_reused in [false, true] {
        for error_type in [ErrorType::InvalidH2, ErrorType::H2Downgrade] {
            let (mut session, _client) =
                live_h1_session_for(b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 0\r\n\r\n").await;
            let mut error = pingora_core::Error::new_up(error_type);
            error.set_retry(true);

            let result = decide_retry(
                &peer,
                &mut session,
                error,
                &mut EdgionHttpContext::default(),
                client_reused,
            );
            assert!(!result.retry(), "ambiguous H2 outcome must not replay POST");
        }
    }
}

#[tokio::test]
async fn decided_h2_policy_distinguishes_ambiguous_and_protocol_qualified_errors() {
    let peer = HttpPeer::new("127.0.0.1:30001", false, String::new());
    for error_type in [ErrorType::InvalidH2, ErrorType::H2Downgrade, ErrorType::H2Error] {
        for incoming_decision in [false, true] {
            for completed_attempts in [1, 3] {
                let (mut session, _client) = live_h1_session().await;
                let mut error = pingora_core::Error::new_up(error_type.clone());
                error.set_retry(incoming_decision);
                let mut ctx = EdgionHttpContext::default();
                ctx.try_cnt = completed_attempts;
                let result = decide_retry(&peer, &mut session, error, &mut ctx, false);
                // An unfrozen synthetic Session has Disabled backing. Positive
                // replay and frozen Never use the real-origin integration path.
                let expected = incoming_decision && error_type == ErrorType::H2Error;
                assert_eq!(
                    result.retry(),
                    expected,
                    "{error_type:?}, decided={incoming_decision}, attempts={completed_attempts}"
                );
                // Real transport tests qualify H2Error using peer protocol
                // evidence. A synthetic H2Error does not itself prove safety;
                // the fork dispatcher owns its final structural replay veto.
            }
        }
    }
}

#[test]
fn ambiguous_decided_h2_retry_requires_every_safety_gate() {
    let cases = [
        (ErrorType::InvalidH2, true, true, true, true, false, Some(true)),
        (ErrorType::InvalidH2, true, true, false, true, false, Some(false)),
        (ErrorType::H2Downgrade, true, true, true, true, false, Some(true)),
        (ErrorType::H2Downgrade, true, true, true, false, false, Some(false)),
        (ErrorType::InvalidH2, true, true, true, true, true, Some(false)),
        (ErrorType::InvalidH2, true, false, true, true, false, Some(false)),
        (ErrorType::H2Error, true, false, false, false, true, None),
        (ErrorType::InvalidH2, false, false, false, false, true, None),
    ];
    for (error_type, decided, backing, method, attempts, deadline, expected) in cases {
        let mut error = pingora_core::Error::new_up(error_type);
        error.set_retry(decided);
        assert_eq!(
            ambiguous_decided_h2_retry_allowed(&error, backing, method, attempts, deadline),
            expected
        );
    }
}

#[test]
fn ai_final_and_dispatch_error_clear_incoming_retry_decision() {
    crate::ai::http::attempt::tests::ai_final_and_dispatch_error_clear_incoming_retry_decision();
}

async fn write_response_status(session: &mut Session, status: u16) {
    let response = ResponseHeader::build(status, None).unwrap();
    session.write_response_header(Box::new(response), false).await.unwrap();
}

fn timeout_retry_candidate(session: &Session, error_type: ErrorType) -> bool {
    let error = pingora_core::Error::new_up(error_type);
    is_backend_request_timeout_retry_candidate(&error, Some(PeerTimeoutSource::BackendRequest), true, session)
}

#[test]
fn pingora_decided_retries_are_bounded_by_the_fail_safe_ceiling() {
    assert!(!pingora_decided_retry_exhausted(1));
    assert!(!pingora_decided_retry_exhausted(MAX_PINGORA_DECIDED_RETRIES));
    assert!(pingora_decided_retry_exhausted(MAX_PINGORA_DECIDED_RETRIES + 1));
}

#[test]
fn local_origin_recording_excludes_status_control_flow() {
    assert!(!should_record_local_origin_failure(
        false,
        &ErrorType::HTTPStatus(503),
        &ErrorSource::Upstream,
    ));
    assert!(!should_record_local_origin_failure(
        true,
        &ErrorType::ReadError,
        &ErrorSource::Upstream,
    ));
    assert!(!should_record_local_origin_failure(
        false,
        &ErrorType::ReadTimedout,
        &ErrorSource::Downstream,
    ));
    for source in [ErrorSource::Upstream, ErrorSource::Internal, ErrorSource::Unset] {
        assert!(should_record_local_origin_failure(
            false,
            &ErrorType::ReadError,
            &source
        ));
    }
}

#[test]
fn ai_failure_classification_excludes_control_flow_and_non_upstream_errors() {
    crate::ai::http::attempt::tests::ai_failure_classification_excludes_control_flow_and_non_upstream_errors();
}

#[test]
fn ambiguous_upstream_requires_explicit_replay_opt_in() {
    crate::ai::http::attempt::tests::ambiguous_upstream_requires_explicit_replay_opt_in();
}

#[tokio::test]
async fn interim_h1_responses_do_not_count_as_final() {
    for status in [100, 102, 103] {
        let (mut session, _client) = live_h1_session().await;
        write_response_status(&mut session, status).await;

        assert_eq!(session.response_written().unwrap().status.as_u16(), status);
        assert!(timeout_retry_candidate(&session, ErrorType::ReadTimedout), "{status}");
        assert!(timeout_retry_candidate(&session, ErrorType::WriteTimedout), "{status}");
    }
}

#[tokio::test]
async fn switching_protocols_and_regular_h1_responses_count_as_final() {
    for status in [101, 200] {
        let (mut session, _client) = live_h1_session().await;
        write_response_status(&mut session, status).await;

        assert_eq!(session.response_written().unwrap().status.as_u16(), status);
        assert!(!timeout_retry_candidate(&session, ErrorType::ReadTimedout), "{status}");
        assert!(!timeout_retry_candidate(&session, ErrorType::WriteTimedout), "{status}");
    }
}

#[tokio::test]
async fn timeout_retry_candidate_requires_every_case_1_5_input() {
    let (session, _client) = live_h1_session().await;

    let downstream_timeout = pingora_core::Error::new_down(ErrorType::ReadTimedout);
    assert!(!is_backend_request_timeout_retry_candidate(
        &downstream_timeout,
        Some(PeerTimeoutSource::BackendRequest),
        true,
        &session,
    ));

    let upstream_timeout = pingora_core::Error::new_up(ErrorType::ReadTimedout);
    assert!(!is_backend_request_timeout_retry_candidate(
        &upstream_timeout,
        Some(PeerTimeoutSource::RequestDeadline),
        true,
        &session,
    ));
    assert!(!is_backend_request_timeout_retry_candidate(
        &upstream_timeout,
        Some(PeerTimeoutSource::BackendRequest),
        false,
        &session,
    ));

    let upstream_read_error = pingora_core::Error::new_up(ErrorType::ReadError);
    assert!(!is_backend_request_timeout_retry_candidate(
        &upstream_read_error,
        Some(PeerTimeoutSource::BackendRequest),
        true,
        &session,
    ));
}
