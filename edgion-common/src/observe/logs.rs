use anyhow::Result;
use std::path::PathBuf;
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

const PINGORA_H1_CLIENT_TARGET: &str = "pingora_core::protocols::http::v1::client";
const PINGORA_H1_PROXY_TARGET: &str = "pingora_proxy::proxy_h1";
// These dependencies can format peer-controlled headers, TLS identities and
// certificate errors. Their diagnostics cannot be made safe by our callers.
// Keep this protection independent of operator RUST_LOG directives.
const SENSITIVE_TRANSPORT_TARGETS: [&str; 3] = ["tonic::transport", "rustls", "h2"];

fn within_target(target: &str, parent: &str) -> bool {
    target == parent || target.strip_prefix(parent).is_some_and(|rest| rest.starts_with("::"))
}

fn system_log_event_allowed(target: &str, level: &Level) -> bool {
    if SENSITIVE_TRANSPORT_TARGETS
        .iter()
        .any(|parent| within_target(target, parent))
    {
        return false;
    }
    let protected_target = [PINGORA_H1_CLIENT_TARGET, PINGORA_H1_PROXY_TARGET]
        .iter()
        .any(|protected| within_target(target, protected));

    !protected_target || level == &Level::ERROR
}

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
    tokio::fs::create_dir_all(&config.log_dir).await?;

    // Fixed-name file appender (nginx-style: fixed name, no date in active file).
    // External log rotation (logrotate or similar) handles archiving.
    let file_name = if config.file_prefix.ends_with(".log") {
        config.file_prefix.clone()
    } else {
        format!("{}.log", config.file_prefix)
    };
    let file_appender = tracing_appender::rolling::never(&config.log_dir, &file_name);

    // Wrap with non_blocking for async writes
    // This creates a background OS thread (not a Tokio task) that handles actual writes
    // The guard MUST be kept alive, otherwise the background thread will shut down
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.level));

    // Only `.json()` and `.with_current_span(false)` differ between the two output formats;
    // box the file layer so JSON and plain-text variants share one subscriber pipeline.
    let file_layer = if config.json_format {
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(non_blocking)
            .with_target(true)
            .with_line_number(true)
            .with_current_span(false)
            .with_ansi(false)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .with_writer(non_blocking)
            .with_target(true)
            .with_line_number(true)
            .with_ansi(false)
            .boxed()
    };

    // Pingora's upstream HTTP/1 implementation logs complete request headers at
    // DEBUG/TRACE. Keep those two upstream targets at ERROR even when an operator
    // enables a broader RUST_LOG directive; serialized headers can no longer use
    // HeaderValue's sensitive metadata for redaction.
    // OTLP's native tonic/rustls/h2 stack can also expose identity and remote
    // failure text at arbitrary verbosity. Preserve binary-owned diagnostics.
    let protected_target_filter = filter_fn(|metadata| system_log_event_allowed(metadata.target(), metadata.level()));
    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(protected_target_filter)
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

    // Subscriber is now live: let the startup-error and panic paths route
    // through structured logging instead of falling back to plain stderr.
    crate::startup::mark_tracing_ready();

    Ok(guard)
}

#[cfg(test)]
#[path = "../../tests/unit/observe/logs_test.rs"]
mod tests;
