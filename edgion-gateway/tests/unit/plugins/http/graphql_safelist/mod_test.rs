use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::EdgionHttpContext;
use bytes::Bytes;
use http::{HeaderMap, HeaderValue};
use serde_json::json;

#[tokio::test]
async fn safelist_shares_get_post_transport_and_never_mutates_admitted_input() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let config: GraphqlSafelistConfig = serde_json::from_value(json!({"operations":[{"query":"{ ping }"}]})).unwrap();
    let plugin = GraphqlSafelist::create(&config, None).unwrap();
    assert!(!plugin.requires_materialized_request_body());
    for (method, query, body, expected) in [
        ("POST", None, br#"{"query":"{ping}"}"#.as_slice(), 200),
        ("POST", None, br#"{"query":"{pong}"}"#.as_slice(), 403),
        ("GET", Some("query=%7Bping%7D"), b"".as_slice(), 200),
        ("GET", Some("query=%7Bpong%7D"), b"".as_slice(), 403),
    ] {
        let mut session = MockPluginSession::new();
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.in_request_body_stage = true;
        session.expect_ctx().return_const(ctx);
        session.expect_get_method().return_const(method.to_owned());
        session.expect_request_query().return_const(query);
        session.expect_declares_body().return_const(Some(!body.is_empty()));
        session
            .expect_request_body_available_len()
            .returning(move |_| Ok(body.len() as u64));
        let mut headers = HeaderMap::new();
        headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        session.expect_request_header_map().return_const(headers);
        session
            .expect_set_response_header()
            .times(if expected == 200 { 0 } else { 1 })
            .returning(|name, value| {
                assert_eq!((name, value), ("Content-Type", "application/json"));
                Ok(())
            });
        if method == "POST" {
            session
                .expect_read_request_body()
                .times(1)
                .return_once(move |_| Ok(Bytes::from_static(body)));
        }
        let result = plugin
            .run_request(&mut session, &mut PluginLog::new("GraphqlSafelist"))
            .await;
        if expected == 200 {
            assert_eq!(result, PluginRunningResult::GoodNext);
        } else {
            assert!(matches!(result, PluginRunningResult::ErrResponse { status: 403, .. }));
        }
        assert_eq!(
            graphql_request::EVALUATIONS.available_permits(),
            graphql_request::MAX_EVALUATIONS
        );
    }
}
