use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use super::*;

#[tokio::test]
async fn rejects_link_local_addresses_before_redis_dial() {
    let resolver = RedisEgressResolver::new(EgressPolicy::default(), Duration::from_secs(5));

    let error = resolver
        .resolve("169.254.169.254".into(), 6379)
        .await
        .expect_err("link-local metadata addresses must be rejected");

    assert_eq!(error.kind(), &ErrorKind::IO);
    assert_eq!(
        error.to_string(),
        "IO Error: egress policy blocked all resolved addresses"
    );
}

#[tokio::test]
async fn applies_private_address_policy_and_preserves_port() {
    let strict = RedisEgressResolver::new(EgressPolicy { block_private: true }, Duration::from_secs(5));
    assert!(strict.resolve("10.0.0.1".into(), 6380).await.is_err());

    let permissive = RedisEgressResolver::new(EgressPolicy::default(), Duration::from_secs(5));
    assert_eq!(
        permissive.resolve("10.0.0.1".into(), 6380).await.unwrap(),
        vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 6380)]
    );
}
