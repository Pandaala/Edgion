//! Shared request-attribute match primitives.
//!
//! These predicate types are shared by the `request_restriction` plugin and the
//! `global_access_control` plugin so their header/cookie/query/path/method/referer
//! matching semantics cannot drift. The grouping/polarity types (`RuleGroup`,
//! `RestrictionMode`, `RequestRestrictionConfig`) stay in `request_restriction`.

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::path_normalization::normalize_config_path;

/// Threshold for pre-compiling values into a HashSet for O(1) lookup.
/// When values.len() > this threshold, a HashSet will be built during compilation.
pub const VALUES_HASHSET_THRESHOLD: usize = 16;

/// A single predicate. Answers one boolean question: does the extracted value match?
///
/// Predicate is true iff `value ∈ values` OR `value` matches `regex`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestrictionRule {
    /// Predicate name for logging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Data source to extract the value from.
    pub source: RestrictionSource,

    /// Key to extract (required for Header/Cookie/Query; ignored otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    /// Exact-match values. Auto HashSet when len > VALUES_HASHSET_THRESHOLD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,

    /// Regex patterns (merged with `|` into a single compiled regex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<Vec<String>>,

    /// Case sensitivity for exact match (default: true).
    /// For regex, use (?i:pattern) for case-insensitive matching.
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,

    /// Behavior when the source value is absent (default: NoMatch).
    #[serde(default)]
    pub on_missing: OnMissing,

    /// SAN type filter — only meaningful for `MtlsSan`. When `None`, raw SAN
    /// strings (with their stored type prefix) are whole-string matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub san_type: Option<SanType>,

    // === Runtime-only compiled matchers ===
    /// HashSet for O(1) exact lookup (when values.len() > threshold).
    #[serde(skip)]
    #[schemars(skip)]
    pub values_set: Option<HashSet<String>>,

    /// Compiled regex (merged from all patterns).
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<Regex>,

    /// Reference to a K8s Secret supplying expected match value(s). Resolved by the
    /// controller into `resolved_secret_values`. Use for credential-bearing headers
    /// (e.g. a CDN-injected origin secret) so the value is not stored inline in the CR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<crate::resources::gateway::SecretObjectReference>,

    /// Controller-resolved expected value(s) from `secret_ref`. Runtime-only:
    /// hidden from the CRD schema and redacted on the admin API, but shipped to the
    /// gateway via conf_sync. Matched as exact, case-sensitive (secrets are never folded).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_secret_values: Option<Vec<String>>,
}

/// Data source for a restriction predicate.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum RestrictionSource {
    /// HTTP request header (requires key)
    Header,
    /// HTTP cookie (requires key)
    Cookie,
    /// URL query parameter (requires key)
    Query,
    /// Request path (no key needed)
    Path,
    /// Entry-time raw path (frozen). For audit / special raw-form match.
    RequestPath,
    /// HTTP method (no key needed)
    Method,
    /// Referer header (no key needed)
    Referer,
    /// mTLS client-certificate subject DN string (no key needed).
    MtlsSubject,
    /// mTLS client-certificate subject CN (no key needed).
    MtlsCn,
    /// mTLS client-certificate SAN list; "any entry matches". Optional `sanType`
    /// scopes matching to one SAN kind.
    MtlsSan,
    /// Structured claim of a verified JWT; `key` is a dot-nested claim path.
    JwtClaim,
}

/// SAN type filter for `MtlsSan`. When set, matching is scoped to SAN entries of
/// this type, on the prefix-stripped value. `IP` is intentionally not offered
/// (no IP-based matching in this plugin).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum SanType {
    #[serde(rename = "DNS")]
    Dns,
    #[serde(rename = "URI")]
    Uri,
    #[serde(rename = "Email")]
    Email,
}

/// Behavior when the target value is missing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "PascalCase")]
pub enum OnMissing {
    /// Missing value => predicate is TRUE.
    Match,
    /// Missing value => predicate is FALSE (default).
    #[default]
    NoMatch,
    /// Missing value => predicate is DROPPED from the group's AND.
    Skip,
}

fn default_case_sensitive() -> bool {
    true
}

impl Default for RestrictionRule {
    fn default() -> Self {
        Self {
            name: None,
            source: RestrictionSource::Header,
            key: None,
            values: None,
            regex: None,
            case_sensitive: default_case_sensitive(),
            on_missing: OnMissing::default(),
            san_type: None,
            values_set: None,
            compiled_regex: None,
            secret_ref: None,
            resolved_secret_values: None,
        }
    }
}

impl RestrictionRule {
    /// Compile the predicate: normalize Path exact values, build the HashSet,
    /// and merge regex patterns.
    ///
    /// For `Path`-source rules, exact-match `values` are normalized at load time
    /// via [`normalize_config_path`] so they inhabit the same normalized space as
    /// the request path produced by `session.get_path()`. `RequestPath`-source and
    /// all other sources are left as-is (raw form). Regex patterns are never
    /// normalized.
    pub fn compile(&mut self) -> Result<(), String> {
        let rule_name = self.name.as_deref().unwrap_or("<unnamed>");

        if self.source == RestrictionSource::Path {
            let path_cfg = crate::path_normalization::get_path_normalization_config();
            if let Some(values) = self.values.as_mut() {
                for v in values.iter_mut() {
                    let normalized = normalize_config_path(v, path_cfg);
                    if normalized != *v {
                        tracing::info!(
                            "request_restriction rule '{}': Path value normalized: {:?} -> {:?}",
                            rule_name,
                            v,
                            normalized
                        );
                        *v = normalized;
                    }
                }
            }
        }

        if let Some(values) = &self.values {
            if values.is_empty() {
                return Err("'values' list cannot be empty".to_string());
            }
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

        if let Some(patterns) = &self.regex {
            if patterns.is_empty() {
                return Err("'regex' list cannot be empty".to_string());
            }
            let combined = patterns
                .iter()
                .map(|p| format!("({})", p))
                .collect::<Vec<_>>()
                .join("|");
            self.compiled_regex = Some(Regex::new(&combined).map_err(|e| format!("Invalid regex: {}", e))?);
        }

        Ok(())
    }

    /// True iff the present value is in `values` OR matches `regex`.
    fn matches_value(&self, value: &str) -> bool {
        if let Some(set) = &self.values_set {
            // ASCII-only folding mirrors the linear path's `eq_ignore_ascii_case`.
            let cv = if self.case_sensitive {
                value.to_string()
            } else {
                value.to_ascii_lowercase()
            };
            if set.contains(&cv) {
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

        if let Some(secrets) = &self.resolved_secret_values {
            // Exact, case-sensitive: a credential must match byte-for-byte.
            if secrets.iter().any(|s| s == value) {
                return true;
            }
        }

        false
    }

    /// Multi-value evaluation with "any candidate matches" semantics.
    ///
    /// An empty slice means the source value is absent → apply `on_missing`.
    /// A non-empty slice yields `Some(true)` iff any candidate satisfies the
    /// predicate (`values` / `regex` / resolved secret).
    pub fn evaluate_multi(&self, values: &[String]) -> Option<bool> {
        if values.is_empty() {
            return match self.on_missing {
                OnMissing::Match => Some(true),
                OnMissing::NoMatch => Some(false),
                OnMissing::Skip => None,
            };
        }
        Some(values.iter().any(|v| self.matches_value(v)))
    }

    /// Scalar convenience wrapper over [`evaluate_multi`].
    ///
    /// `None` (absent) routes through the empty-slice / `on_missing` path; a
    /// present scalar is a single-element "any matches", i.e. one `matches_value`
    /// call — identical to the pre-multi behavior.
    pub fn evaluate(&self, value: Option<&str>) -> Option<bool> {
        match value {
            None => self.evaluate_multi(&[]),
            Some(v) => Some(self.matches_value(v)),
        }
    }

    /// Get rule display name.
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{:?}:{}", self.source, self.key.as_deref().unwrap_or("")))
    }

    /// Stateless per-rule field validation. `None` = valid.
    pub fn field_error(&self) -> Option<String> {
        let rule_name = self.display_name();
        match self.source {
            RestrictionSource::Header | RestrictionSource::Cookie | RestrictionSource::Query => {
                if self.key.as_ref().is_none_or(|k| k.is_empty()) {
                    return Some(format!(
                        "rule '{}': 'key' is required for source {:?}",
                        rule_name, self.source
                    ));
                }
            }
            RestrictionSource::JwtClaim => {
                if self.key.as_ref().is_none_or(|k| k.is_empty()) {
                    return Some(format!(
                        "rule '{}': 'key' (claim path) is required for source JwtClaim",
                        rule_name
                    ));
                }
            }
            RestrictionSource::Path
            | RestrictionSource::RequestPath
            | RestrictionSource::Method
            | RestrictionSource::Referer
            | RestrictionSource::MtlsSubject
            | RestrictionSource::MtlsCn
            | RestrictionSource::MtlsSan => {}
        }
        if self.san_type.is_some() && self.source != RestrictionSource::MtlsSan {
            return Some(format!(
                "rule '{}': 'sanType' is only valid for source MtlsSan",
                rule_name
            ));
        }
        // A secret_ref supplies the match set at resolution time, so it counts as a match source.
        let has_match = self.values.is_some() || self.regex.is_some() || self.secret_ref.is_some();
        if !has_match && self.on_missing != OnMissing::Match {
            return Some(format!(
                "rule '{}': at least one of 'values', 'regex', or 'secretRef' must be specified (unless onMissing is Match)",
                rule_name
            ));
        }
        if let Some(values) = &self.values {
            if values.is_empty() {
                return Some(format!("rule '{}': 'values' list cannot be empty", rule_name));
            }
        }
        if let Some(patterns) = &self.regex {
            if patterns.is_empty() {
                return Some(format!("rule '{}': 'regex' list cannot be empty", rule_name));
            }
            let combined = patterns
                .iter()
                .map(|p| format!("({})", p))
                .collect::<Vec<_>>()
                .join("|");
            if let Err(e) = Regex::new(&combined) {
                return Some(format!("rule '{}': Invalid regex: {}", rule_name, e));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_rule(key: &str, values: &[&str]) -> RestrictionRule {
        RestrictionRule {
            source: RestrictionSource::Header,
            key: Some(key.to_string()),
            values: Some(values.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn test_evaluate_multi_any_match_and_missing() {
        let mut r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X".to_string()),
            values: Some(vec!["admin".to_string(), "dev".to_string()]),
            ..Default::default()
        };
        r.compile().unwrap();
        // Any element matches.
        assert_eq!(r.evaluate_multi(&["guest".to_string(), "dev".to_string()]), Some(true));
        assert_eq!(r.evaluate_multi(&["guest".to_string()]), Some(false));
        // Empty = missing => default OnMissing::NoMatch.
        assert_eq!(r.evaluate_multi(&[]), Some(false));

        let mut m = r.clone();
        m.on_missing = OnMissing::Match;
        assert_eq!(m.evaluate_multi(&[]), Some(true));
        let mut s = r.clone();
        s.on_missing = OnMissing::Skip;
        assert_eq!(s.evaluate_multi(&[]), None);
    }

    #[test]
    fn test_evaluate_values_and_regex() {
        let mut r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("User-Agent".to_string()),
            values: Some(vec!["curl".to_string()]),
            regex: Some(vec!["(?i).*bot.*".to_string()]),
            ..Default::default()
        };
        r.compile().unwrap();
        assert_eq!(r.evaluate(Some("curl")), Some(true)); // exact
        assert_eq!(r.evaluate(Some("Googlebot")), Some(true)); // regex
        assert_eq!(r.evaluate(Some("Mozilla")), Some(false)); // neither
    }

    #[test]
    fn test_evaluate_on_missing_three_states() {
        let mut m = header_rule("X", &["a"]);
        m.on_missing = OnMissing::Match;
        let mut n = header_rule("X", &["a"]);
        n.on_missing = OnMissing::NoMatch;
        let mut s = header_rule("X", &["a"]);
        s.on_missing = OnMissing::Skip;
        m.compile().unwrap();
        n.compile().unwrap();
        s.compile().unwrap();
        assert_eq!(m.evaluate(None), Some(true));
        assert_eq!(n.evaluate(None), Some(false));
        assert_eq!(s.evaluate(None), None);
    }

    #[test]
    fn test_case_insensitive_values() {
        let mut r = RestrictionRule {
            source: RestrictionSource::Method,
            values: Some(vec!["get".to_string()]),
            case_sensitive: false,
            ..Default::default()
        };
        r.compile().unwrap();
        assert_eq!(r.evaluate(Some("GET")), Some(true));
    }

    #[test]
    fn test_case_insensitive_ascii_only_folding_on_hashset_path() {
        // case_sensitive=false uses ASCII-only folding on BOTH the linear (<=16)
        // and the HashSet (>16) path. Build a >16 list to force the HashSet path
        // and confirm non-ASCII chars are not folded (consistent with the linear
        // path's eq_ignore_ascii_case).
        let mut values: Vec<String> = (0..20).map(|i| format!("v{}", i)).collect();
        values.push("ABC".to_string());
        values.push("CAF\u{00C9}".to_string()); // E with acute (U+00C9) is not folded by ASCII lowercasing
        let mut r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X".to_string()),
            values: Some(values),
            case_sensitive: false,
            ..Default::default()
        };
        r.compile().unwrap();
        assert!(r.values_set.is_some()); // HashSet path active
                                         // ASCII letters fold: "abc" matches "ABC".
        assert_eq!(r.evaluate(Some("abc")), Some(true));
        // Same-case non-ASCII still matches itself.
        assert_eq!(r.evaluate(Some("CAF\u{00C9}")), Some(true));
        // Non-ASCII is NOT folded: lowercase "caf\u{00E9}" does not match "CAF\u{00C9}".
        assert_eq!(r.evaluate(Some("caf\u{00E9}")), Some(false));
    }

    #[test]
    fn test_hashset_optimization() {
        let values: Vec<String> = (0..20).map(|i| format!("value{}", i)).collect();
        let mut r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X-Value".to_string()),
            values: Some(values),
            ..Default::default()
        };
        r.compile().unwrap();
        assert!(r.values_set.is_some()); // HashSet compiled above threshold
        assert_eq!(r.evaluate(Some("value5")), Some(true));
        assert_eq!(r.evaluate(Some("value99")), Some(false));
    }

    #[test]
    fn test_no_hashset_below_threshold() {
        let values: Vec<String> = (0..16).map(|i| format!("value{}", i)).collect();
        let mut r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X-Value".to_string()),
            values: Some(values),
            ..Default::default()
        };
        r.compile().unwrap();
        assert!(r.values_set.is_none()); // 16 == threshold, not above
        assert_eq!(r.evaluate(Some("value5")), Some(true));
    }

    #[test]
    fn path_values_normalized_at_compile() {
        // "/admin/./" -> mid-string `/./` removed; trailing `/` preserved.
        // "/api%2Fv1" -> %2F decoded to "/api/v1"
        let mut rule = RestrictionRule {
            name: Some("test-path-norm".to_string()),
            source: RestrictionSource::Path,
            values: Some(vec!["/admin/./".to_string(), "/api%2Fv1".to_string()]),
            ..Default::default()
        };
        rule.compile().expect("compile should succeed");

        let values = rule.values.as_ref().expect("values should be Some");
        assert_eq!(values[0], "/admin/", "mid-string `/./` removed; trailing `/` preserved");
        assert_eq!(values[1], "/api/v1", "encoded slash should be decoded");
    }

    #[test]
    fn path_values_percent_decoded_at_compile() {
        let mut rule = RestrictionRule {
            name: Some("test-path-allow-norm".to_string()),
            source: RestrictionSource::Path,
            values: Some(vec!["/%61dmin".to_string()]),
            ..Default::default()
        };
        rule.compile().expect("compile should succeed");

        let values = rule.values.as_ref().expect("values should be Some");
        assert_eq!(values[0], "/admin", "unreserved percent-encoded char should be decoded");
    }

    #[test]
    fn request_path_values_kept_raw() {
        // RequestPath source: values must NOT be normalized.
        let raw = "/api%2Fv1".to_string();
        let mut rule = RestrictionRule {
            name: Some("test-rp-raw".to_string()),
            source: RestrictionSource::RequestPath,
            values: Some(vec![raw.clone()]),
            ..Default::default()
        };
        rule.compile().expect("compile should succeed");

        let values = rule.values.as_ref().expect("values should be Some");
        assert_eq!(values[0], raw, "RequestPath values must be kept as-is (raw form)");
    }

    #[test]
    fn path_regex_not_normalized() {
        let pattern = "^/api%2Fv[0-9]+/.*".to_string();
        let mut rule = RestrictionRule {
            name: Some("test-regex-untouched".to_string()),
            source: RestrictionSource::Path,
            regex: Some(vec![pattern.clone()]),
            ..Default::default()
        };
        rule.compile().expect("compile should succeed");

        let regex = rule.regex.as_ref().expect("regex should be Some");
        assert_eq!(regex[0], pattern, "regex patterns must not be normalized");
    }

    #[test]
    fn public_path_reexport_is_stable() {
        // Downstream crates import these from plugin_configs; lock that path.
        use crate::resources::edgion_plugins::plugin_configs::{OnMissing, RestrictionRule, RestrictionSource};
        let r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X".into()),
            on_missing: OnMissing::Match,
            ..Default::default()
        };
        assert_eq!(r.evaluate(None), Some(true));
    }

    #[test]
    fn matches_resolved_secret_values_exact_case_sensitive() {
        let mut r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X-CF-Secret".into()),
            ..Default::default()
        };
        // Simulate controller resolution.
        r.resolved_secret_values = Some(vec!["s3cr3t-TOKEN".into()]);
        r.compile().unwrap();
        assert_eq!(r.evaluate(Some("s3cr3t-TOKEN")), Some(true));
        // Secrets are matched case-sensitively, never folded.
        assert_eq!(r.evaluate(Some("s3cr3t-token")), Some(false));
        assert_eq!(r.evaluate(Some("nope")), Some(false));
    }

    #[test]
    fn secret_ref_only_rule_is_field_valid() {
        // A rule with only secret_ref (no inline values/regex) must pass field validation;
        // the expected value arrives at resolution time.
        use crate::resources::gateway::SecretObjectReference;
        let r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X-CF-Secret".into()),
            secret_ref: Some(SecretObjectReference {
                name: "cf-origin-secret".into(),
                group: None,
                kind: None,
                namespace: None,
            }),
            ..Default::default()
        };
        assert_eq!(r.field_error(), None);
    }

    #[test]
    fn mtls_sources_need_no_key_and_validate() {
        use crate::resources::edgion_plugins::plugin_configs::{RestrictionSource, SanType};
        let r = RestrictionRule {
            source: RestrictionSource::MtlsCn,
            values: Some(vec!["alice".to_string()]),
            ..Default::default()
        };
        assert_eq!(r.field_error(), None);

        let san = RestrictionRule {
            source: RestrictionSource::MtlsSan,
            san_type: Some(SanType::Uri),
            values: Some(vec!["spiffe://edgion.io/ns/foo".to_string()]),
            ..Default::default()
        };
        assert_eq!(san.field_error(), None);
    }

    #[test]
    fn san_type_rejected_on_non_san_source() {
        use crate::resources::edgion_plugins::plugin_configs::{RestrictionSource, SanType};
        let r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X".to_string()),
            values: Some(vec!["v".to_string()]),
            san_type: Some(SanType::Dns),
            ..Default::default()
        };
        let err = r.field_error().expect("san_type on Header must be rejected");
        assert!(err.contains("sanType"), "got: {}", err);
    }

    #[test]
    fn no_match_set_no_secret_no_on_missing_match_is_invalid() {
        let r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X".into()),
            on_missing: OnMissing::NoMatch,
            ..Default::default()
        };
        assert!(r.field_error().is_some());
    }
}
