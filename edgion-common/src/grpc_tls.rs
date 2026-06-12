//! Shared TLS loading utilities for gRPC channels.

use crate::conf_sync_config::ConfSyncTlsConfig;

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
