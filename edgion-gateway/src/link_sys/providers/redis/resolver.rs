//! Fred adapter for Edgion's canonical DNS and egress policy.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_common::dns::ResolverSpec;
use fred::error::{Error, ErrorKind};
use fred::types::Resolve;

use crate::plugins::http::common::outbound::egress::{
    default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
};

pub(super) fn redis_egress_resolver() -> Arc<dyn Resolve> {
    Arc::new(RedisEgressResolver::new(
        EgressPolicy {
            block_private: default_block_private(),
        },
        egress_dns_cache_ttl(),
    ))
}

#[cfg(feature = "test-util")]
pub(super) fn redis_egress_resolver_for_test() -> Arc<dyn Resolve> {
    Arc::new(RedisEgressResolver::new_for_test(
        EgressPolicy {
            block_private: default_block_private(),
        },
        egress_dns_cache_ttl(),
    ))
}

struct RedisEgressResolver {
    inner: EgressResolver,
}

impl RedisEgressResolver {
    fn new(policy: EgressPolicy, ttl: std::time::Duration) -> Self {
        Self {
            inner: EgressResolver::new(ResolverSpec::System, policy, ttl),
        }
    }

    #[cfg(feature = "test-util")]
    fn new_for_test(policy: EgressPolicy, ttl: std::time::Duration) -> Self {
        Self {
            inner: EgressResolver::new_for_test(ResolverSpec::System, policy, ttl),
        }
    }
}

#[async_trait]
impl Resolve for RedisEgressResolver {
    async fn resolve(&self, host: fred::bytes_utils::Str, port: u16) -> Result<Vec<std::net::SocketAddr>, Error> {
        self.inner
            .resolve_socket_addrs(&host, port)
            .await
            .map_err(|message| Error::new(ErrorKind::IO, message))
    }
}

#[cfg(test)]
mod tests {
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
}
