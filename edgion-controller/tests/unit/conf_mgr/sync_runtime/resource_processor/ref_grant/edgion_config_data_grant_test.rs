use super::*;
use crate::conf_mgr::ControllerRuntime;
const EDGION_GROUP: &str = "edgion.io";
use edgion_resources::resources::{ReferenceGrant, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::sync::Arc;
use ConfigDataVisibility::*;

#[test]
fn truth_table() {
    // flag off → always allowed (no-op), regardless of everything else
    assert!(decide_edgion_config_data_ref(false, false, Namespace, false));
    // same namespace → always allowed
    assert!(decide_edgion_config_data_ref(true, true, Namespace, false));
    // cross-ns, visibility Cluster → allowed without grant
    assert!(decide_edgion_config_data_ref(true, false, Cluster, false));
    // cross-ns, Namespace, grant allows → allowed
    assert!(decide_edgion_config_data_ref(true, false, Namespace, true));
    // cross-ns, Namespace, no grant → DENIED
    assert!(!decide_edgion_config_data_ref(true, false, Namespace, false));
}

fn grant(from_group: &str, from_kind: &str) -> ReferenceGrant {
    ReferenceGrant {
        metadata: ObjectMeta {
            namespace: Some("target".to_string()),
            name: Some("allow-policy".to_string()),
            ..Default::default()
        },
        spec: ReferenceGrantSpec {
            from: vec![ReferenceGrantFrom {
                group: from_group.to_string(),
                kind: from_kind.to_string(),
                namespace: "source".to_string(),
            }],
            to: vec![ReferenceGrantTo {
                group: EDGION_GROUP.to_string(),
                kind: "EdgionConfigData".to_string(),
                name: Some("policy".to_string()),
            }],
        },
    }
}

#[test]
fn gateway_grant_requires_exact_gateway_api_group() {
    let runtime = ControllerRuntime::default();
    runtime.ref_grant_store.upsert(
        "target/allow-policy".to_string(),
        Arc::new(grant(GATEWAY_GROUP, "Gateway")),
    );

    let owner: edgion_resources::Gateway = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"gateway","namespace":"source","annotations":{
            "edgion.io/inbound-proxy-protocol": r#"{"source":"resourceRef","resourceRef":{"name":"policy","namespace":"target"}}"#
        }},"spec":{"gatewayClassName":"public"}
    })).unwrap();
    let field = edgion_resources::resources::refs::proxy_protocol_fields::GatewayProxyProtocolField::parse(&owner)
        .unwrap()
        .unwrap();
    let query = field.query().unwrap().unwrap();
    assert!(decide_edgion_config_data_ref(
        true,
        false,
        Namespace,
        runtime.ref_grant_store.check_query(query)
    ));
    runtime
        .ref_grant_store
        .upsert("target/allow-policy".into(), Arc::new(grant(EDGION_GROUP, "Gateway")));
    assert!(!decide_edgion_config_data_ref(
        true,
        false,
        Namespace,
        runtime.ref_grant_store.check_query(query)
    ));
}

#[test]
fn existing_plugin_helper_keeps_edgion_group() {
    let runtime = ControllerRuntime::default();
    runtime.ref_grant_store.upsert(
        "target/allow-policy".to_string(),
        Arc::new(grant(EDGION_GROUP, "EdgionPlugins")),
    );

    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"plugins","namespace":"source"},"spec":{"requestPlugins":[{
            "type":"Canary","config":{"enable":true,"activeProfile":"base","profiles":{"base":{"name":"backend"}},"activeProfileRef":{"name":"policy","namespace":"target"}}
        }]}
    }))
    .unwrap();
    let entry =
        edgion_resources::resources::refs::config_data_fields::HttpConfigDataEntry::from_plugins(&owner, 0).unwrap();
    let field = entry.field().unwrap();
    assert!(decide_edgion_config_data_ref(
        true,
        false,
        Namespace,
        runtime.ref_grant_store.check_query(field.query().unwrap())
    ));
}
