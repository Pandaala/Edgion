//! Hot-swappable conf_sync server TLS identity (spec §6 Task 3).
//! Swapping the handle atomically swaps BOTH the served leaf and the client-CA
//! trust anchor. New handshakes pick up the stored config.
//!
//! # Established connections: rotation vs. trust-root change
//!
//! A TLS certificate is consumed exactly once, at handshake. So a swap on its own
//! can never touch an ESTABLISHED session — which is a feature for a routine leaf
//! rotation (the peer is still authorized; zero disruption is the whole point) and a
//! **security hole** for an emergency CA re-key (the peer was authenticated by a root
//! that is now revoked, yet it keeps streaming resolved Secrets on its open HTTP/2
//! connection, without ever re-handshaking).
//!
//! The handle therefore carries a **trust generation**: a [`CancellationToken`] that
//! governs every connection accepted under the CURRENT trust root. [`ServerIdentityHandle::store`]
//! cancels it **if and only if the root certificate changed**; a same-root leaf rotation
//! leaves it untouched. `grpc_server`'s accept loop hands each accepted stream the token
//! paired with its verifier in one snapshot, so a root change tears those sessions
//! down even when it overlaps a handshake; a leaf rotation does not.

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

pub struct ServerIdentity {
    pub config: Arc<ServerConfig>,
    pub cert_pem: String,
    /// The trust anchor (`ca.crt`) this identity's client verifier was built from.
    ///
    /// Kept as bytes, not just as "something the leaf chains to": a SAME-KEY root
    /// renewal mints a DIFFERENT root certificate that every old leaf still chains to,
    /// so a chain check alone cannot see it. Without this field, a replica would keep
    /// serving a verifier holding the OLD root cert until its leaf aged out — benign
    /// today only because webpki keys trust anchors on subject+SPKI (both unchanged by
    /// a same-key renewal), which is an implementation detail we must not depend on.
    /// `identity_sync` compares these bytes against the live root and re-signs on drift
    /// (mirrors `rotation.rs::out_of_sync_reason`'s "leaf secret ca.crt is not the
    /// current root cert" arm).
    pub ca_cert_pem: String,
    pub not_before: OffsetDateTime,
    pub not_after: OffsetDateTime,
    pub serial: String,
}

impl ServerIdentity {
    pub fn from_pems(cert_pem: &str, key_pem: &str, ca_pem: &str) -> anyhow::Result<Self> {
        edgion_common::grpc_tls::validate_identity(cert_pem.as_bytes(), key_pem.as_bytes(), ca_pem.as_bytes())?;
        let (not_before, not_after) = crate::services::conf_sync_ca::cert_util::parse_cert_window(cert_pem)?;
        let serial = {
            let (_, doc) = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes())
                .map_err(|e| anyhow::anyhow!("leaf PEM parse: {e}"))?;
            doc.parse_x509()
                .map_err(|e| anyhow::anyhow!("leaf DER parse: {e}"))?
                .raw_serial_as_string()
        };

        let certs: Vec<_> = rustls_pemfile::certs(&mut std::io::Cursor::new(cert_pem.as_bytes()))
            .collect::<Result<_, _>>()
            .map_err(|e| anyhow::anyhow!("leaf cert chain parse: {e}"))?;
        let key = rustls_pemfile::private_key(&mut std::io::Cursor::new(key_pem.as_bytes()))
            .map_err(|e| anyhow::anyhow!("key parse: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("no private key in PEM"))?;
        let mut roots = RootCertStore::empty();
        for ca in rustls_pemfile::certs(&mut std::io::Cursor::new(ca_pem.as_bytes())) {
            roots
                .add(ca.map_err(|e| anyhow::anyhow!("CA cert parse: {e}"))?)
                .map_err(|e| anyhow::anyhow!("CA cert add: {e}"))?;
        }
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|e| anyhow::anyhow!("client verifier build: {e}"))?;
        let mut config = ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)
            .map_err(|e| anyhow::anyhow!("server config build: {e}"))?;
        // CRITICAL (spec §6 T3): tonic's own server-TLS layer injects ALPN h2 and this
        // design bypasses that layer; tonic clients hard-fail (H2NotNegotiated)
        // when the server does not negotiate h2.
        config.alpn_protocols = vec![b"h2".to_vec()];

        Ok(Self {
            config: Arc::new(config),
            cert_pem: cert_pem.to_string(),
            ca_cert_pem: ca_pem.to_string(),
            not_before,
            not_after,
            serial,
        })
    }
}

struct InstalledIdentity {
    identity: Arc<ServerIdentity>,
    trust_token: Arc<CancellationToken>,
}

pub struct ServerIdentityHandle {
    // Retain lock-free reads on the existing TLS accept path, publishing the verifier
    // and its revocation token together even though cold Controller stores use locks.
    installed: ArcSwap<InstalledIdentity>,
    /// Serializes cold compare/publish/cancel, never handshake reads.
    store_lock: Mutex<()>,
    /// Diagnostic only; the installed token enforces revocation.
    trust_generation: AtomicU64,
}

impl ServerIdentityHandle {
    pub fn new(id: ServerIdentity) -> Self {
        Self {
            installed: ArcSwap::from_pointee(InstalledIdentity {
                identity: Arc::new(id),
                trust_token: Arc::new(CancellationToken::new()),
            }),
            store_lock: Mutex::new(()),
            trust_generation: AtomicU64::new(0),
        }
    }
    pub fn load(&self) -> Arc<ServerIdentity> {
        self.installed.load().identity.clone()
    }

    /// Load the verifier and its governing revocation token from one snapshot.
    pub(super) fn handshake_material(&self) -> (Arc<ServerConfig>, Arc<CancellationToken>) {
        let installed = self.installed.load();
        (installed.identity.config.clone(), installed.trust_token.clone())
    }

    /// How many times the trust ROOT has changed since process start.
    pub fn trust_generation(&self) -> u64 {
        self.trust_generation.load(Ordering::SeqCst)
    }

    /// Install a new identity.
    ///
    /// **This is the line that makes routine rotation zero-disruption AND an emergency
    /// re-key an actual revocation.** Two branches, and the difference is the entire
    /// security story of conf_sync cert rotation:
    ///
    /// * **Same root** (an ordinary leaf rotation — the reconciler re-signs this
    ///   replica's serving leaf every half-life): the trust token is left ALONE.
    ///   Established gateway streams survive; no relist storm, no data-plane blip. The
    ///   peers on those sessions were authenticated by a root that is still trusted, so
    ///   there is nothing to revoke.
    /// * **Different root** (an emergency CA re-key, or an out-of-band BYO-root swap):
    ///   bump the generation, install a FRESH token, and cancel the old one. Every
    ///   connection accepted under the old root dies. Without this, an attacker holding
    ///   a leaked leaf and an ALREADY-OPEN connection keeps receiving the full
    ///   resolved-Secret stream forever after the re-key — no new handshake is ever
    ///   performed on an established HTTP/2 connection, so the new client-CA verifier
    ///   is never consulted again.
    ///
    /// The comparison is a whitespace-trimmed byte compare of `ca_cert_pem`, exactly as
    /// `identity_sync::identity_resign_reason` and `rotation::out_of_sync_reason` do — a
    /// same-key root RENEWAL mints different root bytes and IS a root change here (its
    /// old leaves still chain, but the anchor is a different certificate; tearing the
    /// sessions down is the conservative, correct call, and a renewal is rare).
    pub fn store(&self, id: ServerIdentity) {
        self.store_inner(None, id);
    }

    /// Publish only if the retained identity still owns the installed slot.
    /// Rejection leaves the identity, trust token and generation unchanged.
    pub(crate) fn store_if_current(&self, expected: &Arc<ServerIdentity>, id: ServerIdentity) -> bool {
        self.store_inner(Some(expected), id)
    }

    fn store_inner(&self, expected: Option<&Arc<ServerIdentity>>, id: ServerIdentity) -> bool {
        let generation = {
            let _guard = self.store_lock.lock();
            let old = self.installed.load_full();
            if expected.is_some_and(|expected| !Arc::ptr_eq(expected, &old.identity)) {
                return false;
            }
            let root_changed = old.identity.ca_cert_pem.trim() != id.ca_cert_pem.trim();
            let trust_token = if root_changed {
                Arc::new(CancellationToken::new())
            } else {
                old.trust_token.clone()
            };
            self.installed.store(Arc::new(InstalledIdentity {
                identity: Arc::new(id),
                trust_token,
            }));
            if root_changed {
                // A retained old snapshot always carries this old token, never the
                // new root's live token, even if its handshake has not started yet.
                old.trust_token.cancel();
                Some(self.trust_generation.fetch_add(1, Ordering::SeqCst) + 1)
            } else {
                None
            }
        };
        if let Some(generation) = generation {
            tracing::warn!(
                component = "conf_sync_tls",
                event = "conf_sync_trust_root_changed",
                trust_generation = generation,
                "conf_sync trust root changed: cancelling every conf_sync session established under \
                 the previous root (this is the revocation an emergency CA re-key relies on)"
            );
        }
        true
    }
}

/// Process-wide identity slot: set once at startup when conf_sync TLS is active
/// (cli/mod.rs), read by the rotation reconciler and the file watcher.
static GLOBAL_HANDLE: OnceLock<Arc<ServerIdentityHandle>> = OnceLock::new();

pub fn global_handle() -> &'static OnceLock<Arc<ServerIdentityHandle>> {
    &GLOBAL_HANDLE
}

/// The PEM of the server certificate currently in service, if TLS is active.
/// Reads through the hot-swap handle so callers (e.g. the expiry watch)
/// always observe the post-rotation identity. `None` = plaintext/skip_tls.
pub fn live_server_cert_pem() -> Option<String> {
    global_handle().get().map(|h| h.load().cert_pem.clone())
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_sync/conf_server/tls_identity_test.rs"]
mod tests;
