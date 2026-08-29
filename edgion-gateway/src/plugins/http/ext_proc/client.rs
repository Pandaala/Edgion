//! gRPC client for the external processing service.
//!
//! The channel is built lazily at plugin construction (no blocking connect) and
//! multiplexes request-scoped logical `Process` streams. One matched ExtProc
//! entry retains one logical stream across its request and response phases.
//!
//! TLS is routed through the shared outbound resolver
//! ([`edgion_common::link_sys_core::outbound_tls::resolve_outbound_tls`]) so the
//! ext_proc dial has full parity with the other outbound callers: verify toggle
//! (gated), custom-CA REPLACE, and client mTLS identity. Because tonic's
//! `ClientTlsConfig` exposes none of those knobs, the TLS path is built as a
//! hand-rolled rustls `ClientConfig` plumbed into the channel via a custom
//! `tower` connector (`connect_with_connector_lazy`).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

use edgion_common::ext_proc::proto::external_processor_client::ExternalProcessorClient;
use edgion_common::ext_proc::proto::{processing_response, ProcessingRequest, ProcessingResponse};
use edgion_common::link_sys_core::outbound_tls::{resolve_outbound_tls, EffectiveCa, ResolvedOutboundTls};
use edgion_resources::plugin_configs::ExtProcConfig;
use hyper_util::rt::TokioIo;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;
use tonic::transport::{Channel, Uri};

use crate::plugins::http::common::outbound::egress::{
    default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
};
use edgion_common::dns::ResolverSpec;
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const TARGET_CACHE_CAP: usize = 1_024;
const TARGET_SESSION_CAP: usize = 64;
const PERMIT_WAIT_CAP: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

static TARGET_RUNTIMES: LazyLock<Mutex<HashMap<String, Weak<ExtProcTargetRuntime>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct ExtProcTargetRuntime {
    client: ExternalProcessorClient<Channel>,
    sessions: Arc<Semaphore>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessingPhase {
    RequestHeaders,
    ResponseHeaders,
    RequestBody,
    ResponseBody,
    RequestTrailers,
    ResponseTrailers,
}

impl ProcessingPhase {
    fn from_request(request: &ProcessingRequest) -> Result<Self, tonic::Status> {
        use edgion_common::ext_proc::proto::processing_request::Request;

        match request.request {
            Some(Request::RequestHeaders(_)) => Ok(Self::RequestHeaders),
            Some(Request::ResponseHeaders(_)) => Ok(Self::ResponseHeaders),
            Some(Request::RequestBody(_)) => Ok(Self::RequestBody),
            Some(Request::ResponseBody(_)) => Ok(Self::ResponseBody),
            Some(Request::RequestTrailers(_)) => Ok(Self::RequestTrailers),
            Some(Request::ResponseTrailers(_)) => Ok(Self::ResponseTrailers),
            None => Err(tonic::Status::invalid_argument(
                "ext_proc request has no processing phase",
            )),
        }
    }

    fn accepts(self, response: &ProcessingResponse) -> bool {
        use processing_response::Response;

        matches!(
            (self, response.response.as_ref()),
            (Self::RequestHeaders, Some(Response::RequestHeaders(_)))
                | (Self::ResponseHeaders, Some(Response::ResponseHeaders(_)))
                | (Self::RequestBody, Some(Response::RequestBody(_)))
                | (Self::ResponseBody, Some(Response::ResponseBody(_)))
                | (Self::RequestTrailers, Some(Response::RequestTrailers(_)))
                | (Self::ResponseTrailers, Some(Response::ResponseTrailers(_)))
        )
    }
}

#[cfg(test)]
fn common_response(response: &ProcessingResponse) -> Option<&edgion_common::ext_proc::proto::CommonResponse> {
    use processing_response::Response;

    match response.response.as_ref() {
        Some(Response::RequestHeaders(response)) | Some(Response::ResponseHeaders(response)) => {
            response.response.as_ref()
        }
        Some(Response::RequestBody(response)) | Some(Response::ResponseBody(response)) => response.response.as_ref(),
        _ => None,
    }
}

#[cfg(test)]
fn response_status(
    response: &ProcessingResponse,
) -> Result<edgion_common::ext_proc::proto::common_response::ResponseStatus, tonic::Status> {
    let Some(common) = common_response(response) else {
        return Ok(edgion_common::ext_proc::proto::common_response::ResponseStatus::Continue);
    };
    edgion_common::ext_proc::proto::common_response::ResponseStatus::try_from(common.status)
        .map_err(|_| tonic::Status::failed_precondition("ext_proc processor returned an invalid response status"))
}

async fn send_phase(
    sender: &mut Option<tokio::sync::mpsc::Sender<ProcessingRequest>>,
    request: ProcessingRequest,
    last_phase: bool,
) -> Result<(), tonic::Status> {
    sender
        .as_ref()
        .ok_or_else(|| tonic::Status::internal("ext_proc request stream was already closed"))?
        .send(request)
        .await
        .map_err(|_| tonic::Status::unavailable("ext_proc processor closed the request stream"))?;
    if last_phase {
        sender.take();
    }
    Ok(())
}

/// Thin wrapper around the generated ext_proc gRPC client.
#[derive(Clone)]
pub struct ExtProcClient {
    runtime: Arc<ExtProcTargetRuntime>,
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
        let (host, port) = split_authority(&grpc.target)?;
        let normalized_authority = normalized_authority(&host, port);
        let resolved_tls = if grpc.tls.enabled {
            Some(
                resolve_outbound_tls(
                    &grpc.tls.outbound,
                    grpc.tls.resolved_ca_certificates.as_deref(),
                    grpc.tls.resolved_client_certificate.as_ref(),
                    true,
                    "ExtProc",
                )
                .map_err(|_| "ext_proc TLS configuration is invalid".to_string())?,
            )
        } else {
            None
        };
        let mut hasher = Sha256::new();
        hasher.update(normalized_authority.as_bytes());
        fingerprint_effective_tls(&mut hasher, resolved_tls.as_ref());
        hasher.update([u8::from(default_block_private())]);
        let cache_key = format!("{:x}", hasher.finalize());
        if let Some(runtime) = TARGET_RUNTIMES
            .lock()
            .map_err(|_| "ext_proc target cache lock poisoned".to_string())?
            .get(&cache_key)
            .and_then(Weak::upgrade)
        {
            return Ok(Self { runtime, timeout });
        }

        let channel = Self::build_channel(&normalized_authority, host, port, resolved_tls.as_ref())?;
        let runtime = Arc::new(ExtProcTargetRuntime {
            client: ExternalProcessorClient::new(channel),
            sessions: Arc::new(Semaphore::new(TARGET_SESSION_CAP)),
        });
        let mut cache = TARGET_RUNTIMES
            .lock()
            .map_err(|_| "ext_proc target cache lock poisoned".to_string())?;
        cache.retain(|_, runtime| runtime.strong_count() > 0);
        if let Some(existing) = cache.get(&cache_key).and_then(Weak::upgrade) {
            return Ok(Self {
                runtime: existing,
                timeout,
            });
        }
        if cache.len() >= TARGET_CACHE_CAP {
            return Err("ext_proc target cache reached its bounded capacity".to_string());
        }
        cache.insert(cache_key, Arc::downgrade(&runtime));
        Ok(Self { runtime, timeout })
    }

    fn build_channel(
        normalized_authority: &str,
        host: String,
        port: u16,
        resolved_tls: Option<&ResolvedOutboundTls>,
    ) -> Result<Channel, String> {
        let resolver = Arc::new(EgressResolver::new(
            ResolverSpec::System,
            EgressPolicy {
                block_private: default_block_private(),
            },
            egress_dns_cache_ttl(),
        ));

        if resolved_tls.is_none() {
            let uri = format!("http://{normalized_authority}");
            let connector = build_plain_connector(host, port, resolver);
            return Ok(Channel::from_shared(uri)
                .map_err(|_| "invalid ext_proc grpcService.target".to_string())?
                .connect_timeout(CONNECT_TIMEOUT)
                .connect_with_connector_lazy(connector));
        }

        let resolved = resolved_tls.ok_or_else(|| "ext_proc TLS resolution is missing".to_string())?;
        let client_config = build_rustls_config(resolved)?;

        // The custom connector performs the TLS handshake itself. tonic rejects an
        // `https://` authority unless its OWN built-in TLS is configured ("Connecting
        // to HTTPS without TLS enabled"), so the channel URI uses the `http://`
        // scheme: tonic then treats the connector's stream as the transport as-is,
        // while `build_tls_connector` still upgrades it to rustls (ALPN h2).
        let uri = format!("http://{normalized_authority}");
        let connector = build_tls_connector(client_config, host, port, resolver);

        Ok(Channel::from_shared(uri)
            .map_err(|_| "invalid ext_proc grpcService.target".to_string())?
            .connect_timeout(CONNECT_TIMEOUT)
            .connect_with_connector_lazy(connector))
    }

    /// Open one long-lived `Process` stream for a whole-request transaction
    /// (streamed body mode): the request-header verdict and every body-chunk
    /// verdict travel on the same stream, so the processor keeps its state
    /// across phases.
    ///
    /// The open is LAZY: no network work happens here and no response headers
    /// are awaited. The first `exchange` sends its message and only then
    /// awaits the call — a processor whose stack defers response headers
    /// until it has received (or sent) a message (grpc-go's default) would
    /// otherwise deadlock against an eager open until the timeout.
    pub async fn open_stream(&self) -> Result<ExtProcStream, tonic::Status> {
        let wait = self.timeout.min(PERMIT_WAIT_CAP);
        let permit = tokio::time::timeout(wait, self.runtime.sessions.clone().acquire_owned())
            .await
            .map_err(|_| tonic::Status::resource_exhausted("ext_proc target session limit reached"))?
            .map_err(|_| tonic::Status::unavailable("ext_proc target runtime closed"))?;
        let mut client = self.runtime.client.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let outbound = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(ExtProcStream {
            sender: Some(tx),
            inbound: InboundState::Opening(Box::pin(async move { client.process(outbound).await })),
            timeout: self.timeout,
            _permit: permit,
            #[cfg(test)]
            test_unsolicited: None,
        })
    }
}

/// Inbound side of a request-scoped stream: the un-awaited `process` call
/// until the first exchange resolves it, the live response stream afterwards.
enum InboundState {
    Opening(
        futures::future::BoxFuture<
            'static,
            Result<tonic::Response<tonic::Streaming<ProcessingResponse>>, tonic::Status>,
        >,
    ),
    Open(tonic::Streaming<ProcessingResponse>),
}

/// A request-scoped bidirectional ext_proc stream (see
/// [`ExtProcClient::open_stream`]). Dropping it closes the outbound direction;
/// the processor observes the half-close and ends its side.
pub struct ExtProcStream {
    sender: Option<tokio::sync::mpsc::Sender<ProcessingRequest>>,
    inbound: InboundState,
    timeout: Duration,
    _permit: OwnedSemaphorePermit,
    #[cfg(test)]
    test_unsolicited: Option<tokio::sync::mpsc::Receiver<ProcessingResponse>>,
}

impl ExtProcStream {
    fn reject_observable_unsolicited(&mut self) -> Result<(), tonic::Status> {
        use futures::FutureExt;

        #[cfg(test)]
        if let Some(receiver) = self.test_unsolicited.as_mut() {
            return match receiver.try_recv() {
                Ok(_) => Err(tonic::Status::failed_precondition(
                    "ext_proc processor returned an unsolicited verdict",
                )),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => Ok(()),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    Err(tonic::Status::aborted("ext_proc processor closed the stream"))
                }
            };
        }

        let InboundState::Open(streaming) = &mut self.inbound else {
            return Ok(());
        };
        match streaming.message().now_or_never() {
            None => Ok(()),
            Some(Ok(Some(_))) => Err(tonic::Status::failed_precondition(
                "ext_proc processor returned an unsolicited verdict",
            )),
            Some(Ok(None)) => Err(tonic::Status::aborted("ext_proc processor closed the stream")),
            Some(Err(status)) => Err(status),
        }
    }

    /// Send one message and await its phase-matched verdict under the
    /// per-message timeout (the first call also resolves the lazy stream open
    /// within the same bound). `last` half-closes the outbound direction after
    /// sending, validates the matching verdict, then uses one fresh timeout of
    /// the same configured duration to require clean inbound EOF. Keeping the
    /// terminal drain separately bounded avoids reducing the verdict budget by
    /// processor response latency. `ImmediateResponse` is always accepted; any
    /// other cross-phase verdict is a `failed_precondition`.
    pub async fn exchange(
        &mut self,
        request: ProcessingRequest,
        last: bool,
    ) -> Result<ProcessingResponse, tonic::Status> {
        self.reject_observable_unsolicited()?;
        let phase = ProcessingPhase::from_request(&request)?;
        let exchange = async {
            send_phase(&mut self.sender, request, last).await?;
            if let InboundState::Opening(open) = &mut self.inbound {
                self.inbound = InboundState::Open(open.await?.into_inner());
            }
            match &mut self.inbound {
                InboundState::Open(streaming) => streaming.message().await,
                InboundState::Opening(_) => unreachable!("resolved above"),
            }
        };
        let response = match tokio::time::timeout(self.timeout, exchange).await {
            Ok(Ok(Some(response))) => response,
            Ok(Ok(None)) => return Err(tonic::Status::aborted("ext_proc processor closed the stream")),
            Ok(Err(status)) => return Err(status),
            Err(_) => return Err(tonic::Status::deadline_exceeded("ext_proc verdict timed out")),
        };
        let terminal = matches!(
            response.response,
            Some(processing_response::Response::ImmediateResponse(_))
        );
        if !terminal && !phase.accepts(&response) {
            return Err(tonic::Status::failed_precondition(
                "ext_proc processor returned a verdict for the wrong phase",
            ));
        }
        if last {
            self.drain_clean_eof("ext_proc terminal drain timed out").await?;
        }
        Ok(response)
    }

    async fn drain_clean_eof(&mut self, timeout_message: &'static str) -> Result<(), tonic::Status> {
        let drain = async {
            if let InboundState::Opening(open) = &mut self.inbound {
                self.inbound = InboundState::Open(open.await?.into_inner());
            }
            match &mut self.inbound {
                InboundState::Open(streaming) => match streaming.message().await {
                    Ok(None) => Ok(()),
                    Ok(Some(_)) => Err(tonic::Status::failed_precondition(
                        "ext_proc processor returned a verdict after clean half-close",
                    )),
                    Err(status) => Err(status),
                },
                InboundState::Opening(_) => unreachable!("resolved above"),
            }
        };
        tokio::time::timeout(self.timeout, drain)
            .await
            .map_err(|_| tonic::Status::deadline_exceeded(timeout_message))?
    }

    /// Cleanly half-close the outbound Process direction and wait for the
    /// processor to close its response direction. Retaining the inbound stream
    /// until EOF prevents a successful reduction from being observed as RPC
    /// cancellation by the processor.
    pub async fn finish_success(&mut self) -> Result<(), tonic::Status> {
        self.reject_observable_unsolicited()?;
        self.sender.take();
        self.drain_clean_eof("ext_proc clean half-close timed out").await
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
            EffectiveCa::Custom(bundles) => {
                let mut added = 0usize;
                for bundle in bundles {
                    for cert in CertificateDer::pem_slice_iter(bundle) {
                        let cert = cert.map_err(|e| format!("ext_proc TLS: invalid CA certificate PEM: {e}"))?;
                        roots
                            .add(cert)
                            .map_err(|e| format!("ext_proc TLS: failed to add CA certificate: {e}"))?;
                        added += 1;
                    }
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
type PlainTransport = TokioIo<tokio::net::TcpStream>;

fn normalized_authority(host: &str, port: u16) -> String {
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(ip)) => format!("[{ip}]:{port}"),
        Ok(std::net::IpAddr::V4(ip)) => format!("{ip}:{port}"),
        Err(_) => format!("{}:{port}", host.to_ascii_lowercase()),
    }
}

fn fingerprint_effective_tls(hasher: &mut Sha256, tls: Option<&ResolvedOutboundTls>) {
    let Some(tls) = tls else {
        hasher.update(b"plaintext");
        return;
    };
    hasher.update(b"tls");
    hasher.update([u8::from(tls.verify)]);
    match &tls.ca {
        EffectiveCa::System => hasher.update(b"system-ca"),
        EffectiveCa::Custom(bundles) => {
            hasher.update(b"custom-ca");
            for bundle in bundles {
                hasher.update((bundle.len() as u64).to_be_bytes());
                hasher.update(bundle);
            }
        }
    }
    if let Some((certificate, key)) = &tls.client_identity {
        hasher.update(b"mtls");
        hasher.update((certificate.len() as u64).to_be_bytes());
        hasher.update(certificate);
        hasher.update((key.len() as u64).to_be_bytes());
        hasher.update(key.as_slice());
    } else {
        hasher.update(b"no-mtls");
    }
    if let Some(san) = &tls.san {
        hasher.update(b"san");
        if let Ok(encoded) = serde_json::to_vec(san) {
            hasher.update(encoded);
        }
    }
}

fn split_authority(authority: &str) -> Result<(String, u16), String> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .ok_or_else(|| "invalid ext_proc IPv6 authority".to_string())?;
        let port = suffix
            .strip_prefix(':')
            .ok_or_else(|| "ext_proc target is missing a port".to_string())?
            .parse::<u16>()
            .map_err(|_| "ext_proc target has an invalid port".to_string())?;
        return Ok((host.to_string(), port));
    }
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| "ext_proc target is missing a port".to_string())?;
    let port = port
        .parse::<u16>()
        .map_err(|_| "ext_proc target has an invalid port".to_string())?;
    if host.is_empty() {
        return Err("ext_proc target host is empty".to_string());
    }
    Ok((host.to_string(), port))
}

async fn connect_admitted(
    resolver: &EgressResolver,
    host: &str,
    port: u16,
) -> Result<tokio::net::TcpStream, std::io::Error> {
    let addrs = resolver
        .resolve_socket_addrs(host, port)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
    let mut last_error = None;
    for addr in addrs {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "ext_proc target has no admitted address",
        )
    }))
}

fn build_plain_connector(
    host: String,
    port: u16,
    resolver: Arc<EgressResolver>,
) -> impl tower::Service<
    Uri,
    Response = PlainTransport,
    Error = std::io::Error,
    Future = impl std::future::Future<Output = Result<PlainTransport, std::io::Error>> + Send,
> + Clone
       + Send {
    tower::service_fn(move |_uri: Uri| {
        let host = host.clone();
        let resolver = Arc::clone(&resolver);
        async move { connect_admitted(&resolver, &host, port).await.map(TokioIo::new) }
    })
}

/// Build the `tower` service that tonic drives per dial: TCP connect via
/// `HttpConnector`, then a rustls handshake, yielding a hyper-compatible IO.
fn build_tls_connector(
    client_config: ClientConfig,
    host: String,
    port: u16,
    resolver: Arc<EgressResolver>,
) -> impl tower::Service<
    Uri,
    Response = TlsTransport,
    Error = std::io::Error,
    Future = impl std::future::Future<Output = Result<TlsTransport, std::io::Error>> + Send,
> + Clone
       + Send {
    let tls_connector = TlsConnector::from(Arc::new(client_config));

    tower::service_fn(move |_uri: Uri| {
        let tls = tls_connector.clone();
        let host = host.clone();
        let resolver = Arc::clone(&resolver);
        async move {
            let server_name = ServerName::try_from(host.clone())
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid ext_proc DNS name"))?
                .to_owned();
            let tcp = connect_admitted(&resolver, &host, port).await?;
            let tls_stream = tls.connect(server_name, tcp).await?;
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
    use edgion_common::ext_proc::proto::common_response::ResponseStatus;
    use edgion_common::ext_proc::proto::CommonResponse;
    use edgion_common::ext_proc::proto::{
        processing_request, BodyResponse, HeadersResponse, HttpBody, HttpHeaders, ImmediateResponse,
    };
    use edgion_resources::plugin_configs::GrpcServiceConfig;
    use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
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

    #[test]
    fn processing_phase_accepts_only_matching_response() {
        let header_request = ProcessingRequest {
            request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
        };
        let body_request = ProcessingRequest {
            request: Some(processing_request::Request::RequestBody(HttpBody::default())),
        };
        let header_response = ProcessingResponse {
            response: Some(processing_response::Response::RequestHeaders(HeadersResponse::default())),
            mode_override: None,
        };
        let body_response = ProcessingResponse {
            response: Some(processing_response::Response::RequestBody(BodyResponse::default())),
            mode_override: None,
        };

        let header_phase = ProcessingPhase::from_request(&header_request).unwrap();
        let body_phase = ProcessingPhase::from_request(&body_request).unwrap();
        assert!(header_phase.accepts(&header_response));
        assert!(!header_phase.accepts(&body_response));
        assert!(body_phase.accepts(&body_response));
        assert!(!body_phase.accepts(&header_response));
    }

    #[test]
    fn immediate_response_is_terminal_not_a_phase_verdict() {
        let request = ProcessingRequest {
            request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
        };
        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(
                ImmediateResponse::default(),
            )),
            mode_override: None,
        };

        assert!(!ProcessingPhase::from_request(&request).unwrap().accepts(&immediate));
        assert!(matches!(
            immediate.response,
            Some(processing_response::Response::ImmediateResponse(_))
        ));
    }

    #[test]
    fn standalone_mode_override_is_not_a_phase_verdict() {
        let request = ProcessingRequest {
            request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
        };
        let standalone = ProcessingResponse {
            response: None,
            mode_override: Some(Default::default()),
        };

        assert!(!ProcessingPhase::from_request(&request).unwrap().accepts(&standalone));
    }

    #[tokio::test]
    async fn equivalent_authorities_and_message_timeouts_share_target_runtime() {
        let mut first = ExtProcConfig::default();
        first.grpc_service.target = "EXAMPLE.com:9000".to_string();
        first.grpc_service.timeout_ms = 50;
        let mut second = first.clone();
        second.grpc_service.target = "example.COM:9000".to_string();
        second.grpc_service.timeout_ms = 500;
        let first = ExtProcClient::new(&first).unwrap();
        let second = ExtProcClient::new(&second).unwrap();
        assert!(Arc::ptr_eq(&first.runtime, &second.runtime));
    }

    #[tokio::test]
    async fn distinct_effective_tls_policy_does_not_share_target_runtime() {
        install_test_crypto_provider();
        let mut verified = tls_cfg();
        verified.grpc_service.tls.outbound.verify = Some(true);
        let mut unverified = verified.clone();
        unverified.grpc_service.tls.outbound.verify = Some(false);
        let verified = ExtProcClient::new(&verified).unwrap();
        let unverified = ExtProcClient::new(&unverified).unwrap();
        assert!(!Arc::ptr_eq(&verified.runtime, &unverified.runtime));
    }

    #[tokio::test]
    async fn inherited_tls_hot_change_changes_target_runtime_identity() {
        use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
        use edgion_resources::resources::common::OutboundTlsConfig;

        static GLOBAL_TLS_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = GLOBAL_TLS_TEST.lock().unwrap();
        install_test_crypto_provider();
        let original = outbound_global_default();
        set_outbound_global_default(OutboundTlsConfig::default());
        let inherited_verified = ExtProcClient::new(&tls_cfg()).unwrap();
        set_outbound_global_default(OutboundTlsConfig {
            verify: false,
            ..Default::default()
        });
        let inherited_unverified = ExtProcClient::new(&tls_cfg()).unwrap();
        set_outbound_global_default(original);
        assert!(!Arc::ptr_eq(&inherited_verified.runtime, &inherited_unverified.runtime));
    }

    #[tokio::test]
    async fn session_limit_is_bounded_and_raii_release_reopens_capacity() {
        let mut cfg = ExtProcConfig::default();
        cfg.grpc_service.target = "session-cap.example:9000".to_string();
        cfg.grpc_service.timeout_ms = 5;
        let client = ExtProcClient::new(&cfg).unwrap();
        let mut streams = Vec::new();
        for _ in 0..TARGET_SESSION_CAP {
            streams.push(client.open_stream().await.unwrap());
        }
        let saturated = match client.open_stream().await {
            Ok(_) => panic!("session cap must reject excess logical streams"),
            Err(status) => status,
        };
        assert_eq!(saturated.code(), tonic::Code::ResourceExhausted);
        streams.pop();
        assert!(
            client.open_stream().await.is_ok(),
            "dropping a stream releases its permit"
        );
    }

    #[tokio::test]
    async fn per_message_timeout_includes_blocked_send_and_lazy_open() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        sender
            .send(ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
            })
            .await
            .unwrap();
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = semaphore.acquire_owned().await.unwrap();
        let mut stream = ExtProcStream {
            sender: Some(sender),
            inbound: InboundState::Opening(Box::pin(std::future::pending())),
            timeout: Duration::from_millis(5),
            _permit: permit,
            test_unsolicited: None,
        };
        let request = ProcessingRequest {
            request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
        };
        let status = stream.exchange(request, false).await.unwrap_err();
        assert_eq!(status.code(), tonic::Code::DeadlineExceeded);
        drop(receiver);
    }

    #[tokio::test]
    async fn observable_unsolicited_verdict_is_rejected_before_send() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let (unsolicited_tx, unsolicited_rx) = tokio::sync::mpsc::channel(1);
        unsolicited_tx.send(ProcessingResponse::default()).await.unwrap();
        let permit = Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap();
        let mut stream = ExtProcStream {
            sender: Some(sender),
            inbound: InboundState::Opening(Box::pin(std::future::pending())),
            timeout: Duration::from_millis(5),
            _permit: permit,
            test_unsolicited: Some(unsolicited_rx),
        };
        let status = stream.reject_observable_unsolicited().unwrap_err();
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert_eq!(status.message(), "ext_proc processor returned an unsolicited verdict");
    }

    /// Minimal in-process ext_proc service: echoes a per-phase verdict, so the
    /// long-lived stream can be exercised end to end without external infra.
    mod live_stream {
        use super::*;
        use edgion_common::ext_proc::proto::external_processor_server::{ExternalProcessor, ExternalProcessorServer};
        use edgion_common::ext_proc::proto::{
            processing_request, BodyResponse, HeadersResponse, HttpHeaders, HttpTrailers, TrailersResponse,
        };
        use tokio_stream::wrappers::ReceiverStream;
        use tokio_stream::StreamExt;

        struct EchoProcessor;

        #[derive(Clone, Copy)]
        enum TerminalBehavior {
            Clean,
            ExtraVerdict,
            Status,
            NeverClose,
        }

        #[derive(Clone, Copy)]
        enum TerminalPhase {
            ResponseBody,
            ResponseTrailers,
        }

        struct TerminalProcessor {
            behavior: TerminalBehavior,
            phase: TerminalPhase,
            observed: Mutex<Option<tokio::sync::oneshot::Sender<bool>>>,
        }

        #[tonic::async_trait]
        impl ExternalProcessor for TerminalProcessor {
            type ProcessStream = ReceiverStream<Result<ProcessingResponse, tonic::Status>>;

            async fn process(
                &self,
                request: tonic::Request<tonic::Streaming<ProcessingRequest>>,
            ) -> Result<tonic::Response<Self::ProcessStream>, tonic::Status> {
                let mut inbound = request.into_inner();
                let (tx, rx) = tokio::sync::mpsc::channel(4);
                let behavior = self.behavior;
                let phase = self.phase;
                let observed = self.observed.lock().unwrap().take();
                tokio::spawn(async move {
                    let first = inbound.message().await;
                    let matches_phase = matches!(
                        (phase, first),
                        (
                            TerminalPhase::ResponseBody,
                            Ok(Some(ProcessingRequest {
                                request: Some(processing_request::Request::ResponseBody(_)),
                            }))
                        ) | (
                            TerminalPhase::ResponseTrailers,
                            Ok(Some(ProcessingRequest {
                                request: Some(processing_request::Request::ResponseTrailers(_)),
                            }))
                        )
                    );
                    if !matches_phase {
                        return;
                    }
                    let matching_response = match phase {
                        TerminalPhase::ResponseBody => {
                            processing_response::Response::ResponseBody(BodyResponse::default())
                        }
                        TerminalPhase::ResponseTrailers => {
                            processing_response::Response::ResponseTrailers(TrailersResponse::default())
                        }
                    };
                    if tx
                        .send(Ok(ProcessingResponse {
                            response: Some(matching_response.clone()),
                            mode_override: None,
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let clean_eof = matches!(inbound.message().await, Ok(None));
                    if let Some(observed) = observed {
                        let _ = observed.send(clean_eof);
                    }
                    match behavior {
                        TerminalBehavior::Clean => {}
                        TerminalBehavior::ExtraVerdict => {
                            let _ = tx
                                .send(Ok(ProcessingResponse {
                                    response: Some(matching_response),
                                    mode_override: None,
                                }))
                                .await;
                        }
                        TerminalBehavior::Status => {
                            let _ = tx
                                .send(Err(tonic::Status::resource_exhausted("terminal status detail")))
                                .await;
                        }
                        TerminalBehavior::NeverClose => std::future::pending::<()>().await,
                    }
                });
                Ok(tonic::Response::new(ReceiverStream::new(rx)))
            }
        }

        #[tonic::async_trait]
        impl ExternalProcessor for EchoProcessor {
            type ProcessStream = ReceiverStream<Result<ProcessingResponse, tonic::Status>>;

            async fn process(
                &self,
                request: tonic::Request<tonic::Streaming<ProcessingRequest>>,
            ) -> Result<tonic::Response<Self::ProcessStream>, tonic::Status> {
                let mut inbound = request.into_inner();
                let (tx, rx) = tokio::sync::mpsc::channel(4);
                tokio::spawn(async move {
                    let mut rank = 0u8;
                    while let Some(Ok(message)) = inbound.next().await {
                        let (next_rank, response) = match message.request {
                            Some(processing_request::Request::RequestHeaders(_)) => (
                                1,
                                ProcessingResponse {
                                    response: Some(processing_response::Response::RequestHeaders(
                                        HeadersResponse::default(),
                                    )),
                                    mode_override: None,
                                },
                            ),
                            Some(processing_request::Request::RequestBody(_)) => (
                                2,
                                ProcessingResponse {
                                    response: Some(processing_response::Response::RequestBody(BodyResponse::default())),
                                    mode_override: None,
                                },
                            ),
                            Some(processing_request::Request::ResponseHeaders(_)) => (
                                3,
                                ProcessingResponse {
                                    response: Some(processing_response::Response::ResponseHeaders(
                                        HeadersResponse::default(),
                                    )),
                                    mode_override: None,
                                },
                            ),
                            Some(processing_request::Request::ResponseBody(_)) => (
                                4,
                                ProcessingResponse {
                                    response: Some(
                                        processing_response::Response::ResponseBody(BodyResponse::default()),
                                    ),
                                    mode_override: None,
                                },
                            ),
                            Some(processing_request::Request::ResponseTrailers(_)) => (
                                5,
                                ProcessingResponse {
                                    response: Some(processing_response::Response::ResponseTrailers(
                                        TrailersResponse::default(),
                                    )),
                                    mode_override: None,
                                },
                            ),
                            _ => break,
                        };
                        if next_rank < rank {
                            let _ = tx
                                .send(Err(tonic::Status::failed_precondition("phase order regressed")))
                                .await;
                            break;
                        }
                        rank = next_rank;
                        if tx.send(Ok(response)).await.is_err() {
                            break;
                        }
                    }
                });
                Ok(tonic::Response::new(ReceiverStream::new(rx)))
            }
        }

        async fn spawn_echo_server() -> std::net::SocketAddr {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(
                tonic::transport::Server::builder()
                    .add_service(ExternalProcessorServer::new(EchoProcessor))
                    .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
            );
            addr
        }

        async fn spawn_terminal_server(
            behavior: TerminalBehavior,
            phase: TerminalPhase,
        ) -> (std::net::SocketAddr, tokio::sync::oneshot::Receiver<bool>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
            tokio::spawn(
                tonic::transport::Server::builder()
                    .add_service(ExternalProcessorServer::new(TerminalProcessor {
                        behavior,
                        phase,
                        observed: Mutex::new(Some(observed_tx)),
                    }))
                    .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
            );
            (addr, observed_rx)
        }

        fn cfg_for(addr: std::net::SocketAddr) -> ExtProcConfig {
            ExtProcConfig {
                grpc_service: GrpcServiceConfig {
                    target: addr.to_string(),
                    timeout_ms: 2_000,
                    tls: TlsConfig::default(),
                },
                ..Default::default()
            }
        }

        /// One stream carries the header verdict plus several body-chunk
        /// verdicts, with the final chunk half-closing the outbound side.
        #[tokio::test]
        async fn stream_carries_headers_and_multiple_body_chunks() {
            install_test_crypto_provider();
            let addr = spawn_echo_server().await;
            let client = ExtProcClient::new(&cfg_for(addr)).unwrap();
            let mut stream = client.open_stream().await.unwrap();

            let headers = ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(HttpHeaders {
                    headers: None,
                    end_of_stream: false,
                })),
            };
            let verdict = stream.exchange(headers, false).await.unwrap();
            assert!(matches!(
                verdict.response,
                Some(processing_response::Response::RequestHeaders(_))
            ));

            for (index, last) in [(0u8, false), (1, false), (2, true)] {
                let chunk = ProcessingRequest {
                    request: Some(processing_request::Request::RequestBody(HttpBody {
                        body: vec![index],
                        end_of_stream: last,
                    })),
                };
                let verdict = stream.exchange(chunk, last).await.unwrap();
                assert!(matches!(
                    verdict.response,
                    Some(processing_response::Response::RequestBody(_))
                ));
            }
        }

        #[tokio::test]
        async fn one_process_stream_carries_request_and_response_in_exact_order() {
            install_test_crypto_provider();
            let addr = spawn_echo_server().await;
            let client = ExtProcClient::new(&cfg_for(addr)).unwrap();
            let mut stream = client.open_stream().await.unwrap();
            let messages = vec![
                ProcessingRequest {
                    request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
                },
                ProcessingRequest {
                    request: Some(processing_request::Request::RequestBody(HttpBody {
                        body: b"request".to_vec(),
                        end_of_stream: true,
                    })),
                },
                ProcessingRequest {
                    request: Some(processing_request::Request::ResponseHeaders(HttpHeaders::default())),
                },
                ProcessingRequest {
                    request: Some(processing_request::Request::ResponseBody(HttpBody {
                        body: b"response".to_vec(),
                        end_of_stream: true,
                    })),
                },
            ];
            for (index, message) in messages.into_iter().enumerate() {
                stream.exchange(message, index == 3).await.unwrap();
            }
        }

        #[tokio::test]
        async fn streamed_response_then_trailers_is_ordered_and_half_closes_on_trailers() {
            install_test_crypto_provider();
            let addr = spawn_echo_server().await;
            let client = ExtProcClient::new(&cfg_for(addr)).unwrap();
            let mut stream = client.open_stream().await.unwrap();

            for message in [
                ProcessingRequest {
                    request: Some(processing_request::Request::ResponseHeaders(HttpHeaders {
                        headers: None,
                        end_of_stream: false,
                    })),
                },
                ProcessingRequest {
                    request: Some(processing_request::Request::ResponseBody(HttpBody {
                        body: b"response".to_vec(),
                        end_of_stream: false,
                    })),
                },
            ] {
                stream.exchange(message, false).await.unwrap();
            }
            let verdict = stream
                .exchange(
                    ProcessingRequest {
                        request: Some(processing_request::Request::ResponseTrailers(HttpTrailers::default())),
                    },
                    true,
                )
                .await
                .unwrap();
            assert!(matches!(
                verdict.response,
                Some(processing_response::Response::ResponseTrailers(_))
            ));
        }

        #[tokio::test]
        async fn final_response_body_and_trailers_produce_server_eof_not_cancellation() {
            install_test_crypto_provider();
            for phase in [TerminalPhase::ResponseBody, TerminalPhase::ResponseTrailers] {
                let (addr, observed) = spawn_terminal_server(TerminalBehavior::Clean, phase).await;
                let client = ExtProcClient::new(&cfg_for(addr)).unwrap();
                let mut stream = client.open_stream().await.unwrap();
                let request = match phase {
                    TerminalPhase::ResponseBody => processing_request::Request::ResponseBody(HttpBody {
                        body: b"final".to_vec(),
                        end_of_stream: true,
                    }),
                    TerminalPhase::ResponseTrailers => {
                        processing_request::Request::ResponseTrailers(HttpTrailers::default())
                    }
                };
                stream
                    .exchange(ProcessingRequest { request: Some(request) }, true)
                    .await
                    .unwrap();
                assert!(observed.await.expect("server reports terminal receive state"));
            }
        }

        #[tokio::test]
        async fn terminal_drain_rejects_extra_verdict_and_timeout() {
            install_test_crypto_provider();
            for (behavior, expected) in [
                (TerminalBehavior::ExtraVerdict, tonic::Code::FailedPrecondition),
                (TerminalBehavior::Status, tonic::Code::ResourceExhausted),
                (TerminalBehavior::NeverClose, tonic::Code::DeadlineExceeded),
            ] {
                let (addr, observed) = spawn_terminal_server(behavior, TerminalPhase::ResponseTrailers).await;
                let mut config = cfg_for(addr);
                config.grpc_service.timeout_ms = 50;
                let client = ExtProcClient::new(&config).unwrap();
                let mut stream = client.open_stream().await.unwrap();
                let error = stream
                    .exchange(
                        ProcessingRequest {
                            request: Some(processing_request::Request::ResponseTrailers(HttpTrailers::default())),
                        },
                        true,
                    )
                    .await
                    .unwrap_err();
                assert_eq!(error.code(), expected);
                assert!(observed.await.expect("server reports terminal receive state"));
            }
        }

        /// A message sent after the outbound half-close must fail cleanly, not
        /// hang: the sender was consumed by `last = true`.
        #[tokio::test]
        async fn exchange_after_half_close_fails() {
            install_test_crypto_provider();
            let addr = spawn_echo_server().await;
            let client = ExtProcClient::new(&cfg_for(addr)).unwrap();
            let mut stream = client.open_stream().await.unwrap();

            let eos_chunk = ProcessingRequest {
                request: Some(processing_request::Request::RequestBody(HttpBody {
                    body: Vec::new(),
                    end_of_stream: true,
                })),
            };
            stream.exchange(eos_chunk.clone(), true).await.unwrap();
            let err = stream.exchange(eos_chunk, true).await.unwrap_err();
            assert!(matches!(err.code(), tonic::Code::Internal | tonic::Code::Aborted));
        }
    }

    #[test]
    fn response_status_detects_continue_and_replace_and_rejects_unknown() {
        use edgion_common::ext_proc::proto::{HeadersResponse, ProcessingResponse};

        let response_with_status = |status| ProcessingResponse {
            response: Some(processing_response::Response::RequestHeaders(HeadersResponse {
                response: Some(CommonResponse {
                    status,
                    ..Default::default()
                }),
            })),
            mode_override: None,
        };

        assert_eq!(
            response_status(&response_with_status(ResponseStatus::ContinueAndReplace as i32)).unwrap(),
            ResponseStatus::ContinueAndReplace
        );
        assert_eq!(
            response_status(&response_with_status(ResponseStatus::Continue as i32)).unwrap(),
            ResponseStatus::Continue
        );
        assert_eq!(
            response_status(&response_with_status(99)).unwrap_err().code(),
            tonic::Code::FailedPrecondition
        );
    }

    #[tokio::test]
    async fn final_phase_half_closes_request_stream() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let mut sender = Some(tx);
        let request = ProcessingRequest {
            request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
        };

        send_phase(&mut sender, request, true).await.unwrap();
        assert!(rx.recv().await.is_some());
        assert!(
            rx.recv().await.is_none(),
            "the processor must observe EOF immediately after the final phase"
        );
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

    /// Custom CA via resolved_ca_certificates: supply a valid self-signed CA PEM in a K8s Secret;
    /// the resolver must build the REPLACE-root ClientConfig without error.
    #[tokio::test]
    async fn extproc_custom_inline_ca_builds() {
        install_test_crypto_provider();

        let ca = rcgen::generate_simple_self_signed(vec!["test-ca".into()]).unwrap();
        let ca_pem = ca.cert.pem().into_bytes();

        // Build a K8s Secret carrying the CA PEM under the expected key.
        let mut data = std::collections::BTreeMap::new();
        data.insert("ca.crt".to_string(), k8s_openapi::ByteString(ca_pem));
        let ca_secret = k8s_openapi::api::core::v1::Secret {
            data: Some(data),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        };

        let mut cfg = tls_cfg();
        // Declare a caCertificateRef so the resolver picks up the custom CA.
        cfg.grpc_service.tls.outbound = OutboundTlsLocal {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                    kind: "Secret".to_string(),
                    name: "test-ca-secret".to_string(),
                    group: String::new(),
                    namespace: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        cfg.grpc_service.tls.resolved_ca_certificates = Some(vec![ca_secret]);

        assert!(
            ExtProcClient::new(&cfg).is_ok(),
            "Custom CA via resolved_ca_certificates must build the REPLACE-root channel without error"
        );
    }
}
