//! Observability configuration types

use crate::types::link_sys::StringOutput;
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
            output: StringOutput::LocalFile(crate::types::link_sys::LocalFileWriterCfg {
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
