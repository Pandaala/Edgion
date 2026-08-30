//! SSL/TLS callback logging
//!
//! Non-blocking logger for TLS certificate callbacks. Uses a bounded
//! mpsc channel so callers never block; entries that exceed the channel
//! capacity are dropped and counted via the metrics registry.

use std::sync::OnceLock;
use tokio::sync::mpsc;

use crate::link_sys::{DataSender, LocalFileWriter};
use crate::observe::logs::create_sync_logger;
use crate::observe::metrics::global_metrics;
use crate::TlsConnMeta;

/// Global SSL logger instance
static SSL_LOGGER: OnceLock<SslLogger> = OnceLock::new();

const DEFAULT_CHANNEL_CAPACITY: usize = 65_536;

/// Non-blocking SSL logger backed by a bounded mpsc channel and [`LocalFileWriter`].
pub struct SslLogger {
    tx: mpsc::Sender<String>,
}

impl SslLogger {
    /// Create a new SSL logger with LocalFileWriter backend
    pub fn new(writer: LocalFileWriter) -> Self {
        let (tx, mut rx) = mpsc::channel::<String>(DEFAULT_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            let mut writer = writer;
            if let Err(e) = writer.init().await {
                tracing::error!("Failed to initialize SSL log writer: {}", e);
                return;
            }
            while let Some(log) = rx.recv().await {
                let _ = writer.send(log).await;
            }
        });

        Self { tx }
    }

    /// Log an entry — non-blocking, safe to call from TLS callbacks
    #[inline]
    pub fn log(&self, entry: &TlsConnMeta) {
        // Reserve first so a saturated/closed channel does not pay the JSON
        // serialization cost on the synchronous TLS callback path.
        let Ok(permit) = self.tx.try_reserve() else {
            global_metrics().ssl_log_dropped();
            return;
        };
        let payload = serde_json::to_string(entry).unwrap_or_else(|_| "{}".to_string());
        permit.send(payload);
    }
}

/// Initialize the global SSL logger from configuration
pub async fn init_ssl_logger(config: &edgion_resources::LogConfig) -> anyhow::Result<()> {
    if let Some(logger) = create_sync_logger(config).await? {
        SSL_LOGGER
            .set(logger)
            .map_err(|_| anyhow::anyhow!("SSL logger already initialized"))?;
        tracing::info!("SSL callback logger initialized");
    } else {
        tracing::info!("SSL logger disabled");
    }
    Ok(())
}

/// Log an SSL callback entry — non-blocking, safe to call from TLS callbacks
#[inline]
pub fn log_ssl(entry: &TlsConnMeta) {
    if let Some(logger) = SSL_LOGGER.get() {
        logger.log(entry);
    }
}
