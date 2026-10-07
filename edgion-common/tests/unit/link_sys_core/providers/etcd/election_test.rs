use crate::link_sys_core::providers::etcd::EtcdBootstrapConfig;
use crate::link_sys_core::providers::etcd::EtcdLinkClient;

/// Build + connect an `EtcdLinkClient` against a local etcd. Inlined (rather
/// than sharing ops.rs's private test helper) to keep the test modules
/// independent — see Task 4 deviation note.
async fn test_client() -> EtcdLinkClient {
    let config = EtcdBootstrapConfig {
        endpoints: vec!["http://localhost:2379".to_string()],
        auth: None,
        tls: None,
        timeout: None,
        keep_alive: None,
        namespace: None,
    };
    let client =
        EtcdLinkClient::from_bootstrap("default/etcd-test", config).expect("bootstrap construction must succeed");
    client.init().await.expect("etcd init must connect");
    client
}

#[tokio::test]
#[ignore = "requires a real etcd serving v3election on localhost:2379"]
async fn campaign_then_leader_then_resign() {
    let client = test_client().await;
    let mut sess = client.election_session().await.unwrap();
    let lease = client.lease_grant(30).await.unwrap();
    let lk = sess
        .campaign("edgion/leader-test", b"ctrl-a".to_vec(), lease)
        .await
        .unwrap();
    let leader = sess.leader("edgion/leader-test").await.unwrap();
    assert_eq!(leader.as_deref(), Some(&b"ctrl-a"[..]));
    sess.resign(lk).await.unwrap();
    let leader = sess.leader("edgion/leader-test").await.unwrap();
    assert!(leader.is_none(), "after resign there is no leader");
}
