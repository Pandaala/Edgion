use super::*;
use crate::conf_mgr::conf_center::etcd::leadership_gate::{LeaderState, LeadershipGate};
use edgion_common::link_sys_core::providers::etcd::EtcdBootstrapConfig;
use std::time::Duration;

/// Build + connect an `EtcdLinkClient` against a local etcd, mirroring the
/// `leader_election.rs` test helper.
async fn test_link_client() -> EtcdLinkClient {
    let config = EtcdBootstrapConfig {
        endpoints: vec!["http://localhost:2379".to_string()],
        auth: None,
        tls: None,
        timeout: None,
        keep_alive: None,
        namespace: None,
    };
    let client = EtcdLinkClient::from_bootstrap("default/etcd-storage-test", config)
        .expect("bootstrap construction must succeed");
    client.init().await.expect("etcd init must connect");
    client
}

/// A non-leader (default `is_leader=false`) `LeadershipGate` must fence every
/// mutating storage op. `set_one` must return an error without touching etcd.
#[tokio::test]
#[ignore = "requires a real etcd on localhost:2379"]
async fn storage_write_blocked_when_not_leader() {
    let client = Arc::new(test_link_client().await);
    // Default LeaderState has is_leader=false, so the gate is closed.
    let st = LeaderState::new(Duration::from_secs(15));
    let gate = LeadershipGate::from_state(st);
    let storage = EtcdStorage::new_gated(client, "/edgion-test-gate".into(), gate);

    let err = storage
        .set_one("EdgionRoute", Some("default"), "r1", "content".to_string())
        .await;
    assert!(err.is_err(), "a non-leader must not write to etcd");
}

#[test]
fn failed_mod_revision_compare_is_a_retryable_conflict() {
    let err = cas_conflict("HTTPRoute", Some("default"), "route", 42).expect_err("failed compare must return an error");
    match err {
        ConfWriterError::Conflict(message) => {
            assert!(message.contains("revision 42"));
            assert!(message.contains("retry"));
            assert!(message.contains("HTTPRoute/default/route"));
        }
        other => panic!("expected conflict, got {other:?}"),
    }
}

#[test]
fn update_merge_persists_api_generated_resource_version() {
    let current = r#"
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: data
  namespace: default
  resourceVersion: "17"
  uid: uid-1
spec:
  enable: true
  visibility: Namespace
  data:
    type: Misc
    config: {old: true}
"#;
    let requested = r#"
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: data
  namespace: default
  resourceVersion: "18"
spec:
  enable: true
  visibility: Namespace
  data:
    type: Misc
    config: {new: true}
"#;

    let merged = merge_operator_update_yaml(current, requested).expect("etcd update merge must succeed");
    let merged: yaml_serde::Value = yaml_serde::from_str(&merged).expect("merged resource must be valid YAML");
    assert_eq!(merged["metadata"]["resourceVersion"], "18");
    assert_eq!(merged["metadata"]["uid"], "uid-1");
    assert_eq!(merged["spec"]["data"]["config"]["new"], true);
}

#[test]
fn stale_expected_resource_version_stops_etcd_update_before_txn() {
    let current = r#"metadata: {resourceVersion: "17"}"#;
    let error = ensure_expected_resource_version(current, Some("16"))
        .expect_err("stale etcd update must conflict before its mod-revision transaction");
    assert!(matches!(error, ConfWriterError::Conflict(_)));
}
