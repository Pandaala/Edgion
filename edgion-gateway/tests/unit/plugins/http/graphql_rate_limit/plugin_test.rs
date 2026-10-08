use super::*;
use crate::ctx::EdgionHttpContext;
use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
use http::{HeaderMap, HeaderValue};
use serde_json::json;

fn config() -> GraphqlRateLimitConfig {
    serde_json::from_value(json!({
        "schema": "type Query { users(first: Int = 10): [User!]! } type User { name: String! }",
        "costRules": [{"field":"Query.users", "multipliers":["first"]}],
        "redisRef": "unit/graphql-missing-client",
        "policies": [{"rate":100, "interval":"1h", "algorithm":"FixedWindow"}],
        "onRedisFailure":"Allow"
    }))
    .unwrap()
}

fn filter(cap: Option<&str>) -> Box<dyn RequestFilter> {
    GraphqlRateLimit::create(
        &config(),
        "unit/graphql/#0",
        cap.map(|cap| BodyRequirement {
            max_body_size: Some(cap.into()),
            ..BodyRequirement::fail_close()
        }),
    )
    .unwrap()
}

fn session(method: &'static str, body_len: u64, declared: bool, query: Option<&'static str>) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_set_response_header().returning(|_, _| Ok(()));
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    session.expect_ctx().return_const(ctx);
    session.expect_get_method().return_const(method.to_owned());
    session.expect_declares_body().return_const(Some(declared));
    session
        .expect_request_body_available_len()
        .returning(move |_| Ok(body_len));
    session.expect_request_query().return_const(query);
    let mut headers = HeaderMap::new();
    headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    session.expect_request_header_map().return_const(headers);
    session.expect_record_error().return_const(());
    session.expect_record_outcome().return_const(());
    session.expect_key_get().return_const(Some("unit-identity".to_owned()));
    session
}

fn status(result: PluginRunningResult) -> u16 {
    match result {
        PluginRunningResult::ErrResponse { status, .. } => status,
        other => panic!("expected fixed rejection, got {other:?}"),
    }
}

#[test]
fn constructor_checks_schema_and_absolute_body_policy() {
    let mut invalid = config();
    invalid.schema = "type Query { missing: Absent }".into();
    assert!(GraphqlRateLimit::create(&invalid, "unit", None).is_none());
    assert!(GraphqlRateLimit::create(
        &config(),
        "unit",
        Some(BodyRequirement {
            max_body_size: Some("65k".into()),
            ..Default::default()
        })
    )
    .is_none());
    assert!(!filter(None).requires_materialized_request_body());
}

#[test]
fn constructor_independently_rejects_leaf_multiplier_rules() {
    let mut invalid = config();
    invalid.schema = "type Query { ids(first:Int=100):[Int] }".into();
    invalid.cost_rules[0].field = "Query.ids".into();
    assert!(GraphqlRateLimit::create(&invalid, "unit", None).is_none());
    invalid.cost_rules[0].multipliers.clear();
    assert!(GraphqlRateLimit::create(&invalid, "unit", None).is_some());
}

// Keep permit exhaustion and its successful successor in one test so this
// deliberately held process-wide budget cannot race another evaluation test.
#[tokio::test]
async fn bounded_admission_and_early_replies_do_not_materialize_rejected_input() {
    let _evaluation_test_guard = EVALUATION_TEST_LOCK.lock().await;
    let filter = filter(None);
    let mut log = PluginLog::new("GraphqlRateLimit");

    let mut post_url = session("POST", 10, true, Some("query=%7Busers%7Bname%7D%7D"));
    post_url.expect_read_request_body().times(0);
    assert_eq!(status(filter.run_request(&mut post_url, &mut log).await), 400);

    let mut large_post = session("POST", MAX_ENVELOPE_BYTES as u64 + 1, true, None);
    large_post.expect_read_request_body().times(0);
    assert_eq!(status(filter.run_request(&mut large_post, &mut log).await), 413);

    static LARGE_QUERY: [u8; MAX_ENVELOPE_BYTES + 1] = [b'a'; MAX_ENVELOPE_BYTES + 1];
    let raw = std::str::from_utf8(&LARGE_QUERY).unwrap();
    let mut large_get = session("GET", 0, false, Some(raw));
    large_get.expect_read_request_body().times(0);
    assert_eq!(status(filter.run_request(&mut large_get, &mut log).await), 413);

    let mut get_body = session("GET", 1, true, Some("query=%7Busers%7Bname%7D%7D"));
    get_body.expect_read_request_body().times(0);
    assert_eq!(status(filter.run_request(&mut get_body, &mut log).await), 400);

    let all = EVALUATIONS.try_acquire_many(MAX_EVALUATIONS as u32).unwrap();
    let mut busy = session("POST", 2, true, None);
    busy.expect_read_request_body().times(0);
    assert_eq!(status(filter.run_request(&mut busy, &mut log).await), 503);
    drop(all);

    let mut unavailable = session("POST", 2, true, None);
    unavailable
        .expect_read_request_body()
        .return_once(|_| Err(BodyReadError::Unimplemented));
    assert_eq!(status(filter.run_request(&mut unavailable, &mut log).await), 500);
    assert_eq!(EVALUATIONS.available_permits(), MAX_EVALUATIONS);

    let body = br#"{"query":"{ users { name } }"}"#;
    let mut valid = session("POST", body.len() as u64, true, None);
    valid
        .expect_read_request_body()
        .return_once(move |_| Ok(Bytes::from_static(body)));
    assert_eq!(
        filter.run_request(&mut valid, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(EVALUATIONS.available_permits(), MAX_EVALUATIONS);

    // The borrowed query returned by the mock represents the current URI. This
    // test forbids the entry-frozen and copying APIs, as production borrows it.
    let mut current_get = session("GET", 0, false, Some("query=%7Busers(first%3A20)%7Bname%7D%7D"));
    current_get.expect_get_query().times(0);
    current_get.expect_get_query_param().times(0);
    current_get.expect_read_request_body().times(0);
    assert_eq!(
        filter.run_request(&mut current_get, &mut log).await,
        PluginRunningResult::GoodNext
    );

    let lowered = super::tests::filter(Some("8"));
    let mut get_cap = session("GET", 0, false, Some("query=%7Busers%7Bname%7D%7D"));
    assert_eq!(status(lowered.run_request(&mut get_cap, &mut log).await), 413);
}
