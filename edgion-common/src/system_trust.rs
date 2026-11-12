//! Process-wide `System` trust anchors.
//!
//! `wellKnownCACertificates: System` (or an omitted CA) has exactly one meaning
//! in an Edgion process: the operating-system CA bundle file at the first
//! existing path among the fixed distribution paths [`SYSTEM_CA_FILES`]. Selection is not
//! influenced by configuration or by `SSL_CERT_FILE`. The Gateway backend path
//! (BoringSSL, via Pingora `ServerConf.ca_file`) and every `EffectiveCa::System`
//! consumer (LinkSys, plugins, OTLP, health checks) read this file. Custom trust
//! is expressed per resource with `caCertificateRefs`, which replaces `System`.
//!
//! The file is loaded once per process. The Gateway installs it at startup and
//! refuses to start when it is unusable. Other processes load it on first use;
//! an unusable file yields an empty trust set, so `System`-trust handshakes fail
//! closed. There is no fallback to a compiled-in root set. Changes to the file
//! take effect on process restart.

use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use base64::Engine as _;
use rustls::pki_types::{pem::PemObject, CertificateDer};
use rustls::RootCertStore;

/// Fixed OS CA bundle locations, in priority order; the first existing path is
/// the `System` bundle. Debian, Ubuntu, Alpine and the shipped images use the
/// first, RHEL-family hosts (the Rocky packages) the second, macOS the third.
pub const SYSTEM_CA_FILES: [&str; 3] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
];

/// Upper bound for reading a CA bundle file: the OS bundle, or a local CA file
/// such as `edgion-cli --ca-cert`. Distribution bundles are well below 1 MiB;
/// text between certificate blocks can make a file larger.
pub const MAX_CA_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// PEM labels that carry a certificate rustls does not load. BoringSSL loads
/// them, so they are reported as skipped.
const UNSUPPORTED_CERTIFICATE_LABELS: [&[u8]; 2] = [
    b"-----BEGIN TRUSTED CERTIFICATE-----",
    b"-----BEGIN X509 CERTIFICATE-----",
];

static SYSTEM_TRUST: OnceLock<SystemTrust> = OnceLock::new();

/// Where the process-wide trust set came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemTrustSource {
    /// The bundle file was loaded.
    Loaded(PathBuf),
    /// The bundle file is unusable; `System` trust is empty (fail closed).
    Unavailable(String),
}

/// Immutable `System` trust anchors shared by every TLS client in the process.
pub struct SystemTrust {
    source: SystemTrustSource,
    certificates: Vec<CertificateDer<'static>>,
    roots: Arc<RootCertStore>,
    pem: Vec<u8>,
    skipped: usize,
}

impl SystemTrust {
    /// Load `path` strictly: an unreadable, irregular, oversized or malformed
    /// file, or one without a usable certificate, is an error.
    pub fn load(path: &Path) -> Result<Self> {
        let bundle = load_bundle(path)?;
        anyhow::ensure!(
            !bundle.certificates.is_empty(),
            "CA file {} contains no usable certificates",
            path.display()
        );
        Ok(Self::new(SystemTrustSource::Loaded(path.to_path_buf()), bundle))
    }

    /// Load `path`, recording the failure reason instead of returning it.
    pub fn load_or_unavailable(path: &Path) -> Self {
        Self::load(path).unwrap_or_else(|error| {
            Self::new(
                SystemTrustSource::Unavailable(format!("{error:#}")),
                ParsedBundle::default(),
            )
        })
    }

    fn new(source: SystemTrustSource, bundle: ParsedBundle) -> Self {
        let pem = encode_pem(&bundle.certificates);
        Self {
            source,
            certificates: bundle.certificates,
            roots: Arc::new(bundle.roots),
            pem,
            skipped: bundle.skipped,
        }
    }

    pub fn source(&self) -> &SystemTrustSource {
        &self.source
    }

    /// The loaded bundle file, which Pingora also reads for backend TLS.
    pub fn bundle_file(&self) -> Option<&Path> {
        match &self.source {
            SystemTrustSource::Loaded(path) => Some(path),
            SystemTrustSource::Unavailable(_) => None,
        }
    }

    /// Certificates accepted as rustls trust anchors.
    pub fn certificates(&self) -> &[CertificateDer<'static>] {
        &self.certificates
    }

    /// Certificates in the file that rustls does not use (unparsable DER or
    /// unsupported PEM labels). BoringSSL backends may still trust them.
    pub fn skipped_certificates(&self) -> usize {
        self.skipped
    }

    /// The accepted certificates re-encoded as one PEM bundle, for SDKs fed
    /// PEM input (tonic, redis-rs). Rejected entries are excluded because
    /// redis-rs fails the whole store on one rejected anchor.
    pub fn pem(&self) -> &[u8] {
        &self.pem
    }

    pub fn is_empty(&self) -> bool {
        self.certificates.is_empty()
    }

    /// The shared `System` root store. Clients take this `Arc` instead of
    /// copying the anchors into their own store.
    pub fn root_store(&self) -> Arc<RootCertStore> {
        Arc::clone(&self.roots)
    }
}

/// Load the selected OS bundle strictly and install it before any TLS client
/// is built. Fails when no bundle exists, when it is unusable, or when the
/// trust set was already initialized (a client was built first).
pub fn install_system_trust() -> Result<&'static SystemTrust> {
    let trust = SystemTrust::load(&select_bundle(&SYSTEM_CA_FILES)?)?;
    SYSTEM_TRUST.set(trust).map_err(|_| {
        anyhow::anyhow!(
            "system CA trust was initialized before startup installed it (a TLS client was built too early)"
        )
    })?;
    Ok(system_trust())
}

/// The process-wide trust set. Processes that never call
/// [`install_system_trust`] load the OS bundle on first use; a missing or
/// unusable bundle yields an empty set (see [`SystemTrust::source`]).
pub fn system_trust() -> &'static SystemTrust {
    SYSTEM_TRUST.get_or_init(|| match select_bundle(&SYSTEM_CA_FILES) {
        Ok(path) => SystemTrust::load_or_unavailable(&path),
        Err(error) => SystemTrust::new(
            SystemTrustSource::Unavailable(format!("{error:#}")),
            ParsedBundle::default(),
        ),
    })
}

/// The first candidate that exists. Only a missing path moves on: anything
/// else at a candidate path (a directory, a dangling symlink, an unreadable
/// file) is the bundle, and loading it reports the problem, so a broken
/// higher-priority bundle is never silently replaced by a lower-priority one.
fn select_bundle(candidates: &[&str]) -> Result<PathBuf> {
    for candidate in candidates {
        let path = PathBuf::from(candidate);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => return Ok(path),
            Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
            Err(error) => {
                return Err(error).with_context(|| format!("failed to inspect OS CA bundle {}", path.display()));
            }
        }
    }
    anyhow::bail!(
        "no OS CA bundle found at {}; install the OS CA bundle (ca-certificates)",
        candidates.join(", ")
    )
}

struct ParsedBundle {
    certificates: Vec<CertificateDer<'static>>,
    roots: RootCertStore,
    skipped: usize,
}

impl Default for ParsedBundle {
    fn default() -> Self {
        Self {
            certificates: Vec::new(),
            roots: RootCertStore::empty(),
            skipped: 0,
        }
    }
}

/// Read a PEM bundle and keep the certificates rustls can use as trust
/// anchors. Distribution bundles may carry entries webpki cannot parse; those
/// are skipped (and counted) so one odd root does not disable the whole store.
fn load_bundle(path: &Path) -> Result<ParsedBundle> {
    let bytes = read_bundle(path).with_context(|| format!("failed to read CA file {}", path.display()))?;
    parse_bundle(&bytes).with_context(|| format!("failed to parse CA file {}", path.display()))
}

/// Read at most [`MAX_CA_FILE_BYTES`] from a regular file. The type check runs
/// before `open` so a FIFO cannot block startup, and again on the handle.
fn read_bundle(path: &Path) -> Result<Vec<u8>> {
    anyhow::ensure!(std::fs::metadata(path)?.is_file(), "not a regular file");
    let file = std::fs::File::open(path)?;
    anyhow::ensure!(file.metadata()?.is_file(), "not a regular file");
    let mut bytes = Vec::new();
    file.take(MAX_CA_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_CA_FILE_BYTES,
        "file exceeds {MAX_CA_FILE_BYTES} bytes"
    );
    Ok(bytes)
}

fn parse_bundle(bytes: &[u8]) -> Result<ParsedBundle> {
    let mut bundle = ParsedBundle::default();
    for cert in CertificateDer::pem_slice_iter(bytes) {
        let cert = cert.context("invalid PEM section")?;
        if bundle.roots.add(cert.clone()).is_ok() {
            bundle.certificates.push(cert);
        } else {
            bundle.skipped += 1;
        }
    }
    bundle.skipped += UNSUPPORTED_CERTIFICATE_LABELS
        .iter()
        .map(|label| count_occurrences(bytes, label))
        .sum::<usize>();
    Ok(bundle)
}

/// Whether a line of `bytes` begins a certificate block that rustls cannot load
/// (`TRUSTED CERTIFICATE`, `X509 CERTIFICATE`). Like the PEM parser, only a label
/// at the start of a line counts, so a comment that mentions one does not.
pub(crate) fn has_unsupported_certificate_labels(bytes: &[u8]) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| {
        UNSUPPORTED_CERTIFICATE_LABELS
            .iter()
            .any(|label| line.starts_with(label))
    })
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// Re-encode certificates as plain PEM (64-character lines, no other text).
pub(crate) fn encode_pem(certificates: &[CertificateDer<'static>]) -> Vec<u8> {
    let engine = base64::engine::general_purpose::STANDARD;
    let mut pem = Vec::new();
    for cert in certificates {
        pem.extend_from_slice(b"-----BEGIN CERTIFICATE-----\n");
        let encoded = engine.encode(cert.as_ref());
        for line in encoded.as_bytes().chunks(64) {
            pem.extend_from_slice(line);
            pem.push(b'\n');
        }
        pem.extend_from_slice(b"-----END CERTIFICATE-----\n");
    }
    pem
}

#[cfg(test)]
#[path = "../tests/unit/system_trust_test.rs"]
mod tests;
