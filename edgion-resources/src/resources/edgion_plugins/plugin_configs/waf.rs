//! Native WAF logical-plugin configuration.

use edgion_waf::seclang::{
    compile_with_includes, CompiledGeneration, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink,
    IncludeSource, Phase, PhaseDefaults, SemanticValidationContext,
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
        compile_with_includes(
            "inline-waf-rules",
            self.rules.as_bytes(),
            &mut NoIncludes,
            None,
            context,
            FrontendLimits::default(),
        )
        .map(|bundle| bundle.into_parts().0)
        .map_err(|error| format!("WAF rules are not executable: {error}"))
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
    fn blank_rules_are_rejected_by_the_shared_compile_entry() {
        let config = WafConfig {
            rules: " \n\t".to_string(),
            mode: WafMode::On,
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
            priority: 0,
        };
        assert_eq!(valid.validation_error(), None);

        let invalid = WafConfig {
            rules: "SecRule RESPONSE_BODY \"@contains private\" \"id:2,phase:4,deny\"\n".to_string(),
            mode: WafMode::DetectionOnly,
            priority: 0,
        };
        assert!(invalid.validation_error().is_some());
    }

    #[test]
    fn validation_errors_do_not_render_rule_content() {
        let config = WafConfig {
            rules: "SecRule private-(".to_string(),
            mode: WafMode::On,
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
                priority: 0,
            };
            assert_eq!(config.validation_error(), None, "fixture must compile: {rules}");
        }
    }
}
