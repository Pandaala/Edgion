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
//! that was live at accept time, so a root change tears those sessions down and a
//! rotation does not.

use arc_swap::ArcSwap;
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
        // design bypasses that layer; tonic 0.12 clients hard-fail (H2NotNegotiated)
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

pub struct ServerIdentityHandle {
    identity: ArcSwap<ServerIdentity>,
    /// Bumped on every trust-ROOT change (never on a leaf rotation). Exposed for
    /// tests/observability; the token below is what actually enforces anything.
    trust_generation: AtomicU64,
    /// Governs every connection accepted under the current trust root.
    trust_token: ArcSwap<CancellationToken>,
}

impl ServerIdentityHandle {
    pub fn new(id: ServerIdentity) -> Self {
        Self {
            identity: ArcSwap::from_pointee(id),
            trust_generation: AtomicU64::new(0),
            trust_token: ArcSwap::from_pointee(CancellationToken::new()),
        }
    }
    pub fn load(&self) -> Arc<ServerIdentity> {
        self.identity.load_full()
    }

    /// The token to hand a newly-accepted connection: it is cancelled when the trust
    /// root this connection was authenticated against is replaced.
    pub fn trust_token(&self) -> Arc<CancellationToken> {
        self.trust_token.load_full()
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
        let root_changed = self.identity.load().ca_cert_pem.trim() != id.ca_cert_pem.trim();
        // Install the identity FIRST: any handshake racing this store must land on the
        // new config, and any connection that races in on the old token is torn down by
        // the cancel below anyway.
        self.identity.store(Arc::new(id));
        if root_changed {
            let generation = self.trust_generation.fetch_add(1, Ordering::SeqCst) + 1;
            let old = self.trust_token.swap(Arc::new(CancellationToken::new()));
            old.cancel();
            tracing::warn!(
                component = "conf_sync_tls",
                event = "conf_sync_trust_root_changed",
                trust_generation = generation,
                "conf_sync trust root changed: cancelling every conf_sync session established under \
                 the previous root (this is the revocation an emergency CA re-key relies on)"
            );
        }
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
mod tests {
    use super::*;
    use crate::services::conf_sync_ca::Ca;

    // rustls 0.23 requires a process-level CryptoProvider before `ServerConfig::builder()`
    // can be called. Installing it here (ring) races harmlessly with other test modules
    // (e.g. edgion_common::grpc_tls) doing the same `install_default()` — the first writer
    // wins and the rest are no-ops, which is fine since either provider satisfies this code.
    fn install_test_crypto_provider() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    #[test]
    fn from_pems_builds_h2_config_with_metadata() {
        install_test_crypto_provider();
        let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
        let leaf = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let id = ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, ca.ca_cert_pem()).unwrap();
        assert_eq!(id.config.alpn_protocols, vec![b"h2".to_vec()]);
        assert!(id.not_after > id.not_before);
        assert!(!id.serial.is_empty());
        // The trust anchor the verifier was built from is retained verbatim — this is
        // what lets `identity_sync` detect a same-key root RENEWAL (new root cert, old
        // leaves still chain to it), which no chain check can see.
        assert_eq!(id.ca_cert_pem, ca.ca_cert_pem());
    }

    /// A same-key root renewal yields a DIFFERENT root certificate that the old leaf
    /// still chains to. The retained `ca_cert_pem` is the only thing that distinguishes
    /// the two — the property `identity_sync`'s root-drift arm relies on.
    #[test]
    fn same_key_renewal_changes_the_retained_root_bytes() {
        install_test_crypto_provider();
        let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
        let leaf = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let renewed = ca.renew_root_same_key(time::Duration::days(60)).unwrap();
        // Same key => the old leaf still chains to the renewed root...
        assert!(crate::services::conf_sync_ca::cert_util::chains_to(
            &leaf.cert_pem,
            renewed.ca_cert_pem()
        ));
        // ...but the root CERTIFICATE bytes differ, and that is what we store.
        let id = ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, ca.ca_cert_pem()).unwrap();
        assert_ne!(id.ca_cert_pem.trim(), renewed.ca_cert_pem().trim());
    }

    #[test]
    fn from_pems_rejects_mismatched_material() {
        install_test_crypto_provider();
        let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
        let other = Ca::generate_root(time::Duration::days(30)).unwrap();
        let leaf = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        assert!(ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, other.ca_cert_pem()).is_err());
    }

    // `global_handle()` is a process-wide `OnceLock` shared across every test
    // in this binary. No other test in the crate sets it (checked via grep),
    // so this test is the sole writer — but to stay race-safe against test
    // execution order and any future test that also sets it, we only ensure
    // it is set (tolerating an already-set handle from elsewhere) and then
    // assert on the post-condition, rather than asserting it starts unset.
    #[test]
    fn live_server_cert_pem_reads_through_the_handle() {
        install_test_crypto_provider();
        let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
        let leaf = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let id = ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, ca.ca_cert_pem()).unwrap();
        let handle = Arc::new(ServerIdentityHandle::new(id));
        let _ = global_handle().set(handle);

        // Whichever identity ended up installed first (this test's, or another
        // test's if one races ahead), the handle must be set and readable.
        assert!(live_server_cert_pem().is_some());
    }

    /// Routine leaf rotation (SAME root): the trust token must SURVIVE. Established
    /// gateway streams keep running — that is what makes rotation zero-disruption.
    #[test]
    fn same_root_store_keeps_established_sessions_alive() {
        install_test_crypto_provider();
        let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
        let a = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let b = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(14))
            .unwrap();
        let handle =
            ServerIdentityHandle::new(ServerIdentity::from_pems(&a.cert_pem, &a.key_pem, ca.ca_cert_pem()).unwrap());
        let token = handle.trust_token();
        let generation = handle.trust_generation();

        handle.store(ServerIdentity::from_pems(&b.cert_pem, &b.key_pem, ca.ca_cert_pem()).unwrap());

        assert!(
            !token.is_cancelled(),
            "a same-root leaf rotation must NOT kill sessions"
        );
        assert_eq!(handle.trust_generation(), generation);
        assert!(Arc::ptr_eq(&token, &handle.trust_token()), "token must be unchanged");
    }

    /// Trust-ROOT change (emergency re-key / BYO-root swap): the old token must be
    /// cancelled, the generation bumped, and a fresh uncancelled token installed.
    /// This is the revocation.
    #[test]
    fn root_change_store_cancels_established_sessions() {
        install_test_crypto_provider();
        let ca_a = Ca::generate_root(time::Duration::days(30)).unwrap();
        let ca_b = Ca::generate_root(time::Duration::days(30)).unwrap();
        let a = ca_a
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let b = ca_b
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let handle =
            ServerIdentityHandle::new(ServerIdentity::from_pems(&a.cert_pem, &a.key_pem, ca_a.ca_cert_pem()).unwrap());
        let old = handle.trust_token();
        assert_eq!(handle.trust_generation(), 0);

        handle.store(ServerIdentity::from_pems(&b.cert_pem, &b.key_pem, ca_b.ca_cert_pem()).unwrap());

        assert!(old.is_cancelled(), "a root change MUST tear down old sessions");
        assert_eq!(handle.trust_generation(), 1);
        let fresh = handle.trust_token();
        assert!(!fresh.is_cancelled(), "connections accepted after the re-key must live");
        assert!(!Arc::ptr_eq(&old, &fresh));
    }

    /// A same-key root RENEWAL changes the root CERTIFICATE bytes, so it counts as a
    /// root change here (conservative: the anchor is a different certificate).
    #[test]
    fn same_key_root_renewal_counts_as_a_root_change() {
        install_test_crypto_provider();
        let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
        let leaf = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let renewed = ca.renew_root_same_key(time::Duration::days(60)).unwrap();
        let handle = ServerIdentityHandle::new(
            ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, ca.ca_cert_pem()).unwrap(),
        );
        let old = handle.trust_token();
        let leaf2 = renewed
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        handle.store(ServerIdentity::from_pems(&leaf2.cert_pem, &leaf2.key_pem, renewed.ca_cert_pem()).unwrap());
        assert!(old.is_cancelled());
        assert_eq!(handle.trust_generation(), 1);
    }

    #[test]
    fn handle_swaps_atomically() {
        install_test_crypto_provider();
        let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
        let a = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let b = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(14))
            .unwrap();
        let handle =
            ServerIdentityHandle::new(ServerIdentity::from_pems(&a.cert_pem, &a.key_pem, ca.ca_cert_pem()).unwrap());
        let na_a = handle.load().not_after;
        handle.store(ServerIdentity::from_pems(&b.cert_pem, &b.key_pem, ca.ca_cert_pem()).unwrap());
        assert!(handle.load().not_after > na_a);
    }
}
