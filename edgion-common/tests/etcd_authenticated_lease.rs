//! Real authenticated SDK coverage against a dedicated loopback Etcd fixture.
//! The caller owns the fixture and cleanup; no shared Gateway/Controller topology.
use edgion_common::link_sys_core::providers::etcd::{EtcdBootstrapConfig, EtcdLinkClient};
use edgion_resources::credentials::materials::BasicAuthMaterial;
use std::time::Duration;

fn client(endpoint: &str, password: &str) -> EtcdLinkClient {
    EtcdLinkClient::from_bootstrap(
        "dependency-auth-fixture",
        EtcdBootstrapConfig {
            endpoints: vec![endpoint.to_string()],
            auth: Some(BasicAuthMaterial {
                username: Some("root".to_string()),
                password: password.to_string(),
            }),
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: None,
        },
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires dedicated authenticated Etcd 3.7+ via EDGION_ETCD_AUTH_ENDPOINT"]
async fn authenticated_lease_watch_lock_and_election() {
    let endpoint = std::env::var("EDGION_ETCD_AUTH_ENDPOINT").expect("set the dedicated loopback fixture endpoint");
    let parsed = url::Url::parse(&endpoint).unwrap();
    assert_eq!(parsed.scheme(), "http");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!(parsed.username().is_empty() && parsed.password().is_none());
    tokio::time::timeout(Duration::from_secs(30), async {
        let invalid = client(&endpoint, "incorrect-fixture-password");
        assert!(
            invalid.init().await.is_err(),
            "fixture must reject incorrect credentials"
        );
        invalid.shutdown().await.unwrap();
        let client = client(&endpoint, "integration-etcd-password");
        client.init().await.unwrap();
        let prefix = format!("/edgion-dependency-auth-{}/", std::process::id());
        let key = format!("{prefix}value");
        let lease = client.lease_grant(30).await.unwrap();
        assert!(client.lease_time_to_live(lease).await.unwrap() > 0);
        client.put(&key, b"first".to_vec(), Some(lease)).await.unwrap();
        assert_eq!(client.get_string(&key).await.unwrap().as_deref(), Some("first"));
        let (mut keeper, mut keepalive) = client.lease_keep_alive(lease).await.unwrap();
        keeper.keep_alive().await.unwrap();
        let renewed = tokio::time::timeout(Duration::from_secs(5), keepalive.message())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(renewed.id(), lease);
        assert!(renewed.ttl() > 0);
        drop(keeper);
        drop(keepalive);
        let (_, revision) = client.get_prefix_with_revision(&prefix).await.unwrap();
        let mut watch = client.watch_prefix_from_revision(&prefix, revision + 1).await.unwrap();
        client.put(&key, b"second".to_vec(), Some(lease)).await.unwrap();
        let watch_id = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let response = watch
                    .message()
                    .await
                    .unwrap()
                    .expect("watch ended before the expected event");
                assert!(!response.canceled());
                if let Some(kv) = response
                    .events()
                    .iter()
                    .filter_map(|event| event.kv())
                    .find(|kv| kv.key() == key.as_bytes() && kv.value() == b"second")
                {
                    assert!(kv.mod_revision() > revision);
                    break response.watch_id();
                }
            }
        })
        .await
        .unwrap();
        watch.cancel(watch_id).await.unwrap();
        drop(watch);
        let lock = client
            .try_lock(&format!("{prefix}lock"), 30, Duration::from_secs(3))
            .await
            .unwrap()
            .expect("uncontended lock must be acquired");
        lock.unlock().await.unwrap();
        let mut election = client.election_session().await.unwrap();
        let election_name = format!("{prefix}election");
        let leader = election
            .campaign(&election_name, b"candidate".to_vec(), lease)
            .await
            .unwrap();
        assert_eq!(
            election.leader(&election_name).await.unwrap().as_deref(),
            Some(&b"candidate"[..])
        );
        election.resign(leader).await.unwrap();
        client.lease_revoke(lease).await.unwrap();
        assert!(client.get(&key).await.unwrap().is_none());
        client.delete_prefix(&prefix).await.unwrap();
        client.shutdown().await.unwrap();
    })
    .await
    .expect("authenticated Etcd suite exceeded its deadline");
}
