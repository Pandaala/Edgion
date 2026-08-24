//! Request Restriction plugin configuration
//!
//! This plugin restricts access based on request attributes and identity,
//! evaluated through the shared [`Condition`] enum.
//!
//! ## Model (DNF: OR-of-AND)
//!
//! - `mode` is **required** and states what a match means: `Deny` (match =>
//!   reject, unmatched passes) or `Allow` (match => pass, unmatched is
//!   rejected). There is no default: omitting it is a validation error, so the
//!   polarity is always written out and can never silently flip.
//! - `conditions` explicitly selects a top-level `allOf` or `anyOf`.
//! - An `anyOf` alternative is either one direct Condition or one nested
//!   `allOf` group. Evaluation follows configuration order and short-circuits.
//!
//! A condition is a shared [`Condition`] predicate. Predicates match request
//! attributes (header/cookie/query/path/method) and, additionally,
//! post-authentication identity: mTLS client-cert subject / SAN / CN
//! (`mtlsMatch`) and verified JWT claims (`jwtClaimMatch`). Inline IP sets
//! (`directPeerIpMatch` / `remoteIpMatch` with `ips` and `ipRefs`) are
//! accepted. It is still a local allow/deny matcher, not a full RBAC
//! orchestration layer.
//!
//! ## Configuration Examples
//!
//! ### Deny: block bots OR /admin paths
//! ```yaml
//! requestRestriction:
//!   mode: Deny
//!   conditions:
//!     anyOf:
//!       - type: keyMatch
//!         key:
//!           type: header
//!           name: User-Agent
//!         regex: ["(?i).*Bot.*"]
//!       - type: keyMatch
//!         key:
//!           type: path
//!         values: ["/admin"]
//!   message: "Access denied"
//! ```
//!
//! ### Allow: allow only read-only methods
//! ```yaml
//! requestRestriction:
//!   mode: Allow
//!   conditions:
//!     allOf:
//!       - type: keyMatch
//!         key:
//!           type: method
//!         values: ["GET", "HEAD", "OPTIONS"]
//!   status: 405
//! ```
//!
//! ### AND within a group: deny only DELETE to /admin
//! ```yaml
//! requestRestriction:
//!   mode: Deny
//!   conditions:
//!     allOf:
//!       - type: keyMatch
//!         key:
//!           type: method
//!         values: ["DELETE"]
//!       - type: keyMatch
//!         key:
//!           type: path
//!         values: ["/admin"]
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::conditions::{ConditionSet, ConditionValidationProfile};

/// What a matching rule group means. Required — there is no default, so the
/// polarity is always stated explicitly in the configuration.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum RestrictionMode {
    /// Any matching group => reject; no group matches => pass.
    Deny,
    /// Any matching group => pass; no group matches => reject.
    Allow,
}

/// Request Restriction plugin configuration.
///
/// Bounded-DNF model: `conditions` explicitly selects `allOf` or `anyOf`. The required
/// `mode` states what a match means. A condition is a shared [`Condition`]
/// predicate (no per-condition allow/deny). Predicates match request
/// attributes and post-authentication identity (mTLS subject/SAN/CN, verified
/// JWT claims); it remains a local allow/deny matcher, not a full RBAC
/// orchestration layer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestRestrictionConfig {
    /// Polarity — **required**. `Deny`: a matching group rejects. `Allow`: a
    /// matching group passes and anything unmatched is rejected.
    ///
    /// Modelled as `Option` rather than a serde-required field so that a missing
    /// value is reported by `check_fields` through the normal plugin-validation
    /// path (a readable `Accepted=False` reason) instead of failing the whole
    /// resource's deserialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<RestrictionMode>,

    /// Explicit bounded Boolean expression evaluated in configuration order.
    pub conditions: ConditionSet,

    /// HTTP status code for rejection (default: 403).
    #[serde(default = "default_status")]
    #[schemars(range(min = 100, max = 599))]
    pub status: u16,

    /// Custom rejection message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn default_status() -> u16 {
    403
}

impl Default for RequestRestrictionConfig {
    fn default() -> Self {
        Self {
            mode: None,
            conditions: ConditionSet::default(),
            status: default_status(),
            message: None,
        }
    }
}

impl RequestRestrictionConfig {
    /// Validate configuration and compile matchers.
    ///
    /// Returns `Err(message)` if any condition is malformed; on success every
    /// condition has its compiled matchers (HashSet, regex, IP radix) populated.
    pub fn validate(&mut self) -> Result<(), String> {
        self.validate_and_compile()
    }

    /// Same errors as [`Self::validate_and_compile`], without requiring `&mut self`.
    pub fn check_fields(&self) -> Option<String> {
        self.clone().validate_and_compile().err()
    }

    /// Whether a matching group means "allow".
    ///
    /// `check_fields` rejects a missing `mode`, and the gateway fails closed on
    /// an invalid config before evaluating any request, so the unvalidated
    /// `None` case is unreachable. It resolves to allow-polarity, so unmatched
    /// traffic is rejected rather than admitted.
    pub fn allow_mode(&self) -> bool {
        debug_assert!(
            self.mode.is_some(),
            "allow_mode called on an unvalidated config with no mode"
        );
        !matches!(self.mode, Some(RestrictionMode::Deny))
    }

    /// True if any condition is HMAC with `validateRequestBody`.
    pub fn validates_request_body(&self) -> bool {
        self.conditions.validates_request_body()
    }

    /// Validate and compile all conditions across all groups.
    fn validate_and_compile(&mut self) -> Result<(), String> {
        if self.mode.is_none() {
            return Err("mode must be specified (Deny | Allow)".to_string());
        }
        if self.status < 100 || self.status >= 600 {
            return Err(format!("Invalid status code: {}", self.status));
        }
        self.conditions
            .compile()
            .map_err(|errors| format!("invalid condition regex: {}", errors[0]))?;
        self.conditions
            .validate(ConditionValidationProfile::RequestAsync)
            .map_err(|errors| errors.join("; "))?;
        Ok(())
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for RequestRestrictionConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::KeyGet;
    use crate::resources::common::SecretObjectReference;
    use crate::resources::config_data_ref::EdgionConfigDataRef;
    use crate::resources::edgion_plugins::conditions::{
        AllOfGroup, AnyOfClause, Condition, IpAccessMatch, JwtClaimMatchCondition, KeyExistCondition,
        KeyMatchCondition, MatchCore, MtlsMatchCondition,
    };
    use crate::resources::edgion_plugins::hmac_match::HmacMatchConfig;
    use crate::resources::edgion_plugins::plugin_configs::HmacAuthSecretGroup;

    fn core_values(values: &[&str]) -> MatchCore {
        MatchCore {
            values: Some(values.iter().map(|s| s.to_string()).collect()),
            ..MatchCore::default()
        }
    }

    fn core_regex(patterns: &[&str]) -> MatchCore {
        MatchCore {
            regex: Some(patterns.iter().map(|s| s.to_string()).collect()),
            ..MatchCore::default()
        }
    }

    fn key_match(key: KeyGet, values: &[&str]) -> Condition {
        Condition::KeyMatch(KeyMatchCondition {
            key,
            core: core_values(values),
            negate: false,
            descriptor: Box::from(""),
        })
    }

    fn key_match_regex(key: KeyGet, patterns: &[&str]) -> Condition {
        Condition::KeyMatch(KeyMatchCondition {
            key,
            core: core_regex(patterns),
            negate: false,
            descriptor: Box::from(""),
        })
    }

    fn key_exist(key: KeyGet, negate: bool) -> Condition {
        Condition::KeyExist(KeyExistCondition {
            key,
            negate,
            descriptor: Box::from(""),
        })
    }

    fn group(all_of: Vec<Condition>) -> AnyOfClause {
        AnyOfClause::AllOf(AllOfGroup { name: None, all_of })
    }

    fn deny_cfg(groups: Vec<AnyOfClause>) -> RequestRestrictionConfig {
        RequestRestrictionConfig {
            mode: Some(RestrictionMode::Deny),
            conditions: ConditionSet::AnyOf { any_of: groups },
            ..Default::default()
        }
    }

    #[test]
    fn test_default_config() {
        let config = RequestRestrictionConfig::default();
        assert_eq!(config.mode, None);
        assert!(config.conditions.is_empty());
        assert_eq!(config.status, 403);
        assert_eq!(config.message, None);
    }

    #[test]
    fn test_empty_groups_validation() {
        let mut config = RequestRestrictionConfig {
            mode: Some(RestrictionMode::Deny),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("allOf"), "{err}");
    }

    #[test]
    fn test_missing_mode_is_rejected() {
        let mut config = RequestRestrictionConfig {
            mode: None,
            conditions: ConditionSet::AnyOf {
                any_of: vec![group(vec![key_match(KeyGet::Path, &["/admin"])])],
            },
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("mode must be specified"));

        config.mode = Some(RestrictionMode::Allow);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_allow_mode_resolution() {
        let mut c = RequestRestrictionConfig {
            mode: Some(RestrictionMode::Deny),
            ..Default::default()
        };
        assert!(!c.allow_mode());
        c.mode = Some(RestrictionMode::Allow);
        assert!(c.allow_mode());
    }

    #[test]
    fn test_empty_all_of_in_group_validation() {
        let mut config = deny_cfg(vec![group(vec![])]);
        let err = config.validate().unwrap_err();
        assert!(err.contains("allOf"), "{err}");
    }

    #[test]
    fn test_missing_key_for_header() {
        let mut config = deny_cfg(vec![group(vec![key_match(
            KeyGet::Header { name: String::new() },
            &["bad"],
        )])]);
        let err = config.validate().unwrap_err();
        assert!(err.contains("key"), "{err}");
    }

    #[test]
    fn test_path_no_key_required() {
        let mut config = deny_cfg(vec![group(vec![key_match(KeyGet::Path, &["/admin"])])]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_key_match_needs_values_or_regex() {
        let mut bad = deny_cfg(vec![group(vec![Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Header { name: "X".to_string() },
            core: MatchCore::default(),
            negate: false,
            descriptor: Box::from(""),
        })])]);
        assert!(bad.validate().is_err());

        let mut ok = deny_cfg(vec![group(vec![key_exist(
            KeyGet::Header { name: "X".to_string() },
            true,
        )])]);
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn all_match_variants_require_values_or_regex() {
        let conditions = [
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
                field: crate::resources::edgion_plugins::conditions::MtlsField::Cn,
                san_type: None,
                core: MatchCore::default(),
                negate: true,
                descriptor: Box::from(""),
            }),
        ];

        for condition in conditions {
            let mut config = deny_cfg(vec![group(vec![condition])]);
            let checked = config.check_fields();
            let error = config.validate().expect_err("empty match payload must be rejected");
            assert_eq!(checked.as_deref(), Some(error.as_str()));
            assert!(error.contains("requires values or regex"), "{error}");
        }
    }

    #[test]
    fn empty_values_are_rejected_even_with_valid_regex() {
        let mut config = deny_cfg(vec![group(vec![Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Path,
            core: MatchCore {
                values: Some(vec![]),
                regex: Some(vec!["^/admin$".to_string()]),
                ..MatchCore::default()
            },
            negate: false,
            descriptor: Box::from(""),
        })])]);

        let checked = config.check_fields();
        let error = config
            .validate()
            .expect_err("an explicitly empty values array must be rejected");
        assert_eq!(checked.as_deref(), Some(error.as_str()));
        assert!(error.contains("keyMatch.values cannot be empty"), "{error}");
    }

    #[test]
    fn blank_jwt_claim_is_rejected_by_shared_validation() {
        let mut config = deny_cfg(vec![group(vec![Condition::JwtClaimMatch(JwtClaimMatchCondition {
            claim: " \n".to_string(),
            core: core_values(&["admin"]),
            negate: true,
            descriptor: Box::from(""),
        })])]);

        let checked = config.check_fields();
        let error = config.validate().expect_err("blank JWT claim must be rejected");
        assert_eq!(checked.as_deref(), Some(error.as_str()));
        assert!(error.contains("jwtClaimMatch.claim cannot be empty"), "{error}");
    }

    #[test]
    fn test_invalid_regex() {
        let mut config = deny_cfg(vec![group(vec![key_match_regex(
            KeyGet::Header {
                name: "X-Test".to_string(),
            },
            &["[invalid"],
        )])]);
        let err = config.validate().unwrap_err();
        assert!(err.contains("Invalid") || err.contains("regex"), "{err}");
    }

    #[test]
    fn test_empty_values_list() {
        let mut config = deny_cfg(vec![group(vec![Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Header {
                name: "X-Test".to_string(),
            },
            core: MatchCore {
                values: Some(vec![]),
                ..MatchCore::default()
            },
            negate: false,
            descriptor: Box::from(""),
        })])]);
        let err = config.validate().unwrap_err();
        assert!(
            err.contains("cannot be empty") || err.contains("values or regex"),
            "{err}"
        );
    }

    #[test]
    fn ip_refs_in_rr_group_accepted() {
        let mut config = deny_cfg(vec![group(vec![Condition::DirectPeerIpMatch(IpAccessMatch {
            ips: vec!["10.0.0.0/8".to_string()],
            ip_refs: vec![EdgionConfigDataRef {
                name: "office".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        })])]);
        assert!(
            config.validate().is_ok(),
            "ipRefs in a RequestRestriction group must be accepted: {:?}",
            config.validate().err()
        );
    }

    #[test]
    fn inline_ips_and_path_in_one_group_accepted() {
        let mut config = deny_cfg(vec![group(vec![
            Condition::DirectPeerIpMatch(IpAccessMatch {
                ips: vec!["127.0.0.1".to_string()],
                ..Default::default()
            }),
            key_match(KeyGet::Path, &["/admin"]),
        ])]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn hmac_validate_request_body_accepted_in_rr_all_of() {
        let mut config = deny_cfg(vec![group(vec![Condition::HmacAuth(HmacMatchConfig {
            validate_request_body: true,
            secret_groups: Some(vec![HmacAuthSecretGroup {
                name: "default".to_string(),
                description: None,
                secret_refs: vec![SecretObjectReference {
                    name: "hmac-credentials".to_string(),
                    group: None,
                    kind: None,
                    namespace: None,
                }],
            }]),
            ..Default::default()
        })])]);
        assert!(
            config.validate().is_ok(),
            "HMAC validateRequestBody must stay accepted in RR allOf: {:?}",
            config.validate().err()
        );
        assert!(config.validates_request_body());
    }

    #[test]
    fn check_fields_matches_validate_and_compile() {
        let mut invalid = deny_cfg(vec![group(vec![])]);
        assert_eq!(invalid.check_fields(), invalid.validate().err());

        let mut valid = deny_cfg(vec![group(vec![key_match(KeyGet::Path, &["/ok"])])]);
        assert_eq!(valid.check_fields(), valid.validate().err());
    }

    #[test]
    fn jwt_claim_and_mtls_constructors_validate() {
        let mut jwt = deny_cfg(vec![group(vec![Condition::JwtClaimMatch(JwtClaimMatchCondition {
            claim: "realm_access.roles".to_string(),
            core: core_values(&["admin"]),
            negate: false,
            descriptor: Box::from(""),
        })])]);
        assert!(jwt.validate().is_ok());

        let mut mtls = deny_cfg(vec![group(vec![Condition::MtlsMatch(MtlsMatchCondition {
            field: crate::resources::edgion_plugins::conditions::MtlsField::Cn,
            san_type: None,
            core: core_values(&["alice"]),
            negate: false,
            descriptor: Box::from(""),
        })])]);
        assert!(mtls.validate().is_ok());
    }
}
