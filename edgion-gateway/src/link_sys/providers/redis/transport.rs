//! Canonical redis-rs connection construction; no other module creates sockets.
use super::resolver::RedisEgressResolver;
use edgion_common::link_sys_core::outbound_tls::RedisTlsMaterial;
use edgion_resources::resources::link_sys::redis::{RedisClientConfig, RedisTopologyMode, REDIS_DEFAULT_POOL_SIZE};
use redis::{ConnectionAddr, IntoConnectionInfo, RedisConnectionInfo, RedisResult};
use std::time::Duration;

pub(super) struct Transport {
    pub endpoints: Vec<(String, u16)>,
    pub mode: RedisTopologyMode,
    pub master: Option<String>,
    pub pool_size: usize,
    pub max_redirects: u32,
    pub command_timeout: Duration,
    pub connect_timeout: Duration,
    settings: RedisConnectionInfo,
    tls: Option<redis::TlsCertificates>,
}

impl Transport {
    pub fn new(
        config: &RedisClientConfig,
        auth: &crate::link_sys::credential::CredentialState<
            edgion_resources::credentials::materials::BasicAuthMaterial,
        >,
        tls: Option<RedisTlsMaterial>,
    ) -> anyhow::Result<Self> {
        let errors = config.validation_errors();
        anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
        let endpoints = config
            .parsed_active_endpoints()
            .map_err(|e| anyhow::anyhow!(e.join("; ")))?
            .into_iter()
            .map(|endpoint| (endpoint.host, endpoint.port))
            .collect();
        let mut settings = RedisConnectionInfo::default().set_db(i64::from(config.db.unwrap_or(0)));
        if config.auth.is_some() {
            let material = crate::link_sys::credential::require_basic_material(auth, "Redis")?
                .ok_or_else(|| anyhow::anyhow!("Redis credentials are unavailable"))?;
            settings = settings.set_password(&material.password);
            if let Some(username) = &material.username {
                settings = settings.set_username(username);
            }
        }
        Ok(Self {
            endpoints,
            mode: config
                .topology
                .as_ref()
                .map(|t| t.mode.clone())
                .unwrap_or(RedisTopologyMode::Standalone),
            master: config
                .topology
                .as_ref()
                .and_then(|t| t.sentinel.as_ref())
                .map(|s| s.master_name.clone()),
            pool_size: config
                .pool
                .as_ref()
                .and_then(|p| p.size)
                .unwrap_or(REDIS_DEFAULT_POOL_SIZE) as usize,
            max_redirects: config
                .topology
                .as_ref()
                .and_then(|t| t.cluster.as_ref())
                .and_then(|c| c.max_redirects)
                .unwrap_or(5),
            command_timeout: config.command_timeout().map_err(anyhow::Error::msg)?,
            connect_timeout: config.connect_timeout().map_err(anyhow::Error::msg)?,
            settings,
            tls: tls.map(|tls| redis::TlsCertificates {
                root_cert: Some(tls.root_pem),
                client_tls: tls.client_identity.map(|(client_cert, key)| redis::ClientTlsConfig {
                    client_cert,
                    client_key: key.to_vec(),
                }),
            }),
        })
    }

    fn info(&self, host: &str, port: u16, sentinel: bool) -> RedisResult<redis::ConnectionInfo> {
        let address = if self.tls.is_some() {
            ConnectionAddr::TcpTls {
                host: host.to_owned(),
                port,
                insecure: false,
                tls_params: None,
            }
        } else {
            ConnectionAddr::Tcp(host.to_owned(), port)
        };
        let settings = if sentinel {
            self.settings.clone().set_db(0)
        } else {
            self.settings.clone()
        };
        Ok(address
            .into_connection_info()?
            .set_redis_settings(settings)
            .set_tcp_settings(redis::io::tcp::TcpSettings::default().set_nodelay(true)))
    }

    pub async fn connect(
        &self,
        host: &str,
        port: u16,
        sentinel: bool,
        resolver: RedisEgressResolver,
    ) -> RedisResult<redis::aio::MultiplexedConnection> {
        let info = self.info(host, port, sentinel)?;
        let client = match &self.tls {
            Some(tls) => redis::Client::build_with_tls(info, tls.clone())?,
            None => redis::Client::open(info)?,
        };
        let config = redis::AsyncConnectionConfig::new()
            .set_dns_resolver(resolver)
            .set_connection_timeout(Some(self.connect_timeout))
            .set_response_timeout(Some(self.command_timeout));
        client.get_multiplexed_async_connection_with_config(&config).await
    }

    pub async fn cluster(
        &self,
        initial_nodes: &[(String, u16)],
        resolver: RedisEgressResolver,
    ) -> RedisResult<redis::cluster_async::ClusterConnection> {
        let seeds = initial_nodes
            .iter()
            .map(|(host, port)| self.info(host, *port, false))
            .collect::<RedisResult<Vec<_>>>()?;
        let mut builder = redis::cluster::ClusterClientBuilder::new(seeds)
            .retries(0)
            .async_dns_resolver(resolver)
            .connection_timeout(self.connect_timeout)
            .response_timeout(self.command_timeout)
            .overall_response_timeout(Some(self.command_timeout));
        if let Some(tls) = &self.tls {
            builder = builder.certs(tls.clone());
        }
        builder.build()?.get_async_connection().await
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/redis/transport_test.rs"]
mod tests;
