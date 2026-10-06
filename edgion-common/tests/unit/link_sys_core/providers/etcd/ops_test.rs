use super::*;
use crate::link_sys_core::credential::CredentialState;
use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;

/// Minimal config pointing at a local etcd, mirroring the fixture in
/// `client.rs`'s test module.
fn minimal_config() -> EtcdClientConfig {
    EtcdClientConfig {
        endpoints: vec!["http://localhost:2379".to_string()],
        auth: None,
        tls: None,
        timeout: None,
        keep_alive: None,
        namespace: None,
    }
}

/// Build + connect an `EtcdLinkClient` against a local etcd.
async fn test_client() -> EtcdLinkClient {
    let config = minimal_config();
    let client = EtcdLinkClient::from_config("default/etcd-test", &config, &CredentialState::NotRequired)
        .expect("from_config must succeed");
    client.init().await.expect("etcd init must connect");
    client
}

/// A per-test unique key prefix so parallel/repeated runs never collide.
fn unique_prefix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("/edgion-test/{}", nanos)
}

#[tokio::test]
#[ignore = "requires a real etcd on localhost:2379"]
async fn lease_keep_alive_refreshes_then_errors_after_revoke() {
    let client = test_client().await;
    let lease = client.lease_grant(5).await.unwrap();
    let (mut keeper, mut stream) = client.lease_keep_alive(lease).await.unwrap();
    keeper.keep_alive().await.unwrap();
    let resp = stream.message().await.unwrap().expect("a keepalive response");
    assert!(resp.ttl() > 0, "healthy lease must report ttl > 0");

    client.lease_revoke(lease).await.unwrap();
    // A re-establish against the now-dead lease must surface as Err (spec §4.2 fact 4).
    let err = client.lease_keep_alive(lease).await;
    assert!(err.is_err(), "keepalive establish on a revoked lease must Err");
}

#[tokio::test]
#[ignore = "requires a real etcd on localhost:2379"]
async fn get_kv_surfaces_lease_and_revisions() {
    let client = test_client().await;
    let lease = client.lease_grant(30).await.unwrap();
    let key = format!("{}/kvtest", unique_prefix());
    client.put(&key, "v", Some(lease)).await.unwrap();
    let kv = client.get_kv(&key).await.unwrap().expect("present");
    assert_eq!(kv.value, b"v");
    assert_eq!(kv.lease, lease);
    assert!(kv.create_revision > 0 && kv.mod_revision >= kv.create_revision);
}

#[tokio::test]
#[ignore = "requires a real etcd on localhost:2379"]
async fn guard_txn_detects_inplace_overwrite() {
    let client = test_client().await;
    let lease = client.lease_grant(30).await.unwrap();
    let key = format!("{}/guardtest", unique_prefix());
    client.put(&key, "mine", Some(lease)).await.unwrap();
    let kv = client.get_kv(&key).await.unwrap().unwrap();
    assert!(client
        .leader_guard_txn(&key, kv.create_revision, lease, b"mine")
        .await
        .unwrap());

    // Blind PUT without lease: create_revision is PRESERVED, lease is detached.
    // A create_revision-only guard would wrongly pass; ours must fail (spec §4.4).
    client.put_string(&key, "stolen").await.unwrap();
    assert!(
        !client
            .leader_guard_txn(&key, kv.create_revision, lease, b"mine")
            .await
            .unwrap(),
        "guard must reject an in-place overwrite that preserves create_revision"
    );
}

#[test]
fn namespace_projection_preserves_empty_logical_keys() {
    assert_eq!(strip_namespace("/app/", Some("/app/")), "");
    assert_eq!(strip_namespace("/app/key", Some("/app/")), "key");
    assert_eq!(strip_namespace("/café/key", Some("/café/")), "key");
    assert_eq!(strip_namespace("other", Some("/app/")), "other");
    assert_eq!(strip_namespace("key", None), "key");
    assert_eq!(strip_namespace("key", Some("")), "key");
}
