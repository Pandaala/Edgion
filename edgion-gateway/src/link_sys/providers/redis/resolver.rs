//! redis-rs adapter for Edgion's canonical DNS and egress policy.

use std::sync::Arc;

use edgion_common::dns::ResolverSpec;

use crate::plugins::http::common::outbound::egress::{
    default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
};

#[derive(Clone)]
pub(super) struct RedisEgressResolver {
    inner: Arc<EgressResolver>,
}

impl RedisEgressResolver {
    fn new(policy: EgressPolicy, ttl: std::time::Duration) -> Self {
        Self {
            inner: Arc::new(EgressResolver::new(ResolverSpec::System, policy, ttl)),
        }
    }

    #[cfg(feature = "test-util")]
    fn new_for_test(policy: EgressPolicy, ttl: std::time::Duration) -> Self {
        Self {
            inner: Arc::new(EgressResolver::new_for_test(ResolverSpec::System, policy, ttl)),
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/redis/resolver_test.rs"]
mod tests;

impl redis::io::AsyncDNSResolver for RedisEgressResolver {
    fn resolve<'a, 'b: 'a>(
        &'a self,
        host: &'b str,
        port: u16,
    ) -> redis::RedisFuture<'a, Box<dyn Iterator<Item = std::net::SocketAddr> + Send + 'a>> {
        Box::pin(async move {
            let addresses = self.inner.resolve_socket_addrs(host, port).await.map_err(|_| {
                redis::RedisError::from(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Redis egress resolution failed",
                ))
            })?;
            Ok(Box::new(addresses.into_iter()) as Box<dyn Iterator<Item = std::net::SocketAddr> + Send>)
        })
    }
}

pub(super) fn redis_rs_resolver() -> RedisEgressResolver {
    RedisEgressResolver::new(
        EgressPolicy {
            block_private: default_block_private(),
        },
        egress_dns_cache_ttl(),
    )
}
#[cfg(feature = "test-util")]
pub(super) fn redis_rs_resolver_for_test() -> RedisEgressResolver {
    RedisEgressResolver::new_for_test(
        EgressPolicy {
            block_private: default_block_private(),
        },
        egress_dns_cache_ttl(),
    )
}
