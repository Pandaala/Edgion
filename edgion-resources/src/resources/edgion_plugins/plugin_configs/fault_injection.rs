use crate::gwapi_types::is_valid_header_token;
use crate::response_header_policy::validate_response_header_mutation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Maximum permitted delay duration (matches MockConfig's MAX_DELAY_MS).
pub const MAX_DELAY_MS: u64 = 30_000;

/// Fault Injection plugin configuration.
///
/// Mirrors Envoy's native fault filter: independent ratios for delay and
/// abort; both can fire on the same request. Request matching is delegated
/// to the plugin entry's `conditions:` field — this plugin has no `match`
/// block.
///
/// ## Example
/// ```yaml
/// fault_injection:
///   delay:
///     ratio: 0.10
///     fixedMs: 500
///   abort:
///     ratio: 0.05
///     statusCode: 503
///     body: "injected fault"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FaultInjectionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<DelayConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort: Option<AbortConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DelayConfig {
    /// Probability ratio in [0.0, 1.0]; default 1.0.
    #[serde(default = "default_ratio")]
    pub ratio: f64,

    /// Fixed delay in milliseconds. Mutually exclusive with min_ms/max_ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_ms: Option<u64>,

    /// Lower bound of inclusive uniform-random delay range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_ms: Option<u64>,

    /// Upper bound of inclusive uniform-random delay range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AbortConfig {
    /// Probability ratio in [0.0, 1.0]; default 1.0.
    #[serde(default = "default_ratio")]
    pub ratio: f64,

    /// HTTP status code returned on abort; default 500.
    #[serde(default = "default_abort_status")]
    #[schemars(range(min = 200, max = 599))]
    pub status_code: u16,

    /// Response body. Omitted = empty body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,

    /// Content-Type response header; default "text/plain".
    #[serde(default = "default_content_type")]
    pub content_type: String,

    /// Additional response headers. Order preserved; duplicates permitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<FaultHeaderEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FaultHeaderEntry {
    pub name: String,
    pub value: String,
}

fn default_ratio() -> f64 {
    1.0
}

fn default_abort_status() -> u16 {
    500
}

fn default_content_type() -> String {
    "text/plain".to_string()
}

use crate::resources::edgion_plugins::PluginValidation;

/// Validates an HTTP header value: no control chars (\r, \n, \0); must be
/// ASCII printable, space, or horizontal tab.
fn is_valid_header_value(value: &str) -> bool {
    value.bytes().all(|b| b == b'\t' || (b >= b' ' && b != b'\x7f'))
}

impl FaultInjectionConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.delay.is_none() && self.abort.is_none() {
            return Err("fault_injection: at least one of `delay` / `abort` must be set".into());
        }
        if let Some(d) = &self.delay {
            d.validate()?;
        }
        if let Some(a) = &self.abort {
            a.validate()?;
        }
        Ok(())
    }
}

impl DelayConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.ratio) {
            return Err(format!("delay.ratio must be in [0.0, 1.0], got {}", self.ratio));
        }
        let has_fixed = self.fixed_ms.is_some();
        let has_min = self.min_ms.is_some();
        let has_max = self.max_ms.is_some();
        if has_fixed && (has_min || has_max) {
            return Err("delay.fixedMs is mutually exclusive with delay.minMs/maxMs".into());
        }
        if !has_fixed && !has_min && !has_max {
            return Err("delay must set either delay.fixedMs or both delay.minMs and delay.maxMs".into());
        }
        if has_min ^ has_max {
            return Err("delay.minMs and delay.maxMs must be set together".into());
        }
        if let (Some(min), Some(max)) = (self.min_ms, self.max_ms) {
            if min > max {
                return Err(format!("delay.minMs ({min}) must be <= delay.maxMs ({max})"));
            }
            if max > MAX_DELAY_MS {
                return Err(format!("delay.maxMs ({max}) must be <= {MAX_DELAY_MS}"));
            }
        }
        if let Some(fixed) = self.fixed_ms {
            if fixed > MAX_DELAY_MS {
                return Err(format!("delay.fixedMs ({fixed}) must be <= {MAX_DELAY_MS}"));
            }
        }
        Ok(())
    }
}

impl AbortConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.ratio) {
            return Err(format!("abort.ratio must be in [0.0, 1.0], got {}", self.ratio));
        }
        if !crate::http_status::is_valid_final_http_status(self.status_code) {
            return Err(format!("abort.statusCode ({}) must be in [200, 599]", self.status_code));
        }
        if let Some(hs) = &self.headers {
            for h in hs {
                validate_response_header_mutation(&h.name).map_err(str::to_owned)?;
                if !is_valid_header_token(&h.name) {
                    return Err(format!(
                        "invalid abort.headers name '{}': invalid header name token",
                        h.name
                    ));
                }
                if !is_valid_header_value(&h.value) {
                    return Err(format!(
                        "invalid abort.headers value for '{}': invalid header value",
                        h.name
                    ));
                }
            }
        }
        if !is_valid_header_value(&self.content_type) {
            return Err(format!(
                "invalid abort.contentType '{}': invalid header value",
                self.content_type
            ));
        }
        Ok(())
    }
}

impl PluginValidation for FaultInjectionConfig {
    fn validation_error(&self) -> Option<String> {
        self.validate().err()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/fault_injection_test.rs"]
mod tests;
