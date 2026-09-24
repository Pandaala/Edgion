//! Guardrail unary semantic-window decision configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{CustomSource, ExternalHttpConnection, OutboundRequestSpec, ResolvedSecrets};
use crate::resources::edgion_plugins::PluginValidation;

/// RawWindow's absolute raw/semantic generation ceiling.
pub const GUARDRAIL_MAX_WINDOW_BYTES: usize = 1024 * 1024;
/// RawWindow's global overlap ceiling.
pub const GUARDRAIL_MAX_OVERLAP_BYTES: usize = 256 * 1024;
/// Absolute serialized unary request ceiling.
pub const GUARDRAIL_MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// Absolute decoded replacement-text ceiling.
pub const GUARDRAIL_MAX_REPLACEMENT_BYTES: usize = 1024 * 1024;
/// Absolute per-snapshot in-flight decision ceiling.
pub const GUARDRAIL_MAX_CONCURRENT: usize = 1024;
/// Absolute first-window response-head deadline ceiling.
pub const GUARDRAIL_MAX_HEAD_HOLD_TIMEOUT_MS: u64 = 60_000;

fn default_window_bytes() -> usize {
    4096
}

fn default_overlap_bytes() -> usize {
    256
}

fn default_max_request_bytes() -> usize {
    GUARDRAIL_MAX_REQUEST_BYTES
}

fn default_max_replacement_bytes() -> usize {
    GUARDRAIL_MAX_REPLACEMENT_BYTES
}

fn default_max_concurrent() -> usize {
    64
}

fn default_head_hold_timeout_ms() -> u64 {
    5_000
}

/// Operator-facing configuration for one bounded unary decision per canonical
/// semantic SSE window.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GuardrailConfig {
    /// Connection target, TLS, static timeout, health/rate gates, and bounded
    /// response consumption. Retry, dynamic timeout, and custom success rules
    /// are rejected by Guardrail validation.
    #[serde(flatten)]
    pub conn: ExternalHttpConnection,

    /// Restricted outbound request metadata. Only a path and custom headers
    /// are accepted; Guardrail fixes the method, content type, and body.
    pub request: OutboundRequestSpec,

    /// Inclusive decoded current-semantic threshold for one generation.
    #[serde(default = "default_window_bytes")]
    #[schemars(range(min = 1, max = GUARDRAIL_MAX_WINDOW_BYTES))]
    pub window_bytes: usize,

    /// Per-logical-bucket overlap ceiling. RawWindow applies its independent
    /// global 256 KiB overlap ceiling as well.
    #[serde(default = "default_overlap_bytes")]
    #[schemars(range(min = 0, max = GUARDRAIL_MAX_OVERLAP_BYTES))]
    pub overlap_bytes: usize,

    /// Inclusive cap for the complete serialized decision request.
    #[serde(default = "default_max_request_bytes")]
    #[schemars(range(min = 1, max = GUARDRAIL_MAX_REQUEST_BYTES))]
    pub max_request_bytes: usize,

    /// Inclusive aggregate cap for decoded replacement text.
    #[serde(default = "default_max_replacement_bytes")]
    #[schemars(range(min = 0, max = GUARDRAIL_MAX_REPLACEMENT_BYTES))]
    pub max_replacement_bytes: usize,

    /// Maximum in-flight decisions sharing this immutable snapshot.
    #[serde(default = "default_max_concurrent")]
    #[schemars(range(min = 1, max = GUARDRAIL_MAX_CONCURRENT))]
    pub max_concurrent: usize,

    /// Delay the final downstream head until the first semantic window has a
    /// fail-close decision. Disabled by default for rollout compatibility.
    #[serde(default)]
    pub hold_first_window: bool,

    /// Absolute deadline from Hold activation through the first decision.
    #[serde(default = "default_head_hold_timeout_ms")]
    #[schemars(range(min = 1, max = GUARDRAIL_MAX_HEAD_HOLD_TIMEOUT_MS))]
    pub head_hold_timeout_ms: u64,

    /// Release original bytes and disable Guardrail after a dependency failure.
    /// A valid policy Reject is never softened by this setting.
    #[serde(default)]
    pub fail_open: bool,

    /// Controller-resolved `${secretRef:...}` values carried to the gateway.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_secrets: Option<ResolvedSecrets>,
}

impl Default for GuardrailConfig {
    fn default() -> Self {
        Self {
            conn: ExternalHttpConnection::default(),
            request: OutboundRequestSpec::default(),
            window_bytes: default_window_bytes(),
            overlap_bytes: default_overlap_bytes(),
            max_request_bytes: default_max_request_bytes(),
            max_replacement_bytes: default_max_replacement_bytes(),
            max_concurrent: default_max_concurrent(),
            hold_first_window: false,
            head_hold_timeout_ms: default_head_hold_timeout_ms(),
            fail_open: false,
            resolved_secrets: None,
        }
    }
}

use super::validate_restricted_template;

impl GuardrailConfig {
    /// Return the first admission-time configuration error.
    pub fn validation_error(&self) -> Option<String> {
        if self.hold_first_window && self.fail_open {
            return Some(
                "failOpen is not supported with Guardrail response-head Hold; use failOpen: false".to_string(),
            );
        }
        if let Some(error) = crate::resources::common::validate_connection_target(&self.conn) {
            return Some(format!("target invalid: {error}"));
        }
        if self.conn.retry.is_some() {
            return Some("retry is not supported for Guardrail unary POST decisions".to_string());
        }
        if self.conn.timeout_ms_template.is_some() {
            return Some("timeoutMsTemplate is not supported for Guardrail; use static timeoutMs".to_string());
        }
        if self.conn.success.is_some() {
            return Some("success is not configurable for Guardrail decisions".to_string());
        }
        if let Some(error) = self.request.validation_error() {
            return Some(error);
        }
        if self.request.method.is_some() {
            return Some("request.method is fixed to POST for Guardrail".to_string());
        }
        if self.request.body.is_some() {
            return Some("request.body is fixed to the Guardrail decision envelope".to_string());
        }
        if self.request.args.forward_all || !self.request.args.origin.is_empty() || !self.request.args.custom.is_empty()
        {
            return Some("request.args is not supported for Guardrail".to_string());
        }
        if self.request.cookies.forward_all
            || !self.request.cookies.origin.is_empty()
            || !self.request.cookies.custom.is_empty()
        {
            return Some("request.cookies is not supported for Guardrail".to_string());
        }
        if self.request.headers.forward_all || !self.request.headers.origin.is_empty() {
            return Some("request.headers may contain only custom fields for Guardrail".to_string());
        }
        let Some(path) = self.request.path.as_ref() else {
            return Some("request.path is required for Guardrail".to_string());
        };
        if path.allow_override {
            return Some("request.path.allowOverride is not supported for Guardrail".to_string());
        }
        if let Some(error) = validate_restricted_template(&path.template, "request.path.template") {
            return Some(error);
        }
        for (index, header) in self.request.headers.custom.iter().enumerate() {
            if header.name.eq_ignore_ascii_case("content-type") {
                return Some(
                    "request.headers must not configure Content-Type; Guardrail fixes application/json".to_string(),
                );
            }
            if let CustomSource::Template(template) = &header.source {
                if let Some(error) =
                    validate_restricted_template(template, &format!("request.headers.custom[{index}].template"))
                {
                    return Some(error);
                }
            }
        }
        if !(1..=GUARDRAIL_MAX_WINDOW_BYTES).contains(&self.window_bytes) {
            return Some(format!(
                "windowBytes must be between 1 and {GUARDRAIL_MAX_WINDOW_BYTES}"
            ));
        }
        if self.overlap_bytes > GUARDRAIL_MAX_OVERLAP_BYTES {
            return Some(format!("overlapBytes must be at most {GUARDRAIL_MAX_OVERLAP_BYTES}"));
        }
        if !(1..=GUARDRAIL_MAX_REQUEST_BYTES).contains(&self.max_request_bytes) {
            return Some(format!(
                "maxRequestBytes must be between 1 and {GUARDRAIL_MAX_REQUEST_BYTES}"
            ));
        }
        if self.max_replacement_bytes > GUARDRAIL_MAX_REPLACEMENT_BYTES {
            return Some(format!(
                "maxReplacementBytes must be at most {GUARDRAIL_MAX_REPLACEMENT_BYTES}"
            ));
        }
        if !(1..=GUARDRAIL_MAX_CONCURRENT).contains(&self.max_concurrent) {
            return Some(format!(
                "maxConcurrent must be between 1 and {GUARDRAIL_MAX_CONCURRENT}"
            ));
        }
        if !(1..=GUARDRAIL_MAX_HEAD_HOLD_TIMEOUT_MS).contains(&self.head_hold_timeout_ms) {
            return Some(format!(
                "headHoldTimeoutMs must be between 1 and {GUARDRAIL_MAX_HEAD_HOLD_TIMEOUT_MS}"
            ));
        }
        None
    }
}

impl PluginValidation for GuardrailConfig {
    fn validation_error(&self) -> Option<String> {
        GuardrailConfig::validation_error(self)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/guardrail_test.rs"]
mod tests;
