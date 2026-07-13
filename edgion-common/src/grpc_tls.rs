//! Shared TLS loading utilities for gRPC channels.

use crate::conf_sync::config::ConfSyncTlsConfig;

/// Load server TLS config from certificate files.
///
/// Returns error if any file is missing or invalid — caller must not start the listener.
pub async fn load_server_tls(cfg: &ConfSyncTlsConfig) -> anyhow::Result<tonic::transport::ServerTlsConfig> {
    cfg.validate()?;
    let cert_path = cfg.cert_path();
    let key_path = cfg.key_path();
    let ca_path = cfg.ca_path();

    let cert = tokio::fs::read(&cert_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read server cert {:?}: {}", cert_path, e))?;
    let key = tokio::fs::read(&key_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read server key {:?}: {}", key_path, e))?;
    let ca = tokio::fs::read(&ca_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read CA cert {:?}: {}", ca_path, e))?;

    Ok(tonic::transport::ServerTlsConfig::new()
        .identity(tonic::transport::Identity::from_pem(cert, key))
        .client_ca_root(tonic::transport::Certificate::from_pem(ca)))
}

/// Load client TLS config from certificate files.
///
/// Returns error if any file is missing or invalid — caller must not connect.
pub async fn load_client_tls(cfg: &ConfSyncTlsConfig) -> anyhow::Result<tonic::transport::ClientTlsConfig> {
    cfg.validate()?;
    let ca_path = cfg.ca_path();
    let cert_path = cfg.cert_path();
    let key_path = cfg.key_path();

    let ca = tokio::fs::read(&ca_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read CA cert {:?}: {}", ca_path, e))?;
    let cert = tokio::fs::read(&cert_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read client cert {:?}: {}", cert_path, e))?;
    let key = tokio::fs::read(&key_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read client key {:?}: {}", key_path, e))?;

    Ok(tonic::transport::ClientTlsConfig::new()
        .ca_certificate(tonic::transport::Certificate::from_pem(ca))
        .identity(tonic::transport::Identity::from_pem(cert, key)))
}

/// Build a server TLS config from in-memory PEM bytes (used by the auto-CA path,
/// which mints the controller-server cert in memory rather than mounting files).
pub fn load_server_tls_from_pem(cert_pem: &[u8], key_pem: &[u8], ca_pem: &[u8]) -> tonic::transport::ServerTlsConfig {
    tonic::transport::ServerTlsConfig::new()
        .identity(tonic::transport::Identity::from_pem(cert_pem, key_pem))
        .client_ca_root(tonic::transport::Certificate::from_pem(ca_pem))
}

/// Validate a leaf identity before hot-swapping it into a live TLS config
/// (spec §6 Task 1 item 3). Checks: PEMs parse; leaf within validity NOW;
/// leaf chains to `ca_pem` (issuer + signature); private key matches the leaf.
/// Every consumer (controller server swap, gateway client swap, provision adopt)
/// MUST call this and keep the previous config on error.
pub fn validate_identity(cert_pem: &[u8], key_pem: &[u8], ca_pem: &[u8]) -> anyhow::Result<()> {
    let (_, leaf_doc) =
        x509_parser::pem::parse_x509_pem(cert_pem).map_err(|e| anyhow::anyhow!("leaf cert PEM parse: {e}"))?;
    let leaf = leaf_doc
        .parse_x509()
        .map_err(|e| anyhow::anyhow!("leaf cert DER parse: {e}"))?;
    let (_, ca_doc) =
        x509_parser::pem::parse_x509_pem(ca_pem).map_err(|e| anyhow::anyhow!("CA cert PEM parse: {e}"))?;
    let ca = ca_doc
        .parse_x509()
        .map_err(|e| anyhow::anyhow!("CA cert DER parse: {e}"))?;

    // (b) validity window contains now
    anyhow::ensure!(
        leaf.validity().is_valid(),
        "leaf certificate is not currently valid (expired or not yet valid)"
    );

    // (c) chain: issuer DN matches and the CA key verifies the leaf signature
    anyhow::ensure!(leaf.issuer() == ca.subject(), "leaf issuer does not match CA subject");
    leaf.verify_signature(Some(ca.public_key()))
        .map_err(|e| anyhow::anyhow!("leaf signature does not verify against CA: {e}"))?;

    // (d) private key parses and matches the leaf public key.
    let key_der = rustls_pemfile::private_key(&mut std::io::Cursor::new(key_pem))
        .map_err(|e| anyhow::anyhow!("private key PEM parse: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("no private key found in PEM"))?;
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
        .map_err(|e| anyhow::anyhow!("unsupported private key type: {e}"))?;
    if let Some(spki) = signing_key.public_key() {
        anyhow::ensure!(
            spki.as_ref() == leaf.public_key().raw,
            "private key does not match the leaf certificate public key"
        );
    } else {
        anyhow::bail!("cannot verify private key matches certificate: signing key does not expose its public key");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal valid PEMs are not needed: this constructor does not parse/validate
    // eagerly (tonic defers to handshake). We assert it builds without panicking
    // and returns the expected type.
    #[test]
    fn from_pem_builders_construct() {
        let dummy = b"-----BEGIN CERTIFICATE-----\nMA==\n-----END CERTIFICATE-----\n";
        let _s: tonic::transport::ServerTlsConfig = load_server_tls_from_pem(dummy, dummy, dummy);
    }
}

#[cfg(test)]
mod validate_identity_tests {
    use super::*;

    fn install_test_crypto_provider() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    // Minimal in-test CA/leaf generator (mirrors edgion-controller's conf_sync_ca::Ca,
    // duplicated here because edgion-common cannot depend on edgion-controller).
    fn gen_ca_and_leaf(leaf_days: i64) -> (String, String, String) {
        use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair};
        let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut cp = CertificateParams::default();
        cp.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        cp.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "test-ca");
            dn
        };
        let ca_cert = cp.self_signed(&ca_key).unwrap();
        let issuer = Issuer::from_ca_cert_pem(&ca_cert.pem(), ca_key).unwrap();
        let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut lp = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        lp.not_before = time::OffsetDateTime::now_utc() - time::Duration::minutes(5);
        lp.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(leaf_days);
        let leaf = lp.signed_by(&leaf_key, &issuer).unwrap();
        (leaf.pem(), leaf_key.serialize_pem(), ca_cert.pem())
    }

    #[test]
    fn accepts_valid_identity() {
        install_test_crypto_provider();
        let (cert, key, ca) = gen_ca_and_leaf(30);
        validate_identity(cert.as_bytes(), key.as_bytes(), ca.as_bytes()).unwrap();
    }

    #[test]
    fn rejects_expired_leaf() {
        install_test_crypto_provider();
        let (cert, key, ca) = gen_ca_and_leaf(-1);
        assert!(validate_identity(cert.as_bytes(), key.as_bytes(), ca.as_bytes()).is_err());
    }

    #[test]
    fn rejects_wrong_ca() {
        install_test_crypto_provider();
        let (cert, key, _) = gen_ca_and_leaf(30);
        let (_, _, other_ca) = gen_ca_and_leaf(30);
        assert!(validate_identity(cert.as_bytes(), key.as_bytes(), other_ca.as_bytes()).is_err());
    }

    #[test]
    fn rejects_mismatched_key() {
        install_test_crypto_provider();
        let (cert, _, ca) = gen_ca_and_leaf(30);
        let (_, other_key, _) = gen_ca_and_leaf(30);
        assert!(validate_identity(cert.as_bytes(), other_key.as_bytes(), ca.as_bytes()).is_err());
    }

    #[test]
    fn rejects_garbage() {
        install_test_crypto_provider();
        assert!(validate_identity(b"nope", b"nope", b"nope").is_err());
    }
}
