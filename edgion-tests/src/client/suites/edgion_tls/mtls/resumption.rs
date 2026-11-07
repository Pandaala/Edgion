//! Cross-SNI TLS session resumption probe (gw-infra-01).
//!
//! `mtls-terminate.example.com` (no client authentication) and
//! `mtls.example.com` (`Mutual`) share one listener and one server certificate.
//! An attacker controls its own ClientHello, so the probe offers a session
//! from the terminate host under the Mutual host's SNI. A resumed handshake
//! never requests a client certificate; the Gateway must therefore refuse to
//! resume and fall back to a full handshake that rejects the missing
//! certificate. A same-SNI resumption control proves the probe can resume.

use super::super::mtls_failclose::{TlsProbe, MTLS_SERVER, VALID_CLIENT};
use parking_lot::Mutex;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{ClientSessionStore, Resumption, Tls12ClientSessionValue, Tls13ClientSessionValue};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName, UnixTime};
use rustls::{
    AlertDescription, ClientConfig, DigitallySignedStruct, HandshakeKind, NamedGroup, SignatureScheme,
    SupportedProtocolVersion,
};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

const TERMINATE_HOST: &str = "mtls-terminate.example.com";
const MUTUAL_HOST: &str = "mtls.example.com";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_TLS13_TICKETS: usize = 8;

/// Session store that ignores the server name, modelling an attacker that
/// replays any session it holds under any SNI. `offered` records that rustls
/// retrieved a stored session for the connection; rustls may still drop it for
/// an incompatible config or expiry, which the same-config, seconds-old
/// same-SNI control rules out in practice.
#[derive(Default)]
struct SniAgnosticStore {
    tls12: Mutex<Option<Tls12ClientSessionValue>>,
    tls13: Mutex<Vec<Tls13ClientSessionValue>>,
    offered: AtomicBool,
}

impl fmt::Debug for SniAgnosticStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SniAgnosticStore")
    }
}

impl ClientSessionStore for SniAgnosticStore {
    fn set_kx_hint(&self, _: ServerName<'static>, _: NamedGroup) {}

    fn kx_hint(&self, _: &ServerName<'_>) -> Option<NamedGroup> {
        None
    }

    fn set_tls12_session(&self, _: ServerName<'static>, value: Tls12ClientSessionValue) {
        *self.tls12.lock() = Some(value);
    }

    fn tls12_session(&self, _: &ServerName<'_>) -> Option<Tls12ClientSessionValue> {
        let session = self.tls12.lock().clone();
        if session.is_some() {
            self.offered.store(true, Ordering::Relaxed);
        }
        session
    }

    fn remove_tls12_session(&self, _: &ServerName<'static>) {
        *self.tls12.lock() = None;
    }

    fn insert_tls13_ticket(&self, _: ServerName<'static>, value: Tls13ClientSessionValue) {
        let mut tickets = self.tls13.lock();
        if tickets.len() < MAX_TLS13_TICKETS {
            tickets.push(value);
        }
    }

    fn take_tls13_ticket(&self, _: &ServerName<'static>) -> Option<Tls13ClientSessionValue> {
        let ticket = self.tls13.lock().pop();
        if ticket.is_some() {
            self.offered.store(true, Ordering::Relaxed);
        }
        ticket
    }
}

/// Accept only the pinned fixture leaf, but still verify handshake signatures.
#[derive(Debug)]
struct PinnedServer {
    certificate: CertificateDer<'static>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.certificate.as_ref() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Result of one probe connection.
#[derive(Debug, PartialEq, Eq)]
enum Attempt {
    /// Handshake and one HTTP exchange completed; `resumed` reports the kind.
    Served { resumed: bool },
    /// The peer rejected a missing client certificate during or right after
    /// the handshake (TLS 1.3 reports it after the client's Finished): a
    /// client-authentication alert, or a transport close that can overtake the
    /// alert while unread client records remain. `resumed` is the handshake
    /// kind if it got that far.
    Rejected { resumed: bool },
}

struct ResumptionProbe {
    connector: TlsConnector,
    store: Arc<SniAgnosticStore>,
}

impl ResumptionProbe {
    fn new(version: &'static SupportedProtocolVersion) -> Result<Self, String> {
        let pem = std::fs::read(MTLS_SERVER).map_err(|_| "Failed to read the mTLS server certificate fixture")?;
        let certificate = CertificateDer::pem_slice_iter(&pem)
            .next()
            .ok_or("Server certificate fixture contains no certificate")?
            .map_err(|_| "Server certificate fixture contains invalid PEM")?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let store = Arc::new(SniAgnosticStore::default());
        let mut config = ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[version])
            .map_err(|_| "Failed to configure the resumption probe version")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedServer { certificate, provider }))
            .with_no_client_auth();
        config.resumption = Resumption::store(store.clone());
        Ok(Self {
            connector: TlsConnector::from(Arc::new(config)),
            store,
        })
    }

    /// Connect under `sni`, offering whatever session the store holds, and
    /// complete one HTTP exchange so TLS 1.3 tickets are received.
    async fn attempt(&self, host: &str, port: u16, sni: &str) -> Result<Attempt, String> {
        let server_name = ServerName::try_from(sni.to_owned()).map_err(|_| "Probe SNI is invalid")?;
        tokio::time::timeout(PROBE_TIMEOUT, async {
            let socket = TcpStream::connect((host, port))
                .await
                .map_err(|_| "Resumption probe TCP connect failed".to_string())?;
            let mut stream = match self.connector.connect(server_name, socket).await {
                Ok(stream) => stream,
                Err(error) => return classify_rejection(&error, false, "handshake"),
            };
            let resumed = stream.get_ref().1.handshake_kind() == Some(HandshakeKind::Resumed);
            let request = format!("GET /health HTTP/1.1\r\nHost: {sni}\r\nConnection: close\r\n\r\n");
            if let Err(error) = stream.write_all(request.as_bytes()).await {
                return classify_rejection(&error, resumed, "write");
            }
            let mut response = Vec::new();
            match stream.read_to_end(&mut response).await {
                Ok(_) if response.starts_with(b"HTTP/1.1 ") => Ok(Attempt::Served { resumed }),
                Ok(_) => Ok(Attempt::Rejected { resumed }),
                Err(error) => classify_rejection(&error, resumed, "read"),
            }
        })
        .await
        .map_err(|_| "Resumption probe timed out".to_string())?
    }
}

/// A client-authentication alert or a transport close is a rejection; any
/// other alert or local failure is a probe error. `HandshakeFailure` is also
/// what BoringSSL sends when select_cb rejects a connection, so the separate
/// valid-client control on the Mutual host is what rules out a misconfigured
/// fixture.
fn classify_rejection(error: &std::io::Error, resumed: bool, stage: &str) -> Result<Attempt, String> {
    match error.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
        Some(rustls::Error::AlertReceived(
            AlertDescription::CertificateRequired | AlertDescription::HandshakeFailure,
        )) => Ok(Attempt::Rejected { resumed }),
        Some(other) => Err(format!("Resumption probe {stage} failed: {other}")),
        None => match error.kind() {
            std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::BrokenPipe => Ok(Attempt::Rejected { resumed }),
            _ => Err(format!("Resumption probe {stage} failed: {error}")),
        },
    }
}

/// Run the control and the cross-SNI attack for one protocol version.
pub(crate) async fn require_cross_sni_resumption_refused(
    host: &str,
    port: u16,
    version: &'static SupportedProtocolVersion,
) -> Result<(), String> {
    let probe = ResumptionProbe::new(version)?;
    if probe.attempt(host, port, TERMINATE_HOST).await? != (Attempt::Served { resumed: false }) {
        return Err("Initial terminate-host handshake was not a served full handshake".to_string());
    }
    if probe.attempt(host, port, TERMINATE_HOST).await? != (Attempt::Served { resumed: true }) {
        return Err("Control: the terminate host did not resume its own session".to_string());
    }
    // The Mutual host must accept a valid client certificate, so the rejection
    // below is attributable to the missing certificate, not to the fixture.
    TlsProbe::new(MUTUAL_HOST, MTLS_SERVER, Some(VALID_CLIENT))?
        .run(host, port)
        .await?
        .require_complete()?;
    probe.store.offered.store(false, Ordering::Relaxed);
    let attack = probe.attempt(host, port, MUTUAL_HOST).await?;
    if !probe.store.offered.load(Ordering::Relaxed) {
        return Err("Attack precondition failed: no stored session was offered".to_string());
    }
    match attack {
        Attempt::Rejected { resumed: false } => Ok(()),
        Attempt::Served { resumed: false } => {
            Err("The Mutual host completed a full handshake without a client certificate".to_string())
        }
        resumed => Err(format!(
            "A terminate-host session crossed into the Mutual host without a client certificate: {resumed:?}"
        )),
    }
}
