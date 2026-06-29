//! gRPC client for the external processing service.
//!
//! The channel is built lazily at plugin construction (no blocking connect) and
//! multiplexes every request. Each processing phase opens its own short-lived
//! `Process` bidirectional stream on that shared channel (one stream per phase).
//!
//! TLS is routed through the shared outbound resolver
//! ([`edgion_common::link_sys_core::outbound_tls::resolve_outbound_tls`]) so the
//! ext_proc dial has full parity with the other outbound callers: verify toggle
//! (gated), custom-CA REPLACE, and client mTLS identity. Because tonic's
//! `ClientTlsConfig` exposes none of those knobs, the TLS path is built as a
//! hand-rolled rustls `ClientConfig` plumbed into the channel via a custom
//! `tower` connector (`connect_with_connector_lazy`).

use std::sync::Arc;
use std::time::Duration;

use edgion_common::ext_proc::proto::external_processor_client::ExternalProcessorClient;
use edgion_common::ext_proc::proto::{ProcessingRequest, ProcessingResponse};
use edgion_common::link_sys_core::outbound_tls::{resolve_outbound_tls, EffectiveCa};
use edgion_resources::plugin_configs::ExtProcConfig;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioIo;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;
use tonic::transport::{Channel, Uri};

/// Thin wrapper around the generated ext_proc gRPC client.
#[derive(Clone)]
pub struct ExtProcClient {
    client: ExternalProcessorClient<Channel>,
    timeout: Duration,
}

impl ExtProcClient {
    /// Build a lazily-connecting client from the plugin's full config.
    ///
    /// Returns `Err` for a structurally invalid target or for any TLS
    /// configuration error surfaced by the shared outbound resolver (missing CA,
    /// invalid client identity, provider-cannot-disable-verify, ...). It never
    /// blocks on the network — the actual connection is established on first use.
    /// The config-layer `validation_error` already gates target/timeout, so a
    /// build failure here is unexpected and `ExtProc::build` fails the plugin
    /// closed (drops it). That fail-closed behavior is intentional for ext_proc;
    /// do NOT add a secure fallback.
    pub fn new(cfg: &ExtProcConfig) -> Result<Self, String> {
        let grpc = &cfg.grpc_service;
        let timeout = Duration::from_millis(grpc.timeout_ms);

        // Plain-HTTP path: identical behavior to the pre-TLS-refactor client.
        if !grpc.tls.enabled {
            let uri = format!("http://{}", grpc.target);
            let channel = Channel::from_shared(uri)
                .map_err(|e| format!("invalid ext_proc grpcService.target: {e}"))?
                .connect_timeout(timeout)
                .timeout(timeout)
                .connect_lazy();
            return Ok(Self {
                client: ExternalProcessorClient::new(channel),
                timeout,
            });
        }

        // TLS path: resolve once through the shared outbound resolver.
        let resolved = resolve_outbound_tls(
            &grpc.tls.outbound,
            grpc.tls.resolved_ca_secret.as_ref(),
            grpc.tls.resolved_client_secret.as_ref(),
            true,
            "ExtProc",
        )
        .map_err(|e| format!("ext_proc TLS configuration error: {e:?}"))?;

        let client_config = build_rustls_config(&resolved)?;

        // The custom connector performs the TLS handshake itself. tonic rejects an
        // `https://` authority unless its OWN built-in TLS is configured ("Connecting
        // to HTTPS without TLS enabled"), so the channel URI uses the `http://`
        // scheme: tonic then treats the connector's stream as the transport as-is,
        // while `build_tls_connector` still upgrades it to rustls (ALPN h2).
        let uri = format!("http://{}", grpc.target);
        let connector = build_tls_connector(client_config);

        let channel = Channel::from_shared(uri)
            .map_err(|e| format!("invalid ext_proc grpcService.target: {e}"))?
            .connect_timeout(timeout)
            .timeout(timeout)
            .connect_with_connector_lazy(connector);

        Ok(Self {
            client: ExternalProcessorClient::new(channel),
            timeout,
        })
    }

    /// Send one phase to the processor and await its single verdict.
    ///
    /// Opens a fresh `Process` stream, sends exactly one `ProcessingRequest`,
    /// half-closes the send side, and reads the first `ProcessingResponse`.
    ///
    /// The whole exchange is bounded by an explicit `tokio::time::timeout`:
    /// the endpoint's transport `.timeout()` only bounds time-to-response-headers,
    /// so a processor that accepts the stream and then stalls before emitting a
    /// verdict would otherwise hang the request path forever.
    pub async fn exchange(&self, request: ProcessingRequest) -> Result<ProcessingResponse, tonic::Status> {
        let mut client = self.client.clone();
        let fut = async move {
            let outbound = tokio_stream::once(request);
            let mut inbound = client.process(outbound).await?.into_inner();
            match inbound.message().await? {
                Some(resp) => Ok(resp),
                None => Err(tonic::Status::unavailable(
                    "ext_proc processor closed the stream without a response",
                )),
            }
        };
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(result) => result,
            Err(_) => Err(tonic::Status::deadline_exceeded("ext_proc verdict timed out")),
        }
    }
}

/// Build a rustls [`ClientConfig`] for the ext_proc dial from the resolved
/// outbound TLS settings (verify gate already applied by the resolver).
///
/// Root selection mirrors the LinkSys path:
/// - [`EffectiveCa::Custom`] → the PEM is the ONLY trust root (REPLACE).
/// - [`EffectiveCa::System`] → the built-in webpki roots.
/// - `verify == false` → an accept-all verifier (reachable only because the
///   resolver honored the operator's verify=false (provider_can_disable=true)).
///
/// ALPN is fixed to `h2` so the gRPC channel negotiates HTTP/2.
fn build_rustls_config(
    resolved: &edgion_common::link_sys_core::outbound_tls::ResolvedOutboundTls,
) -> Result<ClientConfig, String> {
    let builder = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("ext_proc rustls provider error: {e}"))?;

    // Verifier / roots stage.
    let builder = if !resolved.verify {
        // Reachable only because resolve_outbound_tls honored the operator's verify=false (provider_can_disable=true).
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAllVerifier::new()))
    } else {
        let mut roots = RootCertStore::empty();
        match &resolved.ca {
            EffectiveCa::System => {
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            }
            EffectiveCa::Custom(ca_pem) => {
                let mut added = 0usize;
                for cert in CertificateDer::pem_slice_iter(ca_pem) {
                    let cert = cert.map_err(|e| format!("ext_proc TLS: invalid CA certificate PEM: {e}"))?;
                    roots
                        .add(cert)
                        .map_err(|e| format!("ext_proc TLS: failed to add CA certificate: {e}"))?;
                    added += 1;
                }
                if added == 0 {
                    return Err("ext_proc TLS CA PEM did not contain any valid certificates".to_string());
                }
            }
        }
        builder.with_root_certificates(roots)
    };

    // Client identity stage.
    let mut config = match &resolved.client_identity {
        Some((cert_pem, key_pem)) => {
            let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
                .collect::<Result<_, _>>()
                .map_err(|e| format!("ext_proc TLS: invalid client certificate PEM: {e}"))?;
            if certs.is_empty() {
                return Err("ext_proc TLS client certificate PEM did not contain any certificates".to_string());
            }
            let key = PrivateKeyDer::from_pem_slice(key_pem.as_slice())
                .map_err(|e| format!("ext_proc TLS: invalid client private key PEM: {e}"))?;
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| format!("ext_proc TLS: failed to configure client authentication: {e}"))?
        }
        None => builder.with_no_client_auth(),
    };

    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(config)
}

/// The connector's output transport type. `TokioIo` adapts the tokio-rustls
/// stream (over a raw tokio `TcpStream`) to hyper's `Read`/`Write`, which is the
/// IO bound tonic's `connect_with_connector_lazy` requires.
type TlsTransport = TokioIo<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>;

/// Build the `tower` service that tonic drives per dial: TCP connect via
/// `HttpConnector`, then a rustls handshake, yielding a hyper-compatible IO.
fn build_tls_connector(
    client_config: ClientConfig,
) -> impl tower::Service<
    Uri,
    Response = TlsTransport,
    Error = std::io::Error,
    Future = impl std::future::Future<Output = Result<TlsTransport, std::io::Error>> + Send,
> + Clone
       + Send {
    let tls_connector = TlsConnector::from(Arc::new(client_config));
    let mut http = HttpConnector::new();
    http.enforce_http(false);

    tower::service_fn(move |uri: Uri| {
        let tls = tls_connector.clone();
        let mut http = http.clone();
        async move {
            let host = uri
                .host()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "ext_proc target missing host"))?
                .to_string();
            let server_name = ServerName::try_from(host)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid ext_proc DNS name"))?
                .to_owned();
            // `HttpConnector` yields a `TokioIo<TcpStream>`; unwrap to the raw
            // tokio `TcpStream` so tokio-rustls sees a native AsyncRead/Write
            // transport and the resulting `TokioIo<TlsStream<TcpStream>>` satisfies
            // tonic's hyper `Read`/`Write` IO bound.
            let tcp = <HttpConnector as tower::Service<Uri>>::call(&mut http, uri)
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            let tls_stream = tls.connect(server_name, tcp.into_inner()).await?;
            Ok::<_, std::io::Error>(TokioIo::new(tls_stream))
        }
    })
}

/// Accept-all server-certificate verifier for the `verify == false` path.
///
/// Reached when an operator explicitly sets `verify=false` and the resolver
/// honored it. The caller owns the security responsibility. It trusts any
/// presented chain and any handshake signature.
#[derive(Debug)]
struct AcceptAllVerifier(rustls::crypto::WebPkiSupportedAlgorithms);

impl AcceptAllVerifier {
    fn new() -> Self {
        Self(rustls::crypto::ring::default_provider().signature_verification_algorithms)
    }
}

impl rustls::client::danger::ServerCertVerifier for AcceptAllVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::plugin_configs::GrpcServiceConfig;
    use edgion_resources::resources::common::outbound_tls::CaSource;
    use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};

    fn install_test_crypto_provider() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    fn tls_cfg() -> ExtProcConfig {
        ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target: "ep:9000".to_string(),
                timeout_ms: 200,
                tls: TlsConfig {
                    enabled: true,
                    ..Default::default()
                },
            },
            ..Default::default()
        }
    }

    /// System CA + verify=true (default outbound) + tls=true must construct a
    /// channel. `connect_with_connector_lazy` never touches the network, so this
    /// proves the connector output type satisfies tonic's IO + `Connection`
    /// bounds and the whole TLS path type-checks.
    #[tokio::test]
    async fn extproc_connector_built_for_system_verify() {
        install_test_crypto_provider();
        let cfg = tls_cfg();
        assert!(ExtProcClient::new(&cfg).is_ok());
    }

    /// The plain-HTTP path (tls=false) must keep building a lazy channel exactly
    /// as before.
    #[tokio::test]
    async fn extproc_plain_http_channel_builds() {
        let mut cfg = tls_cfg();
        cfg.grpc_service.tls.enabled = false;
        assert!(ExtProcClient::new(&cfg).is_ok());
    }

    /// verify=false must build the AcceptAllVerifier branch without error —
    /// the operator's choice is honored regardless of environment.
    #[tokio::test]
    async fn extproc_verify_false_builds() {
        install_test_crypto_provider();

        let mut cfg = tls_cfg();
        cfg.grpc_service.tls.outbound = OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        };
        let result = ExtProcClient::new(&cfg);

        assert!(
            result.is_ok(),
            "verify=false must build the AcceptAllVerifier channel, got: {:?}",
            result.err()
        );
    }

    /// Custom-CA Inline: supply a valid self-signed CA PEM; the resolver must
    /// build the REPLACE-root ClientConfig without error.
    #[tokio::test]
    async fn extproc_custom_inline_ca_builds() {
        install_test_crypto_provider();

        let ca = rcgen::generate_simple_self_signed(vec!["test-ca".into()]).unwrap();
        let ca_pem = ca.cert.pem();

        let mut cfg = tls_cfg();
        cfg.grpc_service.tls.outbound = OutboundTlsLocal {
            ca_source: Some(CaSource::Inline),
            ca_cert: Some(ca_pem),
            ..Default::default()
        };

        assert!(
            ExtProcClient::new(&cfg).is_ok(),
            "Custom-CA Inline must build the REPLACE-root channel without error"
        );
    }
}
