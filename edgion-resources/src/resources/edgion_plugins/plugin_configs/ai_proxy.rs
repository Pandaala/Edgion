//! Configuration for the request-only AI provider proxy plugin.

use std::collections::HashSet;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::KeyGet;
use crate::duration::parse_duration;
use crate::resources::edgion_backend::AI_MODEL_NAME_MAX_BYTES;
use crate::resources::edgion_plugins::PluginValidation;

use super::rate_limit::OnMissingKey;
use super::rate_limit_redis::OnRedisFailure;

pub const AI_QUOTA_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const AI_MODEL_ROUTE_MAX_COUNT: usize = 64;
pub const AI_MODEL_ROUTE_MAX_HEADERS: usize = 8;
pub const AI_MODEL_ROUTE_MAX_CANDIDATES: usize = 16;
pub const AI_MODEL_ROUTE_HEADER_VALUE_MAX_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OnUnknownModel {
    #[default]
    Reject,
    Passthrough,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AiBackendSelection {
    #[default]
    Weighted,
    Priority,
    LowestLatency,
    LowestUsage,
    CostWeighted,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiBackendSelectionConfig {
    #[serde(rename = "type")]
    pub selection_type: AiBackendSelection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiProxyConfig {
    /// Policy used to select among the backends attached by the route.
    #[serde(default)]
    pub backend_selection: AiBackendSelectionConfig,
    #[serde(default = "default_inject_usage_tracking")]
    pub inject_usage_tracking: bool,
    #[serde(default)]
    pub on_unknown_model: OnUnknownModel,
    /// Ordered model/header virtualization rules. Specificity, not declaration
    /// order, selects the one winning rule.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub model_routes: Vec<AiModelRoute>,
    /// Optional cluster-wide token quota. Omission has no Redis or task cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_quota: Option<AiTokenQuotaConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiModelRoute {
    #[serde(rename = "match")]
    pub match_: AiModelRouteMatch,
    pub action: AiModelRouteAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiModelRouteMatch {
    #[schemars(length(min = 1, max = 512))]
    pub model: String,
    #[serde(default)]
    #[schemars(length(max = 8))]
    pub headers: Vec<AiModelHeaderMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiModelHeaderMatch {
    #[schemars(
        length(min = 1, max = crate::gwapi_types::HEADER_NAME_MAX_LEN),
        pattern(crate::gwapi_types::HEADER_NAME_PATTERN)
    )]
    pub name: String,
    #[schemars(length(min = 1, max = 1024))]
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiModelRouteAction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 512))]
    pub rewrite: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 16))]
    pub candidates: Option<Vec<AiModelRouteCandidate>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiModelRouteCandidate {
    pub backend: AiBackendSlotSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0))]
    pub weight: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiBackendSlotSelector {
    pub group: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(default)]
    #[schemars(range(max = 15))]
    pub occurrence: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiModelPattern {
    Exact(String),
    Prefix(String),
}

impl AiModelPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.trim().is_empty() || value.len() > AI_MODEL_NAME_MAX_BYTES {
            return Err(format!(
                "model pattern must contain 1..={AI_MODEL_NAME_MAX_BYTES} UTF-8 bytes"
            ));
        }
        let stars = value.bytes().filter(|byte| *byte == b'*').count();
        match stars {
            0 => Ok(Self::Exact(value.to_string())),
            1 if value.ends_with('*') && value.len() > 1 => Ok(Self::Prefix(value[..value.len() - 1].to_string())),
            _ => Err("model pattern must use at most one trailing '*' after a non-empty prefix".to_string()),
        }
    }

    pub fn matches(&self, value: &str) -> bool {
        match self {
            Self::Exact(exact) => value == exact,
            Self::Prefix(prefix) => value.starts_with(prefix),
        }
    }

    pub fn exactness(&self) -> u8 {
        u8::from(matches!(self, Self::Exact(_)))
    }

    pub fn literal_len(&self) -> usize {
        match self {
            Self::Exact(value) | Self::Prefix(value) => value.len(),
        }
    }

    fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(left), Self::Exact(right)) => left == right,
            (Self::Exact(exact), Self::Prefix(prefix)) | (Self::Prefix(prefix), Self::Exact(exact)) => {
                exact.starts_with(prefix)
            }
            (Self::Prefix(left), Self::Prefix(right)) => left.starts_with(right) || right.starts_with(left),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiTokenQuotaConfig {
    /// Redis LinkSys reference in `namespace/name` form.
    pub redis_ref: String,
    /// Identity dimensions. An explicitly empty list means one global bucket.
    pub key: Vec<KeyGet>,
    /// Missing identity fails closed by default.
    #[serde(default = "default_on_missing_key_deny")]
    pub on_missing_key: OnMissingKey,
    /// Redis dependency failure fails closed before upstream dispatch.
    #[serde(default = "default_on_redis_failure_deny")]
    pub on_redis_failure: OnRedisFailure,
    /// Independent token limits; every limit is charged and any exhausted limit
    /// rejects a subsequent request.
    pub limits: Vec<AiTokenQuotaLimit>,
    /// Action applied when any configured token limit is exhausted.
    #[serde(default)]
    pub on_exceeded: AiTokenQuotaOnExceeded,
    /// Output units accumulated before an immediate asynchronous flush.
    #[serde(default = "default_flush_tokens")]
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
    pub flush_tokens: u64,
    /// Maximum time unflushed output units may remain in memory.
    #[serde(default = "default_flush_interval")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub flush_interval: String,
    /// Hard bound for admission, batch, and final reconciliation Redis calls.
    #[serde(default = "default_request_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub request_timeout: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiTokenQuotaLimit {
    /// Whether this limit is shared across all models or partitioned by the
    /// client-requested logical model name.
    #[serde(default)]
    pub scope: AiTokenQuotaScope,
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
    pub tokens: u64,
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub window: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AiTokenQuotaScope {
    /// One identity bucket shared by every model reached through this AiProxy.
    #[default]
    Identity,
    /// A separate bucket for each requested logical model name and identity.
    /// A canonical name and its configured aliases are distinct buckets.
    IdentityModel,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum AiTokenQuotaOnExceeded {
    #[default]
    Reject,
    Throttle {
        #[serde(rename = "maxDelay")]
        #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
        max_delay: String,
    },
    Downgrade {
        #[schemars(length(max = 512))]
        model: String,
    },
}

const fn default_on_missing_key_deny() -> OnMissingKey {
    OnMissingKey::Deny
}

const fn default_on_redis_failure_deny() -> OnRedisFailure {
    OnRedisFailure::Deny
}

const fn default_flush_tokens() -> u64 {
    100
}

fn default_flush_interval() -> String {
    "1s".to_string()
}

fn default_request_timeout() -> String {
    "500ms".to_string()
}

const fn default_inject_usage_tracking() -> bool {
    true
}

impl PluginValidation for AiProxyConfig {
    fn validation_error(&self) -> Option<String> {
        if self.on_unknown_model == OnUnknownModel::Passthrough
            && !matches!(
                self.backend_selection.selection_type,
                AiBackendSelection::Weighted | AiBackendSelection::Priority
            )
        {
            return Some("onUnknownModel passthrough requires backendSelection.type Weighted or Priority".to_string());
        }
        if let Some(error) = validate_model_routes(&self.model_routes) {
            return Some(error);
        }
        self.token_quota.as_ref().and_then(AiTokenQuotaConfig::validation_error)
    }
}

fn validate_model_routes(routes: &[AiModelRoute]) -> Option<String> {
    if routes.len() > AI_MODEL_ROUTE_MAX_COUNT {
        return Some(format!(
            "modelRoutes must contain at most {AI_MODEL_ROUTE_MAX_COUNT} entries"
        ));
    }
    let mut parsed = Vec::with_capacity(routes.len());
    for (route_index, route) in routes.iter().enumerate() {
        let pattern = match AiModelPattern::parse(&route.match_.model) {
            Ok(pattern) => pattern,
            Err(error) => return Some(format!("modelRoutes[{route_index}].match.model: {error}")),
        };
        if route.match_.headers.len() > AI_MODEL_ROUTE_MAX_HEADERS {
            return Some(format!(
                "modelRoutes[{route_index}].match.headers must contain at most {AI_MODEL_ROUTE_MAX_HEADERS} entries"
            ));
        }
        let mut header_names = HashSet::new();
        for (header_index, header) in route.match_.headers.iter().enumerate() {
            if header.name.len() > crate::gwapi_types::HEADER_NAME_MAX_LEN as usize
                || !crate::gwapi_types::is_valid_header_token(&header.name)
            {
                return Some(format!(
                    "modelRoutes[{route_index}].match.headers[{header_index}].name must be a valid HTTP header name"
                ));
            }
            let normalized = header.name.to_ascii_lowercase();
            if !header_names.insert(normalized) {
                return Some(format!(
                    "modelRoutes[{route_index}].match.headers must not repeat a header name"
                ));
            }
            if header.value.is_empty()
                || header.value.len() > AI_MODEL_ROUTE_HEADER_VALUE_MAX_BYTES
                || !header.value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
            {
                return Some(format!(
                    "modelRoutes[{route_index}].match.headers[{header_index}].value must contain 1..={AI_MODEL_ROUTE_HEADER_VALUE_MAX_BYTES} visible ASCII bytes"
                ));
            }
        }
        if route.action.rewrite.as_deref().is_some_and(|rewrite| {
            rewrite.trim().is_empty() || rewrite.len() > AI_MODEL_NAME_MAX_BYTES || rewrite.contains('*')
        }) {
            return Some(format!(
                "modelRoutes[{route_index}].action.rewrite must be a non-empty exact canonical model name"
            ));
        }
        if let Some(candidates) = &route.action.candidates {
            if candidates.is_empty() || candidates.len() > AI_MODEL_ROUTE_MAX_CANDIDATES {
                return Some(format!(
                    "modelRoutes[{route_index}].action.candidates must contain 1..={AI_MODEL_ROUTE_MAX_CANDIDATES} entries"
                ));
            }
            let mut selectors = HashSet::new();
            for (candidate_index, candidate) in candidates.iter().enumerate() {
                if !valid_slot_selector(&candidate.backend) {
                    return Some(format!(
                        "modelRoutes[{route_index}].action.candidates[{candidate_index}].backend is not a valid EdgionBackend slot selector"
                    ));
                }
                if !selectors.insert(&candidate.backend) {
                    return Some(format!(
                        "modelRoutes[{route_index}].action.candidates contains a duplicate selector"
                    ));
                }
                if candidate.weight.is_some_and(|weight| weight < 0) {
                    return Some(format!(
                        "modelRoutes[{route_index}].action.candidates[{candidate_index}].weight must not be negative"
                    ));
                }
            }
        }
        parsed.push((pattern, route.match_.headers.len()));
    }
    for left in 0..parsed.len() {
        for right in left + 1..parsed.len() {
            let (left_pattern, left_headers) = &parsed[left];
            let (right_pattern, right_headers) = &parsed[right];
            if left_pattern.exactness() == right_pattern.exactness()
                && left_pattern.literal_len() == right_pattern.literal_len()
                && left_headers == right_headers
                && left_pattern.overlaps(right_pattern)
            {
                return Some(format!(
                    "modelRoutes[{left}] and modelRoutes[{right}] have ambiguous equal-specificity matches"
                ));
            }
        }
    }
    None
}

fn valid_slot_selector(selector: &AiBackendSlotSelector) -> bool {
    selector.group == "edgion.io"
        && selector.kind == "EdgionBackend"
        && valid_dns_label(&selector.namespace)
        && valid_kube_name(&selector.name)
        && selector.section.is_none()
        && usize::from(selector.occurrence) < AI_MODEL_ROUTE_MAX_CANDIDATES
}

fn valid_dns_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && value.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
}

fn valid_kube_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 253 && value.split('.').all(valid_dns_label)
}

impl AiTokenQuotaOnExceeded {
    pub fn parsed_max_delay(&self) -> Result<Option<Duration>, String> {
        match self {
            Self::Throttle { max_delay } => parse_duration(max_delay)
                .map(Some)
                .map_err(|error| format!("tokenQuota.onExceeded.maxDelay: {error}")),
            Self::Reject | Self::Downgrade { .. } => Ok(None),
        }
    }
}

impl AiTokenQuotaConfig {
    pub fn parsed_flush_interval(&self) -> Result<Duration, String> {
        parse_duration(&self.flush_interval).map_err(|error| format!("tokenQuota.flushInterval: {error}"))
    }

    pub fn parsed_request_timeout(&self) -> Result<Duration, String> {
        parse_duration(&self.request_timeout).map_err(|error| format!("tokenQuota.requestTimeout: {error}"))
    }

    pub fn parsed_windows(&self) -> Result<Vec<Duration>, String> {
        self.limits
            .iter()
            .enumerate()
            .map(|(index, limit)| {
                parse_duration(&limit.window).map_err(|error| format!("tokenQuota.limits[{index}].window: {error}"))
            })
            .collect()
    }

    fn validation_error(&self) -> Option<String> {
        if !valid_redis_ref(&self.redis_ref) {
            return Some("tokenQuota.redisRef must use namespace/name form".to_string());
        }
        if self.key.len() > 8 {
            return Some("tokenQuota.key must contain at most 8 sources".to_string());
        }
        for (index, key) in self.key.iter().enumerate() {
            if key.name().is_some_and(str::is_empty) {
                return Some(format!("tokenQuota.key[{index}].name must not be empty"));
            }
            if matches!(key, KeyGet::Webhook { .. }) {
                return Some(format!(
                    "tokenQuota.key[{index}] must not use a webhook because quota admission cannot add an outbound dependency"
                ));
            }
        }
        if self.limits.is_empty() || self.limits.len() > 8 {
            return Some("tokenQuota.limits must contain between 1 and 8 entries".to_string());
        }
        for (index, limit) in self.limits.iter().enumerate() {
            if limit.tokens == 0 || limit.tokens > AI_QUOTA_MAX_SAFE_INTEGER {
                return Some(format!(
                    "tokenQuota.limits[{index}].tokens must be between 1 and {AI_QUOTA_MAX_SAFE_INTEGER}"
                ));
            }
        }
        match &self.on_exceeded {
            AiTokenQuotaOnExceeded::Reject => {}
            AiTokenQuotaOnExceeded::Throttle { .. } => {
                let max_delay = match self.on_exceeded.parsed_max_delay() {
                    Ok(Some(value)) => value,
                    Ok(None) => unreachable!("Throttle always has a max delay"),
                    Err(error) => return Some(error),
                };
                if !(Duration::from_millis(1)..=Duration::from_secs(5)).contains(&max_delay) {
                    return Some("tokenQuota.onExceeded.maxDelay must be between 1ms and 5s".to_string());
                }
            }
            AiTokenQuotaOnExceeded::Downgrade { model } => {
                if model.trim().is_empty() {
                    return Some("tokenQuota.onExceeded.model must not be empty".to_string());
                }
                if model.len() > AI_MODEL_NAME_MAX_BYTES {
                    return Some(format!(
                        "tokenQuota.onExceeded.model must not exceed {AI_MODEL_NAME_MAX_BYTES} UTF-8 bytes"
                    ));
                }
                if http::HeaderValue::try_from(model.as_str()).is_err() {
                    return Some("tokenQuota.onExceeded.model must be a valid HTTP header value".to_string());
                }
                if self
                    .limits
                    .iter()
                    .any(|limit| limit.scope != AiTokenQuotaScope::IdentityModel)
                {
                    return Some(
                        "tokenQuota.onExceeded Downgrade requires every limit scope to be identityModel".to_string(),
                    );
                }
            }
        }
        if self.flush_tokens == 0 || self.flush_tokens > AI_QUOTA_MAX_SAFE_INTEGER {
            return Some(format!(
                "tokenQuota.flushTokens must be between 1 and {AI_QUOTA_MAX_SAFE_INTEGER}"
            ));
        }
        let flush_interval = match self.parsed_flush_interval() {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        if flush_interval < Duration::from_millis(10) {
            return Some("tokenQuota.flushInterval must be at least 10ms".to_string());
        }
        let request_timeout = match self.parsed_request_timeout() {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        if request_timeout.is_zero() {
            return Some("tokenQuota.requestTimeout must be greater than zero".to_string());
        }
        let windows = match self.parsed_windows() {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        if windows.iter().any(Duration::is_zero) {
            return Some("tokenQuota limit windows must be greater than zero".to_string());
        }
        None
    }
}

fn valid_redis_ref(value: &str) -> bool {
    let mut parts = value.split('/');
    let Some(namespace) = parts.next() else {
        return false;
    };
    let Some(name) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && !namespace.is_empty()
        && !name.is_empty()
        && [namespace, name].iter().all(|part| {
            part.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_quota() -> AiTokenQuotaConfig {
        AiTokenQuotaConfig {
            redis_ref: "default/ai-quota".to_string(),
            key: vec![KeyGet::ClientIp],
            on_missing_key: OnMissingKey::Deny,
            on_redis_failure: OnRedisFailure::Deny,
            limits: vec![AiTokenQuotaLimit {
                scope: AiTokenQuotaScope::Identity,
                tokens: 100,
                window: "1h".to_string(),
            }],
            on_exceeded: AiTokenQuotaOnExceeded::Reject,
            flush_tokens: 10,
            flush_interval: "100ms".to_string(),
            request_timeout: "500ms".to_string(),
        }
    }

    fn config(token_quota: Option<AiTokenQuotaConfig>) -> AiProxyConfig {
        AiProxyConfig {
            backend_selection: AiBackendSelectionConfig::default(),
            inject_usage_tracking: true,
            on_unknown_model: OnUnknownModel::Reject,
            model_routes: Vec::new(),
            token_quota,
        }
    }

    #[test]
    fn ai_proxy_defaults_are_stable() {
        let config: AiProxyConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.backend_selection.selection_type, AiBackendSelection::Weighted);
        assert!(config.inject_usage_tracking);
        assert_eq!(config.on_unknown_model, OnUnknownModel::Reject);
        assert!(config.model_routes.is_empty());
        assert!(config.token_quota.is_none());

        let passthrough: AiProxyConfig = serde_json::from_str(
            r#"{"backendSelection":{"type":"Priority"},"injectUsageTracking":false,"onUnknownModel":"passthrough"}"#,
        )
        .unwrap();
        assert!(!passthrough.inject_usage_tracking);
        assert_eq!(passthrough.on_unknown_model, OnUnknownModel::Passthrough);
        assert_eq!(
            passthrough.backend_selection.selection_type,
            AiBackendSelection::Priority
        );
        assert!(passthrough.validation_error().is_none());
    }

    #[test]
    fn model_routes_validate_wildcards_headers_selectors_and_ambiguity() {
        let valid: AiProxyConfig = serde_yaml::from_str(
            r#"
modelRoutes:
  - match:
      model: gpt-4*
      headers:
        - name: X-Tier
          value: paid
    action:
      rewrite: gpt-4o
      candidates:
        - backend:
            group: edgion.io
            kind: EdgionBackend
            namespace: ai
            name: primary
            occurrence: 0
          weight: 0
"#,
        )
        .unwrap();
        assert!(valid.validation_error().is_none());

        for yaml in [
            r#"modelRoutes: [{match: {model: "*"}, action: {}}]"#,
            r#"modelRoutes: [{match: {model: "a*b"}, action: {}}]"#,
            r#"modelRoutes: [{match: {model: a, headers: [{name: X-Tier, value: a}, {name: x-tier, value: b}]}, action: {}}]"#,
            r#"modelRoutes: [{match: {model: a}, action: {candidates: []}}]"#,
            r#"modelRoutes: [{match: {model: a}, action: {rewrite: "alias*"}}]"#,
        ] {
            let value: AiProxyConfig = serde_yaml::from_str(yaml).unwrap();
            assert!(value.validation_error().is_some(), "accepted {yaml}");
        }

        let ambiguous: AiProxyConfig = serde_yaml::from_str(
            r#"
modelRoutes:
  - match: {model: gpt*}
    action: {rewrite: a}
  - match: {model: gpt*}
    action: {rewrite: b}
"#,
        )
        .unwrap();
        assert!(ambiguous
            .validation_error()
            .is_some_and(|error| error.contains("ambiguous")));
    }

    #[test]
    fn model_route_headers_require_bounded_visible_ascii() {
        for yaml in [
            format!(
                "modelRoutes: [{{match: {{model: a, headers: [{{name: {}, value: ok}}]}}, action: {{}}}}]",
                "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize + 1)
            ),
            "modelRoutes: [{match: {model: a, headers: [{name: x-tier, value: \"bad\\nvalue\"}]}, action: {}}]"
                .to_string(),
            "modelRoutes: [{match: {model: a, headers: [{name: x-tier, value: \"é\"}]}, action: {}}]".to_string(),
        ] {
            let value: AiProxyConfig = serde_yaml::from_str(&yaml).unwrap();
            assert!(value.validation_error().is_some(), "accepted {yaml}");
        }

        let boundary = format!(
            "modelRoutes: [{{match: {{model: a, headers: [{{name: {}, value: '~'}}]}}, action: {{}}}}]",
            "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize)
        );
        assert!(serde_yaml::from_str::<AiProxyConfig>(&boundary)
            .unwrap()
            .validation_error()
            .is_none());
    }

    #[test]
    fn backend_selection_has_no_inline_candidates_and_gates_passthrough() {
        let error = serde_yaml::from_str::<AiProxyConfig>(
            r#"
backendSelection:
  type: Weighted
  candidates:
    - backendRef: backend-a
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `candidates`"));

        for selection_type in [
            AiBackendSelection::LowestLatency,
            AiBackendSelection::LowestUsage,
            AiBackendSelection::CostWeighted,
        ] {
            let invalid = AiProxyConfig {
                backend_selection: AiBackendSelectionConfig { selection_type },
                inject_usage_tracking: true,
                on_unknown_model: OnUnknownModel::Passthrough,
                model_routes: Vec::new(),
                token_quota: None,
            };
            let first = invalid.validation_error();
            let second = invalid.validation_error();
            assert_eq!(first, second, "validation must be stateless");
            assert!(
                first
                    .as_deref()
                    .is_some_and(|message| message.contains("Weighted or Priority")),
                "accepted passthrough with {selection_type:?}"
            );
        }
    }

    #[test]
    fn token_quota_defaults_fail_closed_and_are_bounded() {
        let config: AiProxyConfig = serde_yaml::from_str(
            r#"
tokenQuota:
  redisRef: default/ai-quota
  key:
    - type: ctx
      name: consumer_id
  limits:
    - tokens: 10000000
      window: 24h
"#,
        )
        .unwrap();
        let quota = config.token_quota.as_ref().unwrap();
        assert_eq!(quota.on_missing_key, OnMissingKey::Deny);
        assert_eq!(quota.on_redis_failure, OnRedisFailure::Deny);
        assert_eq!(quota.on_exceeded, AiTokenQuotaOnExceeded::Reject);
        assert_eq!(quota.flush_tokens, 100);
        assert_eq!(quota.flush_interval, "1s");
        assert_eq!(quota.request_timeout, "500ms");
        assert!(config.validation_error().is_none());
    }

    #[test]
    fn token_quota_rejects_unsafe_or_ambiguous_configuration() {
        let base = base_quota();

        let mut cases = Vec::new();
        let mut invalid_ref = base.clone();
        invalid_ref.redis_ref = "missing-namespace".to_string();
        cases.push(invalid_ref);
        let mut zero_limit = base.clone();
        zero_limit.limits[0].tokens = 0;
        cases.push(zero_limit);
        let mut unsafe_limit = base.clone();
        unsafe_limit.limits[0].tokens = AI_QUOTA_MAX_SAFE_INTEGER + 1;
        cases.push(unsafe_limit);
        let mut zero_timeout = base.clone();
        zero_timeout.request_timeout = "0s".to_string();
        cases.push(zero_timeout);
        let mut webhook = base.clone();
        webhook.key = vec![KeyGet::Webhook {
            webhook_ref: "quota-key".to_string(),
            path_override: None,
            extract: crate::common::WebhookExtract::Body {
                path: "consumer".to_string(),
            },
        }];
        cases.push(webhook);

        for quota in cases {
            assert!(config(Some(quota)).validation_error().is_some());
        }
    }

    #[test]
    fn token_quota_exceeded_actions_are_tagged_and_validated() {
        let mut throttle = base_quota();
        throttle.on_exceeded = AiTokenQuotaOnExceeded::Throttle {
            max_delay: "250ms".to_string(),
        };
        assert!(config(Some(throttle.clone())).validation_error().is_none());
        assert_eq!(
            throttle.on_exceeded.parsed_max_delay().unwrap(),
            Some(Duration::from_millis(250))
        );

        for max_delay in ["0s", "6s", "invalid"] {
            throttle.on_exceeded = AiTokenQuotaOnExceeded::Throttle {
                max_delay: max_delay.to_string(),
            };
            assert!(config(Some(throttle.clone())).validation_error().is_some());
        }

        let parsed: AiTokenQuotaOnExceeded = serde_yaml::from_str("type: Downgrade\nmodel: chat-economy\n").unwrap();
        assert_eq!(
            parsed,
            AiTokenQuotaOnExceeded::Downgrade {
                model: "chat-economy".to_string()
            }
        );

        let mut downgrade = base_quota();
        downgrade.limits[0].scope = AiTokenQuotaScope::IdentityModel;
        downgrade.on_exceeded = parsed;
        assert!(config(Some(downgrade.clone())).validation_error().is_none());

        downgrade.limits[0].scope = AiTokenQuotaScope::Identity;
        assert!(config(Some(downgrade.clone()))
            .validation_error()
            .is_some_and(|error| error.contains("identityModel")));

        downgrade.limits[0].scope = AiTokenQuotaScope::IdentityModel;
        for model in [" ".to_string(), "m".repeat(AI_MODEL_NAME_MAX_BYTES + 1)] {
            downgrade.on_exceeded = AiTokenQuotaOnExceeded::Downgrade { model };
            assert!(config(Some(downgrade.clone())).validation_error().is_some());
        }

        downgrade.on_exceeded = AiTokenQuotaOnExceeded::Downgrade {
            model: "economy\nmodel".to_string(),
        };
        assert!(config(Some(downgrade.clone()))
            .validation_error()
            .is_some_and(|error| error.contains("HTTP header value")));

        downgrade.on_exceeded = AiTokenQuotaOnExceeded::Downgrade {
            model: "économy".to_string(),
        };
        assert!(http::HeaderValue::try_from("économy").is_ok());
        assert!(config(Some(downgrade)).validation_error().is_none());
    }
}
