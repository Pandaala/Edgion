use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::EdgionHttpContext;
use bytes::Bytes;
use http::{HeaderMap, HeaderValue};
use serde_json::json;

#[tokio::test]
async fn shared_transport_enforces_guard_without_redis_or_request_mutation() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let config: GraphqlGuardConfig = serde_json::from_value(json!({"maxAliases":0})).unwrap();
    let guard = GraphqlGuard::create(&config, None).unwrap();
    assert!(!guard.requires_materialized_request_body());
    for (body, allowed) in [
        (br#"{"query":"{ ping }"}"#.as_slice(), true),
        (br#"{"query":"{ a:ping }"}"#.as_slice(), false),
    ] {
        let mut session = MockPluginSession::new();
        session
            .expect_set_response_header()
            .times(if allowed { 0 } else { 1 })
            .returning(|name, value| {
                assert_eq!(name, "Content-Type");
                assert_eq!(value, "application/json");
                Ok(())
            });
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.in_request_body_stage = true;
        session.expect_ctx().return_const(ctx);
        session.expect_get_method().return_const("POST".to_owned());
        session.expect_request_query().return_const(None);
        session.expect_declares_body().return_const(Some(true));
        session
            .expect_request_body_available_len()
            .returning(move |_| Ok(body.len() as u64));
        let mut headers = HeaderMap::new();
        headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        session.expect_request_header_map().return_const(headers);
        session
            .expect_read_request_body()
            .return_once(move |_| Ok(Bytes::from_static(body)));
        let result = guard
            .run_request(&mut session, &mut PluginLog::new("GraphqlGuard"))
            .await;
        if allowed {
            assert_eq!(result, PluginRunningResult::GoodNext);
        } else {
            assert!(matches!(result, PluginRunningResult::ErrResponse { status: 400, .. }));
        }
        assert_eq!(
            graphql_request::EVALUATIONS.available_permits(),
            graphql_request::MAX_EVALUATIONS
        );
    }
}
#[tokio::test]
async fn root_denial_uses_shared_forbidden_json_without_request_mutation() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let config: GraphqlGuardConfig = serde_json::from_value(json!({"rootFields":{"query":{"deny":["ping"]}}})).unwrap();
    let guard = GraphqlGuard::create(&config, None).unwrap();
    let mut session = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    session.expect_ctx().return_const(ctx);
    session.expect_get_method().return_const("GET".to_owned());
    session
        .expect_request_query()
        .return_const(Some("query=%7Ba%3Aping%7D"));
    session.expect_declares_body().return_const(Some(false));
    session.expect_request_body_available_len().returning(|_| Ok(0));
    session.expect_request_header_map().return_const(HeaderMap::new());
    session.expect_set_response_header().times(1).returning(|name, value| {
        assert_eq!((name, value), ("Content-Type", "application/json"));
        Ok(())
    });
    let result = guard
        .run_request(&mut session, &mut PluginLog::new("GraphqlGuard"))
        .await;
    let PluginRunningResult::ErrResponse {
        status,
        body: Some(body),
    } = result
    else {
        panic!("Expected local denial")
    };
    assert_eq!(status, 403);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["errors"][0]["extensions"]["code"],
        "GRAPHQL_OPERATION_FORBIDDEN"
    );
}

#[tokio::test]
async fn measurement_logs_complete_scalars_and_optional_cost_without_reflecting_input() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    for show in [false, true] {
        let config: GraphqlGuardConfig = serde_json::from_value(json!({"mode":"measure","showQueryCost":show,
            "maxAliases":0,"maxCost":1,"schema":"type Query { ping:Int }"}))
        .unwrap();
        let filter = GraphqlGuard::create(&config, None).unwrap();
        let body = br#"{"query":"query Private($token:String) { a:ping }","variables":{"token":"sensitive-value"}}"#;
        let mut session = MockPluginSession::new();
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.in_request_body_stage = true;
        session.expect_ctx().return_const(ctx);
        session.expect_get_method().return_const("POST".to_owned());
        session.expect_request_query().return_const(None);
        session.expect_declares_body().return_const(Some(true));
        session
            .expect_request_body_available_len()
            .returning(move |_| Ok(body.len() as u64));
        let mut headers = HeaderMap::new();
        headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        session.expect_request_header_map().return_const(headers);
        session
            .expect_read_request_body()
            .return_once(move |_| Ok(Bytes::from_static(body)));
        session
            .expect_set_response_header()
            .times(usize::from(show))
            .returning(|name, value| {
                assert_eq!((name, value), ("X-Graphql-Query-Cost", "2"));
                Ok(())
            });
        let mut log = PluginLog::new("GraphqlGuard");
        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert!(log.contains("shape depth=1 aliases=1 roots=1 fields=1"));
        assert!(log.contains("cost=2"));
        assert!(log.contains("measure exceeded=18"));
        let serialized = serde_json::to_string(&log).unwrap();
        for forbidden in ["Private", "sensitive-value", "token", "a:ping"] {
            assert!(!serialized.contains(forbidden));
        }
        assert!(log.ulog.is_none());
        assert_eq!(
            graphql_request::EVALUATIONS.available_permits(),
            graphql_request::MAX_EVALUATIONS
        );
    }
}

#[tokio::test]
async fn measurement_hard_failure_emits_no_estimate_or_cost_header() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let config: GraphqlGuardConfig = serde_json::from_value(json!({"mode":"measure","showQueryCost":true,
        "maxCost":1,"schema":"type Query { ping:Int }","costRules":[{"field":"Query.ping","weight":1_000_000}]}))
    .unwrap();
    let filter = GraphqlGuard::create(&config, None).unwrap();
    let mut session = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    session.expect_ctx().return_const(ctx);
    session.expect_get_method().return_const("GET".to_owned());
    session.expect_request_query().return_const(Some("query=%7Bping%7D"));
    session.expect_declares_body().return_const(Some(false));
    session.expect_request_body_available_len().returning(|_| Ok(0));
    session.expect_request_header_map().return_const(HeaderMap::new());
    session.expect_set_response_header().times(1).returning(|name, value| {
        assert_eq!((name, value), ("Content-Type", "application/json"));
        Ok(())
    });
    let mut log = PluginLog::new("GraphqlGuard");
    assert!(matches!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse { status: 400, .. }
    ));
    assert!(!log.contains("cost="));
    assert!(!log.contains("shape depth="));
}
