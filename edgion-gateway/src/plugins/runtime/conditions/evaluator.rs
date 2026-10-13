//! Condition evaluator for plugin conditional execution
//!
//! This module provides the logic to evaluate conditions at runtime
//! to determine whether a plugin should be executed or skipped.

use super::ip_refs::{BoundIpPredicate, IpConditionCompiler};
use crate::plugins::http::hmac_auth::{HmacVerifier, HmacVerifyOutcome};
use crate::plugins::runtime::body_gate::body_read_failure_result;
use crate::plugins::runtime::conditions::extract::{extract_jwt_claim_values, extract_mtls_values};
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::session_adapter::cookie_value_ref;
use crate::plugins::runtime::traits::header_ext::PluginSessionHeaderExt;
use crate::plugins::runtime::PluginSession;
use chrono::{DateTime, Utc};
use edgion_resources::common::KeyGet;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::conditions::{
    AllOfGroup, AnyOfClause, Condition, ConditionSet, JwtClaimMatchCondition, KeyExistCondition, KeyMatchCondition,
    MatchCore, MtlsMatchCondition, PluginConditions, ProbabilityCondition, TimeRangeCondition,
};
use edgion_resources::resources::edgion_plugins::BodyRequirement;
use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

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
    /// Stable, value-free cause descriptor.
    pub cause: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchState {
    True,
    False,
    Unresolved,
}

pub(crate) struct SetEval<'a> {
    pub(crate) state: MatchState,
    pub(crate) cause: Option<&'a str>,
}

struct CompiledTimeRangeCondition {
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    negate: bool,
}

impl CompiledTimeRangeCondition {
    fn new(condition: &TimeRangeCondition) -> Self {
        Self {
            after: parse_time_range_bound(condition.after.as_deref()),
            before: parse_time_range_bound(condition.before.as_deref()),
            negate: condition.negate,
        }
    }

    fn evaluate(&self) -> bool {
        let now = Utc::now();
        self.evaluate_at(&now)
    }

    fn evaluate_at(&self, now: &DateTime<Utc>) -> bool {
        apply_negate(
            time_range_matches_at(now, self.after.as_ref(), self.before.as_ref()),
            self.negate,
        )
    }
}

#[allow(clippy::large_enum_variant)]
enum CompiledPredicate {
    RateLimitExceeded {
        alias: Box<str>,
        slot: Option<usize>,
    },
    Sync {
        condition: Condition,
        hmac: Option<HmacVerifier>,
    },
    TimeRange(CompiledTimeRangeCondition),
    Ip {
        refs: Option<Arc<BoundIpPredicate>>,
        direct: bool,
        negate: bool,
    },
    HmacBody {
        verifier: HmacVerifier,
        negate: bool,
    },
}

/// Gateway-owned, immutable representation of one schema condition.
pub(crate) struct CompiledCondition {
    predicate: CompiledPredicate,
    descriptor: Box<str>,
    unresolved: bool,
}

impl CompiledCondition {
    fn compile(
        condition: &Condition,
        descriptor: String,
        body_requirement: Option<BodyRequirement>,
        ip: Option<&IpConditionCompiler<'_>>,
    ) -> Self {
        let mut unresolved = condition.has_unresolved_refs()
            || match condition {
                Condition::DirectPeerIpMatch(config) | Condition::RemoteIpMatch(config) => {
                    ip.and_then(|ip| ip.bind(&config.refs)).is_none()
                }
                _ => false,
            };
        let descriptor = condition_descriptor(&descriptor, condition);
        let predicate = match condition {
            Condition::DirectPeerIpMatch(config) | Condition::RemoteIpMatch(config) => CompiledPredicate::Ip {
                refs: ip.and_then(|ip| ip.bind(&config.refs)),
                direct: matches!(condition, Condition::DirectPeerIpMatch(_)),
                negate: config.negate,
            },
            Condition::RateLimitExceeded(config) => CompiledPredicate::RateLimitExceeded {
                alias: config.plugin_alias.clone().into_boxed_str(),
                slot: None,
            },
            Condition::TimeRange(config) => CompiledPredicate::TimeRange(CompiledTimeRangeCondition::new(config)),
            Condition::HmacAuth(config) if config.validate_request_body && !unresolved => CompiledPredicate::HmacBody {
                verifier: HmacVerifier::new(config, body_requirement),
                negate: config.negate,
            },
            _ => {
                // Initialize the private non-IP leaf without cloning the owning
                // condition set: IP bindings must retain their original nodes.
                let mut compiled = condition.clone();
                unresolved |= compiled.compile(0).is_err();
                CompiledPredicate::Sync {
                    hmac: match condition {
                        Condition::HmacAuth(config) if !unresolved => Some(HmacVerifier::new(config, None)),
                        _ => None,
                    },
                    condition: compiled,
                }
            }
        };
        Self {
            predicate,
            descriptor: descriptor.into(),
            unresolved,
        }
    }

    fn evaluate_sync(&self, session: &dyn PluginSession) -> MatchState {
        if self.unresolved {
            return MatchState::Unresolved;
        }
        match &self.predicate {
            CompiledPredicate::Ip { refs, direct, negate } => {
                let address = if *direct {
                    session.direct_peer_ip()
                } else {
                    session.remote_ip()
                }
                .parse()
                .ok();
                refs.as_ref()
                    .map(|refs| refs.evaluate(session, address, *negate))
                    .unwrap_or(MatchState::Unresolved)
            }
            CompiledPredicate::RateLimitExceeded { slot, .. } => {
                use crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalOutcome;
                match slot.map(|slot| session.rate_limit_local_outcome(slot)) {
                    Some(RateLimitLocalOutcome::Exceeded) => MatchState::True,
                    Some(RateLimitLocalOutcome::WithinLimit) => MatchState::False,
                    _ => MatchState::Unresolved,
                }
            }
            CompiledPredicate::Sync { condition, hmac } => {
                let matched = match (condition, hmac) {
                    (Condition::HmacAuth(config), Some(verifier)) => apply_negate(
                        matches!(verifier.verify_headers(session), HmacVerifyOutcome::Verified { .. }),
                        config.negate,
                    ),
                    _ => condition.evaluate(session, None),
                };
                if matched {
                    MatchState::True
                } else {
                    MatchState::False
                }
            }
            CompiledPredicate::TimeRange(condition) => {
                if condition.evaluate() {
                    MatchState::True
                } else {
                    MatchState::False
                }
            }
            // A body predicate cannot be evaluated from a response/body-chunk
            // callback. Schema validation prevents this combination.
            CompiledPredicate::HmacBody { .. } => MatchState::Unresolved,
        }
    }

    async fn evaluate_async(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Result<MatchState, PluginRunningResult> {
        if self.unresolved {
            return Ok(MatchState::Unresolved);
        }
        let CompiledPredicate::HmacBody { verifier, negate } = &self.predicate else {
            return Ok(self.evaluate_sync(session));
        };
        Ok(match verifier.verify(session).await {
            HmacVerifyOutcome::Verified { .. } => {
                if *negate {
                    MatchState::False
                } else {
                    MatchState::True
                }
            }
            HmacVerifyOutcome::NoMatch(_) | HmacVerifyOutcome::LogicalBodyLimit(_) => {
                if *negate {
                    MatchState::True
                } else {
                    MatchState::False
                }
            }
            HmacVerifyOutcome::CaptureFailure(error) | HmacVerifyOutcome::DigestReadFailure(error) => {
                return Err(body_read_failure_result(
                    session,
                    verifier.body_requirement(),
                    log,
                    &error,
                ));
            }
        })
    }

    pub(crate) fn descriptor(&self) -> &str {
        &self.descriptor
    }

    #[cfg(test)]
    fn hmac_credentials_ptr(&self) -> Option<*const ()> {
        match &self.predicate {
            CompiledPredicate::Sync {
                hmac: Some(verifier), ..
            }
            | CompiledPredicate::HmacBody { verifier, .. } => Some(verifier.credentials_ptr()),
            _ => None,
        }
    }
}

fn condition_descriptor(path: &str, condition: &Condition) -> String {
    match condition {
        Condition::KeyExist(value) => format!("{path}:keyExist:{}", value.key.as_log_str()),
        Condition::KeyMatch(value) => format!("{path}:keyMatch:{}", value.key.as_log_str()),
        Condition::SecretMatch(value) => format!("{path}:secretMatch:{}", value.key.as_log_str()),
        Condition::TimeRange(_) => format!("{path}:timeRange"),
        Condition::Probability(_) => format!("{path}:probability"),
        Condition::DirectPeerIpMatch(_) => format!("{path}:directPeerIpMatch"),
        Condition::RemoteIpMatch(_) => format!("{path}:remoteIpMatch"),
        Condition::HmacAuth(_) => format!("{path}:hmacAuth"),
        Condition::JwtClaimMatch(_) => format!("{path}:jwtClaimMatch"),
        Condition::MtlsMatch(_) => format!("{path}:mtlsMatch"),
        Condition::RateLimitExceeded(_) => format!("{path}:rateLimitExceeded"),
    }
}

struct CompiledAllOf {
    name: Option<Box<str>>,
    descriptor: Box<str>,
    predicates: Vec<CompiledCondition>,
}

enum CompiledSetKind {
    AllOf(CompiledAllOf),
    AnyOf(Vec<CompiledAnyOfClause>),
}

#[allow(clippy::large_enum_variant)]
enum CompiledAnyOfClause {
    Condition(CompiledCondition),
    AllOf(CompiledAllOf),
}

/// One immutable, bounded-DNF evaluator shared by every Gateway consumer.
pub(crate) struct CompiledConditionSet {
    kind: CompiledSetKind,
    atomic_count: usize,
    has_unresolved_refs: bool,
    has_ip: bool,
}

impl CompiledConditionSet {
    fn visit_mut(&mut self, visitor: &mut impl FnMut(&mut CompiledCondition)) {
        match &mut self.kind {
            CompiledSetKind::AllOf(group) => group.predicates.iter_mut().for_each(visitor),
            CompiledSetKind::AnyOf(clauses) => {
                for clause in clauses {
                    match clause {
                        CompiledAnyOfClause::Condition(condition) => visitor(condition),
                        CompiledAnyOfClause::AllOf(group) => group.predicates.iter_mut().for_each(&mut *visitor),
                    }
                }
            }
        }
    }
    pub(crate) fn with_ip(
        set: &ConditionSet,
        prefix: &str,
        body_requirement: Option<BodyRequirement>,
        ip: Option<&IpConditionCompiler<'_>>,
    ) -> Self {
        let mut has_unresolved_refs = false;
        let has_ip = match set {
            ConditionSet::AllOf { all_of } => all_of
                .iter()
                .any(|condition| matches!(condition, Condition::DirectPeerIpMatch(_) | Condition::RemoteIpMatch(_))),
            ConditionSet::AnyOf { any_of } => any_of.iter().any(|clause| match clause {
                AnyOfClause::Condition(condition) => {
                    matches!(condition, Condition::DirectPeerIpMatch(_) | Condition::RemoteIpMatch(_))
                }
                AnyOfClause::AllOf(group) => group.all_of.iter().any(|condition| {
                    matches!(condition, Condition::DirectPeerIpMatch(_) | Condition::RemoteIpMatch(_))
                }),
            }),
        };
        let (kind, atomic_count) = match set {
            ConditionSet::AllOf { all_of } => {
                let group = compile_all_of(
                    all_of,
                    None,
                    format!("{prefix}.allOf"),
                    body_requirement,
                    &mut has_unresolved_refs,
                    ip,
                );
                let count = group.predicates.len();
                (CompiledSetKind::AllOf(group), count)
            }
            ConditionSet::AnyOf { any_of } => {
                let mut count = 0;
                let clauses = any_of
                    .iter()
                    .enumerate()
                    .map(|(index, clause)| match clause {
                        AnyOfClause::Condition(condition) => {
                            count += 1;
                            let compiled = CompiledCondition::compile(
                                condition,
                                format!("{prefix}.anyOf[{index}]"),
                                body_requirement.clone(),
                                ip,
                            );
                            has_unresolved_refs |= compiled.unresolved;
                            CompiledAnyOfClause::Condition(compiled)
                        }
                        AnyOfClause::AllOf(AllOfGroup { name, all_of }) => {
                            count += all_of.len();
                            CompiledAnyOfClause::AllOf(compile_all_of(
                                all_of,
                                name.as_deref(),
                                format!("{prefix}.anyOf[{index}].allOf"),
                                body_requirement.clone(),
                                &mut has_unresolved_refs,
                                ip,
                            ))
                        }
                    })
                    .collect();
                (CompiledSetKind::AnyOf(clauses), count)
            }
        };
        Self {
            kind,
            atomic_count,
            has_unresolved_refs,
            has_ip,
        }
    }

    pub(crate) fn requires_config_data_view(&self) -> bool {
        self.has_ip
    }
    pub(crate) fn atomic_count(&self) -> usize {
        self.atomic_count
    }
    pub(crate) fn has_unresolved_refs(&self) -> bool {
        self.has_unresolved_refs
    }

    pub(crate) fn evaluate(
        &self,
        session: &dyn PluginSession,
        mut log: Option<&mut PluginLog>,
        emit: bool,
    ) -> SetEval<'_> {
        match &self.kind {
            CompiledSetKind::AllOf(group) => eval_all_of_sync(group, session, log, emit),
            CompiledSetKind::AnyOf(clauses) => {
                let mut unresolved = false;
                for clause in clauses {
                    let eval = match clause {
                        CompiledAnyOfClause::Condition(condition) => {
                            eval_atom_sync(condition, session, log.as_deref_mut(), emit)
                        }
                        CompiledAnyOfClause::AllOf(group) => eval_all_of_sync(group, session, log.as_deref_mut(), emit),
                    };
                    match eval.state {
                        MatchState::True => return eval,
                        MatchState::False => {}
                        MatchState::Unresolved => unresolved = true,
                    }
                }
                SetEval {
                    state: if unresolved {
                        MatchState::Unresolved
                    } else {
                        MatchState::False
                    },
                    cause: Some("anyOf:exhausted"),
                }
            }
        }
    }

    pub(crate) async fn evaluate_async<'a>(
        &'a self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        emit: bool,
    ) -> Result<SetEval<'a>, PluginRunningResult> {
        match &self.kind {
            CompiledSetKind::AllOf(group) => eval_all_of_async(group, session, log, emit).await,
            CompiledSetKind::AnyOf(clauses) => {
                let mut unresolved = false;
                for clause in clauses {
                    let eval = match clause {
                        CompiledAnyOfClause::Condition(condition) => {
                            eval_atom_async(condition, session, log, emit).await?
                        }
                        CompiledAnyOfClause::AllOf(group) => eval_all_of_async(group, session, log, emit).await?,
                    };
                    match eval.state {
                        MatchState::True => return Ok(eval),
                        MatchState::False => {}
                        MatchState::Unresolved => unresolved = true,
                    }
                }
                Ok(SetEval {
                    state: if unresolved {
                        MatchState::Unresolved
                    } else {
                        MatchState::False
                    },
                    cause: Some("anyOf:exhausted"),
                })
            }
        }
    }
}

fn compile_all_of(
    conditions: &[Condition],
    name: Option<&str>,
    descriptor: String,
    body_requirement: Option<BodyRequirement>,
    unresolved: &mut bool,
    ip: Option<&IpConditionCompiler<'_>>,
) -> CompiledAllOf {
    let mut group_unresolved = false;
    let predicates = conditions
        .iter()
        .enumerate()
        .map(|(index, condition)| {
            let compiled = CompiledCondition::compile(
                condition,
                format!("{descriptor}[{index}]"),
                body_requirement.clone(),
                ip,
            );
            group_unresolved |= compiled.unresolved;
            compiled
        })
        .collect();
    *unresolved |= group_unresolved;
    CompiledAllOf {
        name: name.map(Into::into),
        descriptor: descriptor.into(),
        predicates,
    }
}

fn eval_atom_sync<'a>(
    condition: &'a CompiledCondition,
    session: &dyn PluginSession,
    log: Option<&mut PluginLog>,
    emit: bool,
) -> SetEval<'a> {
    let state = condition.evaluate_sync(session);
    if state == MatchState::Unresolved {
        if let Some(log) = log {
            log.push_err("condition reference unresolved");
        }
    } else if state == MatchState::True && emit {
        if let Some(log) = log {
            log.push(condition.descriptor());
        }
    }
    SetEval {
        state,
        cause: Some(condition.descriptor()),
    }
}

fn eval_all_of_sync<'a>(
    group: &'a CompiledAllOf,
    session: &dyn PluginSession,
    mut log: Option<&mut PluginLog>,
    emit: bool,
) -> SetEval<'a> {
    let mut unresolved = false;
    for predicate in &group.predicates {
        let eval = eval_atom_sync(predicate, session, log.as_deref_mut(), emit);
        match eval.state {
            MatchState::False => return eval,
            MatchState::Unresolved => unresolved = true,
            MatchState::True => {}
        }
    }
    if unresolved {
        return SetEval {
            state: MatchState::Unresolved,
            cause: Some(group.descriptor.as_ref()),
        };
    }
    if let (Some(name), Some(log)) = (&group.name, log) {
        log.push(name);
    }
    SetEval {
        state: MatchState::True,
        cause: Some(group.name.as_deref().unwrap_or(group.descriptor.as_ref())),
    }
}

async fn eval_atom_async<'a>(
    condition: &'a CompiledCondition,
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    emit: bool,
) -> Result<SetEval<'a>, PluginRunningResult> {
    let state = condition.evaluate_async(session, log).await?;
    if state == MatchState::Unresolved {
        log.push_err("condition reference unresolved");
    } else if state == MatchState::True && emit {
        log.push(condition.descriptor());
    }
    Ok(SetEval {
        state,
        cause: Some(condition.descriptor()),
    })
}

async fn eval_all_of_async<'a>(
    group: &'a CompiledAllOf,
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    emit: bool,
) -> Result<SetEval<'a>, PluginRunningResult> {
    let mut unresolved = false;
    for predicate in &group.predicates {
        let eval = eval_atom_async(predicate, session, log, emit).await?;
        match eval.state {
            MatchState::False => return Ok(eval),
            MatchState::Unresolved => unresolved = true,
            MatchState::True => {}
        }
    }
    if unresolved {
        return Ok(SetEval {
            state: MatchState::Unresolved,
            cause: Some(group.descriptor.as_ref()),
        });
    }
    if let Some(name) = &group.name {
        log.push(name);
    }
    Ok(SetEval {
        state: MatchState::True,
        cause: Some(group.name.as_deref().unwrap_or(group.descriptor.as_ref())),
    })
}

/// Compiled skip-first / run-second plugin container.
pub(crate) struct CompiledPluginConditions {
    skip: Option<CompiledConditionSet>,
    run: Option<CompiledConditionSet>,
    atomic_count: usize,
    rate_limit_dependencies: Vec<Option<usize>>,
}

impl CompiledPluginConditions {
    pub(crate) fn new(conditions: PluginConditions) -> Self {
        Self::with_ip(&conditions, None)
    }
    pub(crate) fn with_ip(conditions: &PluginConditions, ip: Option<&IpConditionCompiler<'_>>) -> Self {
        let skip = conditions
            .skip
            .as_ref()
            .map(|set| CompiledConditionSet::with_ip(set, "skip", None, ip));
        let run = conditions
            .run
            .as_ref()
            .map(|set| CompiledConditionSet::with_ip(set, "run", None, ip));
        let atomic_count = skip.as_ref().map_or(0, CompiledConditionSet::atomic_count)
            + run.as_ref().map_or(0, CompiledConditionSet::atomic_count);
        let mut compiled = Self {
            skip,
            run,
            atomic_count,
            rate_limit_dependencies: Vec::new(),
        };
        compiled.bind_rate_limit_dependencies(&std::collections::HashMap::new());
        compiled
    }

    pub(crate) fn bind_rate_limit_dependencies(&mut self, aliases: &std::collections::HashMap<&str, usize>) {
        self.rate_limit_dependencies.clear();
        for set in [&mut self.skip, &mut self.run].into_iter().flatten() {
            set.visit_mut(&mut |condition| {
                if let CompiledPredicate::RateLimitExceeded { alias, slot } = &mut condition.predicate {
                    *slot = aliases.get(alias.as_ref()).copied();
                    self.rate_limit_dependencies.push(*slot);
                }
            });
        }
    }

    pub(crate) fn should_evaluate(&self) -> bool {
        self.skip.is_some() || self.run.is_some()
    }

    pub(crate) fn evaluate(&self, session: &dyn PluginSession) -> EvaluationResult {
        if self
            .skip
            .as_ref()
            .is_some_and(|set| set.evaluate(session, None, false).state == MatchState::True)
        {
            return EvaluationResult::Skip;
        }
        if self
            .run
            .as_ref()
            .is_some_and(|set| set.evaluate(session, None, false).state != MatchState::True)
        {
            return EvaluationResult::Skip;
        }
        EvaluationResult::Run
    }

    pub(crate) fn evaluate_detail<'a>(
        &'a self,
        session: &dyn PluginSession,
        log: &mut PluginLog,
    ) -> ConditionEvalResult<'a> {
        let emit = self.atomic_count > 1;
        if let Some(skip) = &self.skip {
            let eval = skip.evaluate(session, Some(&mut *log), emit);
            if eval.state == MatchState::True {
                return ConditionEvalResult {
                    result: EvaluationResult::Skip,
                    action: "skip",
                    cause: eval.cause,
                };
            }
        }
        if let Some(run) = &self.run {
            let eval = run.evaluate(session, Some(&mut *log), emit);
            if eval.state != MatchState::True {
                return ConditionEvalResult {
                    result: EvaluationResult::Skip,
                    action: "!run",
                    cause: eval.cause,
                };
            }
        }
        ConditionEvalResult {
            result: EvaluationResult::Run,
            action: "",
            cause: None,
        }
    }

    pub(crate) async fn evaluate_detail_async<'a>(
        &'a self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Result<ConditionEvalResult<'a>, PluginRunningResult> {
        use crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalOutcome;
        if self.rate_limit_dependencies.iter().any(|slot| {
            slot.is_none_or(|slot| session.rate_limit_local_outcome(slot) == RateLimitLocalOutcome::NotEvaluated)
        }) {
            log.push_err("local outcome unavailable");
            return Err(PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            });
        }
        let emit = self.atomic_count > 1;
        if let Some(skip) = &self.skip {
            let eval = skip.evaluate_async(session, log, emit).await?;
            if eval.state == MatchState::True {
                return Ok(ConditionEvalResult {
                    result: EvaluationResult::Skip,
                    action: "skip",
                    cause: eval.cause,
                });
            }
        }
        if let Some(run) = &self.run {
            let eval = run.evaluate_async(session, log, emit).await?;
            if eval.state != MatchState::True {
                return Ok(ConditionEvalResult {
                    result: EvaluationResult::Skip,
                    action: "!run",
                    cause: eval.cause,
                });
            }
        }
        Ok(ConditionEvalResult {
            result: EvaluationResult::Run,
            action: "",
            cause: None,
        })
    }
}

pub(crate) trait PluginConditionsExt {
    fn has_unresolved_refs(&self) -> bool;
    #[cfg(test)]
    fn evaluate(&self, session: &dyn PluginSession) -> EvaluationResult;
    #[cfg(test)]
    fn evaluate_detail<'a>(&self, session: &dyn PluginSession, log: &'a mut PluginLog) -> ConditionEvalResult<'a>;
}

pub(crate) trait ConditionExt {
    fn evaluate(&self, session: &dyn PluginSession, log: Option<&mut PluginLog>) -> bool;
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
    fn has_unresolved_refs(&self) -> bool {
        self.skip.as_ref().is_some_and(condition_set_has_unresolved)
            || self.run.as_ref().is_some_and(condition_set_has_unresolved)
    }

    #[cfg(test)]
    fn evaluate(&self, session: &dyn PluginSession) -> EvaluationResult {
        CompiledPluginConditions::new(self.clone()).evaluate(session)
    }

    #[cfg(test)]
    fn evaluate_detail<'a>(&self, session: &dyn PluginSession, log: &'a mut PluginLog) -> ConditionEvalResult<'a> {
        // Test-only convenience retains the compiled snapshot for the duration
        // of this call; callers only assert the copied decision fields.
        let compiled = CompiledPluginConditions::new(self.clone());
        let eval = compiled.evaluate_detail(session, log);
        ConditionEvalResult {
            result: eval.result,
            action: eval.action,
            cause: None,
        }
    }
}

pub(crate) fn condition_set_has_unresolved(set: &ConditionSet) -> bool {
    match set {
        ConditionSet::AllOf { all_of } => all_of.iter().any(ConditionExt::has_unresolved_refs),
        ConditionSet::AnyOf { any_of } => any_of.iter().any(|clause| match clause {
            AnyOfClause::Condition(condition) => condition.has_unresolved_refs(),
            AnyOfClause::AllOf(group) => group.all_of.iter().any(ConditionExt::has_unresolved_refs),
        }),
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
            Condition::DirectPeerIpMatch(_) => false,
            Condition::RemoteIpMatch(_) => false,
            // Runtime HMAC evaluation is available only through
            // CompiledCondition, which owns the snapshot verifier.
            Condition::HmacAuth(_) => false,
            Condition::JwtClaimMatch(c) => evaluate_jwt_claim_match(c, session, log.as_deref_mut()),
            Condition::MtlsMatch(c) => evaluate_mtls_match(c, session, log),
            // This request-only predicate requires a bound Gateway slot.
            Condition::RateLimitExceeded(_) => false,
        };
        matched
    }

    fn has_unresolved_refs(&self) -> bool {
        match self {
            Condition::SecretMatch(condition) => condition.resolved_values.is_none(),
            Condition::DirectPeerIpMatch(condition) | Condition::RemoteIpMatch(condition) => {
                condition.refs.invalid_input
            }
            Condition::HmacAuth(condition) => {
                condition
                    .secret_groups
                    .as_ref()
                    .is_some_and(|groups| !groups.is_empty())
                    && condition
                        .resolved_credentials
                        .as_ref()
                        .is_none_or(|credentials| credentials.is_empty())
            }
            _ => false,
        }
    }
}

/// Borrow live condition inputs for one evaluation. Sources without an
/// equivalent borrowed session accessor retain the canonical owned KeyGet path.
/// No value survives the current predicate evaluation, so later plugin writes
/// remain visible on the next chunk.
fn condition_key_value<'a>(session: &'a dyn PluginSession, key: &KeyGet) -> Option<Cow<'a, str>> {
    match key {
        KeyGet::Header { name } => session.header_str(name).map(Cow::Borrowed),
        KeyGet::Cookie { name } => cookie_value_ref(session.request_header_map(), name).map(Cow::Borrowed),
        KeyGet::Path => Some(Cow::Borrowed(session.get_path())),
        KeyGet::RequestPath => Some(Cow::Borrowed(session.get_request_path())),
        KeyGet::Method => Some(Cow::Borrowed(session.get_method())),
        KeyGet::RemoteIp => (!session.remote_ip().is_empty()).then(|| Cow::Borrowed(session.remote_ip())),
        KeyGet::DirectPeerIp => (!session.direct_peer_ip().is_empty()).then(|| Cow::Borrowed(session.direct_peer_ip())),
        _ => session.key_get(key).map(Cow::Owned),
    }
}

fn evaluate_secret_match(
    condition: &edgion_resources::resources::edgion_plugins::conditions::SecretMatchCondition,
    session: &dyn PluginSession,
) -> bool {
    use subtle::ConstantTimeEq;
    let Some(actual) = condition_key_value(session, &condition.key) else {
        return apply_negate(false, condition.negate);
    };
    let base = condition.resolved_values.as_ref().is_some_and(|expected| {
        expected
            .iter()
            .any(|candidate| candidate.len() == actual.len() && candidate.as_bytes().ct_eq(actual.as_bytes()).into())
    });
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
fn evaluate_multi_match(
    core: &MatchCore,
    candidates: impl IntoIterator<Item = impl AsRef<str>>,
    negate: bool,
    log: Option<&mut PluginLog>,
) -> bool {
    let mut present = false;
    let base = candidates.into_iter().any(|value| {
        present = true;
        core.matches(value.as_ref())
    });
    if present && !base && core.has_uncompiled_regex() {
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

fn parse_time_range_bound(bound: Option<&str>) -> Option<DateTime<Utc>> {
    bound
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn time_range_matches_at(now: &DateTime<Utc>, after: Option<&DateTime<Utc>>, before: Option<&DateTime<Utc>>) -> bool {
    after.is_none_or(|after| now >= after) && before.is_none_or(|before| now < before)
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
        apply_negate(condition_key_value(session, &self.key).is_some(), self.negate)
    }
}

impl KeyMatchConditionExt for KeyMatchCondition {
    fn evaluate(&self, session: &dyn PluginSession, log: Option<&mut PluginLog>) -> bool {
        let Some(value) = condition_key_value(session, &self.key) else {
            return apply_negate(false, self.negate);
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
        let after = parse_time_range_bound(self.after.as_deref());
        let before = parse_time_range_bound(self.before.as_deref());
        apply_negate(
            time_range_matches_at(&now, after.as_ref(), before.as_ref()),
            self.negate,
        )
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
            if let Some(key_value) = condition_key_value(session, key) {
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
#[path = "../../../../tests/unit/plugins/runtime/conditions/evaluator_test.rs"]
mod tests;
