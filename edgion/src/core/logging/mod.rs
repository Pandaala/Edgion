use anyhow::Result;
use std::path::PathBuf;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

mod layer;
mod writer;

pub use layer::AsyncLogLayer;
pub use writer::{log_worker, AsyncLogWriter};

/// Configuration for the logging system
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Log directory path
    pub log_dir: PathBuf,

    /// Log file name prefix
    pub file_prefix: String,

    /// Whether to use JSON format
    pub json_format: bool,

    /// Whether to log to console
    pub console: bool,

    /// Log level filter (e.g., "info", "debug", "warn")
    pub level: String,

    /// Channel buffer size for async logging
    pub buffer_size: usize,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_dir: PathBuf::from("logs"),
            file_prefix: "edgion".to_string(),
            json_format: false,
            console: true,
            level: "info".to_string(),
            buffer_size: 10_000,
        }
    }
}

/// Initialize the logging system
///
/// This sets up a multi-layered logging system with:
/// - File rotation (daily)
/// - Optional JSON formatting
/// - Optional console output
/// - Async non-blocking writes
pub async fn init_logging(config: LogConfig) -> Result<()> {
    // Create log directory if it doesn't exist
    tokio::fs::create_dir_all(&config.log_dir).await?;

    // Create async channel for log messages
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(config.buffer_size);

    let writer = AsyncLogWriter::new(tx);

    // Spawn background worker for file writing with rotation
    let log_dir = config.log_dir.clone();
    let file_prefix = config.file_prefix.clone();
    tokio::spawn(async move {
        if let Err(e) = log_worker(rx, log_dir, file_prefix).await {
            eprintln!("Log worker error: {}", e);
        }
    });

    // Build env filter
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.level));

    // Create async log layer
    let async_layer = AsyncLogLayer {
        json_fmt: config.json_format,
        writer,
    };

    // Build subscriber with layers
    let subscriber = tracing_subscriber::registry().with(env_filter).with(async_layer);

    // Add console layer if enabled
    if config.console {
        let console_layer = tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_line_number(true);

        subscriber.with(console_layer).try_init()?;
    } else {
        subscriber.try_init()?;
    }

    Ok(())
}

/// Initialize logging with default configuration
pub async fn init_default() -> Result<()> {
    init_logging(LogConfig::default()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_logging() {
        // Use temp directory to avoid creating test_logs in the project
        let temp_dir = std::env::temp_dir().join("edgion_test_logs");
        let config = LogConfig {
            log_dir: temp_dir,
            file_prefix: "test".to_string(),
            json_format: true,
            console: false,
            level: "debug".to_string(),
            buffer_size: 1000,
        };

        init_logging(config).await.unwrap();

        tracing::info!(event = "test", message = "This is a test log");
        tracing::debug!(user_id = 123, action = "login");

        // Give time for async writes
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}
