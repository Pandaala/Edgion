//! Request-side AI prompt guard configuration.
//!
//! `AiGuard` selects bounded canonical request text spans by verified role and
//! content type, evaluates admission-compiled DFA-safe local rules over them,
//! and optionally consults one bounded external evaluator. It resolves to
//! exactly one client-visible outcome: allow, in-place mask, fixed error
//! reject, or a structured content-filter refusal.
//!
//! The entry must be ordered before `AiProxy`: the provider request body is
//! derived from the canonical body `AiProxy` captures, so a mask published
//! after `AiProxy` has run would never reach the provider.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::validate_restricted_template;
use crate::resources::common::{CustomSource, ExternalHttpConnection, OutboundRequestSpec, ResolvedSecrets};
use crate::resources::edgion_plugins::PluginValidation;

/// Absolute compiled-rule count ceiling.
pub const AI_GUARD_MAX_RULES: usize = 64;
/// Absolute source-pattern byte ceiling for one rule.
pub const AI_GUARD_MAX_PATTERN_BYTES: usize = 4096;
/// Absolute replacement-text byte ceiling for one rule.
pub const AI_GUARD_MAX_RULE_REPLACEMENT_BYTES: usize = 1024;
/// Absolute selected-span count ceiling.
///
/// Bounded well below `MAX_BODY_SEGMENTS` (4096): splicing `k` disjoint edits
/// yields up to `2k + 1` segments on a body that may already be segmented by a
/// disk spill.
pub const AI_GUARD_MAX_SPANS: usize = 512;
/// Absolute per-span materialized-text ceiling.
pub const AI_GUARD_MAX_SPAN_BYTES: usize = 8 * 1024 * 1024;
/// Absolute aggregate materialized-span ceiling.
pub const AI_GUARD_MAX_TOTAL_SPAN_BYTES: usize = 8 * 1024 * 1024;
/// Absolute aggregate replacement-text ceiling.
pub const AI_GUARD_MAX_REPLACEMENT_BYTES: usize = 1024 * 1024;
/// Absolute serialized evaluator-request ceiling.
pub const AI_GUARD_MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// Absolute per-snapshot in-flight evaluator-call ceiling.
pub const AI_GUARD_MAX_CONCURRENT: usize = 1024;
/// Compiled-regex program and DFA ceiling, matching the existing
/// `edgion-resources` precedent in `json_schema_validation`.
pub const AI_GUARD_REGEX_SIZE_LIMIT: usize = 256 * 1024;

fn default_max_spans() -> usize {
    256
}

fn default_max_replacement_bytes() -> usize {
    64 * 1024
}

fn default_max_request_bytes() -> usize {
    AI_GUARD_MAX_REQUEST_BYTES
}

fn default_max_concurrent() -> usize {
    64
}

/// Canonical message role a rule may be restricted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AiGuardRole {
    System,
    User,
    Assistant,
    Tool,
    /// Accepted for completeness. Anthropic backends reject `developer`
    /// messages outright, so a `developer`-scoped rule never matches there.
    Developer,
}

/// Builtin heuristic detector.
///
/// These are best-effort pattern detectors, not a compliance control: they both
/// miss and over-match. The operator owns the rule set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AiGuardPiiDetector {
    Email,
    IpAddress,
    CreditCard,
    UsSocialSecurityNumber,
    AwsAccessKeyId,
}

impl AiGuardPiiDetector {
    /// Source pattern for this detector.
    pub fn pattern(self) -> &'static str {
        match self {
            Self::Email => r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}",
            Self::IpAddress => r"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b",
            Self::CreditCard => r"\b(?:[0-9][ \-]?){13,19}\b",
            Self::UsSocialSecurityNumber => r"\b[0-9]{3}-[0-9]{2}-[0-9]{4}\b",
            Self::AwsAccessKeyId => r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        }
    }
}

/// What a rule matches against a span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AiGuardMatcher {
    /// Operator-supplied pattern, compiled under the DFA size limits.
    Regex { pattern: String },
    /// Builtin heuristic detector.
    Pii { detector: AiGuardPiiDetector },
}

/// What happens when a rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AiGuardAction {
    /// Count the match and continue, without masking or refusing.
    ///
    /// Use it to stage a rule before enforcing it: matches are reported as a
    /// bounded count in the access log. The rule name and the matched text are
    /// never recorded.
    Allow,
    /// Replace every match inside the span with `replacement`.
    Mask,
    /// Refuse the request.
    #[default]
    Reject,
}

/// How a refusal is rendered to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AiGuardRejectionMode {
    /// Fixed OpenAI-compatible error envelope, HTTP 403.
    #[default]
    Error,
    /// HTTP 200 carrying `finish_reason: "content_filter"`, so agent chains
    /// degrade instead of erroring.
    ContentFilter,
}

/// One compiled-at-admission guard rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiGuardRule {
    /// Stable operator-facing identifier. Never emitted to a client or a log.
    pub name: String,

    /// What this rule matches.
    #[serde(rename = "match")]
    pub matcher: AiGuardMatcher,

    /// What happens on a match.
    #[serde(default)]
    pub action: AiGuardAction,

    /// Restrict the rule to these roles. Absent means every selected span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<AiGuardRole>>,

    /// Replacement text for `action: mask`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

/// One bounded external evaluator. Absent means no prompt-derived content ever
/// leaves the gateway.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AiGuardEvaluator {
    /// Connection target, TLS, static timeout, health/rate gates, and bounded
    /// response consumption. Retry, dynamic timeout, and custom success rules
    /// are rejected by validation.
    #[serde(flatten)]
    pub conn: ExternalHttpConnection,

    /// Restricted outbound request metadata. Only a path and custom headers are
    /// accepted; the method, content type, and body are fixed.
    pub request: OutboundRequestSpec,

    /// Inclusive cap for the complete serialized decision request.
    #[serde(default = "default_max_request_bytes")]
    #[schemars(range(min = 1, max = AI_GUARD_MAX_REQUEST_BYTES))]
    pub max_request_bytes: usize,

    /// Maximum in-flight decisions sharing this immutable snapshot.
    #[serde(default = "default_max_concurrent")]
    #[schemars(range(min = 1, max = AI_GUARD_MAX_CONCURRENT))]
    pub max_concurrent: usize,

    /// Controller-resolved `${secretRef:...}` values carried to the gateway.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_secrets: Option<ResolvedSecrets>,
}

impl Default for AiGuardEvaluator {
    fn default() -> Self {
        Self {
            conn: ExternalHttpConnection::default(),
            request: OutboundRequestSpec::default(),
            max_request_bytes: default_max_request_bytes(),
            max_concurrent: default_max_concurrent(),
            resolved_secrets: None,
        }
    }
}

/// Operator-facing configuration for request-side prompt guarding.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiGuardConfig {
    /// Local rules, evaluated in declared order over the sorted span snapshot.
    #[serde(default)]
    pub rules: Vec<AiGuardRule>,

    /// How a refusal is rendered.
    #[serde(default)]
    pub rejection_mode: AiGuardRejectionMode,

    /// Inclusive ceiling on selected spans.
    #[serde(default = "default_max_spans")]
    #[schemars(range(min = 1, max = AI_GUARD_MAX_SPANS))]
    pub max_spans: usize,

    /// Inclusive per-span materialized-text ceiling. Absent means the resolved
    /// request-body cap.
    ///
    /// There is deliberately no small default: a long-context message is
    /// ordinary LLM traffic, and a guard bound smaller than what `AiProxy`
    /// accepts would turn it into a content-policy refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = AI_GUARD_MAX_SPAN_BYTES))]
    pub max_span_bytes: Option<usize>,

    /// Inclusive aggregate materialized-span ceiling. Absent means the resolved
    /// request-body cap, so the guard never rejects a request `AiProxy` accepts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = AI_GUARD_MAX_TOTAL_SPAN_BYTES))]
    pub max_total_span_bytes: Option<usize>,

    /// Inclusive aggregate replacement-text ceiling.
    #[serde(default = "default_max_replacement_bytes")]
    #[schemars(range(min = 0, max = AI_GUARD_MAX_REPLACEMENT_BYTES))]
    pub max_replacement_bytes: usize,

    /// Proceed unguarded after a dependency, bounds, or parse failure. A valid
    /// policy reject is never softened by this setting, and neither is an
    /// ordering fault.
    #[serde(default)]
    pub fail_open: bool,

    /// One bounded external evaluator. Configuring it is the operator's
    /// explicit consent to send the selected bounded spans off-box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator: Option<AiGuardEvaluator>,
}

impl Default for AiGuardConfig {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            rejection_mode: AiGuardRejectionMode::default(),
            max_spans: default_max_spans(),
            max_span_bytes: None,
            max_total_span_bytes: None,
            max_replacement_bytes: default_max_replacement_bytes(),
            fail_open: false,
            evaluator: None,
        }
    }
}

/// Compile one guard pattern under the shared DFA size limits.
///
/// The `regex` crate is a finite-automata engine with no backreferences and no
/// look-around, so matching is linear in the input; the size limits bound the
/// compiled program and DFA. This is the single compilation entry point, used
/// both by admission (to report `Accepted=False`) and by the gateway
/// constructor, so the two can never disagree.
pub fn compile_guard_pattern(pattern: &str) -> Result<regex::Regex, String> {
    regex::RegexBuilder::new(pattern)
        .size_limit(AI_GUARD_REGEX_SIZE_LIMIT)
        .dfa_size_limit(AI_GUARD_REGEX_SIZE_LIMIT)
        .build()
        .map_err(|_| format!("pattern is invalid or exceeds the {AI_GUARD_REGEX_SIZE_LIMIT}-byte compiled limit"))
}

impl AiGuardConfig {
    /// Return the first admission-time configuration error.
    pub fn validation_error(&self) -> Option<String> {
        if self.rules.is_empty() && self.evaluator.is_none() {
            return Some("at least one rule or an evaluator must be configured".to_string());
        }
        if self.rules.len() > AI_GUARD_MAX_RULES {
            return Some(format!("rules must contain at most {AI_GUARD_MAX_RULES} entries"));
        }
        for (index, rule) in self.rules.iter().enumerate() {
            if let Some(error) = rule.validation_error() {
                return Some(format!("rules[{index}]: {error}"));
            }
            if self.rules[..index].iter().any(|prior| prior.name == rule.name) {
                return Some(format!("rules[{index}]: duplicate rule name"));
            }
        }
        if !(1..=AI_GUARD_MAX_SPANS).contains(&self.max_spans) {
            return Some(format!("maxSpans must be between 1 and {AI_GUARD_MAX_SPANS}"));
        }
        if let Some(span) = self.max_span_bytes {
            if !(1..=AI_GUARD_MAX_SPAN_BYTES).contains(&span) {
                return Some(format!("maxSpanBytes must be between 1 and {AI_GUARD_MAX_SPAN_BYTES}"));
            }
        }
        if let Some(total) = self.max_total_span_bytes {
            if !(1..=AI_GUARD_MAX_TOTAL_SPAN_BYTES).contains(&total) {
                return Some(format!(
                    "maxTotalSpanBytes must be between 1 and {AI_GUARD_MAX_TOTAL_SPAN_BYTES}"
                ));
            }
            if self.max_span_bytes.is_some_and(|span| total < span) {
                return Some("maxTotalSpanBytes must be at least maxSpanBytes".to_string());
            }
        }
        if self.max_replacement_bytes > AI_GUARD_MAX_REPLACEMENT_BYTES {
            return Some(format!(
                "maxReplacementBytes must be at most {AI_GUARD_MAX_REPLACEMENT_BYTES}"
            ));
        }
        if let Some(evaluator) = self.evaluator.as_ref() {
            if let Some(error) = evaluator.validation_error() {
                return Some(format!("evaluator: {error}"));
            }
        }
        None
    }
}

impl AiGuardRule {
    fn validation_error(&self) -> Option<String> {
        if self.name.is_empty() || self.name.len() > 64 {
            return Some("name must be between 1 and 64 bytes".to_string());
        }
        match &self.matcher {
            AiGuardMatcher::Regex { pattern } => {
                if pattern.is_empty() {
                    return Some("match.pattern must not be empty".to_string());
                }
                if pattern.len() > AI_GUARD_MAX_PATTERN_BYTES {
                    return Some(format!(
                        "match.pattern must be at most {AI_GUARD_MAX_PATTERN_BYTES} bytes"
                    ));
                }
                // Trial-compile so a bad pattern fails at ingest rather than in
                // the hot path, and so admission and the gateway constructor
                // report the same error.
                if let Err(error) = compile_guard_pattern(pattern) {
                    return Some(format!("match.pattern {error}"));
                }
            }
            AiGuardMatcher::Pii { .. } => {}
        }
        if let Some(roles) = self.roles.as_ref() {
            if roles.is_empty() {
                return Some("roles must not be empty when present".to_string());
            }
        }
        match self.action {
            AiGuardAction::Mask => {
                let Some(replacement) = self.replacement.as_ref() else {
                    return Some("replacement is required for action mask".to_string());
                };
                if replacement.len() > AI_GUARD_MAX_RULE_REPLACEMENT_BYTES {
                    return Some(format!(
                        "replacement must be at most {AI_GUARD_MAX_RULE_REPLACEMENT_BYTES} bytes"
                    ));
                }
            }
            AiGuardAction::Allow | AiGuardAction::Reject => {
                if self.replacement.is_some() {
                    return Some("replacement is only valid for action mask".to_string());
                }
            }
        }
        None
    }
}

impl AiGuardEvaluator {
    fn validation_error(&self) -> Option<String> {
        if let Some(error) = crate::resources::common::validate_connection_target(&self.conn) {
            return Some(format!("target invalid: {error}"));
        }
        if self.conn.retry.is_some() {
            return Some("retry is not supported; a committed decision POST must never be replayed".to_string());
        }
        if self.conn.timeout_ms_template.is_some() {
            return Some("timeoutMsTemplate is not supported; use static timeoutMs".to_string());
        }
        if self.conn.success.is_some() {
            return Some("success is not configurable for guard decisions".to_string());
        }
        if let Some(error) = self.request.validation_error() {
            return Some(error);
        }
        if self.request.method.is_some() {
            return Some("request.method is fixed to POST".to_string());
        }
        if self.request.body.is_some() {
            return Some("request.body is fixed to the decision envelope".to_string());
        }
        if self.request.args.forward_all || !self.request.args.origin.is_empty() || !self.request.args.custom.is_empty()
        {
            return Some("request.args is not supported".to_string());
        }
        if self.request.cookies.forward_all
            || !self.request.cookies.origin.is_empty()
            || !self.request.cookies.custom.is_empty()
        {
            return Some("request.cookies is not supported".to_string());
        }
        if self.request.headers.forward_all || !self.request.headers.origin.is_empty() {
            return Some("request.headers may contain only custom fields".to_string());
        }
        let Some(path) = self.request.path.as_ref() else {
            return Some("request.path is required".to_string());
        };
        if path.allow_override {
            return Some("request.path.allowOverride is not supported".to_string());
        }
        if let Some(error) = validate_restricted_template(&path.template, "request.path.template") {
            return Some(error);
        }
        for (index, header) in self.request.headers.custom.iter().enumerate() {
            if header.name.eq_ignore_ascii_case("content-type") {
                return Some("request.headers must not configure Content-Type; application/json is fixed".to_string());
            }
            if let CustomSource::Template(template) = &header.source {
                if let Some(error) =
                    validate_restricted_template(template, &format!("request.headers.custom[{index}].template"))
                {
                    return Some(error);
                }
            }
        }
        if !(1..=AI_GUARD_MAX_REQUEST_BYTES).contains(&self.max_request_bytes) {
            return Some(format!(
                "maxRequestBytes must be between 1 and {AI_GUARD_MAX_REQUEST_BYTES}"
            ));
        }
        if !(1..=AI_GUARD_MAX_CONCURRENT).contains(&self.max_concurrent) {
            return Some(format!("maxConcurrent must be between 1 and {AI_GUARD_MAX_CONCURRENT}"));
        }
        None
    }
}

impl PluginValidation for AiGuardConfig {
    fn validation_error(&self) -> Option<String> {
        AiGuardConfig::validation_error(self)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/ai_guard_test.rs"]
mod tests;
