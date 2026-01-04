//! SSL/TLS callback logging
//!
//! Records every TLS certificate callback to ssl_callback.log
//! Uses bounded channel with try_send to guarantee non-blocking behavior.

use serde::Serialize;
use std::sync::mpsc::{self, TrySendError};
use std::sync::OnceLock;
use std::io::Write;

use crate::core::utils::available_cpu_cores;

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

/// SSL logger with bounded channel (guaranteed non-blocking)
pub struct SslLogger {
    tx: mpsc::SyncSender<String>,
}

impl SslLogger {
    /// Create and start a new SSL logger
    pub fn new(log_path: &str) -> std::io::Result<Self> {
        // Bounded channel: try_send never blocks, drops if full
        // Capacity = cores * 10000, same as access log
        let (tx, rx) = mpsc::sync_channel::<String>(available_cpu_cores() * 10_000);
        let path = log_path.to_string();

        std::thread::spawn(move || {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path);

            let mut file = match file {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Failed to open ssl_callback.log: {}", e);
                    return;
                }
            };

            while let Ok(line) = rx.recv() {
                let _ = writeln!(file, "{}", line);
            }
        });

        Ok(Self { tx })
    }

    /// Log an entry (guaranteed non-blocking, drops if channel full)
    #[inline]
    pub fn log(&self, entry: &SslLogEntry) {
        match self.tx.try_send(entry.to_json()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                // Channel full, drop this log entry (non-blocking guarantee)
            }
            Err(TrySendError::Disconnected(_)) => {
                // Receiver dead, nothing we can do
            }
        }
    }
}

/// Initialize the global SSL logger
pub fn init_ssl_logger(log_path: &str) -> std::io::Result<()> {
    let logger = SslLogger::new(log_path)?;
    SSL_LOGGER.set(logger)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::AlreadyExists, "SSL logger already initialized"))?;
    tracing::info!(path = %log_path, "SSL callback logger initialized");
    Ok(())
}

/// Log an SSL callback entry (guaranteed non-blocking)
#[inline]
pub fn log_ssl(entry: &SslLogEntry) {
    if let Some(logger) = SSL_LOGGER.get() {
        logger.log(entry);
    }
}

