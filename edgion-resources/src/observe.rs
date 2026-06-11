//! Observability configuration types

use crate::output::StringOutput;
use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Generic log configuration applicable to all log types
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Args)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct LogConfig {
    /// Enable or disable this log
    #[arg(skip)]
    #[serde(default)]
    pub enabled: bool,

    /// Output destination configuration
    #[arg(skip)]
    #[serde(default)]
    pub output: StringOutput,
}

impl LogConfig {
    /// Create a new LogConfig with default output to a given path
    pub fn with_path(path: impl Into<String>, enabled: bool) -> Self {
        Self {
            enabled,
            output: StringOutput::LocalFile(crate::output::LocalFileWriterCfg {
                path: path.into(),
                queue_size: None,
                rotation: None,
            }),
        }
    }

    /// Create enabled config with default settings
    pub fn enabled_default(path: impl Into<String>) -> Self {
        Self::with_path(path, true)
    }

    /// Create disabled config with default settings
    pub fn disabled_default(path: impl Into<String>) -> Self {
        Self::with_path(path, false)
    }
}

/// Access log output options that are not part of the generic [`LogConfig`].
///
/// Sits alongside the `access_log` section in YAML (e.g. `access_log_options`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessLogOptions {
    /// When `true`, the access log entry includes a `request_uri` field carrying
    /// the entry-time raw path + raw query (the original URI the client sent).
    /// Default `false` — the access log emits only the current normalized
    /// `path` field, no raw path or query.
    pub include_raw_uri: bool,
}

/// Log type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogType {
    /// HTTP/gRPC access log
    Access,
    /// SSL/TLS handshake log
    Ssl,
    /// TCP connection log
    Tcp,
    /// UDP session log
    Udp,
}

impl LogType {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogType::Access => "access",
            LogType::Ssl => "ssl",
            LogType::Tcp => "tcp",
            LogType::Udp => "udp",
        }
    }
}
