use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::EdgionHttpContext;
use http::{HeaderMap, HeaderValue};

pub(crate) static EVALUATION_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn session(body: &'static [u8]) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    session.expect_ctx().return_const(ctx);
    session.expect_get_method().return_const("POST".to_owned());
    session.expect_declares_body().return_const(Some(true));
    session.expect_request_query().return_const(None);
    session
        .expect_request_body_available_len()
        .returning(move |_| Ok(body.len() as u64));
    let mut headers = HeaderMap::new();
    headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    session.expect_request_header_map().return_const(headers);
    session.expect_record_error().return_const(());
    session
        .expect_read_request_body()
        .return_once(move |_| Ok(Bytes::from_static(body)));
    session
}
#[tokio::test]
async fn shared_evaluation_releases_all_permits_after_parse_failure_panic_and_success() {
    let _guard = EVALUATION_TEST_LOCK.lock().await;
    let mut log = PluginLog::new("GraphqlOperationRouter");
    let mut invalid = session(br#"{"query":"invalid @@"}"#);
    assert!(matches!(
        evaluate(&mut invalid, &mut log, MAX_ENVELOPE_BYTES, |_| Ok(())).await,
        Err(PluginRunningResult::ErrResponse { status: 400, .. })
    ));
    assert_eq!(EVALUATIONS.available_permits(), MAX_EVALUATIONS);
    let mut panic_session = session(br#"{"query":"query GetUser { id }"}"#);
    assert!(matches!(
        evaluate::<(), _>(&mut panic_session, &mut log, MAX_ENVELOPE_BYTES, |_| panic!(
            "test evaluation panic"
        ))
        .await,
        Err(PluginRunningResult::ErrResponse { status: 500, .. })
    ));
    assert_eq!(EVALUATIONS.available_permits(), MAX_EVALUATIONS);
    let mut valid = session(br#"{"query":"query GetUser { id }"}"#);
    assert_eq!(
        evaluate(&mut valid, &mut log, MAX_ENVELOPE_BYTES, |parsed| Ok(parsed
            .operation_name()
            .unwrap()
            .len()))
        .await,
        Ok(7)
    );
    assert_eq!(EVALUATIONS.available_permits(), MAX_EVALUATIONS);
}
