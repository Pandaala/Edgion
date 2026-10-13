//! Shared TLS loading utilities for gRPC channels.

use crate::cert_util::{parse_leaf_validity, LeafValidity};
use crate::conf_sync::config::ConfSyncTlsConfig;
use tonic::transport::{Certificate, ClientTlsConfig, Identity};

/// Load client TLS config from certificate files.
///
/// Returns error if any file is missing or invalid — caller must not connect.
pub async fn load_client_tls(cfg: &ConfSyncTlsConfig) -> anyhow::Result<(ClientTlsConfig, LeafValidity)> {
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

    client_tls_from_pems(cert, key, ca)
}

/// Build TLS configuration and metadata from exactly the same certificate bytes.
/// The caller owns identity validation policy; no second filesystem read occurs.
pub fn client_tls_from_pems(
    cert: Vec<u8>,
    key: Vec<u8>,
    ca: Vec<u8>,
) -> anyhow::Result<(ClientTlsConfig, LeafValidity)> {
    let validity =
        parse_leaf_validity(&cert).map_err(|error| anyhow::anyhow!("Client certificate validity: {error}"))?;
    Ok((
        ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(ca))
            .identity(Identity::from_pem(cert, key)),
        validity,
    ))
}

// NOTE: there is deliberately NO server-side TLS builder here. The controller's
// conf_sync server owns its own rustls accept loop and builds every serving
// config through `ServerIdentity::from_pems`, which runs `validate_identity`
// (below) first. The former `load_server_tls` / `load_server_tls_from_pem`
// helpers built a SERVING config with no validation at all — exactly the
// unvalidated-swap path the design forbids — and had no callers left; they were
// removed rather than left as a trap. `load_client_tls` above is live.

/// Validate a leaf identity before hot-swapping it into a live TLS config
/// (spec §6 Task 1 item 3). Checks: PEMs parse; leaf within validity NOW;
/// leaf chains to `ca_pem` (issuer + signature); private key matches the leaf.
/// The first decoded leaf and CA PEM blocks must be labeled CERTIFICATE so
/// validation cannot select a block that the transport's certificate loader skips.
/// Every consumer (controller server swap, gateway client swap, provision adopt)
/// MUST call this and keep the previous config on error.
pub fn validate_identity(cert_pem: &[u8], key_pem: &[u8], ca_pem: &[u8]) -> anyhow::Result<()> {
    let (_, leaf_doc) =
        x509_parser::pem::parse_x509_pem(cert_pem).map_err(|e| anyhow::anyhow!("leaf cert PEM parse: {e}"))?;
    anyhow::ensure!(
        leaf_doc.label == "CERTIFICATE",
        "leaf cert PEM must begin with a CERTIFICATE block"
    );
    let leaf = leaf_doc
        .parse_x509()
        .map_err(|e| anyhow::anyhow!("leaf cert DER parse: {e}"))?;
    let (_, ca_doc) =
        x509_parser::pem::parse_x509_pem(ca_pem).map_err(|e| anyhow::anyhow!("CA cert PEM parse: {e}"))?;
    anyhow::ensure!(
        ca_doc.label == "CERTIFICATE",
        "CA cert PEM must begin with a CERTIFICATE block"
    );
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
#[path = "../tests/unit/grpc_tls_test/validate_identity_tests.rs"]
mod validate_identity_tests;
