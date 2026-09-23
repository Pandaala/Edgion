//! Native WAF logical-plugin configuration.

use edgion_waf::seclang::{
    compile_with_includes, CompiledGeneration, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink,
    IncludeSource, Phase, PhaseDefaults, SemanticValidationContext, DEFAULT_REQUEST_BODY_PREFIX_BYTES,
    MAX_REQUEST_BODY_PREFIX_BYTES,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::PluginValidation;

/// Disruptive-action behavior for one WAF generation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum WafMode {
    /// Apply disruptive rule actions.
    #[default]
    On,
    /// Record findings without disrupting traffic.
    DetectionOnly,
}

/// Request payload range inspected before forwarding.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum WafRequestBodyInspection {
    #[default]
    Prefix,
    Full,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafRequestBodyConfig {
    #[serde(default)]
    pub inspection: WafRequestBodyInspection,
    /// Prefix window, default 64 KiB, at most 1 MiB. Only valid in prefix mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_size: Option<String>,
}

impl WafRequestBodyConfig {
    /// Resolve once during configuration validation and plugin construction.
    pub fn prefix_size_bytes(&self) -> Result<usize, String> {
        if self.inspection == WafRequestBodyInspection::Full && self.prefix_size.is_some() {
            return Err("requestBody.prefixSize is only valid with prefix inspection".to_string());
        }
        let Some(value) = self.prefix_size.as_deref() else {
            return Ok(DEFAULT_REQUEST_BODY_PREFIX_BYTES);
        };
        let bytes = crate::resources::common::parse_byte_size(value)
            .ok_or("requestBody.prefixSize must be a valid byte size")?;
        if bytes == 0 || bytes > MAX_REQUEST_BODY_PREFIX_BYTES as u64 {
            return Err("requestBody.prefixSize must be between 1 byte and 1 MiB".to_string());
        }
        Ok(bytes as usize)
    }
}

/// One logical WAF configuration spanning all supported transaction phases.
#[derive(Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafConfig {
    /// Inline SecLang source. Includes and filesystem paths are not available.
    #[serde(serialize_with = "crate::redact::redact_required::serialize")]
    pub rules: String,

    /// Whether disruptive actions are enforced.
    #[serde(default)]
    pub mode: WafMode,

    /// Prefix inspection uses a bounded memory window; full uses the shared body cache.
    #[serde(default)]
    pub request_body: WafRequestBodyConfig,

    /// Request-wide winner priority when multiple scopes attach a WAF.
    /// Higher values win; equal values prefer Route, then Gateway, then Global.
    #[serde(default)]
    pub priority: u16,
}

impl std::fmt::Debug for WafConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WafConfig")
            .field("rules", &"[redacted]")
            .field("mode", &self.mode)
            .field("request_body", &self.request_body)
            .field("priority", &self.priority)
            .finish()
    }
}

struct NoIncludes;

impl IncludeSource for NoIncludes {
    fn load(&mut self, _request: IncludeRequest<'_>, _sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

impl WafConfig {
    /// Compile the exact strict generation used by both validation and runtime.
    pub fn compile_generation(&self) -> Result<CompiledGeneration, String> {
        self.request_body.prefix_size_bytes()?;
        if self.rules.trim().is_empty() {
            return Err("rules must not be empty".to_string());
        }
        let defaults = [
            Phase::RequestHeaders,
            Phase::RequestBody,
            Phase::ResponseHeaders,
            Phase::ResponseBody,
            Phase::Logging,
        ]
        .map(|phase| PhaseDefaults::pass(phase, true, true));
        let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults)
            .map_err(|error| format!("WAF defaults are invalid: {error}"))?;
        let generation = compile_with_includes(
            "inline-waf-rules",
            self.rules.as_bytes(),
            &mut NoIncludes,
            None,
            context,
            FrontendLimits::default(),
        )
        .map(|bundle| bundle.into_parts().0)
        .map_err(|error| format!("WAF rules are not executable: {error}"))?;
        if self.request_body.inspection == WafRequestBodyInspection::Prefix {
            generation.validate_request_body_prefix().map_err(str::to_string)?;
        }
        Ok(generation)
    }
}

impl PluginValidation for WafConfig {
    fn validation_error(&self) -> Option<String> {
        self.compile_generation().err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_size_is_bounded_and_round_trips_without_changing_full_defaults() {
        for (size, bytes) in [("1", 1), ("16KiB", 16384), ("100k", 102400), ("1m", 1048576)] {
            let config: WafConfig = serde_json::from_value(serde_json::json!({
                "rules": "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny\"",
                "requestBody": {"prefixSize": size}
            }))
            .unwrap();
            assert_eq!(config.request_body.prefix_size_bytes().unwrap(), bytes);
            assert_eq!(config.validation_error(), None);
            let encoded = serde_json::to_value(&config).unwrap();
            assert_eq!(encoded["requestBody"]["prefixSize"], size);
            let restored: WafConfig = serde_json::from_value(encoded).unwrap();
            assert_eq!(restored.request_body.prefix_size_bytes().unwrap(), bytes);
        }
        for size in ["0", "1048577", "2m", "-1", "invalid"] {
            let config = WafConfig {
                rules: "SecAction \"id:1,phase:1,pass\"".to_string(),
                request_body: WafRequestBodyConfig {
                    prefix_size: Some(size.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(config.validation_error().is_some(), "{size}");
        }
        let full = WafRequestBodyConfig {
            inspection: WafRequestBodyInspection::Full,
            ..Default::default()
        };
        assert!(full.prefix_size_bytes().is_ok());
        assert!(WafRequestBodyConfig {
            prefix_size: Some("1m".to_string()),
            ..full
        }
        .prefix_size_bytes()
        .is_err());
    }

    #[test]
    fn prefix_defaults_and_full_validation_share_the_same_compiler() {
        let mut config: WafConfig = serde_json::from_value(serde_json::json!({
            "rules": "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny\""
        }))
        .unwrap();
        assert_eq!(config.request_body.inspection, WafRequestBodyInspection::Prefix);
        assert_eq!(config.validation_error(), None);
        config.request_body.inspection = WafRequestBodyInspection::Full;
        let serialized = serde_json::to_value(&config).unwrap();
        assert_eq!(serialized["requestBody"]["inspection"], "full");
        let restored: WafConfig = serde_json::from_value(serialized).unwrap();
        assert_eq!(restored.request_body.inspection, WafRequestBodyInspection::Full);
        assert_eq!(restored.validation_error(), None);
    }

    #[test]
    fn full_capture_dependency_includes_later_phases_but_not_query_only_headers() {
        for (rules, requires_body) in [
            ("SecRule QUERY_STRING \"@contains query\" \"id:1,phase:1,deny\"", false),
            (
                "SecRule REQUEST_HEADERS:X-Test \"@contains test\" \"id:1,phase:2,deny\"",
                false,
            ),
            ("SecRule REQUEST_BODY \"@contains test\" \"id:1,phase:3,deny\"", true),
            ("SecRule ARGS:item \"@contains test\" \"id:1,phase:5,pass\"", true),
            ("SecRule RESPONSE_STATUS \"@eq 200\" \"id:1,phase:3,pass\"", false),
            (
                "SecRule REQUEST_HEADERS:X-Test \"@streq %{ARGS.item}\" \"id:1,phase:3,pass\"",
                true,
            ),
            ("SecAction \"id:1,phase:3,pass,setvar:tx.item=%{ARGS.item}\"", true),
            (
                "SecRule REQUEST_HEADERS:X-Test \"@streq %{REQUEST_HEADERS.X-Other}\" \"id:1,phase:3,pass\"",
                false,
            ),
        ] {
            let config = WafConfig {
                rules: rules.to_owned(),
                request_body: WafRequestBodyConfig {
                    inspection: WafRequestBodyInspection::Full,
                    ..Default::default()
                },
                ..WafConfig::default()
            };
            let generation = config.compile_generation().unwrap();
            assert_eq!(generation.requires_complete_request_body(), requires_body, "{rules}");
        }
    }

    #[test]
    fn prefix_accepts_body_independent_phase_two_rules() {
        for target in [
            "REQUEST_HEADERS:X-Test",
            "REQUEST_URI",
            "ARGS_GET:item",
            "ARGS_GET_NAMES",
        ] {
            let config = WafConfig {
                rules: format!("SecRule {target} \"!@contains blocked\" \"id:1,phase:2,deny\""),
                ..Default::default()
            };
            let generation = config.compile_generation().unwrap();
            assert!(!generation.requires_complete_request_body(), "{target}");
            assert!(
                !generation.request_body_execution_summary().requires_request_body(),
                "{target}"
            );
            assert_eq!(
                generation.request_body_execution_summary().streaming_raw_rule_units(),
                1
            );
        }
    }

    #[test]
    fn prefix_rejects_whole_body_and_control_dependencies_across_generation() {
        for rules in [
            "SecRule REQUEST_BODY \"!@contains forbidden\" \"id:1,phase:2,deny\"",
            "SecRule REQUEST_BODY \"@endsWith forbidden\" \"id:1,phase:2,deny\"",
            "SecRule ARGS:item \"@contains forbidden\" \"id:1,phase:2,deny\"",
            "SecRule ARGS_NAMES \"@contains forbidden\" \"id:1,phase:2,deny\"",
            "SecRule REQBODY_PROCESSOR \"@streq JSON\" \"id:1,phase:2,deny\"",
            "SecRule REQUEST_BODY_LENGTH \"@gt 10\" \"id:1,phase:2,deny\"",
            "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,t:base64Decode\"",
            "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:3,deny\"",
            "SecRule ARGS:item \"@contains forbidden\" \"id:1,phase:5,pass\"",
            "SecAction \"id:1,phase:1,pass,setvar:tx.flag=1\"",
            "SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,pass,skip:1\"\nSecRule REQUEST_BODY \"@contains forbidden\" \"id:2,phase:2,deny\"",
            "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,chain\"\nSecRule REQUEST_BODY \"@contains secret\" \"t:none\"",
        ] {
            let mut config = WafConfig { rules: rules.to_owned(), ..WafConfig::default() };
            let error = config.validation_error().expect("prefix must reject unsupported semantics");
            assert!(error.contains("inspection prefix"), "{error}");
            config.request_body.inspection = WafRequestBodyInspection::Full;
            assert_eq!(config.validation_error(), None, "full fixture must remain executable: {rules}");
        }
    }

    #[test]
    fn blank_rules_are_rejected_by_the_shared_compile_entry() {
        let config = WafConfig {
            rules: " \n\t".to_string(),
            mode: WafMode::On,
            request_body: WafRequestBodyConfig::default(),
            priority: 0,
        };

        assert_eq!(
            config.compile_generation().err().as_deref(),
            Some("rules must not be empty")
        );
        assert_eq!(config.validation_error().as_deref(), Some("rules must not be empty"));
    }

    #[test]
    fn validation_compiles_supported_rules_without_a_separate_prepare_call() {
        let valid = WafConfig {
            rules: "SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,deny\"\n".to_string(),
            mode: WafMode::On,
            request_body: WafRequestBodyConfig::default(),
            priority: 0,
        };
        assert_eq!(valid.validation_error(), None);

        let invalid = WafConfig {
            rules: "SecRule RESPONSE_BODY \"@contains private\" \"id:2,phase:4,deny\"\n".to_string(),
            mode: WafMode::DetectionOnly,
            request_body: WafRequestBodyConfig::default(),
            priority: 0,
        };
        assert!(invalid.validation_error().is_some());
    }

    #[test]
    fn validation_errors_do_not_render_rule_content() {
        let config = WafConfig {
            rules: "SecRule private-(".to_string(),
            mode: WafMode::On,
            request_body: WafRequestBodyConfig::default(),
            priority: 0,
        };
        let error = config.validation_error().expect("invalid source must be rejected");
        assert!(!error.contains("private"));
    }

    #[test]
    fn rules_round_trip_for_transport_but_redact_from_admin_and_debug_output() {
        let config = WafConfig {
            rules: "SecAction \"id:1,phase:1,pass,msg:'private'\"".to_string(),
            mode: WafMode::On,
            request_body: WafRequestBodyConfig::default(),
            priority: 17,
        };

        let transport = serde_json::to_value(&config).expect("transport serialization succeeds");
        assert!(transport["rules"]
            .as_str()
            .is_some_and(|rules| rules.contains("private")));

        let admin = {
            let _guard = crate::redact::AdminRedactGuard::new();
            serde_json::to_value(&config).expect("admin serialization succeeds")
        };
        assert_eq!(admin["rules"], "[redacted]");
        assert_eq!(admin["priority"], 17);
        assert!(!format!("{config:?}").contains("private"));
    }

    #[test]
    fn priority_defaults_to_zero_and_rejects_out_of_range_values() {
        let base = r#"{"rules":"SecAction \"id:1,phase:1,pass\""}"#;
        let parsed: WafConfig = serde_json::from_str(base).expect("missing priority uses its default");
        assert_eq!(parsed.priority, 0);

        let maximum: WafConfig =
            serde_json::from_str(r#"{"rules":"SecAction \"id:1,phase:1,pass\"","priority":65535}"#)
                .expect("u16 maximum is accepted");
        assert_eq!(maximum.priority, u16::MAX);
        assert!(
            serde_json::from_str::<WafConfig>(r#"{"rules":"SecAction \"id:1,phase:1,pass\"","priority":-1}"#).is_err()
        );
        assert!(
            serde_json::from_str::<WafConfig>(r#"{"rules":"SecAction \"id:1,phase:1,pass\"","priority":65536}"#)
                .is_err()
        );
    }

    #[test]
    fn dynamic_integration_generations_compile_with_the_production_profile() {
        let generations = [
            concat!(
                "SecRule REQUEST_HEADERS:X-Waf-Version \"!@streq v1\" ",
                "\"id:1001,phase:1,deny,status:403\"\n",
                "SecRule REQUEST_BODY \"@contains blocked\" ",
                "\"id:1002,phase:2,deny,status:406\"\n"
            ),
            "SecRule REQUEST_HEADERS:X-Waf-Version \"!@streq v2\" \"id:2001,phase:1,deny,status:451\"\n",
            "SecRule REQUEST_HEADERS:X-Waf-Version \"!@streq v3\" \"id:3001,phase:1,deny,status:429\"\n",
        ];

        for rules in generations {
            let config = WafConfig {
                rules: rules.to_string(),
                mode: WafMode::On,
                request_body: WafRequestBodyConfig::default(),
                priority: 0,
            };
            assert_eq!(config.validation_error(), None, "fixture must compile: {rules}");
        }
    }
}
