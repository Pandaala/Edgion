use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::EdgionHttpContext;
use http::{HeaderMap, HeaderValue};

pub(crate) static EVALUATION_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn session(body: &'static [u8]) -> MockPluginSession {
    session_with_context(body, EdgionHttpContext::default())
}

fn session_with_context(body: &'static [u8], ctx: EdgionHttpContext) -> MockPluginSession {
    session_with_context_reads(body, ctx, 1)
}

fn session_with_context_reads(body: &'static [u8], mut ctx: EdgionHttpContext, reads: usize) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_set_response_header().returning(|_, _| Ok(()));
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
        .times(reads)
        .returning(move |_| Ok(Bytes::from_static(body)));
    session
}

#[tokio::test]
async fn repeated_post_evaluations_keep_first_proof_and_run_each_policy() {
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;
    let _lock = EVALUATION_TEST_LOCK.lock().await;
    let body = br#"{"query":"query First { ping }"}"#;
    let (mut storage, view) = BodyStorage::new(65536, 65536);
    storage.write(&Bytes::from_static(body)).await.unwrap();
    storage.finish().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    ctx.request_body.install(view.clone());
    let mut session = session_with_context_reads(body, ctx, 5);
    let mut log = PluginLog::new("GraphqlGuard");
    let mut calls = 0;
    let mut first_proof = None;
    for attempt in 0..5 {
        let result = evaluate(&mut session, &mut log, 65536, |parsed| {
            calls += 1;
            assert_eq!(parsed.operation_name(), Some("First"));
            match attempt {
                0 | 3 => Err(GraphqlError::Forbidden),
                4 => panic!("test repeated evaluation panic"),
                _ => Ok(()),
            }
        })
        .await;
        match attempt {
            0 | 3 => assert!(matches!(
                result,
                Err(PluginRunningResult::ErrResponse { status: 403, .. })
            )),
            4 => assert!(matches!(
                result,
                Err(PluginRunningResult::ErrResponse { status: 500, .. })
            )),
            _ => assert!(result.is_ok()),
        }
        assert_eq!(calls, attempt + 1);
        assert_eq!(EVALUATIONS.available_permits(), MAX_EVALUATIONS);
        if attempt == 0 {
            assert!(session.ctx().response_cache_graphql_proof.get().is_none());
            view.overwrite_body(Bytes::from_static(body)).unwrap();
        } else {
            let proof = *session.ctx().response_cache_graphql_proof.get().unwrap();
            let fields = (proof.generation, proof.operation, proof.name_digest);
            assert_eq!(*first_proof.get_or_insert(fields), fields);
            assert_eq!(Some(proof.generation), session.ctx().request_body.generation());
            assert!(view.overwrite_body(Bytes::from_static(body)).is_err());
        }
    }
}

#[tokio::test]
async fn successful_graphql_evaluation_freezes_body_but_failed_evaluation_does_not() {
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;
    let _lock = EVALUATION_TEST_LOCK.lock().await;
    for (body, allowed) in [
        (br#"{"query":"{ping}"}"#.as_slice(), true),
        (br#"{"query":"invalid @@"}"#.as_slice(), false),
    ] {
        let (mut storage, view) = BodyStorage::new(65536, 65536);
        storage.write(&Bytes::from_static(body)).await.unwrap();
        storage.finish().await.unwrap();
        let mut ctx = EdgionHttpContext::default();
        ctx.request_body.install(view.clone());
        let mut session = session_with_context(body, ctx);
        let result = evaluate(&mut session, &mut PluginLog::new("GraphqlGuard"), 65536, |_| Ok(())).await;
        assert_eq!(result.is_ok(), allowed);
        let proof = session.ctx().response_cache_graphql_proof.get();
        assert_eq!(proof.is_some(), allowed);
        if let Some(proof) = proof {
            assert_eq!(Some(proof.generation), session.ctx().request_body.generation());
            assert_eq!(
                proof.operation,
                edgion_resources::graphql_request::GraphqlOperationType::Query
            );
        }
        let mutation = view.overwrite_body(Bytes::from_static(b"changed"));
        assert_eq!(mutation.is_err(), allowed);
    }
}

#[tokio::test]
async fn graphql_measurement_freezes_the_same_admitted_body_as_enforcement() {
    use crate::request_body::BodyStorage;
    use edgion_resources::plugin_configs::GraphqlGuardConfig;
    use pingora_core::protocols::http::RequestBodyBuffer;
    let _lock = EVALUATION_TEST_LOCK.lock().await;
    let body = br#"{"query":"{a:ping}"}"#;
    for mode in ["measure", "enforce"] {
        let guard: GraphqlGuardConfig =
            serde_json::from_value(serde_json::json!({"mode":mode,"maxAliases":0})).unwrap();
        let guard = guard.compile().unwrap();
        let (mut storage, view) = BodyStorage::new(65536, 65536);
        storage.write(&Bytes::from_static(body)).await.unwrap();
        storage.finish().await.unwrap();
        let mut ctx = EdgionHttpContext::default();
        ctx.request_body.install(view.clone());
        let mut session = session_with_context(body, ctx);
        let result = evaluate(&mut session, &mut PluginLog::new("GraphqlGuard"), 65536, |parsed| {
            guard.inspect(parsed)
        })
        .await;
        assert_eq!(result.is_ok(), mode == "measure");
        assert_eq!(
            session.ctx().response_cache_graphql_proof.get().is_some(),
            mode == "measure"
        );
        assert_eq!(
            view.overwrite_body(Bytes::from_static(b"changed")).is_err(),
            mode == "measure"
        );
    }
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

#[test]
fn graphql_http_errors_are_fixed_json_and_queue_only_canonical_content_type() {
    for (status, code) in [
        (400, "GRAPHQL_REQUEST_INVALID"),
        (403, "GRAPHQL_OPERATION_FORBIDDEN"),
        (405, "METHOD_NOT_ALLOWED"),
        (413, "PAYLOAD_TOO_LARGE"),
        (415, "UNSUPPORTED_MEDIA_TYPE"),
        (429, "RATE_LIMITED"),
        (500, "INTERNAL_SERVER_ERROR"),
        (503, "GRAPHQL_ADMISSION_BUSY"),
    ] {
        let mut session = MockPluginSession::new();
        session.expect_set_response_header().times(1).returning(|name, value| {
            assert_eq!(name, "Content-Type");
            assert_eq!(value, "application/json");
            Ok(())
        });
        let PluginRunningResult::ErrResponse {
            status: actual,
            body: Some(body),
        } = reject(&mut session, status)
        else {
            panic!("Expected HTTP rejection");
        };
        assert_eq!(actual, status);
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"errors":[{"message":"GraphQL request rejected","extensions":{"code":code}}]})
        );
    }
}

#[test]
fn cost_logging_and_header_are_bounded_numeric_and_opt_in() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    for show in [false, true] {
        let mut session = MockPluginSession::new();
        session
            .expect_set_response_header()
            .times(usize::from(show))
            .returning(|name, value| {
                assert_eq!((name, value), ("X-Graphql-Query-Cost", "4294967295"));
                Ok(())
            });
        let mut log = PluginLog::new("GraphqlRateLimit");
        record_cost(&mut session, &mut log, u32::MAX, show);
        assert!(log.contains("cost=4294967295"));
        assert!(log.ulog.is_none());
    }
}

#[tokio::test]
async fn response_cache_graphql_proof_records_actual_kind_and_selected_name_without_retaining_dom() {
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;
    let _lock = EVALUATION_TEST_LOCK.lock().await;
    let mut digests = Vec::new();
    for (body, expected, name) in [
        (
            br#"{"query":"query First { ping }","operationName":"First"}"#.as_slice(),
            edgion_resources::graphql_request::GraphqlOperationType::Query,
            Some("First"),
        ),
        (
            br#"{"query":"query Other { ping }","operationName":"Other"}"#.as_slice(),
            edgion_resources::graphql_request::GraphqlOperationType::Query,
            Some("Other"),
        ),
        (
            br#"{"query":"mutation Change { ping }","operationName":"Change"}"#.as_slice(),
            edgion_resources::graphql_request::GraphqlOperationType::Mutation,
            Some("Change"),
        ),
        (
            br#"{"query":"{ping}"}"#.as_slice(),
            edgion_resources::graphql_request::GraphqlOperationType::Query,
            None,
        ),
    ] {
        let (mut storage, view) = BodyStorage::new(65536, 65536);
        storage.write(&Bytes::from_static(body)).await.unwrap();
        storage.finish().await.unwrap();
        let mut ctx = EdgionHttpContext::default();
        ctx.request_body.install(view);
        let mut session = session_with_context(body, ctx);
        evaluate(&mut session, &mut PluginLog::new("GraphqlGuard"), 65536, |_| Ok(()))
            .await
            .unwrap();
        let proof = session.ctx().response_cache_graphql_proof.get().unwrap();
        assert_eq!(proof.operation, expected);
        assert_eq!(Some(proof.generation), session.ctx().request_body.generation());
        use sha2::{Digest, Sha256};
        let mut material = b"edgion response cache selected operation\0".to_vec();
        material.push(u8::from(name.is_some()));
        material.extend_from_slice(name.unwrap_or("").as_bytes());
        let expected_digest: [u8; 32] = Sha256::digest(material).into();
        assert_eq!(proof.name_digest, expected_digest);
        digests.push(proof.name_digest);
    }
    assert!(digests[0] != digests[1] && digests[1] != digests[2]);
}

#[tokio::test]
async fn get_evaluation_preserves_selection_freeze_and_absence_of_post_proof() {
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;
    let _lock = EVALUATION_TEST_LOCK.lock().await;
    for (query, name, allowed) in [
        ("query=query%20Named%7Bping%7D", Some("Named"), true),
        ("query=%7Bping%7D", None, true),
        ("query=mutation%20Change%7Bping%7D", Some("Change"), false),
    ] {
        let (mut storage, view) = BodyStorage::new(65536, 65536);
        storage.finish().await.unwrap();
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.in_request_body_stage = true;
        ctx.request_body.install(view.clone());
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx);
        session.expect_get_method().return_const("GET".to_owned());
        session.expect_declares_body().return_const(Some(false));
        session.expect_request_query().return_const(Some(query));
        session.expect_request_body_available_len().returning(|known_bodyless| {
            assert!(known_bodyless);
            Ok(0)
        });
        session.expect_request_header_map().return_const(HeaderMap::new());
        session.expect_read_request_body().never();
        session.expect_set_response_header().returning(|_, _| Ok(()));
        let mut called = false;
        let result = evaluate(&mut session, &mut PluginLog::new("GraphqlGuard"), 65536, |parsed| {
            called = true;
            assert_eq!(parsed.operation_name(), name);
            assert_eq!(
                parsed.operation_type(),
                edgion_resources::graphql_request::GraphqlOperationType::Query
            );
            Ok(7)
        })
        .await;
        if allowed {
            assert_eq!(result, Ok(7));
        } else {
            assert!(matches!(
                result,
                Err(PluginRunningResult::ErrResponse { status: 405, .. })
            ));
        }
        assert_eq!(called, allowed);
        assert!(session.ctx().response_cache_graphql_proof.get().is_none());
        assert_eq!(view.overwrite_body(Bytes::from_static(b"changed")).is_err(), allowed);
        assert_eq!(EVALUATIONS.available_permits(), MAX_EVALUATIONS);
    }
}
