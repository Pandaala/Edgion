//! OTLP transport prepared on configuration updates and dialed under shared policy.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use edgion_common::dns::ResolverSpec;
use edgion_common::link_sys_core::outbound_tls::{resolve_outbound_tls, EffectiveCa};
use edgion_common::tls_validation::parse_canonical_ca_bundle;
use edgion_resources::credentials::SecretMaterialResolver;
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::otlp::{OtlpBearerResolver, OtlpClientConfig};
use hyper_util::rt::TokioIo;
use opentelemetry_otlp::WithTonicConfig;
use opentelemetry_sdk::logs::{LogBatch, LogExporter};
use tonic014::metadata::{MetadataMap, MetadataValue};
use tonic014::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

use crate::plugins::http::common::outbound::egress::{default_block_private, EgressPolicy, EgressResolver};

pub(super) const CONNECT_DEADLINE: Duration = Duration::from_secs(3);

/// Connection-sensitive material has one immutable owner per desired generation.
/// Intentionally has no Debug implementation: Endpoint and metadata contain secrets.
pub(super) struct PreparedTransport {
    endpoint: Endpoint,
    host: Arc<str>,
    port: u16,
    resolver: ResolverSpec,
    metadata: MetadataMap,
    pub timeout: Duration,
}

impl PreparedTransport {
    pub fn prepare(config: &OtlpClientConfig, resolver: ResolverSpec) -> Result<Arc<Self>, &'static str> {
        let parsed = config.validate().map_err(|_| "invalid_config")?;
        let timeout = Duration::from_millis(config.timeout_ms);
        let mut metadata = MetadataMap::new();
        if let Some(auth) = &config.auth {
            let secret = auth.slot.secret.as_ref().ok_or("unresolved_auth")?;
            let data = secret.data.as_ref().ok_or("invalid_auth")?;
            let material = OtlpBearerResolver.parse(data).map_err(|_| "invalid_auth")?;
            let mut value =
                MetadataValue::try_from(format!("Bearer {}", material.token)).map_err(|_| "invalid_auth")?;
            value.set_sensitive(true);
            metadata.insert("authorization", value);
        }
        let mut endpoint = Endpoint::from(parsed.uri)
            .connect_timeout(timeout.min(CONNECT_DEADLINE))
            .timeout(timeout);
        if parsed.tls {
            let empty = TlsConfig::default();
            let local = config.tls.as_ref().filter(|tls| tls.enabled).unwrap_or(&empty);
            let resolved = resolve_outbound_tls(
                &local.outbound,
                local.resolved_ca_certificates.as_deref(),
                local.resolved_client_certificate.as_ref(),
                false,
                "otlp",
            )
            .map_err(|_| "invalid_tls")?;
            if resolved.san.as_ref().is_some_and(|validation| {
                validation.hostname.is_some()
                    || validation
                        .subject_alt_names
                        .as_ref()
                        .is_some_and(|names| !names.is_empty())
            }) {
                return Err("invalid_tls");
            }
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
        Ok(Arc::new(Self {
            endpoint,
            host: parsed.host.into(),
            port: parsed.port,
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
            // This runs AFTER SDK environment-header injection. Secret references
            // are the sole auth source, even if OTEL exporter variables are set.
            .with_interceptor(move |mut request: tonic014::Request<()>| {
                *request.metadata_mut() = metadata.clone();
                Ok(request)
            })
            .build()
            .map_err(|_| "exporter_build_failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use edgion_resources::resources::common::{OutboundTlsLocal, SecretObjectReference, SecretSlot};
    use edgion_resources::resources::link_sys::otlp::OtlpAuth;
    use k8s_openapi::{api::core::v1::Secret, ByteString};

    fn config() -> OtlpClientConfig {
        OtlpClientConfig {
            endpoint: "https://collector.example.test:4317".into(),
            timeout_ms: 1000,
            auth: None,
            tls: None,
        }
    }

    #[test]
    fn auth_unresolved_invalid_and_resolved_are_distinct_safe_states() {
        let mut config = config();
        config.auth = Some(OtlpAuth {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    name: "auth".into(),
                    namespace: None,
                    group: None,
                    kind: None,
                },
                secret: None,
            },
        });
        assert!(matches!(
            PreparedTransport::prepare(&config, ResolverSpec::System),
            Err("unresolved_auth")
        ));
        config.auth.as_mut().unwrap().slot.secret = Some(Secret {
            data: Some([(String::from("bearerToken"), ByteString(b"wrong value".to_vec()))].into()),
            ..Default::default()
        });
        assert!(matches!(
            PreparedTransport::prepare(&config, ResolverSpec::System),
            Err("invalid_auth")
        ));
        config
            .auth
            .as_mut()
            .unwrap()
            .slot
            .secret
            .as_mut()
            .unwrap()
            .data
            .as_mut()
            .unwrap()
            .insert("bearerToken".into(), ByteString(b"valid-token".to_vec()));
        let prepared = PreparedTransport::prepare(&config, ResolverSpec::System).unwrap();
        assert_eq!(prepared.metadata.get("authorization").unwrap(), "Bearer valid-token");
        assert!(prepared.metadata.get("authorization").unwrap().is_sensitive());
    }

    #[test]
    fn inactive_tls_is_inert_and_unsupported_effective_identity_fails() {
        let mut config = config();
        config.tls = Some(TlsConfig {
            enabled: false,
            outbound: OutboundTlsLocal {
                verify: Some(false),
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(PreparedTransport::prepare(&config, ResolverSpec::System).is_ok());
        config.tls.as_mut().unwrap().enabled = true;
        assert!(matches!(
            PreparedTransport::prepare(&config, ResolverSpec::System),
            Err("invalid_config")
        ));
        config.tls.as_mut().unwrap().outbound = OutboundTlsLocal {
            validation: Some(OutboundTlsValidation {
                hostname: Some("override.example.test".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(PreparedTransport::prepare(&config, ResolverSpec::System).is_err());
    }
}
