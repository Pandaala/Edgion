//! Generic body-requirement declaration for body-aware plugins.
//!
//! Edgion runs on Pingora, which streams bodies chunk-by-chunk and does not
//! buffer the full body. A plugin that needs the complete request/response body
//! (HMAC-over-body validation, external body processing, body-key extraction,
//! JSON transforms, ...) declares that need through a [`BodyRequirement`]. The
//! gateway runtime wraps such plugins in a body gate that:
//!
//! - re-uses the plugin condition engine ([`PluginConditions`]) as a SECONDARY
//!   run/skip filter deciding whether the body is required for THIS request;
//! - buffers the body to the resolved gateway-wide physical limit (the buffer is
//!   NOT sized by any per-plugin value), then applies the plugin's own
//!   `maxBodySize` as a
//!   post-read LIMIT (rejecting a body larger than that limit, for this plugin
//!   only, independent of the shared buffer);
//! - on a read failure / over-limit, applies the configured
//!   [`BodyReadFailureMode`] (fail-close → 500, or 413 when the body was too
//!   large; fail-open → skip the plugin as if it were disabled).
//!
//! The requirement can be declared by the operator (the `body` field on a plugin
//! entry) or intrinsically by the plugin implementation. Operator fields tune
//! the intrinsic declaration; omitted operator fields retain intrinsic values.

use super::conditions::PluginConditions;
use crate::resources::common::parse_byte_size;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the runtime does when a body-aware plugin cannot read the body.
///
/// "Cannot read" covers every data-plane failure the read path can surface:
/// buffering not implemented on the binary, the body exceeding the configured
/// cap, or an aborted stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum BodyReadFailureMode {
    /// Reject the request (500, or 413 when the body was too large). Safe default
    /// for security/validation plugins: a plugin that needs the body to make an
    /// allow/deny decision must never be silently bypassed.
    #[default]
    FailClose,
    /// Skip the plugin and continue the request, exactly as if the plugin were
    /// not enabled. Appropriate for best-effort, non-security plugins.
    FailOpen,
}

/// Declares that a plugin needs the fully-buffered body, and how to gate/handle
/// it. See the module docs for the wrapping semantics.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BodyRequirement {
    /// Optional per-plugin body-size **limit**, as a human-readable size
    /// (`"1k"`/`"1m"`/`"64M"`/`"1024"`; 1024-based, case-insensitive).
    ///
    /// This is purely a per-plugin restriction: if the request/response body
    /// exceeds it, THIS plugin treats it as too-large (per `on_read_failure`),
    /// independent of every other plugin. It does NOT size the shared body
    /// buffer — how much is physically buffered is decided solely by the
    /// resolved gateway-wide physical limit. So multiple plugins
    /// can each set their own `maxBodySize` with no conflict; a plugin can only
    /// tighten (a value above the global buffer never triggers). Unset → the
    /// plugin accepts any body the gateway buffers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_size: Option<String>,

    /// SECONDARY run/skip gate deciding whether the body is required for this
    /// request. Re-uses the same [`PluginConditions`] engine as the entry-level
    /// `conditions`: the entry conditions decide whether the plugin runs at all;
    /// these decide whether the body is needed once it does. When the gate
    /// evaluates to "skip", the plugin runs WITHOUT body enforcement (no
    /// fail-close). `None`/empty → the body is always required when the plugin
    /// runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// Behavior when the body cannot be read. Defaults to
    /// [`BodyReadFailureMode::FailClose`].
    #[serde(default)]
    pub on_read_failure: BodyReadFailureMode,
}

impl BodyRequirement {
    /// A minimal fail-close requirement with no explicit cap and no secondary
    /// gate. Handy for plugins that intrinsically always need the body.
    pub fn fail_close() -> Self {
        Self::default()
    }

    /// True when this requirement should reject on a read failure.
    pub fn is_fail_close(&self) -> bool {
        self.on_read_failure == BodyReadFailureMode::FailClose
    }

    /// Parse the per-plugin `maxBodySize` limit into bytes. `None` when unset;
    /// `Some(Err)` when set but unparseable (surfaced as a preparse error).
    pub fn max_body_size_bytes(&self) -> Option<Result<u64, String>> {
        self.max_body_size.as_ref().map(|s| {
            parse_byte_size(s)
                .ok_or_else(|| format!("invalid body.maxBodySize '{s}' (expected e.g. '1k', '1m', '64M', '1024')"))
        })
    }

    /// Validation error for the body requirement, or `None` if valid.
    pub fn validation_error(&self) -> Option<String> {
        match self.max_body_size_bytes() {
            Some(Err(e)) => Some(e),
            Some(Ok(0)) => Some("body.maxBodySize must be greater than 0".to_string()),
            _ => None,
        }
    }

    /// Resolve the effective requirement from the operator's `body:` entry and
    /// the plugin's intrinsic declaration ([`super::edgion_plugin::EdgionPlugin::body_requirement`]).
    ///
    /// The operator can only **tune** a requirement the plugin actually declares;
    /// it cannot **create** one. So when the plugin does not consume the body
    /// (`intrinsic == None`), the result is `None` regardless of any operator
    /// `body:` — the plugin is never wrapped in a body gate (no spurious 500),
    /// and the stray `body:` is surfaced as a preparse validation error instead.
    ///
    /// When the plugin does consume the body, the operator may tune
    /// `max_body_size` / `conditions`; omitted operator fields inherit their
    /// intrinsic values. The operator may NOT loosen an intrinsic fail-close to
    /// fail-open: the stricter `on_read_failure` wins, so a security plugin (e.g.
    /// HMAC body validation) can never be silently downgraded to bypass-on-error.
    pub fn resolve(operator: Option<BodyRequirement>, intrinsic: Option<BodyRequirement>) -> Option<BodyRequirement> {
        let intr = intrinsic?;
        match operator {
            None => Some(intr),
            Some(mut op) => {
                if op.max_body_size.is_none() {
                    op.max_body_size = intr.max_body_size.clone();
                }
                if op.conditions.is_none() {
                    op.conditions = intr.conditions.clone();
                }
                if intr.is_fail_close() {
                    // Stricter fail policy wins: can't downgrade to fail-open.
                    op.on_read_failure = BodyReadFailureMode::FailClose;
                    // Can't narrow enforcement SCOPE either: a fail-close plugin
                    // that intrinsically always needs the body (no intrinsic
                    // conditions) must not have operator `conditions` skip the
                    // body for some requests — that would be a bypass. Operators
                    // may still tune size. A plugin that is intrinsically
                    // conditional keeps operator condition tuning.
                    if intr.conditions.is_none() {
                        op.conditions = None;
                    }
                }
                Some(op)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_fail_close() {
        let req = BodyRequirement::default();
        assert!(req.is_fail_close());
        assert_eq!(req.on_read_failure, BodyReadFailureMode::FailClose);
        assert!(req.max_body_size.is_none());
        assert!(req.conditions.is_none());
    }

    #[test]
    fn deserializes_camel_case_and_fail_open() {
        let req: BodyRequirement =
            serde_json::from_str(r#"{ "maxBodySize": "1m", "onReadFailure": "failOpen" }"#).unwrap();
        assert_eq!(req.max_body_size.as_deref(), Some("1m"));
        assert_eq!(req.max_body_size_bytes(), Some(Ok(1024 * 1024)));
        assert_eq!(req.on_read_failure, BodyReadFailureMode::FailOpen);
        assert!(!req.is_fail_close());
    }

    #[test]
    fn empty_object_defaults_fail_close() {
        let req: BodyRequirement = serde_json::from_str("{}").unwrap();
        assert!(req.is_fail_close());
        assert!(req.max_body_size_bytes().is_none());
        assert!(req.validation_error().is_none());
    }

    #[test]
    fn invalid_and_zero_max_body_size_are_validation_errors() {
        let bad: BodyRequirement = serde_json::from_str(r#"{ "maxBodySize": "notasize" }"#).unwrap();
        assert!(bad.validation_error().is_some());
        let zero: BodyRequirement = serde_json::from_str(r#"{ "maxBodySize": "0" }"#).unwrap();
        assert!(zero.validation_error().unwrap().contains("greater than 0"));
    }

    #[test]
    fn secondary_conditions_parse() {
        let req: BodyRequirement = serde_json::from_str(
            r#"{ "conditions": { "run": [ { "type": "include", "key": { "type": "method" }, "values": ["POST"] } ] } }"#,
        )
        .unwrap();
        assert!(req.conditions.is_some());
    }

    #[test]
    fn resolve_operator_cannot_downgrade_intrinsic_fail_close() {
        // Intrinsic fail-close (e.g. HMAC body validation) + operator failOpen →
        // stays fail-close (security plugin cannot be silently bypassed), but the
        // operator's maxSize tuning is still honored.
        let operator = BodyRequirement {
            max_body_size: Some("2m".to_string()),
            on_read_failure: BodyReadFailureMode::FailOpen,
            ..Default::default()
        };
        let intrinsic = BodyRequirement::fail_close();
        let resolved = BodyRequirement::resolve(Some(operator), Some(intrinsic)).unwrap();
        assert!(resolved.is_fail_close());
        assert_eq!(resolved.max_body_size.as_deref(), Some("2m"));

        // Intrinsic fail-open + operator fail-close → operator can tighten.
        let operator = BodyRequirement::fail_close();
        let intrinsic = BodyRequirement {
            on_read_failure: BodyReadFailureMode::FailOpen,
            ..Default::default()
        };
        let resolved = BodyRequirement::resolve(Some(operator), Some(intrinsic)).unwrap();
        assert!(resolved.is_fail_close());

        // No operator, intrinsic only → intrinsic used as-is.
        let resolved = BodyRequirement::resolve(None, Some(BodyRequirement::fail_close())).unwrap();
        assert!(resolved.is_fail_close());
        // Neither → None.
        assert!(BodyRequirement::resolve(None, None).is_none());

        // Operator body: on a plugin that does NOT consume body (intrinsic None)
        // → None: the operator cannot create a requirement, so a non-body plugin
        // is never gated (no spurious 500).
        let operator = BodyRequirement {
            max_body_size: Some("1m".to_string()),
            ..Default::default()
        };
        assert!(BodyRequirement::resolve(Some(operator), None).is_none());

        // Operator cannot narrow enforcement SCOPE of a fail-close, always-needs-
        // body plugin (no intrinsic conditions): operator conditions are dropped,
        // but size tuning survives.
        let operator = BodyRequirement {
            max_body_size: Some("2m".to_string()),
            conditions: Some(PluginConditions {
                skip: None,
                run: Some(vec![]),
            }),
            on_read_failure: BodyReadFailureMode::FailOpen,
        };
        let resolved = BodyRequirement::resolve(Some(operator), Some(BodyRequirement::fail_close())).unwrap();
        assert!(resolved.is_fail_close());
        assert!(resolved.conditions.is_none(), "operator conditions must be dropped");
        assert_eq!(resolved.max_body_size.as_deref(), Some("2m"));
    }

    #[test]
    fn resolve_inherits_intrinsic_max_when_operator_omits_it() {
        let intrinsic = BodyRequirement {
            max_body_size: Some("64KiB".to_string()),
            conditions: Some(PluginConditions {
                skip: None,
                run: Some(vec![]),
            }),
            ..Default::default()
        };

        let empty_operator: BodyRequirement = serde_json::from_str("{}").unwrap();
        let resolved = BodyRequirement::resolve(Some(empty_operator), Some(intrinsic.clone())).unwrap();
        assert_eq!(resolved.max_body_size.as_deref(), Some("64KiB"));
        assert!(resolved.conditions.is_some());

        let policy_only_operator: BodyRequirement = serde_json::from_str(r#"{ "onReadFailure": "failOpen" }"#).unwrap();
        let resolved = BodyRequirement::resolve(Some(policy_only_operator), Some(intrinsic)).unwrap();
        assert_eq!(resolved.max_body_size.as_deref(), Some("64KiB"));
        assert!(resolved.conditions.is_some());
        assert!(resolved.is_fail_close());
    }

    #[test]
    fn max_body_size_bytes_parses_the_limit() {
        let req: BodyRequirement = serde_json::from_str(r#"{ "maxBodySize": "64M" }"#).unwrap();
        assert_eq!(req.max_body_size_bytes(), Some(Ok(64 * 1024 * 1024)));
    }
}
