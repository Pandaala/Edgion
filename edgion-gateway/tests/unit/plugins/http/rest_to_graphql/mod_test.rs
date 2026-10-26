use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::EdgionHttpContext;
use http::{HeaderMap, HeaderValue, Method};
use parking_lot::Mutex;
use serde_json::json;
use std::sync::Arc;

fn adapter(value: Value) -> RestToGraphql {
    RestToGraphql {
        compiled: serde_json::from_value::<RestToGraphqlConfig>(value)
            .unwrap()
            .compile()
            .unwrap(),
        envelope_limit: 65536,
    }
}

#[tokio::test]
async fn unused_body_media_encoding_caps_and_physical_get_are_enforced() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let filter = adapter(json!({"query":"{ping}"}));
    for (method, declared, length, media, encoding, body, expected) in [
        (Method::GET, true, 2, "application/json", "identity", "{}", 400),
        (Method::POST, true, 2, "text/plain", "identity", "{}", 415),
        (Method::POST, false, 0, "application/json", "gzip", "", 415),
        (Method::POST, true, 65537, "application/json", "identity", "", 413),
        (Method::POST, true, 3, "application/json", "identity", "bad", 400),
    ] {
        let mut session = MockPluginSession::new();
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.in_request_body_stage = true;
        ctx.request_info.method = Some(method.clone());
        session.expect_get_request_method().return_const(Some(match method {
            Method::GET => "GET",
            Method::POST => "POST",
            _ => panic!("unsupported test method"),
        }));
        ctx.request_info.body_declared = Some(declared);
        session.expect_ctx().return_const(ctx);
        session.expect_declares_body().return_const(Some(declared));
        session
            .expect_request_body_available_len()
            .returning(move |_| Ok(length));
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static(media));
        headers.insert("content-encoding", HeaderValue::from_static(encoding));
        session.expect_request_header_map().return_const(headers);
        session.expect_set_response_header().returning(|_, _| Ok(()));
        session.expect_record_error().return_const(());
        if body == "bad" {
            session
                .expect_read_request_body()
                .return_once(move |_| Ok(Bytes::from_static(body.as_bytes())));
        }
        assert!(
            matches!(filter.run_request(&mut session, &mut PluginLog::new("RestToGraphql")).await,
            PluginRunningResult::ErrResponse { status, .. } if status == expected)
        );
    }
    assert_eq!(graphql_request::EVALUATIONS.available_permits(), 4);
}
fn session(method: Method, query: Option<&str>) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.method = Some(method.clone());
    session.expect_get_request_method().return_const(Some(match method {
        Method::GET => "GET",
        Method::POST => "POST",
        _ => panic!("unsupported test method"),
    }));
    ctx.request_query.capture(query.map(str::to_owned)).unwrap();
    ctx.request_info.body_declared = Some(false);
    ctx.request_info.in_request_body_stage = true;
    session.expect_ctx().return_const(ctx);
    session.expect_declares_body().return_const(Some(false));
    session.expect_request_body_available_len().returning(|_| Ok(0));
    session.expect_request_header_map().return_const(HeaderMap::new());
    session.expect_set_response_header().returning(|_, _| Ok(()));
    session
}

#[tokio::test]
async fn admission_and_missing_entry_identity_fail_closed_without_publication() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let filter = adapter(json!({"query":"{ping}"}));
    let permits = graphql_request::EVALUATIONS.try_acquire_many(4).unwrap();
    let mut busy = session(Method::GET, None);
    assert!(matches!(
        filter
            .run_request(&mut busy, &mut PluginLog::new("RestToGraphql"))
            .await,
        PluginRunningResult::ErrResponse { status: 503, .. }
    ));
    drop(permits);
    let mut missing = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    missing.expect_ctx().return_const(ctx);
    missing.expect_get_request_method().returning(|| None);
    missing.expect_set_response_header().returning(|_, _| Ok(()));
    assert!(matches!(
        filter
            .run_request(&mut missing, &mut PluginLog::new("RestToGraphql"))
            .await,
        PluginRunningResult::ErrResponse { status: 500, .. }
    ));
    assert_eq!(graphql_request::EVALUATIONS.available_permits(), 4);
}

#[tokio::test]
async fn native_json_body_publishes_approved_value_only_and_releases_admission() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let filter = adapter(
        json!({"query":"mutation($input:CreateInput!){create(input:$input){id}}",
        "variables":[{"name":"input","bodyPointer":"/payload"}]}),
    );
    let mut session = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    ctx.request_info.method = Some(Method::POST);
    session.expect_get_request_method().returning(|| Some("POST"));
    ctx.request_info.body_declared = Some(true);
    session.expect_ctx().return_const(ctx);
    session.expect_declares_body().return_const(Some(true));
    let input = Bytes::from_static(br#"{"payload":{"id":42},"query":"untrusted"}"#);
    let len = input.len() as u64;
    session.expect_request_body_available_len().returning(move |_| Ok(len));
    session.expect_read_request_body().return_once(move |_| Ok(input));
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    session.expect_request_header_map().return_const(headers);
    session.expect_overwrite_request_body().return_once(|body| {
        let envelope: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(envelope["variables"], json!({"input":{"id":42}}));
        assert_eq!(
            envelope["query"],
            "mutation($input:CreateInput!){create(input:$input){id}}"
        );
        assert_eq!(graphql_request::EVALUATIONS.available_permits(), 4);
        Ok(())
    });
    session.expect_set_upstream_method().returning(|_| Ok(()));
    session.expect_set_upstream_uri().returning(|_| Ok(()));
    session.expect_set_request_header().returning(|_, _| Ok(()));
    assert_eq!(
        filter
            .run_request(&mut session, &mut PluginLog::new("RestToGraphql"))
            .await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn bodyless_get_maps_canonical_keys_and_publishes_exact_post_before_guard() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let filter = adapter(
        json!({"query":"query Read($id:ID!,$n:Int!){user(id:$id,n:$n){id}}","path":"/echo",
        "variables":[{"name":"id","from":{"type":"ctx","name":"id"}},
            {"name":"n","from":{"type":"query","name":"n"}}]}),
    );
    let mut session = session(Method::GET, Some("n=3"));
    session.expect_key_get().returning(|key| match key {
        KeyGet::Ctx { .. } => Some("42".into()),
        KeyGet::Query { .. } => Some("3".into()),
        _ => panic!("wrong accessor"),
    });
    let output = Arc::new(Mutex::new(Bytes::new()));
    let captured = output.clone();
    session
        .expect_overwrite_request_body()
        .times(1)
        .return_once(move |body| {
            *captured.lock() = body;
            Ok(())
        });
    session.expect_set_upstream_method().times(1).returning(|method| {
        assert_eq!(method, "POST");
        Ok(())
    });
    session.expect_set_upstream_uri().times(1).returning(|uri| {
        assert_eq!(uri, "/echo");
        Ok(())
    });
    session.expect_set_request_header().times(1).returning(|name, value| {
        assert_eq!((name, value), ("content-type", "application/json"));
        Ok(())
    });
    assert_eq!(
        filter
            .run_request(&mut session, &mut PluginLog::new("RestToGraphql"))
            .await,
        PluginRunningResult::GoodNext
    );
    let output = output.lock().clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&output).unwrap(),
        json!({"query":filter.compiled.query,"variables":{"id":"42","n":3}})
    );
    let guard: edgion_resources::plugin_configs::GraphqlGuardConfig =
        serde_json::from_value(json!({"rootFields":{"query":{"allow":["user"]}}})).unwrap();
    guard
        .compile()
        .unwrap()
        .inspect(ParsedGraphqlRequest::parse_post(&output, 65536).unwrap())
        .unwrap();
    assert_eq!(graphql_request::EVALUATIONS.available_permits(), 4);
}

#[tokio::test]
async fn original_get_cannot_mutate_even_when_live_method_would_be_post() {
    let filter = adapter(json!({"query":"mutation {write}"}));
    let mut session = session(Method::GET, None);
    session.expect_get_method().times(0).return_const("POST".to_owned());
    // No live method read or publication is allowed on this rejection path.
    assert!(matches!(
        filter
            .run_request(&mut session, &mut PluginLog::new("RestToGraphql"))
            .await,
        PluginRunningResult::ErrResponse { status: 405, .. }
    ));
}

#[tokio::test]
async fn repeated_adapters_before_inspection_reuse_canonical_publication() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let filter = adapter(json!({"query":"{ping}"}));
    let mut session = session(Method::GET, None);
    session.expect_overwrite_request_body().times(2).returning(|body| {
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"query":"{ping}","variables":{}})
        );
        Ok(())
    });
    session.expect_set_upstream_method().times(2).returning(|_| Ok(()));
    session.expect_set_upstream_uri().times(2).returning(|_| Ok(()));
    session.expect_set_request_header().times(2).returning(|_, _| Ok(()));
    for _ in 0..2 {
        assert_eq!(
            filter
                .run_request(&mut session, &mut PluginLog::new("RestToGraphql"))
                .await,
            PluginRunningResult::GoodNext
        );
    }
}

#[tokio::test]
async fn duplicate_query_missing_variables_and_publication_failure_never_continue() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let filter = adapter(
        json!({"query":"query($n:Int!){ping(n:$n)}","variables":[{"name":"n","from":{"type":"query","name":"n"}}]}),
    );
    let mut duplicate = session(Method::GET, Some("n=1&%6e=2"));
    assert!(matches!(
        filter
            .run_request(&mut duplicate, &mut PluginLog::new("RestToGraphql"))
            .await,
        PluginRunningResult::ErrResponse { status: 400, .. }
    ));
    let mut missing = session(Method::GET, None);
    missing.expect_key_get().return_const(None);
    assert!(matches!(
        filter
            .run_request(&mut missing, &mut PluginLog::new("RestToGraphql"))
            .await,
        PluginRunningResult::ErrResponse { status: 400, .. }
    ));
    let mut failure = session(Method::GET, Some("n=1"));
    failure.expect_key_get().return_const(Some("1".into()));
    failure
        .expect_overwrite_request_body()
        .returning(|_| Err(crate::request_body::BodyReadError::StorageFailure));
    failure.expect_record_error().return_const(());
    assert!(matches!(
        filter
            .run_request(&mut failure, &mut PluginLog::new("RestToGraphql"))
            .await,
        PluginRunningResult::ErrResponse { status: 500, .. }
    ));
    assert_eq!(graphql_request::EVALUATIONS.available_permits(), 4);
}

#[test]
fn repeated_headers_and_bounded_serialization_reject_before_mutation() {
    let filter = adapter(
        json!({"query":"query($id:ID!){user(id:$id){id}}","variables":[{"name":"id","from":{"type":"header","name":"X-Id"}}]}),
    );
    let mut session = MockPluginSession::new();
    let mut headers = HeaderMap::new();
    headers.append("x-id", HeaderValue::from_static("1"));
    headers.append("x-id", HeaderValue::from_static("2"));
    session.expect_request_header_map().return_const(headers);
    assert!(filter.construct(&session, None).is_err());
    let mut filter = adapter(
        json!({"query":"query($id:ID!){user(id:$id){id}}","variables":[{"name":"id","from":{"type":"ctx","name":"id"}}]}),
    );
    filter.envelope_limit = 32;
    let mut session = MockPluginSession::new();
    session.expect_key_get().return_const(Some("42".into()));
    assert_eq!(filter.construct(&session, None).unwrap_err(), GraphqlError::TooLarge);
}

#[tokio::test]
async fn original_post_can_publish_mutation_when_live_method_would_be_get() {
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let filter = adapter(json!({"query":"mutation {write}"}));
    let mut session = session(Method::POST, None);
    session.expect_get_method().times(0).return_const("GET".to_owned());
    session.expect_overwrite_request_body().times(1).return_once(|body| {
        let envelope: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(envelope["query"], "mutation {write}");
        Ok(())
    });
    session
        .expect_set_upstream_method()
        .times(1)
        .withf(|method| method == "POST")
        .returning(|_| Ok(()));
    session.expect_set_upstream_uri().returning(|_| Ok(()));
    session.expect_set_request_header().returning(|_, _| Ok(()));
    assert_eq!(
        filter
            .run_request(&mut session, &mut PluginLog::new("RestToGraphql"))
            .await,
        PluginRunningResult::GoodNext
    );
}

#[test]
fn variable_serialization_preserves_bytes_omissions_and_exact_cap() {
    #[derive(Serialize)]
    struct OwnedEnvelope<'a> {
        query: &'a str,
        #[serde(rename = "operationName", skip_serializing_if = "Option::is_none")]
        operation_name: Option<&'a str>,
        variables: serde_json::Map<String, Value>,
    }
    let query = "query Read($z:Int!,$a:Input,$nil:String,$missing:String,$fallback:Int=7){read(z:$z,a:$a,nil:$nil,missing:$missing,fallback:$fallback)}";
    let body = json!({"z":3,"a":{"text":"quote\" and slash\\","items":[true,2]},"nil":null});
    let session = MockPluginSession::new();
    for operation_name in [None, Some("Read")] {
        let mut filter = adapter(json!({
            "query":query,"operationName":operation_name,
            "variables":[
                {"name":"z","bodyPointer":"/z"},
                {"name":"missing","bodyPointer":"/absent"},
                {"name":"nil","bodyPointer":"/nil"},
                {"name":"fallback","bodyPointer":"/absent"},
                {"name":"a","bodyPointer":"/a"}
            ]
        }));
        let variables = [
            ("z".to_owned(), body["z"].clone()),
            ("nil".to_owned(), Value::Null),
            ("a".to_owned(), body["a"].clone()),
        ]
        .into_iter()
        .collect();
        let expected = serde_json::to_vec(&OwnedEnvelope {
            query,
            operation_name,
            variables,
        })
        .unwrap();
        assert_eq!(filter.construct(&session, Some(&body)).unwrap().as_ref(), expected);
        filter.envelope_limit = expected.len();
        assert_eq!(filter.construct(&session, Some(&body)).unwrap().as_ref(), expected);
        filter.envelope_limit -= 1;
        assert_eq!(filter.construct(&session, Some(&body)), Err(GraphqlError::TooLarge));
    }
}
