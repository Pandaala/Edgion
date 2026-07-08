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
#[serde(rename_all = "camelCase", default)]
pub struct AccessLogOptions {
    /// When `true`, the access log entry includes a `request_uri` field carrying
    /// the entry-time raw path + raw query (the original URI the client sent).
    /// Default `false` — the access log emits only the current normalized
    /// `path` field, no raw path or query.
    pub include_raw_uri: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{LocalFileWriterCfg, StringOutput};

    #[test]
    fn log_config_without_fallback_is_backward_compatible() {
        // Old configs have no `fallback` key — must deserialize to None.
        let json = r#"{"enabled":true,"output":{"localFile":{"path":"logs/a.log"}}}"#;
        let cfg: LogConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.fallback.is_none());
        // `skip_serializing_if = "Option::is_none"` must keep the key out of the
        // serialized form, so existing YAML/JSON configs never grow a stale
        // `"fallback": null`.
        let reserialized = serde_json::to_string(&cfg).unwrap();
        assert!(
            !reserialized.contains("fallback"),
            "None fallback must be omitted: {reserialized}"
        );
    }

    #[test]
    fn log_config_with_fallback_roundtrips() {
        let cfg = LogConfig {
            enabled: true,
            output: StringOutput::Es {
                link_sys_ref: crate::output::LinkSysRef {
                    namespace: "default".into(),
                    name: "es-main".into(),
                },
            },
            fallback: Some(StringOutput::LocalFile(LocalFileWriterCfg {
                path: "logs/es_fallback.log".into(),
                ..Default::default()
            })),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: LogConfig = serde_json::from_str(&json).unwrap();
        // Assert the inner config survived the roundtrip, not just the variant —
        // a variant-only check would pass even if the payload were reset to
        // defaults. StringOutput has no PartialEq, so match and compare `path`.
        match back.fallback {
            Some(StringOutput::LocalFile(ref f)) => assert_eq!(f.path, "logs/es_fallback.log"),
            other => panic!("expected LocalFile fallback, got {other:?}"),
        }
    }
}
