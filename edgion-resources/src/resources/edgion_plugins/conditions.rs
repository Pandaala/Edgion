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
use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::path_normalization::{get_path_normalization_config, normalize_config_path};
use crate::resources::common::SecretObjectReference;
use crate::resources::config_data_ref::EdgionConfigDataRef;
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

/// Inline and referenced IP ranges evaluated as one union.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpAccessMatch {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ips: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ip_refs: Vec<EdgionConfigDataRef>,

    #[serde(default)]
    pub negate: bool,

    /// Controller-expanded union of inline IPs and every usable IpList ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_ips: Option<Vec<String>>,

    /// Receiver-local matcher compiled from `resolvedIps` or inline `ips`.
    #[serde(skip)]
    #[schemars(skip)]
    pub matcher: Option<IpRadixMatcher>,

    /// Runtime-only match descriptor (`<ordinal>:<type>`).
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

impl Default for IpAccessMatch {
    fn default() -> Self {
        Self {
            ips: Vec::new(),
            ip_refs: Vec::new(),
            negate: false,
            resolved_ips: None,
            matcher: None,
            descriptor: Box::from(""),
        }
    }
}

impl IpAccessMatch {
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        if self.ips.is_empty() && self.ip_refs.is_empty() {
            return Err("ips or ipRefs must be configured".to_string());
        }
        for (index, reference) in self.ip_refs.iter().enumerate() {
            validate_nonempty_name(&reference.name, &format!("ipRefs[{index}].name"))?;
        }
        let effective = self.resolved_ips.as_ref().unwrap_or(&self.ips);
        if effective.is_empty() {
            self.matcher = None;
            return Ok(());
        }
        // Built without `ip_cidr_helpers` / `IpGroup` so this module does not
        // load `edgion_config_data` (that parent compiles the URL allow-list,
        // which imports `plugin_configs` → this file).
        let mut builder = IpRadixMatcher::builder();
        let mut any_inserted = false;
        for cidr in effective {
            let normalized = to_cidr_format(cidr);
            if self.resolved_ips.is_some() {
                // ConfigData IpList resources are partial-good: retain valid
                // siblings from the controller-expanded union. All-invalid
                // input must compile to an empty matcher (never fail RAP
                // init) so sibling anyOf rules still run.
                if let Ok(normalized) = normalized {
                    if builder.insert(&normalized, "request-access-policy").is_ok() {
                        any_inserted = true;
                    }
                }
            } else {
                let normalized = normalized?;
                builder
                    .insert(&normalized, "request-access-policy")
                    .map_err(|error| error.to_string())?;
                any_inserted = true;
            }
        }
        self.matcher = Some(if self.resolved_ips.is_some() && !any_inserted {
            IpRadixMatcher::empty()
        } else {
            builder.build().map_err(|error| error.to_string())?
        });
        Ok(())
    }
}

fn to_cidr_format(ip_str: &str) -> Result<String, String> {
    if ip_str.contains('/') {
        Ok(ip_str.to_string())
    } else {
        let ip: std::net::IpAddr = ip_str.parse().map_err(|_| format!("Invalid IP address: {ip_str}"))?;
        let prefix_len = if ip.is_ipv4() { 32 } else { 128 };
        Ok(format!("{ip_str}/{prefix_len}"))
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
        }
    }

    fn assign_descriptor(&mut self, ordinal: usize) {
        let ty = self.cond_type();
        let accessor = match self {
            Condition::KeyExist(c) => Some(c.key.as_log_str()),
            Condition::KeyMatch(c) => Some(c.key.as_log_str()),
            Condition::SecretMatch(c) => Some(c.key.as_log_str()),
            Condition::JwtClaimMatch(_) | Condition::MtlsMatch(_) => None,
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
        }
    }

    /// Compile regex / IP matchers and assign the runtime-only descriptor.
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
                // Matcher init errors are reported by placement-profile validation
                // as String; compile still assigns the descriptor so entry-level
                // DirectPeerIpMatch can run when the fields are valid.
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
                if profile == ConditionValidationProfile::EntrySync && c.validate_request_body {
                    return Err(HMAC_BODY_ENTRY_LEVEL_ERROR.to_string());
                }
                Ok(())
            }
            Condition::JwtClaimMatch(c) => {
                validate_nonempty_name(&c.claim, "jwtClaimMatch.claim")?;
                validate_match_core(&c.core, "jwtClaimMatch")
            }
            Condition::MtlsMatch(c) => validate_match_core(&c.core, "mtlsMatch"),
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
        let mut errors: Vec<String> = Vec::new();
        for set in [&self.skip, &self.run].into_iter().flatten() {
            if let Err(mut set_errors) = set.validate(ConditionValidationProfile::EntrySync) {
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
mod tests {
    use super::*;
    use crate::resources::edgion_plugins::hmac_match::HmacMatchConfig;

    fn core_values(values: Vec<String>) -> MatchCore {
        MatchCore {
            values: Some(values),
            ..MatchCore::default()
        }
    }

    fn core_regex(regex: Vec<String>) -> MatchCore {
        MatchCore {
            regex: Some(regex),
            ..MatchCore::default()
        }
    }

    #[test]
    fn test_plugin_conditions_default() {
        let conditions = PluginConditions::default();
        assert!(conditions.is_empty());
        assert!(conditions.skip.is_none());
        assert!(conditions.run.is_none());
    }

    #[test]
    fn plugin_conditions_reject_empty_match_payloads_even_when_negated() {
        let conditions = PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![
                Condition::KeyMatch(KeyMatchCondition {
                    key: KeyGet::Header {
                        name: "X-Test".to_string(),
                    },
                    core: MatchCore::default(),
                    negate: true,
                    descriptor: Box::from(""),
                }),
                Condition::JwtClaimMatch(JwtClaimMatchCondition {
                    claim: "sub".to_string(),
                    core: MatchCore::default(),
                    negate: true,
                    descriptor: Box::from(""),
                }),
                Condition::MtlsMatch(MtlsMatchCondition {
                    field: MtlsField::Cn,
                    san_type: None,
                    core: MatchCore::default(),
                    negate: true,
                    descriptor: Box::from(""),
                }),
            ])),
        };

        let errors = conditions
            .validate()
            .expect_err("empty match payloads must be rejected before runtime");
        assert_eq!(errors.len(), 3, "{errors:?}");
        for condition_type in ["keyMatch", "jwtClaimMatch", "mtlsMatch"] {
            assert!(
                errors.iter().any(|error| error.contains(condition_type)),
                "missing {condition_type} error: {errors:?}"
            );
        }
    }

    #[test]
    fn plugin_conditions_reject_empty_values_even_with_valid_regex() {
        let core = || MatchCore {
            values: Some(vec![]),
            regex: Some(vec!["^allowed$".to_string()]),
            ..MatchCore::default()
        };
        let conditions = PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![
                Condition::KeyMatch(KeyMatchCondition {
                    key: KeyGet::Path,
                    core: core(),
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::JwtClaimMatch(JwtClaimMatchCondition {
                    claim: "sub".to_string(),
                    core: core(),
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::MtlsMatch(MtlsMatchCondition {
                    field: MtlsField::Cn,
                    san_type: None,
                    core: core(),
                    negate: false,
                    descriptor: Box::from(""),
                }),
            ])),
        };

        let errors = conditions
            .validate()
            .expect_err("an explicitly empty values array must always be rejected");
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors.iter().all(|error| error.contains("values cannot be empty")));
    }

    #[test]
    fn plugin_conditions_reject_empty_named_key_get() {
        let conditions = PluginConditions {
            skip: Some(ConditionSet::any_of(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Ctx { name: String::new() },
                negate: false,
                descriptor: Box::from(""),
            })])),
            run: None,
        };

        let errors = conditions.validate().expect_err("empty key name must be rejected");
        assert!(errors.iter().any(|error| error.contains("keyExist.key.name")));
    }

    #[test]
    fn plugin_conditions_reject_blank_jwt_claim_even_when_negated() {
        let conditions = PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![Condition::JwtClaimMatch(
                JwtClaimMatchCondition {
                    claim: "  \t".to_string(),
                    core: core_values(vec!["admin".to_string()]),
                    negate: true,
                    descriptor: Box::from(""),
                },
            )])),
        };

        let errors = conditions
            .validate()
            .expect_err("blank JWT claim must be rejected before runtime");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("jwtClaimMatch.claim cannot be empty"));
    }

    #[test]
    fn secret_match_requires_nonempty_secret_key_and_uses_camel_case_serde() {
        let missing = r#"
type: secretMatch
key: { type: header, name: X-Token }
secretRef: { name: access-token }
"#;
        assert!(
            serde_yaml::from_str::<Condition>(missing).is_err(),
            "secretKey is a required operator field"
        );

        let blank = r#"
type: secretMatch
key: { type: header, name: X-Token }
secretKey: "  "
secretRef: { name: access-token }
"#;
        let condition: Condition = serde_yaml::from_str(blank).expect("secretMatch shape deserializes");
        let error = condition.validate().expect_err("blank secretKey must be rejected");
        assert!(error.contains("secretMatch.secretKey cannot be empty"), "{error}");

        let valid = r#"
type: secretMatch
key: { type: header, name: X-Token }
secretKey: token
secretRef: { name: access-token }
"#;
        let condition: Condition = serde_yaml::from_str(valid).expect("secretMatch with secretKey deserializes");
        assert!(condition.validate().is_ok());
        let yaml = serde_yaml::to_string(&condition).unwrap();
        assert!(yaml.contains("secretKey: token"), "{yaml}");
    }

    #[test]
    fn test_serialize_deserialize() {
        let conditions = PluginConditions {
            skip: Some(ConditionSet::any_of(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Test".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
            run: Some(ConditionSet::all_of(vec![Condition::Probability(
                ProbabilityCondition {
                    ratio: 0.5,
                    key: None,
                    negate: false,
                    descriptor: Box::from(""),
                },
            )])),
        };

        let yaml = serde_yaml::to_string(&conditions).unwrap();
        let deserialized: PluginConditions = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(deserialized.skip.as_ref().unwrap().len(), 1);
        assert_eq!(deserialized.run.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_key_match_compile_single_regex() {
        let mut condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "User-Agent".to_string(),
            },
            core: core_regex(vec![r"^Mozilla.*".to_string()]),
            negate: false,
            descriptor: Box::from(""),
        };

        assert!(condition.compile().is_ok());
        assert!(condition.core.compiled_regex.is_some());

        let regex = condition.core.compiled_regex.as_ref().unwrap();
        assert!(regex.is_match("Mozilla/5.0"));
        assert!(!regex.is_match("Chrome/100.0"));
    }

    #[test]
    fn test_key_match_compile_multi_regex() {
        let mut condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "User-Agent".to_string(),
            },
            core: core_regex(vec![
                r"^Mozilla.*".to_string(),
                r"^Chrome.*".to_string(),
                r"(?i:^safari.*)".to_string(), // case-insensitive
            ]),
            negate: false,
            descriptor: Box::from(""),
        };

        assert!(condition.compile().is_ok());
        assert!(condition.core.compiled_regex.is_some());

        let regex = condition.core.compiled_regex.as_ref().unwrap();
        // Should match Mozilla
        assert!(regex.is_match("Mozilla/5.0"));
        // Should match Chrome
        assert!(regex.is_match("Chrome/100.0"));
        // Should match Safari (case-insensitive)
        assert!(regex.is_match("Safari/605.1"));
        assert!(regex.is_match("safari/605.1"));
        assert!(regex.is_match("SAFARI/605.1"));
        // Should not match curl
        assert!(!regex.is_match("curl/7.64.1"));
    }

    #[test]
    fn test_key_match_compile_hashset() {
        // Create more than 16 values to trigger HashSet compilation
        let values: Vec<String> = (0..20).map(|i| format!("value{}", i)).collect();

        let mut condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "X-Test".to_string(),
            },
            core: core_values(values),
            negate: false,
            descriptor: Box::from(""),
        };

        assert!(condition.compile().is_ok());
        assert!(condition.core.values_set.is_some());

        let set = condition.core.values_set.as_ref().unwrap();
        assert_eq!(set.len(), 20);
        assert!(set.contains("value0"));
        assert!(set.contains("value19"));
        assert!(!set.contains("value20"));
    }

    #[test]
    fn test_key_match_no_hashset_below_threshold() {
        // Create exactly 16 values (threshold, should NOT trigger HashSet)
        let values: Vec<String> = (0..16).map(|i| format!("value{}", i)).collect();

        let mut condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "X-Test".to_string(),
            },
            core: core_values(values),
            negate: false,
            descriptor: Box::from(""),
        };

        assert!(condition.compile().is_ok());
        assert!(condition.core.values_set.is_none()); // Should NOT be compiled
    }

    #[test]
    fn test_key_match_compile_hashset_case_insensitive_folds() {
        let mut values: Vec<String> = (0..20).map(|i| format!("value{}", i)).collect();
        values.push("Production".to_string());
        let mut condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "X-Test".to_string(),
            },
            core: MatchCore {
                values: Some(values),
                case_sensitive: false,
                ..MatchCore::default()
            },
            negate: false,
            descriptor: Box::from(""),
        };

        assert!(condition.compile().is_ok());
        let set = condition.core.values_set.as_ref().unwrap();
        assert!(set.contains("production"));
        assert!(!set.contains("Production"));
        assert!(condition.core.matches("PRODUCTION"));
        assert!(condition.core.matches("production"));
    }

    #[test]
    fn test_plugin_conditions_compile_all_independent() {
        // Mix of valid/invalid across skip and run; compile() must return EVERY
        // error (not just the first) while still compiling the valid conditions
        // and clearing the failing ones' regex to prevent per-request push_err.
        //
        // Layout (order matters — errors are returned in skip → run order):
        //   skip[0] invalid (unclosed bracket)
        //   skip[1] valid
        //   skip[2] invalid (lone repeat)
        //   run[0]  valid
        //   run[1]  invalid (mismatched paren)
        let invalid_bracket = Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Path,
            core: core_regex(vec!["[invalid".to_string()]),
            negate: false,
            descriptor: Box::from(""),
        });
        let valid_skip = Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Path,
            core: core_regex(vec![r"^/api/.*".to_string()]),
            negate: false,
            descriptor: Box::from(""),
        });
        let invalid_repeat = Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Path,
            core: core_regex(vec!["*foo".to_string()]),
            negate: false,
            descriptor: Box::from(""),
        });
        let valid_run = Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Path,
            core: core_regex(vec![r"^/health$".to_string()]),
            negate: true,
            descriptor: Box::from(""),
        });
        let invalid_paren = Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Path,
            core: core_regex(vec!["(unclosed".to_string()]),
            negate: true,
            descriptor: Box::from(""),
        });
        let mut conds = PluginConditions {
            skip: Some(ConditionSet::any_of(vec![invalid_bracket, valid_skip, invalid_repeat])),
            run: Some(ConditionSet::all_of(vec![valid_run, invalid_paren])),
        };

        let errs = conds.compile().expect_err("should collect every regex compile error");
        assert_eq!(errs.len(), 3, "must report all 3 invalid patterns, not just the first");

        let skip: Vec<_> = conds.skip.as_ref().unwrap().iter_conditions().collect();
        // skip[0] invalid → regex cleared
        if let Condition::KeyMatch(c) = &skip[0] {
            assert!(c.core.regex.is_none(), "skip[0] regex must be cleared");
            assert!(c.core.compiled_regex.is_none());
        } else {
            panic!("expected KeyMatch at skip[0]");
        }
        // skip[1] valid → compiled
        if let Condition::KeyMatch(c) = &skip[1] {
            assert!(
                c.core.compiled_regex.is_some(),
                "skip[1] valid condition must be compiled"
            );
        } else {
            panic!("expected KeyMatch at skip[1]");
        }
        // skip[2] invalid → regex cleared
        if let Condition::KeyMatch(c) = &skip[2] {
            assert!(c.core.regex.is_none(), "skip[2] regex must be cleared");
            assert!(c.core.compiled_regex.is_none());
        } else {
            panic!("expected KeyMatch at skip[2]");
        }

        let run: Vec<_> = conds.run.as_ref().unwrap().iter_conditions().collect();
        // run[0] valid → compiled
        if let Condition::KeyMatch(c) = &run[0] {
            assert!(
                c.core.compiled_regex.is_some(),
                "run[0] valid condition must be compiled"
            );
        } else {
            panic!("expected KeyMatch at run[0]");
        }
        // run[1] invalid → regex cleared
        if let Condition::KeyMatch(c) = &run[1] {
            assert!(c.core.regex.is_none(), "run[1] regex must be cleared");
            assert!(c.core.compiled_regex.is_none());
        } else {
            panic!("expected KeyMatch at run[1]");
        }
    }

    #[test]
    fn test_plugin_conditions_validate_collects_all_errors() {
        // Two TimeRange conditions with malformed bounds across skip and run.
        // validate() must return both errors (not just the first), in skip → run order.
        let bad_after = Condition::TimeRange(TimeRangeCondition {
            after: Some("not-a-timestamp".to_string()),
            before: None,
            negate: false,
            descriptor: Box::from(""),
        });
        let bad_before = Condition::TimeRange(TimeRangeCondition {
            after: None,
            before: Some("also-bad".to_string()),
            negate: false,
            descriptor: Box::from(""),
        });
        let conds = PluginConditions {
            skip: Some(ConditionSet::any_of(vec![bad_after])),
            run: Some(ConditionSet::all_of(vec![bad_before])),
        };

        let errs = conds.validate().expect_err("should collect every validate error");
        assert_eq!(errs.len(), 2, "must report both malformed bounds, not just the first");
    }

    #[test]
    fn test_key_get_serialization() {
        // KeyGet uses tagged enum with lower camel-case values.
        assert_eq!(
            serde_json::to_string(&KeyGet::RemoteIp).unwrap(),
            r#"{"type":"remoteIp"}"#
        );
        assert_eq!(
            serde_json::to_string(&KeyGet::Header {
                name: "X-Test".to_string()
            })
            .unwrap(),
            r#"{"type":"header","name":"X-Test"}"#
        );
        assert_eq!(serde_json::to_string(&KeyGet::Path).unwrap(), r#"{"type":"path"}"#);
    }

    #[test]
    fn unknown_key_paths_reports_typo_inside_flattened_match_core() {
        // First production `#[serde(flatten)]` on Condition: a typo inside the
        // flattened MatchCore region must be reported against the real
        // EdgionPlugins spec schema, not a toy struct.
        let schema = crate::resources::edgion_plugins::edgion_plugins_spec_schema();
        let value = serde_json::json!({
            "requestPlugins": [{
                "type": "IpRestriction",
                "config": {"defaultAction": "allow"},
                "conditions": {
                    "run": {
                        "allOf": [{
                            "type": "keyMatch",
                            "key": {"type": "header", "name": "X-Test"},
                            "values": ["ok"],
                            "valuez": "typo"
                        }]
                    }
                }
            }]
        });
        let findings = crate::schema_check::unknown_key_paths(&value, schema);
        assert!(
            findings.iter().any(|p| p.contains("valuez")),
            "flattened MatchCore typo must be reported, got {findings:?}"
        );
    }

    #[test]
    fn condition_size_stays_under_named_ceiling() {
        let measured = std::mem::size_of::<Condition>();
        eprintln!("size_of::<Condition>()={measured} CONDITION_SIZE_CEILING={CONDITION_SIZE_CEILING}");
        assert!(
            measured <= CONDITION_SIZE_CEILING,
            "size_of::<Condition>()={measured} exceeds CONDITION_SIZE_CEILING={CONDITION_SIZE_CEILING}"
        );
    }

    #[test]
    fn diagnostic_descriptor_is_value_free_and_ordinal_bearing() {
        let secret = "super-secret-token";
        let mut conds = PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![
                Condition::KeyMatch(KeyMatchCondition {
                    key: KeyGet::Header {
                        name: "X-A".to_string(),
                    },
                    core: core_values(vec![secret.to_string()]),
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::KeyMatch(KeyMatchCondition {
                    key: KeyGet::Header {
                        name: "X-A".to_string(),
                    },
                    core: core_values(vec![secret.to_string()]),
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::KeyExist(KeyExistCondition {
                    key: KeyGet::Header {
                        name: "X-B".to_string(),
                    },
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::TimeRange(TimeRangeCondition {
                    after: Some("2020-01-01T00:00:00Z".to_string()),
                    before: None,
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::Probability(ProbabilityCondition {
                    ratio: 0.5,
                    key: None,
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::Probability(ProbabilityCondition {
                    ratio: 0.5,
                    key: Some(KeyGet::Header {
                        name: "X-User".to_string(),
                    }),
                    negate: false,
                    descriptor: Box::from(""),
                }),
            ])),
        };
        conds.compile().expect("compile assigns descriptors");
        let run: Vec<_> = conds.run.as_ref().unwrap().iter_conditions().collect();
        assert_eq!(run[0].descriptor(), "0:keyMatch:hdr:X-A");
        assert_eq!(run[1].descriptor(), "1:keyMatch:hdr:X-A");
        assert_eq!(run[2].descriptor(), "2:keyExist:hdr:X-B");
        assert_eq!(run[3].descriptor(), "3:timeRange");
        assert_eq!(run[4].descriptor(), "4:probability");
        assert_eq!(run[5].descriptor(), "5:probability:hdr:X-User");
        for c in run {
            assert!(
                !c.descriptor().contains(secret),
                "descriptor must not contain match values: {}",
                c.descriptor()
            );
        }
    }

    #[test]
    fn empty_skip_and_run_compile_ok() {
        let mut empty = PluginConditions::default();
        empty.compile().expect("empty skip/run compile");
        let mut none_vecs = PluginConditions {
            skip: Some(ConditionSet::any_of(vec![])),
            run: Some(ConditionSet::all_of(vec![])),
        };
        none_vecs.compile().expect("empty vecs compile");
    }

    fn hmac_body_yaml(validate_body: bool) -> String {
        format!(
            r#"
type: hmacAuth
secretGroups:
  - name: default
    secretRefs:
      - name: hmac-credentials
algorithms: [hmac-sha256]
validateRequestBody: {validate_body}
"#
        )
    }

    #[test]
    fn hmac_match_condition_rejects_standalone_only_fields() {
        for field in [
            "anonymous: guest",
            "realm: edge",
            "authFailureDelayMs: 1",
            "hideCredentials: true",
            "upstreamHeaderFields: [X-Consumer]",
        ] {
            let yaml = format!("type: hmacAuth\n{field}\n");
            assert!(serde_yaml::from_str::<Condition>(&yaml).is_err(), "accepted: {field}");
        }
    }

    #[test]
    fn hmac_match_condition_redacts_admin_output_and_round_trips_conf_sync() {
        let condition = Condition::HmacAuth(HmacMatchConfig {
            resolved_credentials: Some(std::collections::HashMap::from([(
                "alice".to_string(),
                crate::resources::edgion_plugins::plugin_configs::HmacCredential {
                    secret: b"policy-conf-sync-secret".to_vec(),
                    headers: std::collections::HashMap::new(),
                    group: "default".to_string(),
                },
            )])),
            ..Default::default()
        });

        let admin_json = {
            let _guard = crate::redact::AdminRedactGuard::new();
            serde_json::to_string(&condition).unwrap()
        };
        assert!(!admin_json.contains("policy-conf-sync-secret"), "{admin_json}");
        assert!(admin_json.contains("[redacted]"), "{admin_json}");

        let wire = serde_json::to_vec(&condition).unwrap();
        let decoded: Condition = serde_json::from_slice(&wire).unwrap();
        let Condition::HmacAuth(config) = decoded else {
            panic!("unexpected condition type")
        };
        assert_eq!(
            config
                .resolved_credentials
                .as_ref()
                .and_then(|credentials| credentials.get("alice"))
                .map(|credential| credential.secret.as_slice()),
            Some(b"policy-conf-sync-secret".as_slice())
        );
    }

    #[test]
    fn entry_level_hmac_validate_request_body_is_rejected() {
        let condition: Condition = serde_yaml::from_str(&hmac_body_yaml(true)).unwrap();
        let err = condition
            .validate()
            .expect_err("entry-level HMAC body must be rejected");
        assert!(err.contains("validateRequestBody"), "{err}");

        let conds = PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![condition])),
        };
        let errs = conds.validate().expect_err("PluginConditions uses entry-level path");
        assert!(errs.iter().any(|e| e.contains("validateRequestBody")));
    }

    #[test]
    fn request_async_set_accepts_hmac_validate_request_body() {
        let condition: Condition = serde_yaml::from_str(&hmac_body_yaml(true)).unwrap();
        ConditionSet::all_of(vec![condition])
            .validate(ConditionValidationProfile::RequestAsync)
            .expect("request-async ConditionSet accepts validateRequestBody");
    }

    #[test]
    fn hmac_fields_stay_siblings_of_type() {
        let condition: Condition = serde_yaml::from_str(&hmac_body_yaml(false)).unwrap();
        let yaml = serde_yaml::to_string(&condition).unwrap();
        assert!(yaml.contains("type: hmacAuth"), "{yaml}");
        assert!(yaml.contains("secretGroups:"), "{yaml}");
        assert!(!yaml.contains("hmac:"), "{yaml}");
        assert!(!yaml.contains("matchConfig"), "{yaml}");
    }

    #[test]
    fn path_key_match_normalizes_exact_values_request_path_does_not() {
        let mut path_match = KeyMatchCondition {
            key: KeyGet::Path,
            core: core_values(vec!["/%61dmin".to_string()]),
            negate: false,
            descriptor: Box::from(""),
        };
        path_match.compile().expect("path keyMatch compiles");
        assert_eq!(path_match.core.values.as_ref().unwrap()[0], "/admin");

        let mut request_path_match = KeyMatchCondition {
            key: KeyGet::RequestPath,
            core: core_values(vec!["/%61dmin".to_string()]),
            negate: false,
            descriptor: Box::from(""),
        };
        request_path_match.compile().expect("requestPath keyMatch compiles");
        assert_eq!(request_path_match.core.values.as_ref().unwrap()[0], "/%61dmin");
    }

    #[test]
    fn jwt_claim_and_mtls_match_compile_without_path_normalization() {
        let mut jwt = JwtClaimMatchCondition {
            claim: "sub".to_string(),
            core: core_values(vec!["/%61dmin".to_string()]),
            negate: false,
            descriptor: Box::from(""),
        };
        jwt.compile().expect("jwtClaimMatch compiles");
        assert_eq!(jwt.core.values.as_ref().unwrap()[0], "/%61dmin");

        let mut mtls = MtlsMatchCondition {
            field: MtlsField::San,
            san_type: Some(SanType::Dns),
            core: core_values(vec!["/%61dmin".to_string()]),
            negate: false,
            descriptor: Box::from(""),
        };
        mtls.compile().expect("mtlsMatch compiles");
        assert_eq!(mtls.core.values.as_ref().unwrap()[0], "/%61dmin");
    }

    #[test]
    fn jwt_claim_and_mtls_descriptors_are_value_free() {
        let secret = "super-secret-token";
        let mut conds = PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![
                Condition::JwtClaimMatch(JwtClaimMatchCondition {
                    claim: "sub".to_string(),
                    core: core_values(vec![secret.to_string()]),
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::MtlsMatch(MtlsMatchCondition {
                    field: MtlsField::San,
                    san_type: None,
                    core: core_values(vec![secret.to_string()]),
                    negate: false,
                    descriptor: Box::from(""),
                }),
            ])),
        };
        conds.compile().expect("compile assigns descriptors");
        let run: Vec<_> = conds.run.as_ref().unwrap().iter_conditions().collect();
        assert_eq!(run[0].descriptor(), "0:jwtClaimMatch");
        assert_eq!(run[1].descriptor(), "1:mtlsMatch");
        for c in run {
            assert!(
                !c.descriptor().contains(secret),
                "descriptor must not contain match values: {}",
                c.descriptor()
            );
        }
    }
}
