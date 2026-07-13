//! Hot-swappable conf_sync server TLS identity (spec §6 Task 3).
//! Swapping the handle atomically swaps BOTH the served leaf and the client-CA
//! trust anchor. Established connections are never affected (certs are only
//! used at handshake); new handshakes pick up the stored config.

use arc_swap::ArcSwap;
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use std::sync::{Arc, OnceLock};
use time::OffsetDateTime;

pub struct ServerIdentity {
    pub config: Arc<ServerConfig>,
    pub cert_pem: String,
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
            not_before,
            not_after,
            serial,
        })
    }
}

pub struct ServerIdentityHandle(ArcSwap<ServerIdentity>);

impl ServerIdentityHandle {
    pub fn new(id: ServerIdentity) -> Self {
        Self(ArcSwap::from_pointee(id))
    }
    pub fn load(&self) -> Arc<ServerIdentity> {
        self.0.load_full()
    }
    pub fn store(&self, id: ServerIdentity) {
        self.0.store(Arc::new(id));
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
