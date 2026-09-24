use super::{
    ai_failure_wants_retry, classify_ai_proxy_failure, is_backend_request_timeout_retry_candidate,
    pingora_decided_retry_exhausted, should_record_local_origin_failure, AiProxyFailureClass,
    MAX_PINGORA_DECIDED_RETRIES,
};
use crate::PeerTimeoutSource;
use pingora_core::{ErrorSource, ErrorType};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use tokio::io::AsyncWriteExt;

async fn live_h1_session() -> (Session, tokio::io::DuplexStream) {
    let (mut client, server) = tokio::io::duplex(4096);
    client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    session.read_request().await.unwrap();
    (session, client)
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
    assert_eq!(
        classify_ai_proxy_failure(
            &ErrorType::HTTPStatus(503),
            &ErrorSource::Upstream,
            Some(PeerTimeoutSource::BackendRequest),
        ),
        None
    );
    assert_eq!(
        classify_ai_proxy_failure(&ErrorType::ReadError, &ErrorSource::Downstream, None),
        None
    );
    assert_eq!(
        classify_ai_proxy_failure(&ErrorType::ReadError, &ErrorSource::Upstream, None),
        Some(AiProxyFailureClass::AmbiguousUpstream)
    );
    assert_eq!(
        classify_ai_proxy_failure(&ErrorType::InternalError, &ErrorSource::Upstream, None),
        None
    );
    assert_eq!(
        classify_ai_proxy_failure(
            &ErrorType::ReadTimedout,
            &ErrorSource::Upstream,
            Some(PeerTimeoutSource::BackendRequest),
        ),
        Some(AiProxyFailureClass::BackendRequestTimeout)
    );
}

#[test]
fn ambiguous_upstream_requires_explicit_replay_opt_in() {
    assert!(!ai_failure_wants_retry(
        AiProxyFailureClass::AmbiguousUpstream,
        true,
        false
    ));
    assert!(ai_failure_wants_retry(
        AiProxyFailureClass::AmbiguousUpstream,
        true,
        true
    ));
    assert!(ai_failure_wants_retry(
        AiProxyFailureClass::BackendRequestTimeout,
        true,
        false
    ));
    assert!(!ai_failure_wants_retry(
        AiProxyFailureClass::BackendRequestTimeout,
        false,
        true
    ));
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
