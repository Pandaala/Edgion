//! SSL/TLS callback logging
//!
//! Records every TLS certificate callback with enhanced features:
//! - Batch processing for better I/O performance
//! - Log rotation (by time or size)
//! - Metrics integration for dropped logs
//! - Non-blocking guarantee for TLS callbacks

use std::sync::OnceLock;
use tokio::sync::mpsc;

use crate::core::gateway::link_sys::{DataSender, LocalFileWriter};
use crate::core::gateway::observe::logs::create_sync_logger;
use crate::types::TlsConnMeta;

/// Global SSL logger instance
static SSL_LOGGER: OnceLock<SslLogger> = OnceLock::new();

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
    pub fn log(&self, entry: &TlsConnMeta) {
        // UnboundedSender::send() never blocks
        // If the receiver is dropped, this will silently fail (acceptable for logs)
        let _ = self
            .tx
            .send(serde_json::to_string(entry).unwrap_or_else(|_| "{}".to_string()));
    }
}

/// Initialize the global SSL logger from configuration
pub async fn init_ssl_logger(config: &crate::types::LogConfig) -> anyhow::Result<()> {
    if let Some(logger) = create_sync_logger(config).await? {
        SSL_LOGGER
            .set(logger)
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
pub fn log_ssl(entry: &TlsConnMeta) {
    if let Some(logger) = SSL_LOGGER.get() {
        logger.log(entry);
    }
}
