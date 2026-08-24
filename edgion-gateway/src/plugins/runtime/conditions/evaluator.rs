//! Condition evaluator for plugin conditional execution
//!
//! This module provides the logic to evaluate conditions at runtime
//! to determine whether a plugin should be executed or skipped.

use crate::plugins::http::hmac_auth::{HmacVerifier, HmacVerifyOutcome};
use crate::plugins::runtime::conditions::extract::{extract_jwt_claim_values, extract_mtls_values};
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::PluginSession;
use chrono::Utc;
use edgion_resources::resources::edgion_plugins::conditions::{
    Condition, JwtClaimMatchCondition, KeyExistCondition, KeyMatchCondition, MatchCore, MtlsMatchCondition,
    PluginConditions, ProbabilityCondition, TimeRangeCondition,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Result of condition evaluation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationResult {
    /// Plugin should run
    Run,
    /// Plugin should be skipped
    Skip,
}

/// Result of condition evaluation with matched condition info
pub struct ConditionEvalResult<'a> {
    pub result: EvaluationResult,
    /// The action that caused skip: "skip" or "!run"
    pub action: &'static str,
    /// The condition that matched (for logging)
    pub matched: Option<&'a Condition>,
}

/// Gateway-owned, build-time representation of one schema condition.
/// Header-only HMAC owns one verifier for the lifetime of the runtime snapshot.
pub(crate) struct CompiledCondition {
    condition: Condition,
    hmac_headers: Option<HmacVerifier>,
}

impl CompiledCondition {
    pub(crate) fn new(condition: Condition) -> Self {
        let hmac_headers = match &condition {
            Condition::HmacAuth(config) if !config.validate_request_body && !condition.has_unresolved_refs() => {
                Some(HmacVerifier::new(config, None))
            }
            _ => None,
        };
        Self {
            condition,
            hmac_headers,
        }
    }

    pub(crate) fn evaluate(&self, session: &dyn PluginSession, mut log: Option<&mut PluginLog>) -> bool {
        if self.condition.has_unresolved_refs() {
            return false;
        }
        let matched = match (&self.condition, &self.hmac_headers) {
            (Condition::HmacAuth(config), Some(verifier)) => {
                let base = matches!(verifier.verify_headers(session), HmacVerifyOutcome::Verified { .. });
                apply_negate(base, config.negate)
            }
            _ => self.condition.evaluate(session, log.as_deref_mut()),
        };
        if matched && self.hmac_headers.is_some() {
            if let Some(log) = log {
                log.push(self.condition.descriptor());
            }
        }
        matched
    }

    pub(crate) fn condition(&self) -> &Condition {
        &self.condition
    }

    #[cfg(test)]
    fn hmac_credentials_ptr(&self) -> Option<*const ()> {
        self.hmac_headers.as_ref().map(HmacVerifier::credentials_ptr)
    }
}

/// Compiled skip-OR / run-AND condition container shared by every synchronous
/// Gateway consumer (entry wrappers, body gates, and dye rules).
pub(crate) struct CompiledPluginConditions {
    skip: Vec<CompiledCondition>,
    run: Vec<CompiledCondition>,
}

impl CompiledPluginConditions {
    pub(crate) fn new(conditions: PluginConditions) -> Self {
        Self {
            skip: conditions
                .skip
                .unwrap_or_default()
                .into_iter()
                .map(CompiledCondition::new)
                .collect(),
            run: conditions
                .run
                .unwrap_or_default()
                .into_iter()
                .map(CompiledCondition::new)
                .collect(),
        }
    }

    pub(crate) fn should_evaluate(&self) -> bool {
        !self.skip.is_empty() || !self.run.is_empty()
    }

    pub(crate) fn evaluate(&self, session: &dyn PluginSession) -> EvaluationResult {
        if self.skip.iter().any(|condition| condition.evaluate(session, None)) {
            return EvaluationResult::Skip;
        }
        if self.run.iter().any(|condition| !condition.evaluate(session, None)) {
            return EvaluationResult::Skip;
        }
        EvaluationResult::Run
    }

    pub(crate) fn evaluate_detail<'a>(
        &'a self,
        session: &dyn PluginSession,
        log: &mut PluginLog,
    ) -> ConditionEvalResult<'a> {
        for condition in &self.skip {
            if condition.evaluate(session, Some(&mut *log)) {
                return ConditionEvalResult {
                    result: EvaluationResult::Skip,
                    action: "skip",
                    matched: Some(condition.condition()),
                };
            }
        }
        for condition in &self.run {
            if !condition.evaluate(session, Some(&mut *log)) {
                return ConditionEvalResult {
                    result: EvaluationResult::Skip,
                    action: "!run",
                    matched: Some(condition.condition()),
                };
            }
        }
        ConditionEvalResult {
            result: EvaluationResult::Run,
            action: "",
            matched: None,
        }
    }
}

// === Gateway-side evaluation extension traits ===
//
// The condition *data* types live in `edgion-resources` (the shared schema
// crate). Their runtime evaluation depends on gateway types (`PluginSession`,
// `PluginLog`, `EvaluationResult`), so the evaluation methods cannot be inherent
// impls on the foreign types — the orphan rule forbids it across the crate
// boundary. They are provided here as gateway-local extension traits instead;
// call sites get the methods by importing the trait.

pub(crate) trait PluginConditionsExt {
    /// Log-less evaluation for body-stage filters and dye (no PluginLog, no cond_skip).
    #[cfg(test)]
    fn evaluate(&self, session: &dyn PluginSession) -> EvaluationResult;
    #[cfg(test)]
    fn evaluate_detail<'a>(&'a self, session: &dyn PluginSession, log: &mut PluginLog) -> ConditionEvalResult<'a>;
    fn has_unresolved_refs(&self) -> bool;
}

pub(crate) trait ConditionExt {
    fn evaluate(&self, session: &dyn PluginSession, log: Option<&mut PluginLog>) -> bool;
    fn cond_detail(&self) -> String;
    fn has_unresolved_refs(&self) -> bool;
}

pub(crate) trait KeyExistConditionExt {
    fn evaluate(&self, session: &dyn PluginSession) -> bool;
}

pub(crate) trait KeyMatchConditionExt {
    fn evaluate(&self, session: &dyn PluginSession, log: Option<&mut PluginLog>) -> bool;
    fn match_value(&self, value: &str, log: Option<&mut PluginLog>) -> bool;
}

pub(crate) trait TimeRangeConditionExt {
    fn evaluate(&self) -> bool;
}

pub(crate) trait ProbabilityConditionExt {
    fn evaluate(&self, session: &dyn PluginSession) -> bool;
}

impl PluginConditionsExt for PluginConditions {
    /// Log-less evaluation for body-stage filters and dye.
    ///
    /// Body filters run per-chunk and do not surface PluginLog entries, so this
    /// path never allocates a PluginLog. The only diagnostic that the logged
    /// path would emit ("regex not compiled") is silently dropped — that
    /// condition indicates a build-time compile failure and should already be
    /// visible through `EdgionPlugin::validation_error`.
    #[cfg(test)]
    fn evaluate(&self, session: &dyn PluginSession) -> EvaluationResult {
        if let Some(skip_conditions) = &self.skip {
            for condition in skip_conditions {
                if condition.evaluate(session, None) {
                    return EvaluationResult::Skip;
                }
            }
        }

        if let Some(run_conditions) = &self.run {
            for condition in run_conditions {
                if !condition.evaluate(session, None) {
                    return EvaluationResult::Skip;
                }
            }
        }

        EvaluationResult::Run
    }

    /// Evaluate conditions and return matched condition for logging.
    #[cfg(test)]
    fn evaluate_detail<'a>(&'a self, session: &dyn PluginSession, log: &mut PluginLog) -> ConditionEvalResult<'a> {
        // Check skip conditions (OR logic) - any match causes skip
        if let Some(skip_conditions) = &self.skip {
            for condition in skip_conditions {
                if condition.evaluate(session, Some(&mut *log)) {
                    return ConditionEvalResult {
                        result: EvaluationResult::Skip,
                        action: "skip",
                        matched: Some(condition),
                    };
                }
            }
        }

        // Check run conditions (AND logic) - all must match to run
        if let Some(run_conditions) = &self.run {
            for condition in run_conditions {
                if !condition.evaluate(session, Some(&mut *log)) {
                    return ConditionEvalResult {
                        result: EvaluationResult::Skip,
                        action: "!run",
                        matched: Some(condition),
                    };
                }
            }
        }

        ConditionEvalResult {
            result: EvaluationResult::Run,
            action: "",
            matched: None,
        }
    }

    fn has_unresolved_refs(&self) -> bool {
        self.skip
            .iter()
            .chain(self.run.iter())
            .flatten()
            .any(ConditionExt::has_unresolved_refs)
    }
}

impl ConditionExt for Condition {
    /// Evaluate a single condition. Pass `None` to suppress PluginLog writes
    /// (body-stage / dye); pass `Some(log)` on the request/response path.
    ///
    /// On a match, if `log` is `Some`, push the compiled descriptor. A
    /// non-match logs nothing. This keeps `Option<&mut PluginLog>` (required
    /// deviation from the PROGRAM `&mut PluginLog` wording): body/dye must
    /// stay silent — per-chunk `push` would explode volume and violate the
    /// body-phase 🔴 (no PluginLog).
    fn evaluate(&self, session: &dyn PluginSession, mut log: Option<&mut PluginLog>) -> bool {
        // Resolution failure is not a normal non-match and must never be
        // inverted by `negate`. Consumer-specific policy decides whether this
        // fixed non-match rejects the request or permits another candidate.
        if self.has_unresolved_refs() {
            return false;
        }
        let matched = match self {
            Condition::KeyExist(c) => c.evaluate(session),
            Condition::KeyMatch(c) => c.evaluate(session, log.as_deref_mut()),
            Condition::TimeRange(c) => c.evaluate(),
            Condition::Probability(c) => c.evaluate(session),
            Condition::SecretMatch(c) => evaluate_secret_match(c, session),
            Condition::DirectPeerIpMatch(c) => evaluate_ip_match(c, session.direct_peer_ip()),
            Condition::RemoteIpMatch(c) => evaluate_ip_match(c, session.remote_ip()),
            Condition::HmacAuth(c) => evaluate_hmac_headers(c, session),
            Condition::JwtClaimMatch(c) => evaluate_jwt_claim_match(c, session, log.as_deref_mut()),
            Condition::MtlsMatch(c) => evaluate_mtls_match(c, session, log.as_deref_mut()),
        };
        if matched {
            if let Some(log) = log {
                log.push(self.descriptor());
            }
        }
        matched
    }

    /// Get brief condition detail: key info
    fn cond_detail(&self) -> String {
        match self {
            Condition::KeyExist(c) => c.key.as_log_str(),
            Condition::KeyMatch(c) => c.key.as_log_str(),
            Condition::TimeRange(_) => "time".to_string(),
            Condition::Probability(c) => format!("{:.0}%", c.ratio * 100.0),
            Condition::SecretMatch(c) => c.key.as_log_str(),
            Condition::DirectPeerIpMatch(_) => "directPeerIp".to_string(),
            Condition::RemoteIpMatch(_) => "remoteIp".to_string(),
            Condition::HmacAuth(_) => "hmacAuth".to_string(),
            Condition::JwtClaimMatch(c) => c.claim.clone(),
            Condition::MtlsMatch(c) => c.field.as_log_str().to_string(),
        }
    }

    fn has_unresolved_refs(&self) -> bool {
        match self {
            Condition::SecretMatch(condition) => condition.resolved_values.is_none(),
            Condition::DirectPeerIpMatch(condition) | Condition::RemoteIpMatch(condition) => {
                !condition.ip_refs.is_empty() && condition.resolved_ips.is_none()
            }
            Condition::HmacAuth(condition) => {
                condition
                    .secret_groups
                    .as_ref()
                    .is_some_and(|groups| !groups.is_empty())
                    && !condition
                        .resolved_credentials
                        .as_ref()
                        .is_some_and(|credentials| !credentials.is_empty())
            }
            _ => false,
        }
    }
}

fn evaluate_secret_match(
    condition: &edgion_resources::resources::edgion_plugins::conditions::SecretMatchCondition,
    session: &dyn PluginSession,
) -> bool {
    use subtle::ConstantTimeEq;
    let Some(actual) = session.key_get(&condition.key) else {
        return apply_negate(false, condition.negate);
    };
    let base = condition.resolved_values.as_ref().is_some_and(|expected| {
        expected
            .iter()
            .any(|candidate| candidate.len() == actual.len() && candidate.as_bytes().ct_eq(actual.as_bytes()).into())
    });
    apply_negate(base, condition.negate)
}

fn evaluate_ip_match(
    condition: &edgion_resources::resources::edgion_plugins::conditions::IpAccessMatch,
    ip: &str,
) -> bool {
    let base = ip
        .parse::<std::net::IpAddr>()
        .ok()
        .and_then(|addr| condition.matcher.as_ref().map(|matcher| matcher.contains(&addr)))
        .unwrap_or(false);
    apply_negate(base, condition.negate)
}

fn evaluate_hmac_headers(
    condition: &edgion_resources::resources::edgion_plugins::hmac_match::HmacMatchConfig,
    session: &dyn PluginSession,
) -> bool {
    // Entry-level HMAC is header-only (D-27). Body HMAC is illegal here;
    // RequestRestriction allOf compiles those arms to HmacVerifier::verify.
    if condition.validate_request_body {
        return apply_negate(false, condition.negate);
    }
    let base = matches!(
        HmacVerifier::verify_headers_config(condition, session),
        HmacVerifyOutcome::Verified { .. }
    );
    apply_negate(base, condition.negate)
}

fn evaluate_jwt_claim_match(
    condition: &JwtClaimMatchCondition,
    session: &dyn PluginSession,
    log: Option<&mut PluginLog>,
) -> bool {
    evaluate_multi_match(
        &condition.core,
        extract_jwt_claim_values(session, &condition.claim),
        condition.negate,
        log,
    )
}

fn evaluate_mtls_match(
    condition: &MtlsMatchCondition,
    session: &dyn PluginSession,
    log: Option<&mut PluginLog>,
) -> bool {
    evaluate_multi_match(
        &condition.core,
        extract_mtls_values(session, condition.field, condition.san_type.as_ref()),
        condition.negate,
        log,
    )
}

/// Any candidate matching MatchCore is base true. Empty candidates are missing
/// (base false), then negate.
fn evaluate_multi_match(core: &MatchCore, candidates: Vec<String>, negate: bool, log: Option<&mut PluginLog>) -> bool {
    if candidates.is_empty() {
        return apply_negate(false, negate);
    }
    let base = candidates.iter().any(|value| core.matches(value));
    if !base && core.has_uncompiled_regex() {
        if let Some(log) = log {
            log.push_err("regex not compiled");
        }
    }
    apply_negate(base, negate)
}

fn apply_negate(base: bool, negate: bool) -> bool {
    if negate {
        !base
    } else {
        base
    }
}

/// Values (exact / HashSet / case fold) OR compiled regex. Logs when regex was configured but not compiled.
fn match_core(core: &MatchCore, value: &str, log: Option<&mut PluginLog>) -> bool {
    if core.matches(value) {
        return true;
    }
    if core.has_uncompiled_regex() {
        if let Some(log) = log {
            log.push_err("regex not compiled");
        }
    }
    false
}

impl KeyExistConditionExt for KeyExistCondition {
    fn evaluate(&self, session: &dyn PluginSession) -> bool {
        apply_negate(session.key_get(&self.key).is_some(), self.negate)
    }
}

impl KeyMatchConditionExt for KeyMatchCondition {
    fn evaluate(&self, session: &dyn PluginSession, log: Option<&mut PluginLog>) -> bool {
        let value = match session.key_get(&self.key) {
            Some(v) => v,
            None => return apply_negate(false, self.negate),
        };
        self.match_value(&value, log)
    }

    /// Check if a value matches any of the configured patterns, then apply negate.
    fn match_value(&self, value: &str, log: Option<&mut PluginLog>) -> bool {
        apply_negate(match_core(&self.core, value, log), self.negate)
    }
}

impl TimeRangeConditionExt for TimeRangeCondition {
    /// Evaluate time range condition
    fn evaluate(&self) -> bool {
        let now = Utc::now();
        let mut base = true;

        // Check 'after' bound
        if let Some(after_str) = &self.after {
            if let Ok(after_time) = chrono::DateTime::parse_from_rfc3339(after_str) {
                if now < after_time {
                    base = false;
                }
            }
        }

        // Check 'before' bound
        if base {
            if let Some(before_str) = &self.before {
                if let Ok(before_time) = chrono::DateTime::parse_from_rfc3339(before_str) {
                    if now >= before_time {
                        base = false;
                    }
                }
            }
        }

        apply_negate(base, self.negate)
    }
}

impl ProbabilityConditionExt for ProbabilityCondition {
    fn evaluate(&self, session: &dyn PluginSession) -> bool {
        // Clamp ratio to valid range
        let ratio = self.ratio.clamp(0.0, 1.0);

        let base = if ratio <= 0.0 {
            false
        } else if ratio >= 1.0 {
            true
        } else if let Some(key) = &self.key {
            if let Some(key_value) = session.key_get(key) {
                // Use hash for deterministic sampling
                let mut hasher = DefaultHasher::new();
                key_value.hash(&mut hasher);
                let hash = hasher.finish();
                let normalized = (hash as f64) / (u64::MAX as f64);
                normalized < ratio
            } else {
                rand::random::<f64>() < ratio
            }
        } else {
            rand::random::<f64>() < ratio
        };

        apply_negate(base, self.negate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::common::KeyGet;
    use edgion_resources::resources::edgion_plugins::{HmacCredential, HmacMatchConfig};

    /// Create a mock session with key_get that returns values based on the key
    fn create_mock_session_with_key_get(key_values: Vec<(KeyGet, Option<String>)>) -> MockPluginSession {
        let mut mock = MockPluginSession::new();

        // Convert to owned map
        let values_map: std::collections::HashMap<String, Option<String>> =
            key_values.into_iter().map(|(k, v)| (format!("{:?}", k), v)).collect();

        mock.expect_key_get().returning(move |key| {
            let key_str = format!("{:?}", key);
            values_map.get(&key_str).cloned().flatten()
        });

        mock
    }

    #[test]
    fn compiled_header_hmac_reuses_one_credentials_arc_across_evaluations() {
        let condition = Condition::HmacAuth(HmacMatchConfig {
            resolved_credentials: Some(std::collections::HashMap::from([(
                "alice".to_string(),
                HmacCredential {
                    secret: b"header-hmac-secret-at-least-32-bytes".to_vec(),
                    headers: std::collections::HashMap::new(),
                    group: "default".to_string(),
                },
            )])),
            descriptor: Box::from("0:hmacAuth"),
            ..Default::default()
        });
        let compiled = CompiledCondition::new(condition);
        let credentials_ptr = compiled
            .hmac_credentials_ptr()
            .expect("header-only HMAC must prebuild one verifier");
        let mut session = MockPluginSession::new();
        session.expect_header_value().returning(|_| None);

        assert!(!compiled.evaluate(&session, None));
        assert!(!compiled.evaluate(&session, None));
        assert_eq!(compiled.hmac_credentials_ptr(), Some(credentials_ptr));
    }

    fn core_values(values: Vec<&str>) -> MatchCore {
        MatchCore {
            values: Some(values.into_iter().map(str::to_string).collect()),
            ..MatchCore::default()
        }
    }

    fn core_regex(patterns: Vec<&str>) -> MatchCore {
        MatchCore {
            regex: Some(patterns.into_iter().map(str::to_string).collect()),
            ..MatchCore::default()
        }
    }

    #[test]
    fn test_empty_conditions_run() {
        let conditions = PluginConditions::default();
        let mut mock = MockPluginSession::new();
        mock.expect_key_get().returning(|_| None);
        assert_eq!(conditions.evaluate(&mock), EvaluationResult::Run);
    }

    #[test]
    fn test_skip_condition_matched() {
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            })]),
            run: None,
        };

        let session = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Internal".to_string(),
            },
            Some("true".to_string()),
        )]);
        assert_eq!(conditions.evaluate(&session), EvaluationResult::Skip);
    }

    #[test]
    fn test_skip_condition_not_matched() {
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            })]),
            run: None,
        };

        let mut mock = MockPluginSession::new();
        mock.expect_key_get().returning(|_| None);
        assert_eq!(conditions.evaluate(&mock), EvaluationResult::Run);
    }

    #[test]
    fn test_run_condition_all_satisfied() {
        let conditions = PluginConditions {
            skip: None,
            run: Some(vec![
                Condition::KeyExist(KeyExistCondition {
                    key: KeyGet::Header {
                        name: "Authorization".to_string(),
                    },
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::KeyMatch(KeyMatchCondition {
                    key: KeyGet::Method,
                    core: core_values(vec!["GET", "POST"]),
                    negate: false,
                    descriptor: Box::from(""),
                }),
            ]),
        };

        let session = create_mock_session_with_key_get(vec![
            (
                KeyGet::Header {
                    name: "Authorization".to_string(),
                },
                Some("Bearer token".to_string()),
            ),
            (KeyGet::Method, Some("GET".to_string())),
        ]);
        assert_eq!(conditions.evaluate(&session), EvaluationResult::Run);
    }

    #[test]
    fn test_run_condition_not_all_satisfied() {
        let conditions = PluginConditions {
            skip: None,
            run: Some(vec![
                Condition::KeyExist(KeyExistCondition {
                    key: KeyGet::Header {
                        name: "Authorization".to_string(),
                    },
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::KeyMatch(KeyMatchCondition {
                    key: KeyGet::Method,
                    core: core_values(vec!["POST"]),
                    negate: false,
                    descriptor: Box::from(""),
                }),
            ]),
        };

        // Method is GET, not POST
        let session = create_mock_session_with_key_get(vec![
            (
                KeyGet::Header {
                    name: "Authorization".to_string(),
                },
                Some("Bearer token".to_string()),
            ),
            (KeyGet::Method, Some("GET".to_string())),
        ]);
        assert_eq!(conditions.evaluate(&session), EvaluationResult::Skip);
    }

    #[test]
    fn test_key_match_exact() {
        let condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "X-Environment".to_string(),
            },
            core: core_values(vec!["production"]),
            negate: false,
            descriptor: Box::from(""),
        };

        let session = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Environment".to_string(),
            },
            Some("production".to_string()),
        )]);
        let mut log = PluginLog::new("test");
        assert!(condition.evaluate(&session, Some(&mut log)));

        let session2 = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Environment".to_string(),
            },
            Some("staging".to_string()),
        )]);
        assert!(!condition.evaluate(&session2, Some(&mut log)));
    }

    #[test]
    fn match_core_default_case_sensitive_and_unnegated_matches_today() {
        // YAML without caseSensitive / negate must keep today's verdicts:
        // exact match is case-sensitive; missing key is base false for KeyMatch.
        let key_match: KeyMatchCondition = serde_yaml::from_str(
            r#"
key:
  type: header
  name: X-Env
values:
  - production
"#,
        )
        .unwrap();
        assert!(key_match.core.case_sensitive);
        assert!(!key_match.negate);

        let mut log = PluginLog::new("test");
        let present = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Env".to_string(),
            },
            Some("production".to_string()),
        )]);
        let case_mismatch = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Env".to_string(),
            },
            Some("Production".to_string()),
        )]);
        let missing = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Env".to_string(),
            },
            None,
        )]);

        assert!(key_match.evaluate(&present, Some(&mut log)));
        assert!(!key_match.evaluate(&case_mismatch, Some(&mut log)));
        assert!(!key_match.evaluate(&missing, Some(&mut log)));
    }

    #[test]
    fn negated_key_match_missing_key_keeps_exclude_verdict() {
        // Former Exclude: missing key was true. KeyMatch base is false; negate
        // after the missing check yields true — Exclude-preserving.
        let condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "X-Env".to_string(),
            },
            core: core_values(vec!["production"]),
            negate: true,
            descriptor: Box::from(""),
        };
        let mut log = PluginLog::new("test");
        let missing = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Env".to_string(),
            },
            None,
        )]);
        assert!(condition.evaluate(&missing, Some(&mut log)));
    }

    #[test]
    fn negated_time_range_inverts_window() {
        let in_window = TimeRangeCondition {
            after: Some("2020-01-01T00:00:00Z".to_string()),
            before: Some("2030-12-31T23:59:59Z".to_string()),
            negate: true,
            descriptor: Box::from(""),
        };
        assert!(!in_window.evaluate());

        let past = TimeRangeCondition {
            after: Some("2020-01-01T00:00:00Z".to_string()),
            before: Some("2020-12-31T23:59:59Z".to_string()),
            negate: true,
            descriptor: Box::from(""),
        };
        assert!(past.evaluate());
    }

    #[test]
    fn negated_remote_ip_match() {
        let condition = KeyMatchCondition {
            key: KeyGet::RemoteIp,
            core: core_values(vec!["1.2.3.4"]),
            negate: true,
            descriptor: Box::from(""),
        };
        let mut log = PluginLog::new("test");
        let hit = create_mock_session_with_key_get(vec![(KeyGet::RemoteIp, Some("1.2.3.4".to_string()))]);
        let miss = create_mock_session_with_key_get(vec![(KeyGet::RemoteIp, Some("5.6.7.8".to_string()))]);
        assert!(!condition.evaluate(&hit, Some(&mut log)));
        assert!(condition.evaluate(&miss, Some(&mut log)));
    }

    #[test]
    fn test_time_range_within() {
        let condition = TimeRangeCondition {
            after: Some("2020-01-01T00:00:00Z".to_string()),
            before: Some("2030-12-31T23:59:59Z".to_string()),
            negate: false,
            descriptor: Box::from(""),
        };

        assert!(condition.evaluate());
    }

    #[test]
    fn test_time_range_past() {
        let condition = TimeRangeCondition {
            after: Some("2020-01-01T00:00:00Z".to_string()),
            before: Some("2020-12-31T23:59:59Z".to_string()),
            negate: false,
            descriptor: Box::from(""),
        };

        assert!(!condition.evaluate());
    }

    #[test]
    fn test_time_range_validate_rejects_malformed_after() {
        // Invalid month (13) — operator typo. Must surface as a preparse error
        // (caller pushes into preparse_errors → seal_fail_closed) instead of
        // being silently dropped at evaluation time, which would leave the
        // time-gate permanently inert. See F-002.
        let condition = TimeRangeCondition {
            after: Some("2024-13-01T00:00:00Z".to_string()),
            before: None,
            negate: false,
            descriptor: Box::from(""),
        };
        assert!(condition.validate().is_err());
    }

    #[test]
    fn test_time_range_validate_rejects_malformed_before() {
        let condition = TimeRangeCondition {
            after: None,
            before: Some("not-a-timestamp".to_string()),
            negate: false,
            descriptor: Box::from(""),
        };
        assert!(condition.validate().is_err());
    }

    #[test]
    fn test_time_range_validate_accepts_well_formed_bounds() {
        let condition = TimeRangeCondition {
            after: Some("2020-01-01T00:00:00Z".to_string()),
            before: Some("2030-12-31T23:59:59Z".to_string()),
            negate: false,
            descriptor: Box::from(""),
        };
        assert!(condition.validate().is_ok());
    }

    #[test]
    fn test_time_range_validate_accepts_unset_bounds() {
        let condition = TimeRangeCondition {
            after: None,
            before: None,
            negate: false,
            descriptor: Box::from(""),
        };
        assert!(condition.validate().is_ok());
    }

    #[test]
    fn test_probability_always() {
        let condition = ProbabilityCondition {
            ratio: 1.0,
            key: None,
            negate: false,
            descriptor: Box::from(""),
        };

        let mut mock = MockPluginSession::new();
        mock.expect_key_get().returning(|_| None);
        assert!(condition.evaluate(&mock));
    }

    #[test]
    fn test_probability_never() {
        let condition = ProbabilityCondition {
            ratio: 0.0,
            key: None,
            negate: false,
            descriptor: Box::from(""),
        };

        let mut mock = MockPluginSession::new();
        mock.expect_key_get().returning(|_| None);
        assert!(!condition.evaluate(&mock));
    }

    #[test]
    fn test_key_match_path_regex() {
        let mut condition = KeyMatchCondition {
            key: KeyGet::Path,
            core: core_regex(vec!["^/api/.*", "^/admin/.*"]),
            negate: false,
            descriptor: Box::from(""),
        };
        condition.compile().unwrap();

        let mut log = PluginLog::new("test");
        let session1 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/api/users".to_string()))]);
        assert!(condition.evaluate(&session1, Some(&mut log)));

        let session2 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/admin/settings".to_string()))]);
        assert!(condition.evaluate(&session2, Some(&mut log)));

        let session3 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/public/index.html".to_string()))]);
        assert!(!condition.evaluate(&session3, Some(&mut log)));
    }

    #[test]
    fn test_negated_key_match_path_keeps_exclude_present_verdict() {
        let condition = KeyMatchCondition {
            key: KeyGet::Path,
            core: core_values(vec!["/health", "/ready", "/metrics"]),
            negate: true,
            descriptor: Box::from(""),
        };

        let mut log = PluginLog::new("test");
        let session1 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/api/users".to_string()))]);
        assert!(condition.evaluate(&session1, Some(&mut log)));

        let session2 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/health".to_string()))]);
        assert!(!condition.evaluate(&session2, Some(&mut log)));

        let session3 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/ready".to_string()))]);
        assert!(!condition.evaluate(&session3, Some(&mut log)));
    }

    #[test]
    fn test_regex_prefix_equivalent_of_former_wildcard() {
        // Fixture mapping is prefix-only: `/api/*` → `^/api/.*`.
        let mut core = core_regex(vec!["^/api/.*"]);
        core.compile(false).unwrap();
        assert!(core.matches("/api/users"));
        assert!(!core.matches("/public/index"));
    }

    fn log_entries(log: &PluginLog) -> Vec<String> {
        log.log
            .as_ref()
            .map(|buf| buf.entries().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn key_match(name: &str, values: &[&str]) -> Condition {
        Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Header { name: name.to_string() },
            core: core_values(values.to_vec()),
            negate: false,
            descriptor: Box::from(""),
        })
    }

    #[test]
    fn descriptor_and_short_circuit_log_sequence() {
        // AND (run): first two match, third fails — only the two successes push.
        let mut run = PluginConditions {
            skip: None,
            run: Some(vec![
                key_match("X-A", &["yes"]),
                key_match("X-B", &["yes"]),
                key_match("X-C", &["yes"]),
            ]),
        };
        run.compile().expect("compile run descriptors");
        let run_session = create_mock_session_with_key_get(vec![
            (
                KeyGet::Header {
                    name: "X-A".to_string(),
                },
                Some("yes".to_string()),
            ),
            (
                KeyGet::Header {
                    name: "X-B".to_string(),
                },
                Some("yes".to_string()),
            ),
            (
                KeyGet::Header {
                    name: "X-C".to_string(),
                },
                Some("no".to_string()),
            ),
        ]);
        let mut run_log = PluginLog::new("test");
        let run_eval = run.evaluate_detail(&run_session, &mut run_log);
        assert_eq!(run_eval.result, EvaluationResult::Skip);
        assert_eq!(
            log_entries(&run_log),
            vec!["0:keyMatch:hdr:X-A".to_string(), "1:keyMatch:hdr:X-B".to_string()]
        );

        // OR (skip): first match short-circuits — only that descriptor is pushed.
        let mut skip = PluginConditions {
            skip: Some(vec![key_match("X-A", &["yes"]), key_match("X-B", &["yes"])]),
            run: None,
        };
        skip.compile().expect("compile skip descriptors");
        let skip_session = create_mock_session_with_key_get(vec![
            (
                KeyGet::Header {
                    name: "X-A".to_string(),
                },
                Some("yes".to_string()),
            ),
            (
                KeyGet::Header {
                    name: "X-B".to_string(),
                },
                Some("yes".to_string()),
            ),
        ]);
        let mut skip_log = PluginLog::new("test");
        let skip_eval = skip.evaluate_detail(&skip_session, &mut skip_log);
        assert_eq!(skip_eval.result, EvaluationResult::Skip);
        assert_eq!(log_entries(&skip_log), vec!["0:keyMatch:hdr:X-A".to_string()]);
    }

    #[test]
    fn condition_log_stays_inside_tier1_or_records_spill() {
        let mut run = PluginConditions {
            skip: None,
            run: Some(vec![
                key_match("X-A", &["yes"]),
                key_match("X-B", &["yes"]),
                key_match("X-C", &["yes"]),
            ]),
        };
        run.compile().expect("compile descriptors");
        let session = create_mock_session_with_key_get(vec![
            (
                KeyGet::Header {
                    name: "X-A".to_string(),
                },
                Some("yes".to_string()),
            ),
            (
                KeyGet::Header {
                    name: "X-B".to_string(),
                },
                Some("yes".to_string()),
            ),
            (
                KeyGet::Header {
                    name: "X-C".to_string(),
                },
                Some("yes".to_string()),
            ),
        ]);
        let mut log = PluginLog::new("host");
        let eval = run.evaluate_detail(&session, &mut log);
        assert_eq!(eval.result, EvaluationResult::Run);
        log.push("Allow");

        let entries = log_entries(&log);
        assert_eq!(
            entries,
            vec![
                "0:keyMatch:hdr:X-A".to_string(),
                "1:keyMatch:hdr:X-B".to_string(),
                "2:keyMatch:hdr:X-C".to_string(),
                "Allow".to_string(),
            ]
        );
        let aggregate: usize = entries.iter().map(|e| e.len()).sum();
        assert!(
            aggregate <= crate::observe::logs::BUFFER_CAPACITY,
            "aggregate {aggregate} exceeded BUFFER_CAPACITY"
        );
        assert!(log.ulog.is_none(), "realistic N stays in tier-1");
        assert!(log.log_full.is_none());
    }

    #[test]
    fn body_evaluate_stays_silent() {
        let mut skip = PluginConditions {
            skip: Some(vec![key_match("X-A", &["yes"])]),
            run: None,
        };
        skip.compile().expect("compile descriptors");
        let session = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-A".to_string(),
            },
            Some("yes".to_string()),
        )]);
        assert_eq!(skip.evaluate(&session), EvaluationResult::Skip);
    }

    #[test]
    fn direct_peer_ip_match_in_run_is_reachable() {
        use edgion_resources::resources::edgion_plugins::conditions::IpAccessMatch;
        let mut ip = IpAccessMatch {
            ips: vec!["10.0.0.0/8".to_string()],
            ..Default::default()
        };
        ip.validate_and_init().expect("compile IP matcher");
        let mut conds = PluginConditions {
            skip: None,
            run: Some(vec![Condition::DirectPeerIpMatch(ip)]),
        };
        conds.compile().expect("compile descriptors");

        let mut hit = MockPluginSession::new();
        hit.expect_direct_peer_ip().return_const("10.1.2.3".to_string());
        assert_eq!(conds.evaluate(&hit), EvaluationResult::Run);

        let mut miss = MockPluginSession::new();
        miss.expect_direct_peer_ip().return_const("203.0.113.1".to_string());
        assert_eq!(conds.evaluate(&miss), EvaluationResult::Skip);
    }

    fn jwt_session(claims: Option<&'static str>) -> MockPluginSession {
        let mut s = MockPluginSession::new();
        s.expect_get_ctx_var().returning(move |k| {
            if k == "jwt_claims" {
                claims.map(str::to_string)
            } else {
                None
            }
        });
        s
    }

    fn jwt_match(claim: &str, values: &[&str], negate: bool) -> Condition {
        let mut c = JwtClaimMatchCondition {
            claim: claim.to_string(),
            core: core_values(values.to_vec()),
            negate,
            descriptor: Box::from(""),
        };
        c.compile().expect("jwtClaimMatch compiles");
        Condition::JwtClaimMatch(c)
    }

    #[test]
    fn jwt_claim_match_present_absent_and_negated() {
        let mut log = PluginLog::new("test");
        let present = jwt_session(Some(r#"{"sub":"u1","roles":["admin","dev"]}"#));
        let absent = jwt_session(None);

        let hit = jwt_match("sub", &["u1"], false);
        assert!(hit.evaluate(&present, Some(&mut log)));
        assert!(!hit.evaluate(&absent, Some(&mut log)));

        let miss_value = jwt_match("sub", &["other"], false);
        assert!(!miss_value.evaluate(&present, Some(&mut log)));

        let nested = jwt_match("roles", &["admin"], false);
        assert!(nested.evaluate(&present, Some(&mut log)));

        let negated_present = jwt_match("sub", &["u1"], true);
        assert!(!negated_present.evaluate(&present, Some(&mut log)));
        assert!(negated_present.evaluate(&absent, Some(&mut log)));
    }

    fn cert_session(cert: Option<crate::ctx::ClientCertInfo>) -> MockPluginSession {
        let mut s = MockPluginSession::new();
        s.expect_client_cert_info().returning(move || cert.clone());
        s
    }

    fn multi_san_cert() -> crate::ctx::ClientCertInfo {
        crate::ctx::ClientCertInfo {
            subject: "CN=alice, O=acme".to_string(),
            sans: vec![
                "alice.example.com".to_string(),
                "uri:spiffe://edgion.io/ns/foo".to_string(),
                "email:alice@example.com".to_string(),
            ],
            cn: Some("alice".to_string()),
            fingerprint: "ab:cd".to_string(),
        }
    }

    fn mtls_match(
        field: edgion_resources::resources::edgion_plugins::conditions::MtlsField,
        san_type: Option<edgion_resources::resources::edgion_plugins::conditions::SanType>,
        values: &[&str],
        negate: bool,
    ) -> Condition {
        let mut c = MtlsMatchCondition {
            field,
            san_type,
            core: core_values(values.to_vec()),
            negate,
            descriptor: Box::from(""),
        };
        c.compile().expect("mtlsMatch compiles");
        Condition::MtlsMatch(c)
    }

    #[test]
    fn mtls_match_present_absent_and_negated() {
        use edgion_resources::resources::edgion_plugins::conditions::MtlsField;
        let mut log = PluginLog::new("test");
        let present = cert_session(Some(multi_san_cert()));
        let absent = cert_session(None);

        let cn = mtls_match(MtlsField::Cn, None, &["alice"], false);
        assert!(cn.evaluate(&present, Some(&mut log)));
        assert!(!cn.evaluate(&absent, Some(&mut log)));

        let negated_cn = mtls_match(MtlsField::Cn, None, &["alice"], true);
        assert!(!negated_cn.evaluate(&present, Some(&mut log)));
        assert!(negated_cn.evaluate(&absent, Some(&mut log)));

        let subject = mtls_match(MtlsField::Subject, None, &["CN=alice, O=acme"], false);
        assert!(subject.evaluate(&present, Some(&mut log)));
    }

    #[test]
    fn mtls_match_any_san_entry_on_multi_san_cert() {
        use edgion_resources::resources::edgion_plugins::conditions::{MtlsField, SanType};
        let mut log = PluginLog::new("test");
        let present = cert_session(Some(multi_san_cert()));

        let untyped = mtls_match(MtlsField::San, None, &["uri:spiffe://edgion.io/ns/foo"], false);
        assert!(untyped.evaluate(&present, Some(&mut log)));

        let dns = mtls_match(MtlsField::San, Some(SanType::Dns), &["alice.example.com"], false);
        assert!(dns.evaluate(&present, Some(&mut log)));

        let uri = mtls_match(
            MtlsField::San,
            Some(SanType::Uri),
            &["spiffe://edgion.io/ns/foo"],
            false,
        );
        assert!(uri.evaluate(&present, Some(&mut log)));

        let miss = mtls_match(MtlsField::San, None, &["other.example.com"], false);
        assert!(!miss.evaluate(&present, Some(&mut log)));
    }

    #[test]
    fn request_path_differs_from_path_for_percent_encoded_admin() {
        let raw = "/%61dmin";
        let normalized = "/admin";
        let session = create_mock_session_with_key_get(vec![
            (KeyGet::Path, Some(normalized.to_string())),
            (KeyGet::RequestPath, Some(raw.to_string())),
        ]);
        assert_ne!(
            session.key_get(&KeyGet::Path),
            session.key_get(&KeyGet::RequestPath),
            "Path is normalized; RequestPath stays raw for /%61dmin"
        );
        assert_eq!(session.key_get(&KeyGet::RequestPath).as_deref(), Some(raw));
        assert_eq!(session.key_get(&KeyGet::Path).as_deref(), Some(normalized));

        let mut request_path_match = KeyMatchCondition {
            key: KeyGet::RequestPath,
            core: core_values(vec![raw]),
            negate: false,
            descriptor: Box::from(""),
        };
        request_path_match.compile().expect("requestPath keyMatch compiles");
        assert_eq!(request_path_match.core.values.as_ref().unwrap()[0], raw);

        let mut path_match = KeyMatchCondition {
            key: KeyGet::Path,
            core: core_values(vec![raw]),
            negate: false,
            descriptor: Box::from(""),
        };
        path_match.compile().expect("path keyMatch compiles");
        assert_eq!(path_match.core.values.as_ref().unwrap()[0], normalized);

        let mut log = PluginLog::new("test");
        assert!(request_path_match.evaluate(&session, Some(&mut log)));
        assert!(path_match.evaluate(&session, Some(&mut log)));
    }

    #[test]
    fn unresolved_references_never_match_when_negated() {
        let session = MockPluginSession::new();
        let cases: Vec<Condition> = vec![
            serde_json::from_value(serde_json::json!({
                "type": "remoteIpMatch",
                "ipRefs": [{ "name": "missing" }],
                "negate": true
            }))
            .unwrap(),
            serde_json::from_value(serde_json::json!({
                "type": "secretMatch",
                "key": { "type": "header", "name": "X-Token" },
                "secretKey": "token",
                "secretRef": { "name": "missing" },
                "negate": true
            }))
            .unwrap(),
            serde_json::from_value(serde_json::json!({
                "type": "hmacAuth",
                "secretGroups": [{
                    "name": "default",
                    "secretRefs": [{ "name": "missing" }]
                }],
                "negate": true
            }))
            .unwrap(),
            serde_json::from_value(serde_json::json!({
                "type": "hmacAuth",
                "secretGroups": [{
                    "name": "default",
                    "secretRefs": [{ "name": "empty" }]
                }],
                "resolvedCredentials": {},
                "negate": true
            }))
            .unwrap(),
        ];

        for condition in cases {
            assert!(condition.has_unresolved_refs());
            assert!(!condition.evaluate(&session, None));
        }
    }

    #[test]
    fn resolved_empty_ip_union_is_not_unresolved() {
        let mut condition: Condition = serde_json::from_value(serde_json::json!({
            "type": "remoteIpMatch",
            "ipRefs": [{ "name": "empty" }],
            "negate": true
        }))
        .unwrap();
        let Condition::RemoteIpMatch(ip) = &mut condition else {
            unreachable!()
        };
        ip.resolved_ips = Some(Vec::new());

        let mut session = MockPluginSession::new();
        session.expect_remote_ip().return_const("192.0.2.1".to_string());
        assert!(!condition.has_unresolved_refs());
        assert!(condition.evaluate(&session, None));
    }
}
