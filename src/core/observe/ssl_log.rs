//! SSL/TLS callback logging
//!
//! Records every TLS certificate callback with enhanced features:
//! - Batch processing for better I/O performance
//! - Log rotation (by time or size)
//! - Metrics integration for dropped logs
//! - Non-blocking guarantee for TLS callbacks

use serde::Serialize;
use std::sync::OnceLock;
use tokio::sync::mpsc;

use crate::core::link_sys::{DataSender, LocalFileWriter};

/// Global SSL logger instance
static SSL_LOGGER: OnceLock<SslLogger> = OnceLock::new();

/// SSL callback log entry
#[derive(Serialize, Default)]
pub struct SslLogEntry {
    /// Timestamp in milliseconds
    pub ts: i64,
    /// Server Name Indication from client
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    /// Matched certificate name (namespace/name)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert: Option<String>,
    /// Whether mTLS is enabled
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtls: Option<bool>,
    /// Error message if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SslLogEntry {
    /// Create a new entry with current timestamp
    pub fn new() -> Self {
        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            ..Default::default()
        }
    }

    /// Set SNI
    pub fn sni(&mut self, sni: impl Into<String>) -> &mut Self {
        self.sni = Some(sni.into());
        self
    }

    /// Set matched certificate
    pub fn cert(&mut self, cert: impl Into<String>) -> &mut Self {
        self.cert = Some(cert.into());
        self
    }

    /// Set mTLS flag
    pub fn mtls(&mut self, enabled: bool) -> &mut Self {
        self.mtls = Some(enabled);
        self
    }

    /// Set error message
    pub fn error(&mut self, msg: impl Into<String>) -> &mut Self {
        self.error = Some(msg.into());
        self
    }

    /// Serialize to JSON
    #[inline]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// SSL logger with enhanced features via LocalFileWriter
pub struct SslLogger {
    /// Unbounded channel to bridge sync API to async LocalFileWriter
    /// Using unbounded channel ensures log_ssl() is truly non-blocking
    tx: mpsc::UnboundedSender<String>,
}

impl SslLogger {
    /// Create a new SSL logger with LocalFileWriter backend
    pub fn new(writer: LocalFileWriter) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        // Spawn async task to forward logs to LocalFileWriter
        tokio::spawn(async move {
            let mut writer = writer;

            // Initialize the writer
            if let Err(e) = writer.init().await {
                tracing::error!("Failed to initialize SSL log writer: {}", e);
                return;
            }

            // Forward logs from channel to writer
            while let Some(log) = rx.recv().await {
                // LocalFileWriter handles:
                // - Batch writing (up to 1000 logs per flush)
                // - Log rotation (time or size based)
                // - Metrics for dropped logs
                let _ = writer.send(log).await;
            }
        });

        Self { tx }
    }

    /// Log an entry (guaranteed non-blocking, always safe to call from TLS callback)
    #[inline]
    pub fn log(&self, entry: &SslLogEntry) {
        // UnboundedSender::send() never blocks
        // If the receiver is dropped, this will silently fail (acceptable for logs)
        let _ = self.tx.send(entry.to_json());
    }
}

/// Initialize the global SSL logger from configuration
pub async fn init_ssl_logger(config: &crate::types::LogConfig) -> anyhow::Result<()> {
    use crate::core::observe::create_sync_logger;
    
    if let Some(logger) = create_sync_logger(config).await? {
        SSL_LOGGER.set(logger)
            .map_err(|_| anyhow::anyhow!("SSL logger already initialized"))?;
        tracing::info!("SSL callback logger initialized with enhanced features");
    } else {
        tracing::info!("SSL logger disabled");
    }
    Ok(())
}

/// Log an SSL callback entry (guaranteed non-blocking)
///
/// This function is safe to call from any context, including TLS callbacks.
/// It uses an unbounded channel internally to ensure no blocking occurs.
#[inline]
pub fn log_ssl(entry: &SslLogEntry) {
    if let Some(logger) = SSL_LOGGER.get() {
        logger.log(entry);
    }
}
