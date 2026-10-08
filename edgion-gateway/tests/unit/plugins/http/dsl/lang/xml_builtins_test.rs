use super::*;
use crate::plugins::http::dsl::lang::body_scan_budget::BodyScanBudget;
use crate::plugins::http::dsl::lang::vm::{execute_safe, VmLimits};
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::PluginLog;
use crate::request_body::BodyStorage;
use crate::EdgionHttpContext;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::{DslConfig, DslErrorPolicy};
use pingora_core::protocols::http::RequestBodyBuffer;

fn vm(source: &str, limits: VmLimits) -> Vm {
    let config = DslConfig {
        name: "xml-test".into(),
        source: Some(source.into()),
        ..Default::default()
    };
    Vm::new(config.compile_and_validate().unwrap(), limits).unwrap()
}

async fn storage_session(body: &[u8], memory: usize) -> MockPluginSession {
    let cap = body.len() as u64;
    let (mut storage, view) = BodyStorage::new(memory, cap);
    storage.write(&Bytes::copy_from_slice(body)).await.unwrap();
    storage.finish().await.unwrap();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = cap;
    ctx.request_body.install(view);
    let mut session = MockPluginSession::new();
    session.expect_declares_body().return_const(Some(true));
    session.expect_request_body_available_len().return_const(Ok(cap));
    session.expect_ctx().return_const(ctx);
    session
}

#[tokio::test]
async fn xml_body_scans_memory_and_spill_without_materializing_replay() {
    let body = format!(
        "<s:r xmlns:s='urn:s'><s:v id='7'>alice</s:v><padding>{}</padding></s:r>",
        "x".repeat(128 * 1024)
    );
    for memory in [body.len(), 1] {
        let mut session = storage_session(body.as_bytes(), memory).await;
        let mut scans = BodyScanBudget::default();
        let runtime = DslRuntime {
            max_body_scan_passes: 1,
            ..Default::default()
        };
        let limits = VmLimits::default();
        let mut ops = DslBodyOps::new(&mut session, &mut scans, &limits, &runtime);
        let outcome = ops.parse_xml(ValueWorkBudget::default()).await.unwrap();
        let Value::Xml(root) = outcome.result.unwrap() else {
            panic!("expected XML");
        };
        let mut work = outcome.work;
        let node = root.find("urn:s", "v", 0, &mut work).unwrap().unwrap();
        assert_eq!(node.value(16, &mut work).unwrap(), "alice");
        assert_eq!(node.attr_value("", "id", &mut work).unwrap(), Some("7"));
        assert_eq!(scans.reserve(body.len() as u64, 1).unwrap(), 0);
        // XML reads leave the canonical replay bytes unchanged.
        assert_eq!(
            session.ctx().request_body.cached_len().unwrap(),
            Some(body.len() as u64)
        );
    }
}

#[test]
fn xml_scanner_overflow_invalid_utf8_and_exhaustion_return_accounted_outcomes() {
    let mut scanner = XmlBodyScanner::new(ValueWorkBudget::default());
    assert!(scanner.requires_blocking_context());
    assert_eq!(scanner.push(b"<r/>\xff").unwrap(), ScanControl::Continue);
    assert!(matches!(scanner.finish().unwrap().result, Ok(Value::Nil)));
    let mut scanner = XmlBodyScanner::new(ValueWorkBudget::default());
    assert_eq!(scanner.push(&vec![b'x'; MAX_XML_BYTES + 1]).unwrap(), ScanControl::Stop);
    assert!(scanner.source.is_empty());
    assert!(matches!(
        scanner.finish().unwrap().result,
        Err(RuntimeError::BodyAccess { .. })
    ));
    let mut scanner = XmlBodyScanner::new(ValueWorkBudget::exhausted());
    assert_eq!(scanner.push(b"<r/>").unwrap(), ScanControl::Stop);
    assert!(scanner.source.is_empty());
    let outcome = scanner.finish().unwrap();
    assert!(matches!(outcome.result, Err(RuntimeError::BodyAccess { .. })));
}

#[tokio::test]
async fn xml_physical_preflight_rejects_before_scanning_or_budget_debit() {
    let mut session = MockPluginSession::new();
    session.expect_declares_body().return_const(Some(true));
    session.expect_request_body_available_len().return_const(Ok(100));
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 16;
    session.expect_ctx().return_const(ctx);
    let mut scans = BodyScanBudget::default();
    let limits = VmLimits::default();
    let runtime = DslRuntime::default();
    let mut ops = DslBodyOps::new(&mut session, &mut scans, &limits, &runtime);
    assert!(matches!(
        ops.parse_xml(ValueWorkBudget::default()).await,
        Err(RuntimeError::BodyAccess { .. })
    ));
    assert_eq!(scans.reserve(16, 1).unwrap(), 16);
}

#[tokio::test]
async fn xml_vm_traversal_construction_and_nil_contract() {
    let source = r#"
        let root = xml.parse("<r xmlns='urn:r'>a<c v='7'>one</c><c>two</c>b</r>")
        if xml.kind(root) != "element" { return deny(400, nil) }
        if xml.name(root) != "r" { return deny(401, nil) }
        if xml.namespace(root) != "urn:r" { return deny(402, nil) }
        if xml.child_count(root) != 4 { return deny(403, nil) }
        if xml.kind(xml.child(root, 0)) != "text" { return deny(404, nil) }
        let first = xml.find(root, "urn:r", "c", 0)
        if xml.attribute_count(first) != 1 { return deny(405, nil) }
        if xml.value(xml.attribute(first, 0)) != "7" { return deny(406, nil) }
        if xml.value(xml.find(root, "urn:r", "c", 1)) != "two" { return deny(407, nil) }
        if xml.attr_value(first, "", "v") != "7" { return deny(408, nil) }
        if xml.find(root, "urn:r", "c", 2) != nil { return deny(409, nil) }
        if xml.child(root, -1) != nil { return deny(410, nil) }
        if xml.name(xml.child(root, 0)) != nil { return deny(411, nil) }
        if xml.value(nil) != nil { return deny(412, nil) }
        let made = xml.element("", "out", {"a": xml.attr("", "id", "7")}, {"a": xml.text("<&"), "b": first})
        let reparsed = xml.parse(xml.stringify(made))
        if xml.value(reparsed) != "<&one" { return deny(413, nil) }
        return next()
    "#;
    let mut session = MockPluginSession::new();
    let result = vm(source, VmLimits::default())
        .execute(&mut session, &mut PluginLog::new("test"), &DslRuntime::default())
        .await
        .unwrap();
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn xml_body_failures_and_output_cap_bypass_ignore_without_publication() {
    let source = "xml.set_body(xml.element(\"\", \"r\", {}, {\"text\": xml.text(\"1234567890\")}))\nreturn next()";
    let mut session = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 8;
    session.expect_ctx().return_const(ctx);
    session.expect_overwrite_request_body().times(0);
    session.expect_record_outcome().return_const(());
    let machine = vm(source, VmLimits::default());
    let result = execute_safe(
        &machine,
        &mut session,
        &mut PluginLog::new("test"),
        &DslErrorPolicy::Ignore,
        &DslRuntime::default(),
    )
    .await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: Some(Bytes::from_static(b"Request body processing failed"))
        }
    );
}

#[tokio::test]
async fn xml_set_body_serializes_then_calls_one_canonical_overwrite() {
    let source = "xml.set_body(xml.element(\"\", \"r\", {}, {\"text\": xml.text(\"<&\")}))\nreturn next()";
    let mut session = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 64;
    session.expect_ctx().return_const(ctx);
    session
        .expect_overwrite_request_body()
        .withf(|body| body.as_ref() == b"<r>&lt;&amp;</r>")
        .times(1)
        .returning(|_| Ok(()));
    assert_eq!(
        vm(source, VmLimits::default())
            .execute(&mut session, &mut PluginLog::new("test"), &DslRuntime::default())
            .await
            .unwrap(),
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn xml_plain_string_limits_and_bad_types_keep_typed_contracts() {
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let runtime = DslRuntime::default();
    assert!(matches!(
        vm("xml.kind(1)", VmLimits::default())
            .execute(&mut session, &mut log, &runtime)
            .await,
        Err(RuntimeError::TypeError { .. })
    ));
    assert!(matches!(
        vm("to_str(xml.text(\"x\"))", VmLimits::default())
            .execute(&mut session, &mut log, &runtime)
            .await,
        Err(RuntimeError::TypeError { got: "Xml", .. })
    ));
    let limits = VmLimits {
        max_string_len: 3,
        ..Default::default()
    };
    assert!(matches!(
        vm("xml.stringify(xml.element(\"\", \"r\", {}, {}))", limits)
            .execute(&mut session, &mut log, &runtime)
            .await,
        Err(RuntimeError::StringTooLong { limit: 3, .. })
    ));
    assert!(matches!(
        vm("xml.element(\"\", \"bad:name\", {}, {})", VmLimits::default())
            .execute(&mut session, &mut log, &runtime)
            .await,
        Err(RuntimeError::ApiError { .. })
    ));
    assert_eq!(
        vm(
            "if xml.parse(\"<!DOCTYPE r><r/>\") != nil { return deny(400, nil) }",
            VmLimits::default()
        )
        .execute(&mut session, &mut log, &runtime)
        .await
        .unwrap(),
        PluginRunningResult::GoodNext
    );
}
