//! Entry-level dye suite (traffic tagging) — success + failure outcomes.
//!
//! One rule shape, two value provenances: omitted `value` renders the
//! plugin-recorded outcome value; present `value` is an operator template on
//! the shared `${type:name}` engine (secretRef excluded — client-visible
//! output). See skills/01-architecture/02-gateway/entry-level-dye.md.

use super::conditions::PluginConditions;
use crate::gwapi_types::is_valid_header_token;
use crate::resources::common::request_template::{CompiledTemplate, SecretPolicy, TemplateAdmission, VariablePolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Header names whose overwrite would corrupt framing or bypass routing
/// machinery. Checked case-insensitively, both directions.
pub const DENIED_DYE_HEADER_NAMES: [&str; 7] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "te",
    "upgrade",
    "trailer",
];

/// Outcomes a dye rule can render. `Success` is the selection-success arm
/// (resolved-value provenance); the rest are failure behaviors (stable
/// `<type_prefix>:<behavior>:<cause>` provenance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DyeOutcome {
    Success,
    Fallback,
    Reject,
    FailOpen,
    FailClose,
}

/// The configured entry's execution stage, not the dye header direction.
/// Request entries may own later callbacks (ExtProc); only callbacks before
/// response-header commit can contribute to their response dye capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DyeStage {
    Request,
    UpstreamResponseFilter,
    UpstreamResponseBodyFilter,
    UpstreamResponse,
}

impl DyeOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            DyeOutcome::Success => "success",
            DyeOutcome::Fallback => "fallback",
            DyeOutcome::Reject => "reject",
            DyeOutcome::FailOpen => "fail-open",
            DyeOutcome::FailClose => "fail-close",
        }
    }

    pub fn is_failure(&self) -> bool {
        !matches!(self, DyeOutcome::Success)
    }
}

fn default_on() -> Vec<DyeOutcome> {
    vec![DyeOutcome::Success]
}

/// One dye rule: which outcomes render (`on`), under which request
/// conditions, into which header, with what value.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DyeRule {
    /// Header name (valid HTTP field-name token, not in the denied list).
    pub name: String,
    /// Outcomes this rule renders. Default: `[success]`.
    #[serde(default = "default_on")]
    pub on: Vec<DyeOutcome>,
    /// Operator-authored value template (`${type:name}` engine, secretRef
    /// excluded). Omitted => the plugin-recorded outcome value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Optional render gate; never gates the anti-spoof strip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,
}

/// Entry-level dye suite, sibling of `type`/`config`/`alias`/`conditions`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DyeSuite {
    /// Rules rendered onto the upstream request (success outcomes only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request: Vec<DyeRule>,
    /// Rules rendered onto the downstream response.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response: Vec<DyeRule>,
}

/// HTTP field-value byte filter: keeps tab and visible bytes, drops CR/LF/NUL/DEL.
pub fn sanitize_field_value(v: &str) -> String {
    v.chars()
        .filter(|c| *c == '\t' || (*c as u32 >= 0x20 && *c as u32 != 0x7f))
        .collect()
}

fn is_valid_field_value(value: &str) -> bool {
    value.bytes().all(|b| b == b'\t' || b >= 0x20 && b != 0x7f)
}

impl DyeSuite {
    /// Rule-local + capability validation. Called from entry-level validation
    /// (controller ingest and gateway compile share this function).
    ///
    /// `is_request_stage`: entry sits in a request-stage array (request-direction
    /// rules are meaningless elsewhere — headers already sent upstream).
    /// `capabilities`: renderable outcomes resolved for the entry's stage and config.
    /// `key_header_names`: the plugin's own header-typed key sources
    /// (request-direction collision with the anti-spoof strip).
    pub fn validation_error(
        &self,
        is_request_stage: bool,
        capabilities: &[DyeOutcome],
        key_header_names: &[&str],
    ) -> Option<String> {
        for (direction, rules) in [("request", &self.request), ("response", &self.response)] {
            for rule in rules {
                if !is_valid_header_token(&rule.name) {
                    return Some(format!("dye.{}: invalid header name '{}'", direction, rule.name));
                }
                if DENIED_DYE_HEADER_NAMES.contains(&rule.name.to_ascii_lowercase().as_str()) {
                    return Some(format!(
                        "dye.{}: header '{}' is not allowed as a dye header",
                        direction, rule.name
                    ));
                }
                if rule.on.is_empty() {
                    return Some(format!("dye.{}: '{}' has an empty on list", direction, rule.name));
                }
                let has_success = rule.on.contains(&DyeOutcome::Success);
                if has_success && rule.on.iter().any(DyeOutcome::is_failure) {
                    return Some(format!(
                        "dye.{}: '{}' must not mix success with failure outcomes in one rule \
                         (different value and write semantics); split into two rules",
                        direction, rule.name
                    ));
                }
                for o in &rule.on {
                    if !capabilities.contains(o) {
                        return Some(format!(
                            "dye.{}: '{}' declares outcome '{}' which this plugin cannot \
                             render in this stage and configuration (configured dye would never render)",
                            direction,
                            rule.name,
                            o.as_str()
                        ));
                    }
                }
                if let Some(value) = &rule.value {
                    if !is_valid_field_value(value) {
                        return Some(format!(
                            "dye.{}: invalid value bytes for header '{}'",
                            direction, rule.name
                        ));
                    }
                    match CompiledTemplate::compile(value) {
                        Err(e) => {
                            return Some(format!(
                                "dye.{}: '{}' value template error: {}",
                                direction, rule.name, e
                            ))
                        }
                        Ok(tpl) => {
                            if (TemplateAdmission {
                                variables: VariablePolicy::Request,
                                secrets: SecretPolicy::Deny,
                            })
                            .validate(&tpl)
                            .is_err()
                            {
                                return Some(format!(
                                    "dye.{}: '{}' value must not reference secretRef \
                                     (dye values are client-visible)",
                                    direction, rule.name
                                ));
                            }
                        }
                    }
                }
            }
        }
        for rule in &self.request {
            if !is_request_stage {
                return Some(format!(
                    "dye.request: '{}' — request-direction rules are only valid on \
                     request-stage entries",
                    rule.name
                ));
            }
            if rule.on.iter().any(DyeOutcome::is_failure) {
                return Some(format!(
                    "dye.request: '{}' — request-direction rules support only [success] in v1",
                    rule.name
                ));
            }
            if key_header_names.iter().any(|k| k.eq_ignore_ascii_case(&rule.name)) {
                return Some(format!(
                    "dye.request: header '{}' collides with the plugin's own header key \
                     source; the anti-spoof strip would remove the routing key",
                    rule.name
                ));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, on: &[DyeOutcome], value: Option<&str>) -> DyeRule {
        DyeRule {
            name: name.to_string(),
            on: on.to_vec(),
            value: value.map(|v| v.to_string()),
            conditions: None,
        }
    }

    const ALL_CAPS: &[DyeOutcome] = &[
        DyeOutcome::Success,
        DyeOutcome::Fallback,
        DyeOutcome::Reject,
        DyeOutcome::FailOpen,
        DyeOutcome::FailClose,
    ];

    fn suite(request: Vec<DyeRule>, response: Vec<DyeRule>) -> DyeSuite {
        DyeSuite { request, response }
    }

    #[test]
    fn on_defaults_to_success_in_serde() {
        let r: DyeRule = serde_yaml::from_str("name: X-Track").unwrap();
        assert_eq!(r.on, vec![DyeOutcome::Success]);
        // kebab-case round trip
        let r2: DyeRule = serde_yaml::from_str("name: X-O\non: [fail-open, fail-close]").unwrap();
        assert_eq!(r2.on, vec![DyeOutcome::FailOpen, DyeOutcome::FailClose]);
    }

    #[test]
    fn valid_minimal_suite_passes() {
        let s = suite(vec![rule("X-Track", &[DyeOutcome::Success], None)], vec![]);
        assert!(s.validation_error(true, ALL_CAPS, &[]).is_none());
    }

    #[test]
    fn invalid_and_denied_names_rejected() {
        let s = suite(vec![], vec![rule("Bad Name", &[DyeOutcome::Success], None)]);
        assert!(s
            .validation_error(true, ALL_CAPS, &[])
            .unwrap()
            .contains("invalid header name"));
        for denied in [
            "Host",
            "content-length",
            "Transfer-Encoding",
            "Connection",
            "TE",
            "Upgrade",
            "Trailer",
        ] {
            let s = suite(vec![], vec![rule(denied, &[DyeOutcome::Success], None)]);
            assert!(
                s.validation_error(true, ALL_CAPS, &[]).is_some(),
                "{denied} must be denied"
            );
        }
    }

    #[test]
    fn capability_check_rejects_undeclared_outcome() {
        let s = suite(vec![], vec![rule("X-O", &[DyeOutcome::FailOpen], None)]);
        let err = s.validation_error(true, &[DyeOutcome::Success], &[]).unwrap();
        assert!(err.contains("fail-open"), "{err}");
    }

    #[test]
    fn mixing_success_with_failure_rejected() {
        let s = suite(
            vec![],
            vec![rule("X-O", &[DyeOutcome::Success, DyeOutcome::Reject], None)],
        );
        assert!(s.validation_error(true, ALL_CAPS, &[]).unwrap().contains("mix"));
    }

    #[test]
    fn request_direction_success_only_and_request_stage_only() {
        let s = suite(vec![rule("X-O", &[DyeOutcome::FailOpen], None)], vec![]);
        assert!(s.validation_error(true, ALL_CAPS, &[]).is_some());
        // request-direction rules invalid on non-request-stage entries
        let s2 = suite(vec![rule("X-T", &[DyeOutcome::Success], None)], vec![]);
        assert!(s2.validation_error(false, ALL_CAPS, &[]).is_some());
    }

    #[test]
    fn key_collision_rejected_case_insensitively() {
        let s = suite(vec![rule("X-Backend-Target", &[DyeOutcome::Success], None)], vec![]);
        assert!(s
            .validation_error(true, ALL_CAPS, &["x-backend-target"])
            .unwrap()
            .contains("collide"));
    }

    #[test]
    fn template_values_validated() {
        // valid template
        let s = suite(
            vec![],
            vec![rule("X-D", &[DyeOutcome::FailOpen], Some("v ${header:X-Id}"))],
        );
        assert!(s.validation_error(true, ALL_CAPS, &[]).is_none());
        // parse error
        let s = suite(
            vec![],
            vec![rule("X-D", &[DyeOutcome::FailOpen], Some("bad ${header:X"))],
        );
        assert!(s.validation_error(true, ALL_CAPS, &[]).is_some());
        // secretRef rejected
        let s = suite(
            vec![],
            vec![rule("X-D", &[DyeOutcome::FailOpen], Some("${secretRef:ns/n:k}"))],
        );
        assert!(s.validation_error(true, ALL_CAPS, &[]).unwrap().contains("secretRef"));
        // control bytes in literal rejected
        let s = suite(
            vec![],
            vec![rule("X-D", &[DyeOutcome::FailOpen], Some("evil\r\nInjected: 1"))],
        );
        assert!(s.validation_error(true, ALL_CAPS, &[]).is_some());
    }

    #[test]
    fn sanitize_field_value_strips_control_bytes() {
        assert_eq!(sanitize_field_value("ok value"), "ok value");
        assert_eq!(sanitize_field_value("a\r\nb\u{0}c\u{7f}d"), "abcd");
        assert_eq!(sanitize_field_value("tab\tok"), "tab\tok");
    }
}
