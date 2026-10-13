//! Plugin condition types for conditional execution
//!
//! This module defines the condition structures that allow plugins to be
//! conditionally executed based on various criteria such as:
//! - Key existence/matching in headers, query params, cookies
//! - Time ranges (before/after specific timestamps)
//! - Probability-based execution (for canary/sampling)
//! - `keyMatch` with `negate: true` as the exclude spelling
//!
//! ## Condition Evaluation Logic
//!
//! ```text
//! Request arrives
//!     │
//!     ▼
//! Check the explicit skip condition set
//!     │
//!     ├─ Set satisfied ──► Skip plugin
//!     │
//!     ▼ None satisfied
//! Check the explicit run condition set
//!     │
//!     ├─ Set satisfied ──► Execute plugin
//!     │
//!     └─ Set not satisfied ──► Skip plugin
//! ```
//!
//! ## YAML Example
//!
//! ```yaml
//! conditions:
//!   skip:
//!     anyOf:
//!       - type: keyExist
//!         key:
//!           type: header
//!           name: "X-Internal-Request"
//!       - type: keyMatch
//!         key:
//!           type: path
//!         values: ["/health", "/ready"]
//!         negate: true
//!   run:
//!     allOf:
//!       - type: timeRange
//!         after: "2024-01-01T00:00:00Z"
//!         before: "2025-12-31T23:59:59Z"
//!       - type: probability
//!         ratio: 0.1
//! ```

use crate::common::KeyGet;

crate::redact::impl_redacted_debug!(SecretMatchCondition);
use super::ip_reference_set::{IpReferenceSet, IpReferenceWire};
use crate::path_normalization::{get_path_normalization_config, normalize_config_path};
use crate::resources::common::SecretObjectReference;
use crate::resources::edgion_plugins::hmac_match::HmacMatchConfig;
pub use crate::resources::edgion_plugins::mtls_field::{MtlsField, SanType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Threshold for pre-compiling values into a HashSet for O(1) lookup.
/// When values.len() > this threshold, a HashSet will be built during compilation.
pub const VALUES_HASHSET_THRESHOLD: usize = 16;

/// Named ceiling for `size_of::<Condition>()`.
///
/// Raised when the four former AccessCondition variants joined the enum.
/// Do not box variants unless the measured size exceeds 512.
pub const CONDITION_SIZE_CEILING: usize = 512;

fn default_true() -> bool {
    true
}

/// Plugin conditions configuration
///
/// Defines when a plugin should be executed or skipped. Each action contains
/// an explicit bounded [`ConditionSet`].
///
/// When both are specified, `skip` is evaluated first.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginConditions {
    /// A matching set causes the plugin to be skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<ConditionSet>,

    /// The plugin runs only when this set matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<ConditionSet>,
}

/// Placement-specific validation for a shared condition set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionValidationProfile {
    /// Plugin entry, secondary body gate, and dye conditions.
    EntrySync,
    /// RequestRestriction and RequestAccessPolicy conditions.
    RequestAsync,
    /// Request-stage RateLimitRedis entry run conditions only.
    RateLimitRedisRun,
}

/// A fixed-depth Boolean expression over shared [`Condition`] leaves.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum ConditionSet {
    AllOf {
        #[serde(rename = "allOf")]
        all_of: Vec<Condition>,
    },
    AnyOf {
        #[serde(rename = "anyOf")]
        any_of: Vec<AnyOfClause>,
    },
}

impl Default for ConditionSet {
    fn default() -> Self {
        Self::AllOf { all_of: Vec::new() }
    }
}

/// One alternative of a top-level [`ConditionSet::AnyOf`].
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum AnyOfClause {
    Condition(Condition),
    AllOf(AllOfGroup),
}

/// A one-level conjunction nested directly below `anyOf`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AllOfGroup {
    /// Optional attribution name. It has no effect on Boolean matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Direct Condition leaves combined with AND.
    pub all_of: Vec<Condition>,
}

impl<'de> Deserialize<'de> for AnyOfClause {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let value = serde_json::Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| D::Error::custom("anyOf clause must be an object"))?;
        let has_type = object.contains_key("type");
        let has_all_of = object.contains_key("allOf");
        match (has_type, has_all_of) {
            (true, false) => {
                if object.contains_key("name") {
                    return Err(D::Error::custom("name is only valid on an allOf group"));
                }
                serde_json::from_value(value)
                    .map(Self::Condition)
                    .map_err(D::Error::custom)
            }
            (false, true) => serde_json::from_value(value).map(Self::AllOf).map_err(D::Error::custom),
            (true, true) => Err(D::Error::custom(
                "anyOf clause must contain exactly one of type or allOf",
            )),
            (false, false) => Err(D::Error::custom(
                "anyOf clause must contain exactly one of type or allOf",
            )),
        }
    }
}

/// Condition type enumeration
///
/// Each variant represents a different type of condition check.
/// Uses internally tagged enum for YAML representation:
/// ```yaml
/// - type: keyExist
///   key:
///     type: header
///     name: "X-Test"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Condition {
    /// Check if a key exists in the specified source
    KeyExist(KeyExistCondition),

    /// Check if a key matches a specific value or pattern
    KeyMatch(KeyMatchCondition),

    /// Check if current time is within a specified range
    TimeRange(TimeRangeCondition),

    /// Execute with a specified probability
    Probability(ProbabilityCondition),

    /// Exact match against Controller-resolved Secret values.
    SecretMatch(SecretMatchCondition),

    /// Match the physical transport peer against a compiled IP set.
    DirectPeerIpMatch(IpAccessMatch),

    /// Match the layer-resolved client IP against a compiled IP set.
    RemoteIpMatch(IpAccessMatch),

    /// Header-only (entry-level) or full (RAP allOf) HMAC authentication.
    HmacAuth(HmacMatchConfig),

    /// Match a JWT claim stored in ctx (`jwt_claims`, then `oidc_claims`).
    JwtClaimMatch(JwtClaimMatchCondition),

    /// Match an mTLS client-certificate field (subject / CN / SAN).
    MtlsMatch(MtlsMatchCondition),

    /// An earlier local limiter in this resource exhausted its allowance.
    RateLimitExceeded(RateLimitExceededCondition),
}

/// Reference to a local limiter in the same request plugin array.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RateLimitExceededCondition {
    pub plugin_alias: String,
    /// Runtime-only, value-free diagnostic descriptor.
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

/// Shared exact-values / regex match fields, flattened into KeyMatch.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MatchCore {
    /// Exact values to match (any match satisfies, OR logic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,

    /// Regex patterns to match (merged into a single regex with `|`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<Vec<String>>,

    /// Default true. Applies to `values` only; regex uses `(?i)` if needed.
    #[serde(default = "default_true")]
    pub case_sensitive: bool,

    /// Pre-compiled HashSet for O(1) lookup when values.len() > 16.
    #[serde(skip)]
    #[schemars(skip)]
    pub values_set: Option<HashSet<String>>,

    /// Compiled regex (runtime only, merged from all patterns).
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<regex::Regex>,
}

impl Default for MatchCore {
    fn default() -> Self {
        Self {
            values: None,
            regex: None,
            case_sensitive: true,
            values_set: None,
            compiled_regex: None,
        }
    }
}

impl MatchCore {
    fn has_match_payload(&self) -> bool {
        self.values.as_ref().is_some_and(|values| !values.is_empty())
            || self.regex.as_ref().is_some_and(|patterns| !patterns.is_empty())
    }

    /// Compile exact values (optional path normalization + HashSet) and merge regex.
    ///
    /// `normalize_paths` is supplied by the enclosing variant (`key.type: path`).
    /// Exact `values` are normalized; regex is never normalized.
    pub fn compile(&mut self, normalize_paths: bool) -> Result<(), regex::Error> {
        if normalize_paths {
            if let Some(values) = self.values.as_mut() {
                let path_cfg = get_path_normalization_config();
                for v in values.iter_mut() {
                    *v = normalize_config_path(v, path_cfg);
                }
            }
        }

        if let Some(patterns) = &self.regex {
            if !patterns.is_empty() {
                let combined = patterns.join("|");
                self.compiled_regex = Some(regex::Regex::new(&combined)?);
            }
        }

        if let Some(values) = &self.values {
            if values.len() > VALUES_HASHSET_THRESHOLD {
                self.values_set = Some(if self.case_sensitive {
                    values.iter().cloned().collect()
                } else {
                    // ASCII-only case folding to stay consistent with the linear
                    // path's `eq_ignore_ascii_case` regardless of list size.
                    values.iter().map(|v| v.to_ascii_lowercase()).collect()
                });
            }
        }
        Ok(())
    }

    /// True iff the present value is in `values` OR matches compiled `regex`.
    pub fn matches(&self, value: &str) -> bool {
        if let Some(set) = &self.values_set {
            if self.case_sensitive {
                if set.contains(value) {
                    return true;
                }
            } else if set.contains(&value.to_ascii_lowercase()) {
                return true;
            }
        } else if let Some(values) = &self.values {
            for v in values {
                let m = if self.case_sensitive {
                    value == v
                } else {
                    value.eq_ignore_ascii_case(v)
                };
                if m {
                    return true;
                }
            }
        }

        if let Some(re) = &self.compiled_regex {
            if re.is_match(value) {
                return true;
            }
        }

        false
    }

    /// True when regex patterns were configured but compile did not produce a matcher.
    pub fn has_uncompiled_regex(&self) -> bool {
        self.compiled_regex.is_none() && self.regex.as_ref().is_some_and(|p| !p.is_empty())
    }
}

/// Key existence condition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyExistCondition {
    /// The key to check for existence (unified KeyGet accessor)
    pub key: KeyGet,

    /// Invert the result after the missing-value check. Default false.
    #[serde(default)]
    pub negate: bool,

    /// Runtime-only match descriptor (`<ordinal>:<type>[:<accessor>]`).
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

/// Key match condition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyMatchCondition {
    /// The key to match (unified KeyGet accessor)
    pub key: KeyGet,

    /// Exact values / regex / case-sensitivity (flattened onto the condition object).
    #[serde(flatten)]
    #[schemars(flatten)]
    pub core: MatchCore,

    /// Invert the result after the missing-value check. Default false.
    #[serde(default)]
    pub negate: bool,

    /// Runtime-only match descriptor (`<ordinal>:<type>[:<accessor>]`).
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

/// Time range condition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeRangeCondition {
    /// Condition is satisfied only after this time (RFC3339 format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    /// Condition is satisfied only before this time (RFC3339 format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,

    /// Invert the result. Default false.
    #[serde(default)]
    pub negate: bool,

    /// Runtime-only match descriptor (`<ordinal>:<type>[:<accessor>]`).
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

impl TimeRangeCondition {
    /// Validate that `after` / `before` are parseable as RFC3339.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(s) = &self.after {
            chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|e| format!("timeRange.after '{s}' is not a valid RFC3339 timestamp: {e}"))?;
        }
        if let Some(s) = &self.before {
            chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|e| format!("timeRange.before '{s}' is not a valid RFC3339 timestamp: {e}"))?;
        }
        Ok(())
    }
}

/// Probability condition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProbabilityCondition {
    /// Probability ratio (0.0 to 1.0)
    pub ratio: f64,

    /// Optional key for deterministic sampling (unified KeyGet accessor)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeyGet>,

    /// Invert the result. Default false.
    #[serde(default)]
    pub negate: bool,

    /// Runtime-only match descriptor (`<ordinal>:<type>[:<accessor>]`).
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

/// Exact match against Controller-resolved Secret values.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretMatchCondition {
    /// Request value to compare with the selected Secret data entry.
    pub key: KeyGet,
    /// Exact key to read from `secretRef.data`.
    pub secret_key: String,
    pub secret_ref: SecretObjectReference,
    #[serde(default)]
    pub negate: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_values: Option<Vec<String>>,
    /// Runtime-only match descriptor (`<ordinal>:<type>[:<accessor>]`).
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

/// An ordered union of referenced IpLists, with optional predicate negation.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpAccessMatch {
    #[serde(flatten)]
    pub refs: IpReferenceSet,
    #[serde(default)]
    pub negate: bool,
    /// Runtime-only match descriptor (`<ordinal>:<type>`).
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

impl<'de> Deserialize<'de> for IpAccessMatch {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            negate: bool,
            #[serde(flatten)]
            refs: IpReferenceWire,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            refs: wire.refs.into_set(),
            negate: wire.negate,
            descriptor: Box::from(""),
        })
    }
}

impl IpAccessMatch {
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        self.refs.validate()
    }
}

fn validate_nonempty_name(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} cannot be empty"))
    } else {
        Ok(())
    }
}

const HMAC_BODY_ENTRY_LEVEL_ERROR: &str =
    "hmacAuth.validateRequestBody is only valid inside RequestRestriction or RequestAccessPolicy conditions";

impl Condition {
    /// Condition type name used in compiled descriptors.
    pub fn cond_type(&self) -> &'static str {
        match self {
            Condition::KeyExist(_) => "keyExist",
            Condition::KeyMatch(_) => "keyMatch",
            Condition::TimeRange(_) => "timeRange",
            Condition::Probability(_) => "probability",
            Condition::SecretMatch(_) => "secretMatch",
            Condition::DirectPeerIpMatch(_) => "directPeerIpMatch",
            Condition::RemoteIpMatch(_) => "remoteIpMatch",
            Condition::HmacAuth(_) => "hmacAuth",
            Condition::JwtClaimMatch(_) => "jwtClaimMatch",
            Condition::MtlsMatch(_) => "mtlsMatch",
            Condition::RateLimitExceeded(_) => "rateLimitExceeded",
        }
    }

    /// Compiled match descriptor, or empty before `compile`.
    pub fn descriptor(&self) -> &str {
        match self {
            Condition::KeyExist(c) => &c.descriptor,
            Condition::KeyMatch(c) => &c.descriptor,
            Condition::TimeRange(c) => &c.descriptor,
            Condition::Probability(c) => &c.descriptor,
            Condition::SecretMatch(c) => &c.descriptor,
            Condition::DirectPeerIpMatch(c) => &c.descriptor,
            Condition::RemoteIpMatch(c) => &c.descriptor,
            Condition::HmacAuth(c) => &c.descriptor,
            Condition::JwtClaimMatch(c) => &c.descriptor,
            Condition::MtlsMatch(c) => &c.descriptor,
            Condition::RateLimitExceeded(c) => &c.descriptor,
        }
    }

    fn assign_descriptor(&mut self, ordinal: usize) {
        let ty = self.cond_type();
        let accessor = match self {
            Condition::KeyExist(c) => Some(c.key.as_log_str()),
            Condition::KeyMatch(c) => Some(c.key.as_log_str()),
            Condition::SecretMatch(c) => Some(c.key.as_log_str()),
            Condition::JwtClaimMatch(_) | Condition::MtlsMatch(_) | Condition::RateLimitExceeded(_) => None,
            Condition::TimeRange(_)
            | Condition::DirectPeerIpMatch(_)
            | Condition::RemoteIpMatch(_)
            | Condition::HmacAuth(_) => None,
            Condition::Probability(c) => c.key.as_ref().map(|k| k.as_log_str()),
        };
        let descriptor: Box<str> = match accessor {
            Some(acc) => format!("{ordinal}:{ty}:{acc}").into(),
            None => format!("{ordinal}:{ty}").into(),
        };
        match self {
            Condition::KeyExist(c) => c.descriptor = descriptor,
            Condition::KeyMatch(c) => c.descriptor = descriptor,
            Condition::TimeRange(c) => c.descriptor = descriptor,
            Condition::Probability(c) => c.descriptor = descriptor,
            Condition::SecretMatch(c) => c.descriptor = descriptor,
            Condition::DirectPeerIpMatch(c) => c.descriptor = descriptor,
            Condition::RemoteIpMatch(c) => c.descriptor = descriptor,
            Condition::HmacAuth(c) => c.descriptor = descriptor,
            Condition::JwtClaimMatch(c) => c.descriptor = descriptor,
            Condition::MtlsMatch(c) => c.descriptor = descriptor,
            Condition::RateLimitExceeded(c) => c.descriptor = descriptor,
        }
    }

    /// Compile regexes, validate IP references and assign the runtime-only descriptor.
    ///
    /// `ordinal` is the stable atomic traversal index inside the owning set.
    /// This resource-layer diagnostic descriptor never contains match values,
    /// claim paths, certificate fields, secret names, IPs, or credential
    /// material. Gateway access-log descriptors are rebuilt with full set paths.
    pub fn compile(&mut self, ordinal: usize) -> Result<(), regex::Error> {
        self.assign_descriptor(ordinal);
        match self {
            Condition::KeyMatch(c) => c.compile(),
            Condition::JwtClaimMatch(c) => c.compile(),
            Condition::MtlsMatch(c) => c.compile(),
            Condition::DirectPeerIpMatch(c) | Condition::RemoteIpMatch(c) => {
                // Reference-shape errors are reported by placement-profile validation.
                // The Gateway binds references from the actual effective owner;
                // this schema module never owns a matcher.
                let _ = c.validate_and_init();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Entry-level validation (`PluginConditions`, body, dye). Implements D-27.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_for_profile(ConditionValidationProfile::EntrySync)
    }

    pub fn validate_for_profile(&self, profile: ConditionValidationProfile) -> Result<(), String> {
        match self {
            Condition::KeyExist(c) => validate_key_get(&c.key, "keyExist.key"),
            Condition::KeyMatch(c) => {
                validate_match_core(&c.core, "keyMatch")?;
                validate_key_get(&c.key, "keyMatch.key")
            }
            Condition::TimeRange(c) => c.validate(),
            Condition::Probability(c) => c
                .key
                .as_ref()
                .map_or(Ok(()), |key| validate_key_get(key, "probability.key")),
            Condition::SecretMatch(c) => {
                validate_key_get(&c.key, "secretMatch.key")?;
                validate_nonempty_name(&c.secret_key, "secretMatch.secretKey")?;
                validate_nonempty_name(&c.secret_ref.name, "secretMatch.secretRef.name")
            }
            Condition::DirectPeerIpMatch(c) | Condition::RemoteIpMatch(c) => c.clone().validate_and_init(),
            Condition::HmacAuth(c) => {
                if !c.has_credentials_source() {
                    return Err("hmacAuth condition requires at least one secret group".to_string());
                }
                if let Some(error) = c.detect_validation_error() {
                    return Err(error);
                }
                if profile != ConditionValidationProfile::RequestAsync && c.validate_request_body {
                    return Err(HMAC_BODY_ENTRY_LEVEL_ERROR.to_string());
                }
                Ok(())
            }
            Condition::JwtClaimMatch(c) => {
                validate_nonempty_name(&c.claim, "jwtClaimMatch.claim")?;
                validate_match_core(&c.core, "jwtClaimMatch")
            }
            Condition::MtlsMatch(c) => validate_match_core(&c.core, "mtlsMatch"),
            Condition::RateLimitExceeded(c) => {
                if profile != ConditionValidationProfile::RateLimitRedisRun {
                    return Err(
                        "rateLimitExceeded is only valid in request-stage RateLimitRedis run conditions".to_string(),
                    );
                }
                match super::entry::validate_alias_str(&c.plugin_alias) {
                    Some(error) => Err(format!("rateLimitExceeded.pluginAlias: {error}")),
                    None => Ok(()),
                }
            }
        }
    }

    /// True for the two IP-set variants.
    pub fn is_ip_match(&self) -> bool {
        matches!(self, Condition::DirectPeerIpMatch(_) | Condition::RemoteIpMatch(_))
    }

    /// HMAC condition that asks to validate the request body digest.
    pub fn validates_request_body(&self) -> bool {
        matches!(self, Condition::HmacAuth(config) if config.validate_request_body)
    }

    /// Clear regex patterns so the uncompiled-regex runtime guard cannot fire.
    pub fn clear_regex(&mut self) {
        match self {
            Condition::KeyMatch(c) => c.core.regex = None,
            Condition::JwtClaimMatch(c) => c.core.regex = None,
            Condition::MtlsMatch(c) => c.core.regex = None,
            _ => {}
        }
    }
}

fn validate_match_core(core: &MatchCore, condition_type: &str) -> Result<(), String> {
    if core.values.as_ref().is_some_and(Vec::is_empty) {
        return Err(format!("{condition_type}.values cannot be empty"));
    }
    if core.has_match_payload() {
        Ok(())
    } else {
        Err(format!("{condition_type} requires values or regex"))
    }
}

fn validate_key_get(key: &KeyGet, field: &str) -> Result<(), String> {
    match key.name() {
        Some(name) => validate_nonempty_name(name, &format!("{field}.name")),
        None => Ok(()),
    }
}

impl AnyOfClause {
    pub fn iter_conditions(&self) -> Box<dyn Iterator<Item = &Condition> + '_> {
        match self {
            Self::Condition(condition) => Box::new(std::iter::once(condition)),
            Self::AllOf(group) => Box::new(group.all_of.iter()),
        }
    }

    pub fn iter_conditions_mut(&mut self) -> Box<dyn Iterator<Item = &mut Condition> + '_> {
        match self {
            Self::Condition(condition) => Box::new(std::iter::once(condition)),
            Self::AllOf(group) => Box::new(group.all_of.iter_mut()),
        }
    }
}

impl ConditionSet {
    pub fn all_of(conditions: Vec<Condition>) -> Self {
        Self::AllOf { all_of: conditions }
    }

    pub fn any_of(conditions: Vec<Condition>) -> Self {
        Self::AnyOf {
            any_of: conditions.into_iter().map(AnyOfClause::Condition).collect(),
        }
    }

    pub fn iter_conditions(&self) -> Box<dyn Iterator<Item = &Condition> + '_> {
        match self {
            Self::AllOf { all_of } => Box::new(all_of.iter()),
            Self::AnyOf { any_of } => Box::new(any_of.iter().flat_map(AnyOfClause::iter_conditions)),
        }
    }

    pub fn iter_conditions_mut(&mut self) -> Box<dyn Iterator<Item = &mut Condition> + '_> {
        match self {
            Self::AllOf { all_of } => Box::new(all_of.iter_mut()),
            Self::AnyOf { any_of } => Box::new(any_of.iter_mut().flat_map(AnyOfClause::iter_conditions_mut)),
        }
    }

    pub fn atomic_count(&self) -> usize {
        self.iter_conditions().count()
    }

    pub fn len(&self) -> usize {
        self.atomic_count()
    }

    pub fn is_empty(&self) -> bool {
        self.atomic_count() == 0
    }

    pub fn ip_match_count(&self) -> usize {
        self.iter_conditions()
            .filter(|condition| condition.is_ip_match())
            .count()
    }

    pub fn validates_request_body(&self) -> bool {
        self.iter_conditions().any(Condition::validates_request_body)
    }

    /// Compile every atomic matcher in stable configuration order.
    pub fn compile(&mut self) -> Result<(), Vec<regex::Error>> {
        let mut errors = Vec::new();
        for (ordinal, condition) in self.iter_conditions_mut().enumerate() {
            if let Err(error) = condition.compile(ordinal) {
                condition.clear_regex();
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validate structure and every atom for the consumer placement.
    pub fn validate(&self, profile: ConditionValidationProfile) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        match self {
            Self::AllOf { all_of } => {
                if all_of.is_empty() {
                    errors.push("allOf must be non-empty".to_string());
                }
            }
            Self::AnyOf { any_of } => {
                if any_of.is_empty() {
                    errors.push("anyOf must be non-empty".to_string());
                }
                for (index, clause) in any_of.iter().enumerate() {
                    if let AnyOfClause::AllOf(group) = clause {
                        if group.all_of.is_empty() {
                            errors.push(format!("anyOf[{index}].allOf must be non-empty"));
                        }
                        if let Some(name) = &group.name {
                            if name.trim().is_empty() {
                                errors.push(format!("anyOf[{index}].name cannot be empty"));
                            }
                        }
                    }
                }
            }
        }
        for condition in self.iter_conditions() {
            if let Err(error) = condition.validate_for_profile(profile) {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl PluginConditions {
    pub fn is_empty(&self) -> bool {
        self.skip.as_ref().is_none_or(ConditionSet::is_empty) && self.run.as_ref().is_none_or(ConditionSet::is_empty)
    }

    /// Compile all conditions. Collects every error instead of short-circuiting.
    pub fn compile(&mut self) -> Result<(), Vec<regex::Error>> {
        let mut errors: Vec<regex::Error> = Vec::new();
        for set in [&mut self.skip, &mut self.run].into_iter().flatten() {
            if let Err(mut set_errors) = set.compile() {
                errors.append(&mut set_errors);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validate non-regex configuration. Collects every error instead of short-circuiting.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        self.validate_with_run_profile(ConditionValidationProfile::EntrySync)
    }

    /// Permit local outcome dependencies only in this Redis entry's run set.
    pub fn validate_for_rate_limit_redis(&self) -> Result<(), Vec<String>> {
        self.validate_with_run_profile(ConditionValidationProfile::RateLimitRedisRun)
    }

    fn validate_with_run_profile(&self, run_profile: ConditionValidationProfile) -> Result<(), Vec<String>> {
        let mut errors: Vec<String> = Vec::new();
        for (set, profile) in [
            (&self.skip, ConditionValidationProfile::EntrySync),
            (&self.run, run_profile),
        ] {
            let Some(set) = set else { continue };
            if let Err(mut set_errors) = set.validate(profile) {
                errors.append(&mut set_errors);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn atomic_count(&self) -> usize {
        self.skip.as_ref().map_or(0, ConditionSet::atomic_count)
            + self.run.as_ref().map_or(0, ConditionSet::atomic_count)
    }
}

fn normalize_paths_for_key(key: &KeyGet) -> bool {
    matches!(key, KeyGet::Path)
}

/// JWT claim match. `claim` is a dot-nested path into stored claims JSON.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JwtClaimMatchCondition {
    /// Dot-nested claim path (e.g. `realm_access.roles`).
    pub claim: String,

    /// Exact values / regex / case-sensitivity (flattened onto the condition object).
    #[serde(flatten)]
    #[schemars(flatten)]
    pub core: MatchCore,

    /// Invert the result after the missing-value check. Default false.
    #[serde(default)]
    pub negate: bool,

    /// Runtime-only match descriptor (`<ordinal>:<type>[:<accessor>]`).
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

/// mTLS client-certificate match.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MtlsMatchCondition {
    /// Certificate field to read.
    pub field: MtlsField,

    /// SAN type filter — only meaningful for `field: san`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub san_type: Option<SanType>,

    /// Exact values / regex / case-sensitivity (flattened onto the condition object).
    #[serde(flatten)]
    #[schemars(flatten)]
    pub core: MatchCore,

    /// Invert the result after the missing-value check. Default false.
    #[serde(default)]
    pub negate: bool,

    /// Runtime-only match descriptor (`<ordinal>:<type>[:<accessor>]`).
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

impl KeyMatchCondition {
    pub fn compile(&mut self) -> Result<(), regex::Error> {
        self.core.compile(normalize_paths_for_key(&self.key))
    }
}

impl JwtClaimMatchCondition {
    pub fn compile(&mut self) -> Result<(), regex::Error> {
        self.core.compile(false)
    }
}

impl MtlsMatchCondition {
    pub fn compile(&mut self) -> Result<(), regex::Error> {
        self.core.compile(false)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/conditions_test.rs"]
mod tests;
