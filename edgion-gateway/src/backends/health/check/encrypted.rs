//! Health-local TLS transport. No client or channel outlives its owning probe.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use edgion_common::link_sys_core::outbound_tls::resolve_outbound_tls_with_defaults;
use edgion_common::tls_validation::{check_key_matches_cert, parse_canonical_ca_bundle};
use edgion_resources::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use edgion_resources::resources::common::OutboundTlsConfig;
#[cfg(test)]
use edgion_resources::resources::health_check::ActiveHealthCheckConfig;
use edgion_resources::resources::refs::health_tls_fields::HealthTlsRefs;
use hyper_util::rt::TokioIo;

use super::probes::grpc_health_proto::{HealthCheckRequest, HealthCheckResponse, SERVING};
use super::probes::ProbeError;
use crate::backends::validate_endpoint_backend_addr;
use crate::link_sys::http_client::{build_outbound_client, OutboundClientConfig, OutboundRedirect};
use crate::plugins::http::common::outbound::pinned_resolver::EndpointPinnedResolver;
use crate::tls::validation::build_verified_outbound_tls;

pub(super) struct EncryptedProbeTransport {
    hostname: String,
    tls: rustls::ClientConfig,
}

impl EncryptedProbeTransport {
    pub(super) fn build(owner: &edgion_resources::EdgionBackendTrafficPolicy) -> Result<Self, ProbeError> {
        let config = owner
            .spec
            .health_check
            .as_ref()
            .and_then(|h| h.active.as_ref())
            .ok_or(ProbeError::InvalidTlsConfig)?;
        let namespace = owner.metadata.namespace.as_deref().unwrap_or("default");
        let references = HealthTlsRefs::from_policy(owner);
        let field = references
            .field()
            .map_err(|_| ProbeError::InvalidTlsConfig)?
            .ok_or(ProbeError::InvalidTlsConfig)?;
        let proof = edgion_common::link_sys_core::outbound_tls::validate_outbound_tls_field(field.outbound())
            .map_err(|_| ProbeError::InvalidTlsConfig)?;
        if config.tls_validation_error().is_some()
            || config.tls_namespace_validation_error(namespace).is_some()
            || config.resolved_tls_error.is_some()
        {
            return Err(ProbeError::InvalidTlsConfig);
        }
        let local = proof.field().policy();
        // Revalidate every bundle after conf_sync. A valid sibling bundle must
        // not hide an empty or malformed CA from another configured reference.
        for secret in config.resolved_ca_certificates.as_deref().unwrap_or_default() {
            let pem = secret
                .data
                .as_ref()
                .and_then(|data| data.get(CA_CERT))
                .ok_or(ProbeError::InvalidTlsConfig)?;
            parse_canonical_ca_bundle(&pem.0).map_err(|_| ProbeError::InvalidTlsConfig)?;
        }
        if let Some(secret) = &config.resolved_client_certificate {
            let data = secret.data.as_ref().ok_or(ProbeError::InvalidTlsConfig)?;
            let pem = |key| {
                data.get(key)
                    .and_then(|value| std::str::from_utf8(&value.0).ok())
                    .ok_or(ProbeError::InvalidTlsConfig)
            };
            if secret.type_.as_deref() != Some("kubernetes.io/tls") {
                return Err(ProbeError::InvalidTlsConfig);
            }
            check_key_matches_cert(pem(CERT)?, pem(KEY)?).map_err(|_| ProbeError::InvalidTlsConfig)?;
        }
        let resolved = resolve_outbound_tls_with_defaults(
            local,
            proof.field().ca_material(),
            proof.field().client_material(),
            false,
            &OutboundTlsConfig::default(),
        )
        .map_err(|_| ProbeError::InvalidTlsConfig)?;
        let validation = resolved.san.as_ref().ok_or(ProbeError::InvalidTlsConfig)?;
        let hostname = validation.hostname.clone().ok_or(ProbeError::InvalidTlsConfig)?;
        // URL parsing has additional numeric-host spellings beyond IpAddr parsing.
        // Such hosts bypass reqwest's custom resolver and must never escape the pin.
        encrypted_url(&hostname, "127.0.0.1:443".parse().expect("static socket"), "/")?;
        let tls = build_verified_outbound_tls(
            &resolved,
            validation.subject_alt_names.clone().unwrap_or_default(),
            "ActiveHealthCheck",
        )
        .map_err(|_| ProbeError::InvalidTlsConfig)?;
        Ok(Self { hostname, tls })
    }

    pub(super) async fn https(
        &self,
        address: SocketAddr,
        path: &str,
        host: Option<&str>,
        expected_statuses: &[u16],
        timeout: Duration,
    ) -> Result<(), ProbeError> {
        validate_address(address)?;
        let url = encrypted_url(&self.hostname, address, path)?;
        let client = build_outbound_client(OutboundClientConfig {
            pool_max_idle_per_host: Some(0),
            connect_timeout: Some(timeout),
            total_timeout: Some(timeout),
            disable_proxy: true,
            redirect: OutboundRedirect::Deny,
            preconfigured_tls: Some(self.tls.clone()),
            dns_resolver: Some(Arc::new(EndpointPinnedResolver::new(address, false))),
            ..Default::default()
        })
        .map_err(|_| ProbeError::ClientUnavailable)?;
        let mut request = client.get(url);
        if let Some(host) = host {
            request = request.header("Host", host);
        }
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                ProbeError::Timeout
            } else {
                ProbeError::RequestFailed(error)
            }
        })?;
        let status = response.status().as_u16();
        if expected_statuses.contains(&status) {
            Ok(())
        } else {
            Err(ProbeError::UnexpectedStatus(status))
        }
    }

    pub(super) async fn grpcs(
        &self,
        address: SocketAddr,
        service_name: &str,
        timeout: Duration,
    ) -> Result<(), ProbeError> {
        validate_address(address)?;
        let mut tls = self.tls.clone();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let connector = tokio_rustls::TlsConnector::from(Arc::new(tls));
        let server_name =
            rustls::pki_types::ServerName::try_from(self.hostname.clone()).map_err(|_| ProbeError::InvalidTlsConfig)?;
        let dial = tower::service_fn(move |_uri: http::Uri| {
            let connector = connector.clone();
            let server_name = server_name.clone();
            async move {
                // Recheck the exact endpoint at dial time; never resolve the TLS name.
                validate_address(address).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::PermissionDenied, "unsafe health-check endpoint")
                })?;
                let tcp = tokio::net::TcpStream::connect(address).await?;
                let tls = connector.connect(server_name, tcp).await?;
                Ok::<_, std::io::Error>(TokioIo::new(tls))
            }
        });
        // The custom connector owns TLS. Keep the connector URI plaintext to
        // avoid a second TLS upgrade, but send the correct HTTPS HTTP/2 origin.
        let origin = format!("https://{}:{}", self.hostname, address.port())
            .parse()
            .map_err(|_| ProbeError::InvalidUri)?;
        let endpoint = tonic::transport::Endpoint::from_shared(format!("http://{}:{}", self.hostname, address.port()))
            .map_err(|_| ProbeError::InvalidUri)?
            .origin(origin)
            .connect_timeout(timeout)
            .timeout(timeout);
        tokio::time::timeout(timeout, async {
            let channel = endpoint
                .connect_with_connector(dial)
                .await
                .map_err(ProbeError::GrpcConnectionFailed)?;
            let mut client = tonic::client::Grpc::new(channel);
            client.ready().await.map_err(ProbeError::GrpcConnectionFailed)?;
            let request = tonic::Request::new(HealthCheckRequest {
                service: service_name.to_string(),
            });
            let path = http::uri::PathAndQuery::from_static("/grpc.health.v1.Health/Check");
            let codec: tonic_prost::ProstCodec<HealthCheckRequest, HealthCheckResponse> = Default::default();
            let response = client
                .unary(request, path, codec)
                .await
                .map_err(|error| ProbeError::GrpcCallFailed(Box::new(error)))?;
            let status = response.into_inner().status;
            if status == SERVING {
                Ok(())
            } else {
                Err(ProbeError::GrpcNotServing(status))
            }
        })
        .await
        .map_err(|_| ProbeError::Timeout)?
    }
}

fn validate_address(address: SocketAddr) -> Result<(), ProbeError> {
    validate_endpoint_backend_addr(&pingora_core::protocols::l4::socket::SocketAddr::Inet(address), false)
        .map_err(|_| ProbeError::UnsafeEndpoint)
}

fn encrypted_url(hostname: &str, address: SocketAddr, path: &str) -> Result<reqwest::Url, ProbeError> {
    if !path.starts_with('/') {
        return Err(ProbeError::InvalidPath);
    }
    let mut url = reqwest::Url::parse("https://placeholder.invalid").map_err(|_| ProbeError::InvalidUri)?;
    url.set_host(Some(hostname)).map_err(|_| ProbeError::InvalidTlsConfig)?;
    if !matches!(url.host(), Some(url::Host::Domain(_))) {
        return Err(ProbeError::InvalidTlsConfig);
    }
    url.set_port(Some(address.port())).map_err(|_| ProbeError::InvalidUri)?;
    // Preserve path query semantics without permitting either to replace authority.
    let (path, query) = path
        .split_once('?')
        .map_or((path, None), |(path, query)| (path, Some(query)));
    url.set_path(path);
    url.set_query(query);
    Ok(url)
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/health/check/encrypted_test.rs"]
mod tests;
