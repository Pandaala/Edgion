use anyhow::Result;
use std::path::PathBuf;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

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
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_dir: PathBuf::from("logs"),
            file_prefix: "edgion".to_string(),
            json_format: false,
            console: true,
            level: "info".to_string(),
        }
    }
}

pub async fn init_logging(config: LogConfig) -> Result<WorkerGuard> {
    // Create log directory if it doesn't exist
    tokio::fs::create_dir_all(&config.log_dir).await?;

    // Create daily rotating file appender
    let file_appender = tracing_appender::rolling::daily(&config.log_dir, &config.file_prefix);
    
    // Wrap with non_blocking for async writes
    // This creates a background OS thread (not a Tokio task) that handles actual writes
    // The guard MUST be kept alive, otherwise the background thread will shut down
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // Build subscriber based on format and console settings
    // We need to handle different layer types separately due to type constraints
    if config.json_format {
        // Build env filter
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(&config.level));

        // JSON format file layer
        let file_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(non_blocking)
            .with_target(true)
            .with_line_number(true)
            .with_current_span(false)
            .with_ansi(false);

        let subscriber = tracing_subscriber::registry()
            .with(env_filter)
            .with(file_layer);

        if config.console {
            let console_layer = tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_line_number(true)
                .with_ansi(true);
            subscriber.with(console_layer).try_init()?;
        } else {
            subscriber.try_init()?;
        }
    } else {
    // Build env filter
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(&config.level));

        // Plain text format file layer
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(non_blocking)
            .with_target(true)
            .with_line_number(true)
            .with_ansi(false);

        let subscriber = tracing_subscriber::registry()
            .with(env_filter)
            .with(file_layer);

    if config.console {
        let console_layer = tracing_subscriber::fmt::layer()
            .with_target(true)
                .with_line_number(true)
                .with_ansi(true);
        subscriber.with(console_layer).try_init()?;
    } else {
        subscriber.try_init()?;
        }
    }

    Ok(guard)
}

/// Initialize logging with default configuration
pub async fn init_default() -> Result<WorkerGuard> {
    init_logging(LogConfig::default()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_logging_from_different_threads() {
        let temp_dir = std::env::temp_dir().join("edgion_test_multithread");
        let config = LogConfig {
            log_dir: temp_dir,
            file_prefix: "multithread".to_string(),
            json_format: false,
            console: false,
            level: "info".to_string(),
        };

        let _guard = init_logging(config).await.unwrap();

        // Test from Tokio task
        tokio::spawn(async {
            tracing::info!("Log from tokio task");
        }).await.unwrap();

        // Test from regular thread
        std::thread::spawn(|| {
            tracing::info!("Log from regular thread");
        }).join().unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}
