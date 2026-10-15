use super::{validate_etcd_identity, EtcdIdentityError};
use crate::conf_mgr::conf_center::common::{
    clear_runtime_invalid, get_runtime_invalid, parse_resource_content, store_runtime_invalid, RuntimeInvalidEnvelopes,
};
use crate::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;
use crate::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, LocalRawStore, ProcessResult, ProcessorHandler, ResourceProcessor, WorkItemResult,
};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::HTTPRoute;
use std::sync::Arc;

const CONF_PREFIX: &str = "/edgion/conf/";

#[test]
fn tls_route_etcd_content_parser_rejects_noncanonical_versions() {
    use edgion_resources::TLSRoute;
    // InitApply and runtime Apply both call this parser before raw-store insertion.
    for version in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!(42)),
        Some(serde_json::json!("gateway.networking.k8s.io/v1alpha2")),
        Some(serde_json::json!("gateway.networking.k8s.io/v1")),
    ] {
        let valid = version.as_ref().and_then(serde_json::Value::as_str) == Some("gateway.networking.k8s.io/v1");
        let mut value = serde_json::json!({
            "kind": "TLSRoute", "metadata": {"name": "route", "namespace": "default"},
            "spec": {"hostnames": ["example.com"]}
        });
        if let Some(version) = version {
            value["apiVersion"] = version;
        }
        let parsed = parse_resource_content::<TLSRoute>(&value.to_string());
        assert_eq!(parsed.is_ok(), valid);
        if let Ok(route) = parsed {
            assert_eq!(route.metadata.name.as_deref(), Some("route"));
            assert_eq!(route.spec.hostnames.as_deref(), Some(&["example.com".to_string()][..]));
        }
    }
}

#[test]
fn namespaced_identity_match_returns_key_identity() {
    assert_eq!(
        validate_etcd_identity(
            "/edgion/conf/HTTPRoute/default/r1",
            CONF_PREFIX,
            "HTTPRoute",
            "default/r1"
        ),
        Ok("default/r1".to_string())
    );
}

#[test]
fn namespaced_identity_mismatch_returns_key_identity() {
    assert_eq!(
        validate_etcd_identity(
            "/edgion/conf/HTTPRoute/default/r1",
            CONF_PREFIX,
            "HTTPRoute",
            "other/r2"
        ),
        Err(EtcdIdentityError::Mismatch {
            key_identity: "default/r1".to_string()
        })
    );
}

#[test]
fn cluster_scoped_identity_match_returns_key_identity() {
    assert_eq!(
        validate_etcd_identity("/edgion/conf/GatewayClass/_/gc1", CONF_PREFIX, "GatewayClass", "gc1"),
        Ok("gc1".to_string())
    );
}

#[test]
fn empty_content_name_is_rejected() {
    assert_eq!(
        validate_etcd_identity(
            "/edgion/conf/HTTPRoute/default/r1",
            CONF_PREFIX,
            "HTTPRoute",
            "default/"
        ),
        Err(EtcdIdentityError::Mismatch {
            key_identity: "default/r1".to_string()
        })
    );
}

#[test]
fn missing_name_in_etcd_key_is_rejected() {
    assert_eq!(
        validate_etcd_identity("/edgion/conf/HTTPRoute/default", CONF_PREFIX, "HTTPRoute", "default/r1"),
        Err(EtcdIdentityError::MalformedKey)
    );
}

#[test]
fn empty_name_in_etcd_key_is_rejected() {
    assert_eq!(
        validate_etcd_identity("/edgion/conf/HTTPRoute/default/", CONF_PREFIX, "HTTPRoute", "default/"),
        Err(EtcdIdentityError::MalformedKey)
    );
}

#[test]
fn extra_key_segment_is_rejected() {
    assert_eq!(
        validate_etcd_identity(
            "/edgion/conf/HTTPRoute/default/r1/extra",
            CONF_PREFIX,
            "HTTPRoute",
            "default/r1"
        ),
        Err(EtcdIdentityError::MalformedKey)
    );
}

#[test]
fn key_prefix_may_contain_conf_segment() {
    assert_eq!(
        validate_etcd_identity(
            "/tenant/conf/edgion/conf/HTTPRoute/default/r1",
            "/tenant/conf/edgion/conf/",
            "HTTPRoute",
            "default/r1"
        ),
        Ok("default/r1".to_string())
    );
}

#[test]
fn nested_backend_marker_after_conf_prefix_is_rejected() {
    assert_eq!(
        validate_etcd_identity(
            "/edgion/conf/HTTPRoute/junk/x/conf/HTTPRoute/default/r1",
            CONF_PREFIX,
            "HTTPRoute",
            "default/r1"
        ),
        Err(EtcdIdentityError::MalformedKey)
    );
}

#[test]
fn valid_event_clears_invalid_envelope() {
    let pending = RuntimeInvalidEnvelopes::default();
    store_runtime_invalid(
        &pending,
        "default/r1".to_string(),
        "IdentityMismatch",
        "mismatch".to_string(),
    );
    clear_runtime_invalid(&pending, "default/r1");
    assert_eq!(get_runtime_invalid(&pending, "default/r1"), None);
}

#[test]
fn invalid_envelope_remains_until_valid_repair() {
    let pending = RuntimeInvalidEnvelopes::default();
    store_runtime_invalid(
        &pending,
        "default/r1".to_string(),
        "ParseError",
        "malformed".to_string(),
    );
    assert_eq!(
        get_runtime_invalid(&pending, "default/r1").unwrap().reason,
        "ParseError"
    );
    assert!(get_runtime_invalid(&pending, "default/r1").is_some());
    clear_runtime_invalid(&pending, "default/r1");
    assert!(get_runtime_invalid(&pending, "default/r1").is_none());
}

struct NoopHttpRouteHandler;

#[async_trait::async_trait]
impl ProcessorHandler<HTTPRoute> for NoopHttpRouteHandler {
    async fn parse(&self, obj: HTTPRoute, _ctx: &HandlerContext) -> ProcessResult<HTTPRoute> {
        ProcessResult::Continue(obj)
    }
}

#[tokio::test]
async fn etcd_valid_to_malformed_to_repair_revokes_and_recovers() {
    const VALID: &str = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  hostnames:
    - valid.example.com
  rules: []
"#;
    const REPAIRED: &str = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  hostnames:
    - repaired.example.com
  rules: []
"#;
    let key = "default/r1";
    let processor = Arc::new(ResourceProcessor::<HTTPRoute>::new(
        "HTTPRoute",
        100,
        Arc::new(NoopHttpRouteHandler),
        Arc::new(ControllerRuntime::default()),
    ));
    let raw_store = LocalRawStore::<HTTPRoute>::new();
    let invalid = RuntimeInvalidEnvelopes::default();
    let valid = parse_resource_content::<HTTPRoute>(VALID).unwrap();
    raw_store.upsert(key.to_string(), Arc::new(valid.clone()));
    assert!(matches!(
        processor.on_init_apply(valid, None).await,
        WorkItemResult::Processed { .. }
    ));

    // This mirrors the runtime Apply(parse error) event transition. The
    // envelope stays authoritative until a later valid event clears it.
    raw_store.remove(key);
    store_runtime_invalid(
        &invalid,
        key.to_string(),
        "ParseError",
        "malformed resource".to_string(),
    );
    assert!(get_runtime_invalid(&invalid, key).is_some());
    assert!(matches!(
        processor
            .process_work_item(key, None, None, TriggerChain::default())
            .await,
        WorkItemResult::Deleted { .. }
    ));
    assert!(processor.get(key).is_none());
    assert!(get_runtime_invalid(&invalid, key).is_some());

    let repaired = parse_resource_content::<HTTPRoute>(REPAIRED).unwrap();
    clear_runtime_invalid(&invalid, key);
    raw_store.upsert(key.to_string(), Arc::new(repaired.clone()));
    assert!(matches!(
        processor
            .process_work_item(key, Some(repaired), None, TriggerChain::default())
            .await,
        WorkItemResult::Processed { .. }
    ));
    assert_eq!(
        processor.get(key).unwrap().spec.hostnames.as_deref(),
        Some(&["repaired.example.com".to_string()][..])
    );
    assert!(get_runtime_invalid(&invalid, key).is_none());
}

fn runtime_controller(prefix: &str) -> super::EtcdResourceController<HTTPRoute> {
    use edgion_common::link_sys_core::providers::etcd::{EtcdBootstrapConfig, EtcdLinkClient};
    let client = EtcdLinkClient::from_bootstrap(
        "test/invalid-ingress",
        EtcdBootstrapConfig {
            endpoints: vec!["http://127.0.0.1:2379".into()],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: None,
        },
    )
    .unwrap();
    let processor = Arc::new(ResourceProcessor::new(
        "HTTPRoute",
        100,
        Arc::new(NoopHttpRouteHandler),
        Arc::new(ControllerRuntime::default()),
    ));
    let (_, rx) = tokio::sync::mpsc::channel(256);
    super::EtcdResourceController::new(
        "HTTPRoute",
        processor,
        Arc::new(super::EtcdStatusHandler::new(
            Arc::new(client),
            prefix.trim_end_matches("/conf/").to_owned(),
        )),
        rx,
        Arc::from(prefix),
    )
}

fn route_document(host: &str) -> super::EtcdDocument {
    super::EtcdDocument::Text(format!("apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: r1\n  namespace: default\nspec:\n  hostnames: [{host}]\n  rules: []\n"))
}

// Exercise the real queue and processor without status RPCs. Ingress is performed
// exclusively through the production controller methods below.
async fn process_next(controller: &super::EtcdResourceController<HTTPRoute>) -> WorkItemResult<HTTPRoute> {
    let queue = controller.processor.workqueue();
    let item = tokio::time::timeout(std::time::Duration::from_secs(1), queue.dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.key, "default/r1");
    let raw = controller.fs_raw_store.get(&item.key).map(|obj| (*obj).clone());
    let result = controller
        .processor
        .process_work_item(&item.key, raw, None, item.trigger_chain)
        .await;
    queue.done(&item.key);
    result
}

#[tokio::test]
async fn invalid_utf8_runtime_revokes_recovers_and_clears_on_delete() {
    let controller = runtime_controller(CONF_PREFIX);
    let key = "/edgion/conf/HTTPRoute/default/r1";
    controller
        .apply_runtime_document(key, &route_document("valid.example.com"))
        .await;
    assert!(matches!(
        process_next(&controller).await,
        WorkItemResult::Processed { .. }
    ));
    controller
        .apply_runtime_document(key, &super::EtcdDocument::InvalidUtf8)
        .await;
    assert!(controller.fs_raw_store.get("default/r1").is_none());
    let invalid = get_runtime_invalid(&controller.invalid_envelopes, "default/r1").unwrap();
    assert_eq!(invalid.reason, "ParseError");
    assert_eq!(
        invalid.message,
        "etcd resource value is not valid UTF-8; replace it with UTF-8 YAML"
    );
    assert!(matches!(
        process_next(&controller).await,
        WorkItemResult::Deleted { .. }
    ));
    assert!(controller.processor.get("default/r1").is_none());
    controller
        .apply_runtime_document(key, &super::EtcdDocument::InvalidUtf8)
        .await;
    assert_eq!(
        get_runtime_invalid(&controller.invalid_envelopes, "default/r1").unwrap(),
        invalid
    );
    assert!(matches!(process_next(&controller).await, WorkItemResult::Skipped));
    controller
        .apply_runtime_document(key, &route_document("repaired.example.com"))
        .await;
    assert!(get_runtime_invalid(&controller.invalid_envelopes, "default/r1").is_none());
    assert!(matches!(
        process_next(&controller).await,
        WorkItemResult::Processed { .. }
    ));
    assert_eq!(
        controller
            .processor
            .get("default/r1")
            .unwrap()
            .spec
            .hostnames
            .as_deref(),
        Some(&["repaired.example.com".to_owned()][..])
    );
    controller
        .apply_runtime_document(key, &super::EtcdDocument::InvalidUtf8)
        .await;
    assert!(matches!(
        process_next(&controller).await,
        WorkItemResult::Deleted { .. }
    ));
    controller.delete_runtime_document(key).await;
    assert!(get_runtime_invalid(&controller.invalid_envelopes, "default/r1").is_none());
    assert!(matches!(process_next(&controller).await, WorkItemResult::Skipped));
}

#[tokio::test]
async fn invalid_utf8_coalesces_to_latest_repair_or_delete() {
    let controller = runtime_controller(CONF_PREFIX);
    let key = "/edgion/conf/HTTPRoute/default/r1";
    controller
        .apply_runtime_document(key, &route_document("valid.example.com"))
        .await;
    let _ = process_next(&controller).await;
    controller
        .apply_runtime_document(key, &super::EtcdDocument::InvalidUtf8)
        .await;
    controller
        .apply_runtime_document(key, &route_document("repaired.example.com"))
        .await;
    assert!(matches!(
        process_next(&controller).await,
        WorkItemResult::Processed { .. }
    ));
    assert!(get_runtime_invalid(&controller.invalid_envelopes, "default/r1").is_none());
    assert_eq!(
        controller
            .processor
            .get("default/r1")
            .unwrap()
            .spec
            .hostnames
            .as_deref(),
        Some(&["repaired.example.com".to_owned()][..])
    );
    controller
        .apply_runtime_document(key, &super::EtcdDocument::InvalidUtf8)
        .await;
    controller.delete_runtime_document(key).await;
    assert!(matches!(
        process_next(&controller).await,
        WorkItemResult::Deleted { .. }
    ));
    assert!(controller.processor.get("default/r1").is_none());
    assert!(controller.fs_raw_store.get("default/r1").is_none());
    assert!(get_runtime_invalid(&controller.invalid_envelopes, "default/r1").is_none());
    assert_eq!(controller.processor.workqueue().len(), 0);
}

#[tokio::test]
async fn invalid_utf8_respects_exact_configured_key_identity() {
    let prefix = "/tenant/conf/edgion/conf/";
    let controller = runtime_controller(prefix);
    let valid_key = format!("{prefix}HTTPRoute/default/r1");
    controller
        .apply_runtime_document(&valid_key, &route_document("valid.example.com"))
        .await;
    let _ = process_next(&controller).await;
    for key in [
        "/edgion/conf/HTTPRoute/default/r1",
        "/tenant/conf/edgion/conf/HTTPRoute/default/",
        "/tenant/conf/edgion/conf/HTTPRoute/default/r1/extra",
        "/tenant/conf/edgion/conf/Secret/default/r1",
        "/tenant/conf/edgion/conf/HTTPRoute//r1",
    ] {
        controller
            .apply_runtime_document(key, &super::EtcdDocument::InvalidUtf8)
            .await;
        assert_eq!(controller.processor.workqueue().len(), 0);
        assert!(controller.fs_raw_store.get("default/r1").is_some());
        assert!(get_runtime_invalid(&controller.invalid_envelopes, "default/r1").is_none());
    }
    controller
        .apply_runtime_document(&valid_key, &super::EtcdDocument::InvalidUtf8)
        .await;
    assert!(matches!(
        process_next(&controller).await,
        WorkItemResult::Deleted { .. }
    ));
}

#[test]
fn invalid_document_parser_has_fixed_deduplicated_error() {
    use crate::conf_mgr::conf_center::common::ErrorStatus;
    let error = super::parse_etcd_document::<HTTPRoute>(&super::EtcdDocument::InvalidUtf8)
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "etcd resource value is not valid UTF-8; replace it with UTF-8 YAML"
    );
    let status = ErrorStatus::error_for_persist("ParseError", &error, None).unwrap();
    let persisted = serde_json::to_string(&status).unwrap();
    assert!(ErrorStatus::error_for_persist("ParseError", &error, Some(&persisted)).is_none());
    assert!(super::parse_etcd_document::<HTTPRoute>(&route_document("valid.example.com")).is_ok());
    assert!(super::parse_etcd_document::<HTTPRoute>(&super::EtcdDocument::Text("not: [yaml".into())).is_err());
}
