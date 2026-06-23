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

/// Build a client TLS config from in-memory PEM bytes.
pub fn load_client_tls_from_pem(cert_pem: &[u8], key_pem: &[u8], ca_pem: &[u8]) -> tonic::transport::ClientTlsConfig {
    tonic::transport::ClientTlsConfig::new()
        .ca_certificate(tonic::transport::Certificate::from_pem(ca_pem))
        .identity(tonic::transport::Identity::from_pem(cert_pem, key_pem))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal valid PEMs are not needed: these constructors do not parse/validate
    // eagerly (tonic defers to handshake). We assert they build without panicking
    // and return the expected types.
    #[test]
    fn from_pem_builders_construct() {
        let dummy = b"-----BEGIN CERTIFICATE-----\nMA==\n-----END CERTIFICATE-----\n";
        let _s: tonic::transport::ServerTlsConfig = load_server_tls_from_pem(dummy, dummy, dummy);
        let _c: tonic::transport::ClientTlsConfig = load_client_tls_from_pem(dummy, dummy, dummy);
    }
}
