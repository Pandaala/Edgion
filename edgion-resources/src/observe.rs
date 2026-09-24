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

    /// Fallback sink used when `output` reports unhealthy (first-healthy
    /// degradation). Registration order is priority: `output` is primary,
    /// `fallback` is the standby. Typically a bounded `LocalFile` so access
    /// logs survive an ES/Kafka/OTLP outage. `None` = no fallback (existing
    /// behavior). Only one level — there is no fallback-of-fallback.
    #[arg(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<StringOutput>,
}

impl LogConfig {
    /// Create a new LogConfig with default output to a given path
    pub fn with_path(path: impl Into<String>, enabled: bool) -> Self {
        Self {
            enabled,
            output: StringOutput::LocalFile(crate::output::LocalFileWriterCfg {
                path: path.into(),
                ..Default::default()
            }),
            fallback: None,
        }
    }

    /// Create enabled config with default settings
    pub fn enabled_default(path: impl Into<String>) -> Self {
        Self::with_path(path, true)
    }
}

/// Access log output options that are not part of the generic [`LogConfig`].
///
/// Sits alongside the `access_log` section in YAML (e.g. `access_log_options`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct AccessLogOptions {
    /// When `true`, the access log entry includes a `request_uri` field carrying
    /// the entry-time raw path + raw query (the original URI the client sent).
    /// Default `false` — the access log emits only the current normalized
    /// `path` field, no raw path or query.
    pub include_raw_uri: bool,
}

#[cfg(test)]
#[path = "../tests/unit/observe_test.rs"]
mod tests;
