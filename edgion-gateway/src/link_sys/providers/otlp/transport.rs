//! OTLP transport prepared on configuration updates and dialed under shared policy.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use edgion_common::dns::ResolverSpec;
use edgion_common::link_sys_core::outbound_tls::{EffectiveCa, PreparedOtlpTls};
use edgion_common::tls_validation::parse_canonical_ca_bundle;
use edgion_resources::credentials::SecretMaterialResolver;
use edgion_resources::resources::link_sys::otlp::OtlpBearerResolver;
use edgion_resources::resources::link_sys::SystemConfig;
use hyper_util::rt::TokioIo;
use opentelemetry_otlp::{RetryPolicy, WithTonicConfig};
use opentelemetry_sdk::logs::{LogBatch, LogExporter};
use tonic::metadata::{MetadataMap, MetadataValue};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

use crate::plugins::http::common::outbound::egress::{default_block_private, EgressPolicy, EgressResolver};

pub(super) const CONNECT_DEADLINE: Duration = Duration::from_secs(3);

/// Connection-sensitive material has one immutable owner per desired generation.
/// Intentionally has no Debug implementation: Endpoint and metadata contain secrets.
pub(super) struct PreparedTransport {
    _input: Arc<PreparedOtlpTls>,
    endpoint: Endpoint,
    host: Arc<str>,
    port: u16,
    resolver: ResolverSpec,
    metadata: MetadataMap,
    pub timeout: Duration,
}

impl PreparedTransport {
    pub fn prepare(input: Arc<PreparedOtlpTls>, resolver: ResolverSpec) -> Result<Arc<Self>, &'static str> {
        let SystemConfig::Otlp(config) = &input.owner().spec.config else {
            return Err("invalid_config");
        };
        let parsed = input.endpoint();
        let timeout = Duration::from_millis(config.timeout_ms);
        let mut metadata = MetadataMap::new();
        if config.auth.is_some() {
            let secret = edgion_common::link_sys_core::credential::checked_auth_secret(input.owner())
                .map_err(|_| "invalid_auth")?
                .ok_or("unresolved_auth")?;
            let data = secret.data.as_ref().ok_or("invalid_auth")?;
            let material = OtlpBearerResolver.parse(data).map_err(|_| "invalid_auth")?;
            let mut value =
                MetadataValue::try_from(format!("Bearer {}", material.token)).map_err(|_| "invalid_auth")?;
            value.set_sensitive(true);
            metadata.insert("authorization", value);
        }
        let mut endpoint = Endpoint::from(parsed.uri.clone())
            .connect_timeout(timeout.min(CONNECT_DEADLINE))
            .timeout(timeout);
        if let Some(resolved) = input.resolve_tls().map_err(|_| "invalid_tls")? {
            let mut tls = ClientTlsConfig::new()
                .domain_name(parsed.host.clone())
                .timeout(timeout.min(CONNECT_DEADLINE));
            match resolved.ca {
                EffectiveCa::System => tls = tls.with_webpki_roots(),
                EffectiveCa::Custom(bundles) => {
                    for bundle in bundles {
                        parse_canonical_ca_bundle(&bundle).map_err(|_| "invalid_tls")?;
                        tls = tls.ca_certificate(Certificate::from_pem(bundle));
                    }
                }
            }
            if let Some((cert, key)) = resolved.client_identity {
                tls = tls.identity(Identity::from_pem(cert, key.as_slice()));
            }
            endpoint = endpoint.tls_config(tls).map_err(|_| "invalid_tls")?;
        }
        let host = parsed.host.clone().into();
        let port = parsed.port;
        Ok(Arc::new(Self {
            _input: input,
            endpoint,
            host,
            port,
            resolver,
            metadata,
            timeout,
        }))
    }

    /// Re-resolve on every dial and classify the exact addresses passed to TCP.
    /// There is no hostname dialing fallback, proxy environment, or redirect path.
    async fn dial(host: &str, port: u16, resolver: ResolverSpec) -> io::Result<tokio::net::TcpStream> {
        let resolver = EgressResolver::new(
            resolver,
            EgressPolicy {
                block_private: default_block_private(),
            },
            Duration::ZERO,
        );
        let addresses = resolver
            .resolve_socket_addrs(host, port)
            .await
            .map_err(|_| io::Error::other("connect_failed"))?;
        for address in addresses {
            if let Ok(stream) = tokio::net::TcpStream::connect(address).await {
                return Ok(stream);
            }
        }
        Err(io::Error::other("connect_failed"))
    }

    pub async fn connect(&self) -> Result<Channel, &'static str> {
        let host = self.host.clone();
        let port = self.port;
        let resolver = self.resolver.clone();
        let connector = tower::service_fn(move |_uri| {
            let host = host.clone();
            let resolver = resolver.clone();
            async move { Self::dial(&host, port, resolver).await.map(TokioIo::new) }
        });
        tokio::time::timeout(CONNECT_DEADLINE, self.endpoint.connect_with_connector(connector))
            .await
            .map_err(|_| "connect_failed")?
            .map_err(|_| "connect_failed")
    }

    /// TLS 1.3 client completion does not prove the server accepted mTLS, and
    /// tonic connection readiness does not await peer HTTP/2 SETTINGS. One empty
    /// Export confirms peer acceptance before this SAME exporter is published.
    /// This is candidate preparation only, never the periodic active probe.
    pub async fn prepare_exporter(&self) -> Result<opentelemetry_otlp::LogExporter, &'static str> {
        tokio::time::timeout(CONNECT_DEADLINE, async {
            let channel = self.connect().await?;
            let exporter = self.exporter(channel)?;
            exporter
                .export(LogBatch::new(&[]))
                .await
                .map_err(|_| "connect_failed")?;
            Ok(exporter)
        })
        .await
        .map_err(|_| "connect_failed")?
    }

    pub async fn probe(&self) -> bool {
        matches!(
            tokio::time::timeout(
                CONNECT_DEADLINE,
                Self::dial(&self.host, self.port, self.resolver.clone())
            )
            .await,
            Ok(Ok(_))
        )
    }

    pub fn exporter(&self, channel: Channel) -> Result<opentelemetry_otlp::LogExporter, &'static str> {
        let metadata = self.metadata.clone();
        opentelemetry_otlp::LogExporter::builder()
            .with_tonic()
            .with_channel(channel)
            // Preserve one attempt per export; generation preparation owns retries.
            .with_retry_policy(RetryPolicy::disabled())
            // This runs AFTER SDK environment-header injection. Secret references
            // are the sole auth source, even if OTEL exporter variables are set.
            .with_interceptor(move |mut request: tonic::Request<()>| {
                *request.metadata_mut() = metadata.clone();
                Ok(request)
            })
            .build()
            .map_err(|_| "exporter_build_failed")
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/otlp/transport_test.rs"]
mod tests;
