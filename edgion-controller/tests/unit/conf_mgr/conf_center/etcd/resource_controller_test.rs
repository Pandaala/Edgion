use super::{validate_etcd_identity, EtcdIdentityError};
use crate::conf_mgr::conf_center::common::{
    clear_runtime_invalid, get_runtime_invalid, parse_resource_content, store_runtime_invalid, RuntimeInvalidEnvelopes,
};
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
