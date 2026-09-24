use std::time::Duration;

use edgion_resources::resources::link_sys::redis::RedisPool;

use super::*;

#[test]
fn aggregate_health_uses_any_connected_semantics() {
    assert!(!any_connected([false, false, false]));
    assert!(any_connected([false, true, false]));
    assert!(any_connected([true, true, true]));
}

#[test]
fn uninitialized_pool_has_no_connected_member() {
    let config = RedisClientConfig {
        endpoints: vec!["redis://127.0.0.1:6379".to_string()],
        auth: None,
        db: None,
        timeout: None,
        pool: Some(RedisPool { size: Some(2) }),
        topology: None,
        tls: None,
    };
    let client = RedisLinkClient::from_config("test/redis", &config, &CredentialState::NotRequired).unwrap();

    assert!(!pool_has_connected_member(client.pool()));
    let coverage = client.connection_coverage();
    assert_eq!(coverage.configured, 2);
    assert_eq!(coverage.live, 0);
}

#[tokio::test]
async fn init_installs_egress_resolver_before_fred_connects() {
    let config = RedisClientConfig {
        endpoints: vec!["redis://169.254.169.254:6379".to_string()],
        auth: None,
        db: None,
        timeout: None,
        pool: Some(RedisPool { size: Some(1) }),
        topology: None,
        tls: None,
    };
    let client = RedisLinkClient::from_config("test/blocked-redis", &config, &CredentialState::NotRequired).unwrap();

    let error = tokio::time::timeout(Duration::from_secs(1), client.init())
        .await
        .expect("egress rejection must happen before the connection timeout")
        .expect_err("link-local Redis targets must be rejected");

    assert!(
        error
            .to_string()
            .contains("egress policy blocked all resolved addresses"),
        "unexpected initialization error: {error}"
    );
}
