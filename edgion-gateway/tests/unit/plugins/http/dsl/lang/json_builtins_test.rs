use super::super::body_scan_budget::BodyScanBudget;
use super::super::vm::VmLimits;
use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::request_body::BodyStorage;
use crate::EdgionHttpContext;
use bytes::Bytes;
use pingora_core::protocols::http::RequestBodyBuffer;

async fn storage_session(body: &[u8], memory: usize) -> MockPluginSession {
    let (mut storage, view) = BodyStorage::new(memory, body.len() as u64);
    storage.write(&Bytes::copy_from_slice(body)).await.unwrap();
    storage.finish().await.unwrap();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = MAX_DSL_JSON_BYTES as u64;
    ctx.request_body.install(view);
    let mut session = MockPluginSession::new();
    session.expect_declares_body().return_const(Some(true));
    session
        .expect_request_body_available_len()
        .return_const(Ok(body.len() as u64));
    session.expect_ctx().return_const(ctx);
    session
}

#[tokio::test]
async fn json_body_memory_and_spill_preserve_exact_utf8_and_numbers() {
    let source = r#"{"variables":{"é":[null,184467440737095516160,0.123456789012345678901]}}"#;
    for memory in [source.len(), 1] {
        let mut session = storage_session(source.as_bytes(), memory).await;
        let mut scans = BodyScanBudget::default();
        let runtime = DslRuntime::default();
        let limits = VmLimits {
            max_string_len: MAX_DSL_JSON_BYTES,
            ..Default::default()
        };
        let outcome = DslBodyOps::new(&mut session, &mut scans, &limits, &runtime)
            .read_json(ValueWorkBudget::default())
            .await
            .unwrap();
        let Value::Str(text) = outcome.result.unwrap() else {
            panic!("expected strict JSON");
        };
        assert_eq!(text.as_ref(), source);
    }
}

#[tokio::test]
async fn json_body_malformed_is_nil_and_limit_is_body_access() {
    for bytes in [
        b"{\"a\":1,\"a\":2}".as_slice(),
        b"\xff".as_slice(),
        br#"{"x":"\uD800"}"#.as_slice(),
        br#""\uD800""#.as_slice(),
    ] {
        let mut session = storage_session(bytes, 1).await;
        let mut scans = BodyScanBudget::default();
        let runtime = DslRuntime::default();
        let limits = VmLimits::default();
        let outcome = DslBodyOps::new(&mut session, &mut scans, &limits, &runtime)
            .read_json(ValueWorkBudget::default())
            .await
            .unwrap();
        assert!(matches!(outcome.result.unwrap(), Value::Nil));
    }
    let mut session = storage_session(b"{\"a\":1234}", 1).await;
    let mut scans = BodyScanBudget::default();
    let runtime = DslRuntime::default();
    let limits = VmLimits {
        max_string_len: 4,
        ..Default::default()
    };
    assert!(matches!(
        DslBodyOps::new(&mut session, &mut scans, &limits, &runtime)
            .read_json(ValueWorkBudget::default())
            .await,
        Err(RuntimeError::BodyAccess { .. })
    ));
}

fn vm(source: &str, limits: VmLimits) -> Vm {
    let config = edgion_resources::plugin_configs::DslConfig {
        name: "json-test".into(),
        source: Some(source.into()),
        ..Default::default()
    };
    Vm::new(config.compile_and_validate().unwrap(), limits).unwrap()
}

#[tokio::test]
async fn json_builtins_reconstruct_dynamic_envelope_and_preserve_absent_null() {
    let source = r#"
        let record = json.read("{\"query\":\"query Q { a }\",\"operationName\":\"Q\"}")
        let input = json.read("{\"id\":\"x\",\"variables\":{\"n\":184467440737095516160,\"f\":0.123456789012345678901}}")
        if json.get_raw(input,"missing") != nil { return deny(500, "failed") }
        if json.get_raw(json.read("{\"v\":null}"),"v") != "null" { return deny(500, "failed") }
        let output = json.set_raw(record,"variables",json.get_raw(input,"variables"))
        if json.get_string(output,"operationName") != "Q" { return deny(500, "failed") }
        for key in json.keys(input) { if key != "id" && key != "variables" { return deny(500, "failed") } }
        json.set_body(output)
    "#;
    let mut session = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.in_request_body_stage = true;
    ctx.request_body_config.max_body_size_bytes = MAX_DSL_JSON_BYTES as u64;
    session.expect_ctx().return_const(ctx);
    session.expect_overwrite_request_body().times(1).withf(|body| body.as_ref() == br#"{"query":"query Q { a }","operationName":"Q","variables":{"n":184467440737095516160,"f":0.123456789012345678901}}"#).returning(|_| Ok(()));
    assert_eq!(
        vm(source, VmLimits::default())
            .execute(
                &mut session,
                &mut crate::plugins::runtime::PluginLog::new("test"),
                &DslRuntime::default()
            )
            .await
            .unwrap(),
        edgion_resources::filters::PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn json_set_body_rolls_back_malformed_oversized_and_late_stage() {
    use super::super::vm::execute_safe;
    use edgion_resources::filters::PluginRunningResult;
    use edgion_resources::plugin_configs::DslErrorPolicy;
    for (source, cap, stage) in [
        ("json.set_body(\"{\")", 64, true),
        ("json.set_body(\"{\\\"a\\\":1234}\")", 4, true),
        ("json.set_body(\"{}\")", 64, false),
        ("json.set_body(1)", 64, true),
    ] {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.in_request_body_stage = stage;
        ctx.request_body_config.max_body_size_bytes = cap;
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx);
        session.expect_overwrite_request_body().times(0);
        session.expect_record_outcome().return_const(());
        let result = execute_safe(
            &vm(source, VmLimits::default()),
            &mut session,
            &mut crate::plugins::runtime::PluginLog::new("test"),
            &DslErrorPolicy::Ignore,
            &DslRuntime::default(),
        )
        .await;
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }
}

#[tokio::test]
async fn json_set_body_obeys_canonical_frozen_generation() {
    use bytes::Bytes;
    let (mut storage, view) = BodyStorage::new(64, 64);
    storage.write(&Bytes::from_static(b"{\"old\":true}")).await.unwrap();
    storage.finish().await.unwrap();
    view.freeze_mutation();
    let original_generation = view.generation();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.in_request_body_stage = true;
    ctx.request_body_config.max_body_size_bytes = 64;
    ctx.request_body.install(view.clone());
    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(ctx);
    let for_overwrite = view.clone();
    session
        .expect_overwrite_request_body()
        .times(1)
        .returning(move |body| for_overwrite.overwrite_body(body));
    assert!(matches!(
        vm("json.set_body(\"{}\")", VmLimits::default())
            .execute(
                &mut session,
                &mut crate::plugins::runtime::PluginLog::new("test"),
                &DslRuntime::default()
            )
            .await,
        Err(RuntimeError::BodyAccess { .. })
    ));
    assert_eq!(view.generation(), original_generation);
}

#[test]
fn json_set_body_source_and_wire_reject_response_factory() {
    use crate::plugins::http::dsl::plugin::DslPlugin;
    use edgion_resources::plugin_configs::DslConfig;
    let mut config = DslConfig {
        name: "json-test".into(),
        source: Some("json.set_body(\"{}\")".into()),
        ..Default::default()
    };
    let script = config.compile_and_validate().unwrap();
    assert!(script.references_request_body());
    assert!(script.references_request_body_mutation());
    assert!(DslPlugin::from_config_response(&config).is_none());
    config.bytecode = Some(script.serialize_base64().unwrap());
    config.source = None;
    assert!(DslPlugin::from_config_response(&config).is_none());
}

#[tokio::test]
async fn invalid_surrogates_read_as_nil_and_never_publish_a_body() {
    let source = r#"
        if json.read("\"\\uD800\"") != nil { return deny(500, "invalid scalar") }
        if json.get_raw("{\"x\":\"\\uD800\"}", "x") != nil { return deny(500, "invalid raw") }
        if json.get_string("{\"x\":\"\\uD800\"}", "x") != nil { return deny(500, "invalid string") }
    "#;
    assert_eq!(
        vm(source, VmLimits::default())
            .execute(
                &mut MockPluginSession::new(),
                &mut crate::plugins::runtime::PluginLog::new("test"),
                &DslRuntime::default()
            )
            .await
            .unwrap(),
        edgion_resources::filters::PluginRunningResult::GoodNext
    );
    for source in [
        r#"json.set_body("\"\\uD800\"")"#,
        r#"json.set_body("{\"x\":\"\\uD800\"}")"#,
    ] {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.in_request_body_stage = true;
        ctx.request_body_config.max_body_size_bytes = 65536;
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx);
        session.expect_overwrite_request_body().times(0);
        assert!(matches!(
            vm(source, VmLimits::default())
                .execute(
                    &mut session,
                    &mut crate::plugins::runtime::PluginLog::new("test"),
                    &DslRuntime::default()
                )
                .await,
            Err(RuntimeError::BodyAccess { .. })
        ));
    }
}
