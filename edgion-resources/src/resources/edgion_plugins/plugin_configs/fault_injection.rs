use crate::gwapi_types::is_valid_header_token;
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
        if !(100..=599).contains(&self.status_code) {
            return Err(format!("abort.statusCode ({}) must be in [100, 599]", self.status_code));
        }
        if let Some(hs) = &self.headers {
            for h in hs {
                if h.name.eq_ignore_ascii_case("content-length") {
                    return Err(format!(
                        "abort.headers must not contain Content-Length (runtime overrides it): '{}'",
                        h.name
                    ));
                }
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
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_full() {
        let json = r#"{
            "delay": { "ratio": 0.5, "fixedMs": 300 },
            "abort": {
                "ratio": 0.25,
                "statusCode": 503,
                "body": "injected",
                "contentType": "text/plain",
                "headers": [{ "name": "X-Fault", "value": "1" }]
            }
        }"#;
        let cfg: FaultInjectionConfig = serde_json::from_str(json).unwrap();
        let d = cfg.delay.as_ref().unwrap();
        assert_eq!(d.ratio, 0.5);
        assert_eq!(d.fixed_ms, Some(300));
        let a = cfg.abort.as_ref().unwrap();
        assert_eq!(a.status_code, 503);
        assert_eq!(a.body.as_deref(), Some("injected"));
        assert_eq!(a.content_type, "text/plain");
        assert_eq!(a.headers.as_ref().unwrap()[0].name, "X-Fault");
        let back = serde_json::to_string(&cfg).unwrap();
        assert!(back.contains("\"fixedMs\":300"));
        assert!(back.contains("\"statusCode\":503"));
    }

    #[test]
    fn defaults_apply() {
        let json = r#"{ "abort": {} }"#;
        let cfg: FaultInjectionConfig = serde_json::from_str(json).unwrap();
        let a = cfg.abort.as_ref().unwrap();
        assert_eq!(a.ratio, 1.0);
        assert_eq!(a.status_code, 500);
        assert_eq!(a.content_type, "text/plain");
        assert!(a.body.is_none());
        assert!(a.headers.is_none());
    }

    #[test]
    fn default_impl_is_empty() {
        let cfg = FaultInjectionConfig::default();
        assert!(cfg.delay.is_none());
        assert!(cfg.abort.is_none());
    }

    fn cfg_delay_fixed(ms: u64) -> FaultInjectionConfig {
        FaultInjectionConfig {
            delay: Some(DelayConfig {
                ratio: 1.0,
                fixed_ms: Some(ms),
                min_ms: None,
                max_ms: None,
            }),
            abort: None,
        }
    }

    fn cfg_abort(status: u16) -> FaultInjectionConfig {
        FaultInjectionConfig {
            delay: None,
            abort: Some(AbortConfig {
                ratio: 1.0,
                status_code: status,
                body: None,
                content_type: "text/plain".to_string(),
                headers: None,
            }),
        }
    }

    #[test]
    fn validate_ok_minimal_delay() {
        assert!(cfg_delay_fixed(0).validate().is_ok());
        assert!(cfg_delay_fixed(MAX_DELAY_MS).validate().is_ok());
    }

    #[test]
    fn validate_ok_minimal_abort() {
        assert!(cfg_abort(200).validate().is_ok());
        assert!(cfg_abort(599).validate().is_ok());
    }

    #[test]
    fn validate_rule1_both_blocks_missing() {
        let err = FaultInjectionConfig::default().validate().unwrap_err();
        assert!(err.contains("delay") && err.contains("abort"), "got: {err}");
    }

    #[test]
    fn validate_rule2_fixed_and_range_both_set() {
        let mut cfg = cfg_delay_fixed(100);
        cfg.delay.as_mut().unwrap().min_ms = Some(10);
        cfg.delay.as_mut().unwrap().max_ms = Some(50);
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("delay.fixedMs"), "got: {err}");
    }

    #[test]
    fn validate_rule2_neither_fixed_nor_range_set() {
        let cfg = FaultInjectionConfig {
            delay: Some(DelayConfig {
                ratio: 1.0,
                fixed_ms: None,
                min_ms: None,
                max_ms: None,
            }),
            abort: None,
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("delay") && err.contains("must"), "got: {err}");
    }

    #[test]
    fn validate_rule3_range_only_min() {
        let cfg = FaultInjectionConfig {
            delay: Some(DelayConfig {
                ratio: 1.0,
                fixed_ms: None,
                min_ms: Some(10),
                max_ms: None,
            }),
            abort: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rule3_range_min_gt_max() {
        let cfg = FaultInjectionConfig {
            delay: Some(DelayConfig {
                ratio: 1.0,
                fixed_ms: None,
                min_ms: Some(500),
                max_ms: Some(100),
            }),
            abort: None,
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("minMs") && err.contains("maxMs"), "got: {err}");
    }

    #[test]
    fn validate_rule3_range_max_too_large() {
        let cfg = FaultInjectionConfig {
            delay: Some(DelayConfig {
                ratio: 1.0,
                fixed_ms: None,
                min_ms: Some(0),
                max_ms: Some(MAX_DELAY_MS + 1),
            }),
            abort: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rule4_fixed_too_large() {
        assert!(cfg_delay_fixed(MAX_DELAY_MS + 1).validate().is_err());
    }

    #[test]
    fn validate_rule5_delay_ratio_out_of_range() {
        let mut cfg = cfg_delay_fixed(100);
        cfg.delay.as_mut().unwrap().ratio = 1.5;
        assert!(cfg.validate().is_err());
        cfg.delay.as_mut().unwrap().ratio = -0.1;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rule6_abort_ratio_out_of_range() {
        let mut cfg = cfg_abort(500);
        cfg.abort.as_mut().unwrap().ratio = 2.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rule7_status_out_of_range() {
        assert!(cfg_abort(99).validate().is_err());
        assert!(cfg_abort(600).validate().is_err());
    }

    #[test]
    fn validate_rule8_bad_header_name() {
        let mut cfg = cfg_abort(500);
        cfg.abort.as_mut().unwrap().headers = Some(vec![FaultHeaderEntry {
            name: "Bad Header\n".to_string(),
            value: "v".to_string(),
        }]);
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_lowercase().contains("header") || err.contains("name"),
            "got: {err}"
        );
    }

    #[test]
    fn validate_rule9_rejects_content_length_in_headers() {
        let mut cfg = cfg_abort(500);
        cfg.abort.as_mut().unwrap().headers = Some(vec![FaultHeaderEntry {
            name: "Content-Length".to_string(),
            value: "10".to_string(),
        }]);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_lowercase().contains("content-length"), "got: {err}");
    }
}
