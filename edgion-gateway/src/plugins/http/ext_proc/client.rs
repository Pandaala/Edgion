//! gRPC client for the external processing service.
//!
//! The channel is built lazily at plugin construction (no blocking connect) and
//! multiplexes request-scoped logical `Process` streams. One matched ExtProc
//! entry retains one logical stream across its request and response phases.
//!
//! TLS is routed through the shared outbound resolver
//! ([`edgion_common::link_sys_core::outbound_tls::resolve_validated_outbound_tls`]) so the
//! ext_proc dial has full parity with the other outbound callers: verify toggle
//! (gated), custom-CA REPLACE, and client mTLS identity. Because tonic's
//! `ClientTlsConfig` exposes none of those knobs, the TLS path is built as a
//! hand-rolled rustls `ClientConfig` plumbed into the channel via a custom
//! `tower` connector (`connect_with_connector_lazy`).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Weak};
use std::time::Duration;

use edgion_common::ext_proc::proto::external_processor_client::ExternalProcessorClient;
use edgion_common::ext_proc::proto::{processing_response, ProcessingRequest, ProcessingResponse};
use edgion_common::link_sys_core::outbound_tls::{
    resolve_validated_outbound_tls, validate_active_tls_inheritance, validate_local_tls_selection, EffectiveCa,
    GlobalOutboundTlsSnapshot, ResolvedOutboundTls, ValidatedLocalTlsSelection,
};
use edgion_resources::resources::refs::plugin_fields::BoundPluginEntry;
use edgion_resources::EdgionPlugin;
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
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
    #[allow(clippy::result_large_err)]
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
include!("../../../../tests/support/plugins/http/ext_proc/client_test/93_function_item.rs");

#[cfg(test)]
include!("../../../../tests/support/plugins/http/ext_proc/client_test/106_function_item.rs");

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
    /// Check the actual source and active TLS material before either cache is used.
    pub(super) fn validate_tls_entry<'a>(
        entry: &'a BoundPluginEntry<'_>,
        defaults: &GlobalOutboundTlsSnapshot,
    ) -> Result<Option<ValidatedLocalTlsSelection<'a>>, String> {
        let error = || "ext_proc TLS configuration is invalid".to_string();
        let EdgionPlugin::ExtProc(config) = entry.config() else {
            return Err(error());
        };
        entry.owner_namespace().map_err(|_| error())?;
        if !config.grpc_service.tls.enabled {
            return Ok(None);
        }
        let selection = entry.outbound_tls_selection().map_err(|_| error())?;
        let proof = validate_local_tls_selection(selection).map_err(|_| error())?;
        if proof.active_field().is_none() {
            return Err(error());
        }
        validate_active_tls_inheritance(&proof, defaults).map_err(|_| error())?;
        Ok(Some(proof))
    }

    /// Build a lazily-connecting client from the actual plugin entry and captured TLS defaults.
    ///
    /// Returns `Err` for a structurally invalid target or for any TLS
    /// configuration error surfaced by the shared outbound resolver (missing CA,
    /// invalid client identity, provider-cannot-disable-verify, ...). It never
    /// blocks on the network — the actual connection is established on first use.
    /// The config-layer `validation_error` already gates target/timeout, so a
    /// build failure here is unexpected and `ExtProc::create_attached` fails the plugin
    /// closed (drops it). That fail-closed behavior is intentional for ext_proc;
    /// do NOT add a secure fallback.
    pub fn new(entry: &BoundPluginEntry<'_>, defaults: &GlobalOutboundTlsSnapshot) -> Result<Self, String> {
        let EdgionPlugin::ExtProc(cfg) = entry.config() else {
            return Err("ext_proc TLS configuration is invalid".into());
        };
        let grpc = &cfg.grpc_service;
        let timeout = Duration::from_millis(grpc.timeout_ms);
        let (host, port) = split_authority(&grpc.target)?;
        let normalized_authority = normalized_authority(&host, port);
        let resolved_tls = Self::validate_tls_entry(entry, defaults)?
            .as_ref()
            .map(|proof| {
                resolve_validated_outbound_tls(proof, true, defaults)
                    .map_err(|_| "ext_proc TLS configuration is invalid".to_string())
            })
            .transpose()?;
        if let Some(resolved) = resolved_tls.as_ref() {
            validate_effective_tls_capabilities(resolved)?;
        }
        let mut hasher = Sha256::new();
        hasher.update(normalized_authority.as_bytes());
        fingerprint_effective_tls(&mut hasher, resolved_tls.as_ref());
        hasher.update([u8::from(default_block_private())]);
        let cache_key = format!("{:x}", hasher.finalize());
        if let Some(runtime) = TARGET_RUNTIMES.lock().get(&cache_key).and_then(Weak::upgrade) {
            return Ok(Self { runtime, timeout });
        }

        let channel = Self::build_channel(&normalized_authority, host, port, resolved_tls.as_ref())?;
        let runtime = Arc::new(ExtProcTargetRuntime {
            client: ExternalProcessorClient::new(channel),
            sessions: Arc::new(Semaphore::new(TARGET_SESSION_CAP)),
        });
        let mut cache = TARGET_RUNTIMES.lock();
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
        })
    }
}

/// Inbound side of a request-scoped stream: the un-awaited `process` call
/// until the first exchange resolves it, the live response stream afterwards.
#[allow(clippy::large_enum_variant)]
enum InboundState {
    Opening(
        futures::future::BoxFuture<
            'static,
            Result<tonic::Response<tonic::Streaming<ProcessingResponse>>, tonic::Status>,
        >,
    ),
    Open(tonic::Streaming<ProcessingResponse>),
    #[cfg(test)]
    Test(tokio::sync::mpsc::Receiver<Result<ProcessingResponse, tonic::Status>>),
}

/// A request-scoped bidirectional ext_proc stream (see
/// [`ExtProcClient::open_stream`]). Dropping it closes the outbound direction;
/// the processor observes the half-close and ends its side.
pub struct ExtProcStream {
    sender: Option<tokio::sync::mpsc::Sender<ProcessingRequest>>,
    inbound: InboundState,
    timeout: Duration,
    _permit: OwnedSemaphorePermit,
}

impl ExtProcStream {
    #[allow(clippy::result_large_err)]
    fn reject_observable_unsolicited(&mut self) -> Result<(), tonic::Status> {
        use futures::FutureExt;

        #[cfg(test)]
        if let InboundState::Test(receiver) = &mut self.inbound {
            return match receiver.try_recv() {
                Ok(Ok(_)) => Err(tonic::Status::failed_precondition(
                    "ext_proc processor returned an unsolicited verdict",
                )),
                Ok(Err(status)) => Err(status),
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

    async fn next_response(&mut self) -> Result<Option<ProcessingResponse>, tonic::Status> {
        if let InboundState::Opening(open) = &mut self.inbound {
            self.inbound = InboundState::Open(open.await?.into_inner());
        }
        match &mut self.inbound {
            InboundState::Open(streaming) => streaming.message().await,
            InboundState::Opening(_) => unreachable!("resolved above"),
            #[cfg(test)]
            InboundState::Test(receiver) => match receiver.recv().await {
                Some(result) => result.map(Some),
                None => Ok(None),
            },
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
        let timeout = self.timeout;
        let exchange = async {
            send_phase(&mut self.sender, request, last).await?;
            self.next_response().await
        };
        let response = match tokio::time::timeout(timeout, exchange).await {
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
        let timeout = self.timeout;
        let drain = async {
            match self.next_response().await {
                Ok(None) => Ok(()),
                Ok(Some(_)) => Err(tonic::Status::failed_precondition(
                    "ext_proc processor returned a verdict after clean half-close",
                )),
                Err(status) => Err(status),
            }
        };
        tokio::time::timeout(timeout, drain)
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

/// Reject effective identity policies the ExtProc transport cannot enforce.
/// Check after inheritance is resolved and before any target runtime is reused.
fn validate_effective_tls_capabilities(resolved: &ResolvedOutboundTls) -> Result<(), String> {
    if let Some(validation) = resolved.san.as_ref() {
        if validation.hostname.is_some() {
            return Err("ext_proc TLS validation.hostname is unsupported; remove the hostname override".to_string());
        }
        if validation
            .subject_alt_names
            .as_ref()
            .is_some_and(|names| !names.is_empty())
        {
            return Err("ext_proc TLS validation.subjectAltNames is unsupported; remove the SAN policy".to_string());
        }
    }
    Ok(())
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
#[path = "../../../../tests/unit/plugins/http/ext_proc/client_test.rs"]
pub(super) mod tests;
