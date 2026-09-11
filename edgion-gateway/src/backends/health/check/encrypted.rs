//! Health-local TLS transport. No client or channel outlives its owning probe.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use edgion_common::link_sys_core::outbound_tls::resolve_outbound_tls_with_defaults;
use edgion_common::tls_validation::{check_key_matches_cert, parse_canonical_ca_bundle};
use edgion_resources::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use edgion_resources::resources::common::OutboundTlsConfig;
use edgion_resources::resources::health_check::ActiveHealthCheckConfig;
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
    pub(super) fn build(config: &ActiveHealthCheckConfig, namespace: &str) -> Result<Self, ProbeError> {
        if config.tls_validation_error().is_some()
            || config.tls_namespace_validation_error(namespace).is_some()
            || config.resolved_tls_error.is_some()
        {
            return Err(ProbeError::InvalidTlsConfig);
        }
        let local = config.tls.as_ref().ok_or(ProbeError::InvalidTlsConfig)?;
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
            config.resolved_ca_certificates.as_deref(),
            config.resolved_client_certificate.as_ref(),
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
            let codec: tonic::codec::ProstCodec<HealthCheckRequest, HealthCheckResponse> = Default::default();
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
mod tests {
    use super::*;

    #[test]
    fn every_resolved_ca_bundle_must_be_valid_without_controller_error_marker() {
        let ca = rcgen::generate_simple_self_signed(vec!["probe.example.com".to_owned()]).unwrap();
        let mut config = config(&ca.cert);
        config
            .tls
            .as_mut()
            .unwrap()
            .validation
            .as_mut()
            .unwrap()
            .ca_certificate_refs
            .as_mut()
            .unwrap()
            .push(serde_json::from_value(serde_json::json!({"kind": "Secret", "name": "second-ca"})).unwrap());
        for invalid in [
            String::new(),
            "not a PEM certificate".to_owned(),
            "-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----".to_owned(),
        ] {
            config.resolved_ca_certificates = Some(vec![
                secret(&[("ca.crt", ca.cert.pem())]),
                secret(&[("ca.crt", invalid)]),
            ]);
            assert!(config.resolved_tls_error.is_none());
            assert!(matches!(
                EncryptedProbeTransport::build(&config, "default"),
                Err(ProbeError::InvalidTlsConfig)
            ));
        }
    }

    fn secret(entries: &[(&str, String)]) -> k8s_openapi::api::core::v1::Secret {
        k8s_openapi::api::core::v1::Secret {
            type_: Some("kubernetes.io/tls".to_string()),
            data: Some(
                entries
                    .iter()
                    .map(|(key, value)| (key.to_string(), k8s_openapi::ByteString(value.as_bytes().to_vec())))
                    .collect(),
            ),
            ..Default::default()
        }
    }

    fn config(ca: &rcgen::Certificate) -> ActiveHealthCheckConfig {
        let mut config: ActiveHealthCheckConfig = serde_json::from_value(serde_json::json!({
            "type": "https",
            "tls": { "validation": {
                "hostname": "probe.example.com",
                "caCertificateRefs": [{"kind": "Secret", "name": "probe-ca"}]
            }}
        }))
        .unwrap();
        config.resolved_ca_certificates = Some(vec![secret(&[("ca.crt", ca.pem())])]);
        config
    }

    fn handshake(
        transport: EncryptedProbeTransport,
        certificate: &rcgen::Certificate,
        key: &rcgen::KeyPair,
        client_ca: Option<&rcgen::Certificate>,
    ) -> Result<(), rustls::Error> {
        use rustls::pki_types::{pem::PemObject, PrivateKeyDer, ServerName};
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .unwrap();
        let builder = if let Some(ca) = client_ca {
            let mut roots = rustls::RootCertStore::empty();
            roots.add(ca.der().clone()).unwrap();
            builder.with_client_cert_verifier(
                rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                    .build()
                    .unwrap(),
            )
        } else {
            builder.with_no_client_auth()
        };
        let server_config = builder
            .with_single_cert(
                vec![certificate.der().clone()],
                PrivateKeyDer::from_pem_slice(key.serialize_pem().as_bytes()).unwrap(),
            )
            .unwrap();
        let mut server = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
        let mut client = rustls::ClientConnection::new(
            Arc::new(transport.tls),
            ServerName::try_from(transport.hostname).unwrap(),
        )
        .unwrap();
        for _ in 0..8 {
            let mut bytes = Vec::new();
            client.write_tls(&mut bytes).unwrap();
            server.read_tls(&mut bytes.as_slice()).unwrap();
            server.process_new_packets()?;
            bytes.clear();
            server.write_tls(&mut bytes).unwrap();
            client.read_tls(&mut bytes.as_slice()).unwrap();
            client.process_new_packets()?;
            if !client.is_handshaking() && !server.is_handshaking() {
                return Ok(());
            }
        }
        panic!("in-memory health TLS handshake did not complete");
    }

    #[test]
    fn health_tls_rejects_expired_peer_even_with_trusted_certificate() {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["probe.example.com".to_string()]).unwrap();
        let valid = params.self_signed(&key).unwrap();
        assert!(handshake(
            EncryptedProbeTransport::build(&config(&valid), "default").unwrap(),
            &valid,
            &key,
            None
        )
        .is_ok());
        params.not_before = rcgen::date_time_ymd(2000, 1, 1);
        params.not_after = rcgen::date_time_ymd(2001, 1, 1);
        let expired = params.self_signed(&key).unwrap();
        assert!(matches!(
            handshake(
                EncryptedProbeTransport::build(&config(&expired), "default").unwrap(),
                &expired,
                &key,
                None
            ),
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Expired | rustls::CertificateError::ExpiredContext { .. }
            ))
        ));
    }

    #[test]
    fn health_tls_requires_valid_client_identity_for_mtls() {
        let server = rcgen::generate_simple_self_signed(vec!["probe.example.com".to_owned()]).unwrap();
        let client_ca_key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::default();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let client_ca = params.self_signed(&client_ca_key).unwrap();
        let issuer = rcgen::Issuer::from_ca_cert_pem(&client_ca.pem(), client_ca_key).unwrap();
        let client_key = rcgen::KeyPair::generate().unwrap();
        let mut client_params = rcgen::CertificateParams::new(vec!["client.example.com".to_owned()]).unwrap();
        client_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
        let client = client_params.signed_by(&client_key, &issuer).unwrap();
        let mut config = config(&server.cert);
        assert!(handshake(
            EncryptedProbeTransport::build(&config, "default").unwrap(),
            &server.cert,
            &server.signing_key,
            Some(&client_ca)
        )
        .is_err());
        config.tls.as_mut().unwrap().client_certificate_ref =
            Some(serde_json::from_value(serde_json::json!({"name": "probe-client"})).unwrap());
        assert!(matches!(
            EncryptedProbeTransport::build(&config, "default"),
            Err(ProbeError::InvalidTlsConfig)
        ));
        config.resolved_client_certificate = Some(secret(&[
            ("tls.crt", client.pem()),
            ("tls.key", client_key.serialize_pem()),
        ]));
        for protocol in [
            edgion_resources::resources::health_check::HealthCheckType::Https,
            edgion_resources::resources::health_check::HealthCheckType::Grpcs,
        ] {
            config.r#type = protocol;
            assert!(handshake(
                EncryptedProbeTransport::build(&config, "default").unwrap(),
                &server.cert,
                &server.signing_key,
                Some(&client_ca)
            )
            .is_ok());
        }
        for (certificate, private_key) in [
            (
                format!("{}\ninvalid trailing material", client.pem()),
                client_key.serialize_pem(),
            ),
            (
                client.pem(),
                format!("{}\ninvalid trailing material", client_key.serialize_pem()),
            ),
        ] {
            config.resolved_client_certificate = Some(secret(&[("tls.crt", certificate), ("tls.key", private_key)]));
            assert!(config.resolved_tls_error.is_none());
            assert!(matches!(
                EncryptedProbeTransport::build(&config, "default"),
                Err(ProbeError::InvalidTlsConfig)
            ));
        }
        let wrong_key = rcgen::KeyPair::generate().unwrap();
        config.resolved_client_certificate = Some(secret(&[
            ("tls.crt", client.pem()),
            ("tls.key", wrong_key.serialize_pem()),
        ]));
        assert!(matches!(
            EncryptedProbeTransport::build(&config, "default"),
            Err(ProbeError::InvalidTlsConfig)
        ));
    }

    #[test]
    fn encrypted_hostname_cannot_escape_endpoint_pin() {
        let address = "10.0.0.1:8443".parse().unwrap();
        for hostname in ["127.0.0.1", "2130706433", "0x7f000001", "0177.0.0.1", "[::1]"] {
            assert!(matches!(
                encrypted_url(hostname, address, "/"),
                Err(ProbeError::InvalidTlsConfig)
            ));
        }
        let url = encrypted_url(
            "probe.example.com",
            address,
            "//elsewhere.example/p%2Fa?q=value#fragment",
        )
        .unwrap();
        assert_eq!(url.host_str(), Some("probe.example.com"));
        assert_eq!(url.port(), Some(8443));
        assert_eq!(url.path(), "//elsewhere.example/p%2Fa");
        assert_eq!(url.query(), Some("q=value%23fragment"));
        assert!(url.fragment().is_none());
    }

    #[test]
    fn missing_tls_material_fails_closed_without_building_plaintext() {
        let config = ActiveHealthCheckConfig {
            r#type: edgion_resources::resources::health_check::HealthCheckType::Https,
            ..Default::default()
        };
        assert!(matches!(
            EncryptedProbeTransport::build(&config, "default"),
            Err(ProbeError::InvalidTlsConfig)
        ));
    }
}
