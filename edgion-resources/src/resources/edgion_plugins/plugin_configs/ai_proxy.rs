//! Configuration for the request-only AI provider proxy plugin.

use std::collections::HashSet;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::KeyGet;
use crate::duration::parse_duration;
use crate::resources::edgion_backend::{AiProvider, AI_MODEL_NAME_MAX_BYTES};
use crate::resources::edgion_plugins::PluginValidation;

use super::rate_limit_local::OnKeyMiss;
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

/// Client API selected by the route. Unsupported operations fail validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AiApi {
    OpenaiChat,
    OpenaiResponses,
    AnthropicMessages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AiConversion {
    ChatToAnthropic,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AiUsageObservation {
    #[default]
    Off,
    Provider,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiProxyConfig {
    pub api: AiApi,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversion: Option<AiConversion>,
    #[serde(default)]
    pub usage_observation: AiUsageObservation,
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
    #[serde(default = "default_on_key_miss_deny")]
    pub on_key_miss: OnKeyMiss,
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

const fn default_on_key_miss_deny() -> OnKeyMiss {
    OnKeyMiss::Deny
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
    false
}

impl AiProxyConfig {
    /// Shared family check for declaration admission and captured runtime candidates.
    pub fn accepts_provider(&self, provider: AiProvider) -> bool {
        match (self.api, self.conversion) {
            (AiApi::OpenaiChat, None) => matches!(provider, AiProvider::OpenAI | AiProvider::OpenAICompatible),
            (AiApi::OpenaiChat, Some(AiConversion::ChatToAnthropic)) => provider == AiProvider::Anthropic,
            _ => false,
        }
    }
}

impl PluginValidation for AiProxyConfig {
    fn validation_error(&self) -> Option<String> {
        if self.api != AiApi::OpenaiChat {
            return Some(
                "AiProxy currently supports only api openaiChat; native Responses and Messages are not implemented"
                    .to_string(),
            );
        }
        if self.inject_usage_tracking && self.usage_observation == AiUsageObservation::Off && self.conversion.is_none()
        {
            return Some(
                "injectUsageTracking requires usageObservation provider or explicit chatToAnthropic conversion"
                    .to_string(),
            );
        }
        if self.usage_observation == AiUsageObservation::Off
            && (self.token_quota.is_some() || self.backend_selection.selection_type == AiBackendSelection::LowestUsage)
        {
            return Some("tokenQuota and LowestUsage require usageObservation provider".to_string());
        }
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
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/ai_proxy_test.rs"]
mod tests;
