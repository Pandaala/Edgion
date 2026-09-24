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
#[path = "../../../../tests/unit/link_sys/providers/redis/resolver_test.rs"]
mod tests;
