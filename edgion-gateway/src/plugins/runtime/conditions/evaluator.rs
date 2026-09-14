//! Condition evaluator for plugin conditional execution
//!
//! This module provides the logic to evaluate conditions at runtime
//! to determine whether a plugin should be executed or skipped.

use crate::plugins::http::hmac_auth::{HmacVerifier, HmacVerifyOutcome};
use crate::plugins::runtime::body_gate::body_read_failure_result;
use crate::plugins::runtime::conditions::extract::{extract_jwt_claim_values, extract_mtls_values};
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::PluginSession;
use chrono::{DateTime, Utc};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::conditions::{
    AllOfGroup, AnyOfClause, Condition, ConditionSet, JwtClaimMatchCondition, KeyExistCondition, KeyMatchCondition,
    MatchCore, MtlsMatchCondition, PluginConditions, ProbabilityCondition, TimeRangeCondition,
};
use edgion_resources::resources::edgion_plugins::BodyRequirement;
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
    fn compile(condition: &Condition, descriptor: String, body_requirement: Option<BodyRequirement>) -> Self {
        let unresolved = condition.has_unresolved_refs();
        let descriptor = condition_descriptor(&descriptor, condition);
        let predicate = match condition {
            Condition::RateLimitExceeded(config) => CompiledPredicate::RateLimitExceeded {
                alias: config.plugin_alias.clone().into_boxed_str(),
                slot: None,
            },
            Condition::TimeRange(config) => CompiledPredicate::TimeRange(CompiledTimeRangeCondition::new(config)),
            Condition::HmacAuth(config) if config.validate_request_body && !unresolved => CompiledPredicate::HmacBody {
                verifier: HmacVerifier::new(config, body_requirement),
                negate: config.negate,
            },
            _ => CompiledPredicate::Sync {
                hmac: match condition {
                    Condition::HmacAuth(config) if !unresolved => Some(HmacVerifier::new(config, None)),
                    _ => None,
                },
                condition: condition.clone(),
            },
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
    pub(crate) fn new(set: &ConditionSet, prefix: &str, body_requirement: Option<BodyRequirement>) -> Self {
        let mut has_unresolved_refs = false;
        let (kind, atomic_count) = match set {
            ConditionSet::AllOf { all_of } => {
                let group = compile_all_of(
                    all_of,
                    None,
                    format!("{prefix}.allOf"),
                    body_requirement,
                    &mut has_unresolved_refs,
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
                            has_unresolved_refs |= condition.has_unresolved_refs();
                            CompiledAnyOfClause::Condition(CompiledCondition::compile(
                                condition,
                                format!("{prefix}.anyOf[{index}]"),
                                body_requirement.clone(),
                            ))
                        }
                        AnyOfClause::AllOf(AllOfGroup { name, all_of }) => {
                            count += all_of.len();
                            CompiledAnyOfClause::AllOf(compile_all_of(
                                all_of,
                                name.as_deref(),
                                format!("{prefix}.anyOf[{index}].allOf"),
                                body_requirement.clone(),
                                &mut has_unresolved_refs,
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
        }
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
) -> CompiledAllOf {
    let mut group_unresolved = false;
    let predicates = conditions
        .iter()
        .enumerate()
        .map(|(index, condition)| {
            group_unresolved |= condition.has_unresolved_refs();
            CompiledCondition::compile(condition, format!("{descriptor}[{index}]"), body_requirement.clone())
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
        let skip = conditions
            .skip
            .as_ref()
            .map(|set| CompiledConditionSet::new(set, "skip", None));
        let run = conditions
            .run
            .as_ref()
            .map(|set| CompiledConditionSet::new(set, "run", None));
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
            Condition::DirectPeerIpMatch(c) => evaluate_ip_match(c, session.direct_peer_ip()),
            Condition::RemoteIpMatch(c) => evaluate_ip_match(c, session.remote_ip()),
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
                !condition.ip_refs.is_empty() && condition.resolved_ips.is_none()
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
        apply_negate(session.key_get(&self.key).is_some(), self.negate)
    }
}

impl KeyMatchConditionExt for KeyMatchCondition {
    fn evaluate(&self, session: &dyn PluginSession, log: Option<&mut PluginLog>) -> bool {
        let Some(value) = session.key_get(&self.key) else {
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
    use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use edgion_resources::common::KeyGet;
    use edgion_resources::resources::edgion_plugins::{HmacCredential, HmacMatchConfig};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

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

    fn fixed_utc(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("valid fixed timestamp")
            .with_timezone(&Utc)
    }

    fn compile_time_range(condition: &TimeRangeCondition) -> CompiledCondition {
        CompiledCondition::compile(
            &Condition::TimeRange(condition.clone()),
            "conditions.allOf[0]".to_string(),
            None,
        )
    }

    fn compiled_time_range(compiled: &CompiledCondition) -> &CompiledTimeRangeCondition {
        let CompiledPredicate::TimeRange(condition) = &compiled.predicate else {
            panic!("timeRange must use the compiled time predicate");
        };
        condition
    }

    #[test]
    fn compiled_time_range_stores_parsed_utc_bounds() {
        let condition = TimeRangeCondition {
            after: Some("2025-01-01T02:00:00+02:00".to_string()),
            before: Some("2025-01-01T03:00:00+02:00".to_string()),
            negate: false,
            descriptor: Box::from(""),
        };
        let compiled = compile_time_range(&condition);
        let time_range = compiled_time_range(&compiled);

        assert_eq!(time_range.after.as_ref(), Some(&fixed_utc("2025-01-01T00:00:00Z")));
        assert_eq!(time_range.before.as_ref(), Some(&fixed_utc("2025-01-01T01:00:00Z")));
        assert_eq!(compiled.descriptor(), "conditions.allOf[0]:timeRange");
    }

    #[test]
    fn time_range_matches_at_is_after_inclusive_and_before_exclusive() {
        let condition = TimeRangeCondition {
            after: Some("2025-01-01T00:00:00Z".to_string()),
            before: Some("2025-01-01T01:00:00Z".to_string()),
            negate: false,
            descriptor: Box::from(""),
        };
        let compiled = compile_time_range(&condition);
        let time_range = compiled_time_range(&compiled);

        assert!(!time_range.evaluate_at(&fixed_utc("2024-12-31T23:59:59Z")));
        assert!(time_range.evaluate_at(&fixed_utc("2025-01-01T00:00:00Z")));
        assert!(time_range.evaluate_at(&fixed_utc("2025-01-01T00:30:00Z")));
        assert!(!time_range.evaluate_at(&fixed_utc("2025-01-01T01:00:00Z")));
    }

    #[test]
    fn compiled_time_range_evaluate_at_applies_negation_once() {
        let condition = TimeRangeCondition {
            after: Some("2025-01-01T00:00:00Z".to_string()),
            before: Some("2025-01-01T01:00:00Z".to_string()),
            negate: true,
            descriptor: Box::from(""),
        };
        let compiled = compile_time_range(&condition);
        let time_range = compiled_time_range(&compiled);

        assert!(!time_range.evaluate_at(&fixed_utc("2025-01-01T00:30:00Z")));
        assert!(time_range.evaluate_at(&fixed_utc("2025-01-01T01:00:00Z")));
    }

    #[test]
    fn compiled_time_range_treats_invalid_bound_as_absent_defensively() {
        let condition = TimeRangeCondition {
            after: Some("not-a-timestamp".to_string()),
            before: Some("2025-01-01T01:00:00Z".to_string()),
            negate: false,
            descriptor: Box::from(""),
        };
        assert!(condition.validate().is_err());

        let compiled = compile_time_range(&condition);
        let time_range = compiled_time_range(&compiled);
        assert!(time_range.after.is_none());
        assert_eq!(time_range.before.as_ref(), Some(&fixed_utc("2025-01-01T01:00:00Z")));
        assert!(time_range.evaluate_at(&fixed_utc("2025-01-01T00:30:00Z")));
        assert!(!time_range.evaluate_at(&fixed_utc("2025-01-01T01:00:00Z")));
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
        let compiled = CompiledCondition::compile(&condition, "conditions.allOf[0]".to_string(), None);
        let credentials_ptr = compiled
            .hmac_credentials_ptr()
            .expect("header-only HMAC must prebuild one verifier");
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[]);

        assert_eq!(compiled.evaluate_sync(&session), MatchState::False);
        assert_eq!(compiled.evaluate_sync(&session), MatchState::False);
        assert_eq!(compiled.hmac_credentials_ptr(), Some(credentials_ptr));
    }

    fn unresolved_secret_condition() -> Condition {
        Condition::SecretMatch(
            edgion_resources::resources::edgion_plugins::conditions::SecretMatchCondition {
                key: KeyGet::Header {
                    name: "X-Secret".to_string(),
                },
                secret_key: "token".to_string(),
                secret_ref: edgion_resources::resources::common::SecretObjectReference {
                    name: "missing".to_string(),
                    group: None,
                    kind: None,
                    namespace: None,
                },
                negate: false,
                resolved_values: None,
                descriptor: Box::from(""),
            },
        )
    }

    #[test]
    fn mixed_any_of_skips_unresolved_direct_and_attributes_named_fallback() {
        let set = ConditionSet::AnyOf {
            any_of: vec![
                AnyOfClause::Condition(unresolved_secret_condition()),
                AnyOfClause::AllOf(AllOfGroup {
                    name: Some("header-fallback".to_string()),
                    all_of: vec![key_match("X-Fallback", &["yes"])],
                }),
            ],
        };
        let compiled = CompiledConditionSet::new(&set, "conditions", None);
        let session = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Fallback".to_string(),
            },
            Some("yes".to_string()),
        )]);
        let mut log = PluginLog::new("test");

        let eval = compiled.evaluate(&session, Some(&mut log), true);
        assert_eq!(eval.state, MatchState::True);
        assert_eq!(eval.cause, Some("header-fallback"));
        assert!(log.contains("header-fallback"));
        assert!(log.contains("conditions.anyOf[1].allOf[0]:keyMatch:hdr:X-Fallback"));
    }

    #[test]
    fn mixed_any_of_matching_direct_short_circuits_later_named_group() {
        let set = ConditionSet::AnyOf {
            any_of: vec![
                AnyOfClause::Condition(key_match("X-First", &["yes"])),
                AnyOfClause::AllOf(AllOfGroup {
                    name: Some("must-not-run".to_string()),
                    all_of: vec![key_match("X-Later", &["yes"])],
                }),
            ],
        };
        let compiled = CompiledConditionSet::new(&set, "conditions", None);
        let session = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-First".to_string(),
            },
            Some("yes".to_string()),
        )]);
        let mut log = PluginLog::new("test");

        let eval = compiled.evaluate(&session, Some(&mut log), true);
        assert_eq!(eval.state, MatchState::True);
        assert_eq!(eval.cause, Some("conditions.anyOf[0]:keyMatch:hdr:X-First"));
        assert!(!log.contains("must-not-run"));
    }

    #[tokio::test]
    async fn unresolved_then_body_hmac_preserves_body_terminal_error() {
        let secret = b"body-hmac-secret-at-least-32-bytes";
        let date = Utc::now().to_rfc2822();
        let digest = "SHA-256=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let signing_string = format!("date: {date}\ndigest: {digest}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("valid HMAC key");
        mac.update(signing_string.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        let authorization = format!(
            r#"hmac username="alice", algorithm="hmac-sha256", headers="date digest", signature="{signature}""#
        );
        let body_hmac = Condition::HmacAuth(HmacMatchConfig {
            validate_request_body: true,
            resolved_credentials: Some(std::collections::HashMap::from([(
                "alice".to_string(),
                HmacCredential {
                    secret: secret.to_vec(),
                    headers: std::collections::HashMap::new(),
                    group: "default".to_string(),
                },
            )])),
            ..Default::default()
        });
        let set = ConditionSet::all_of(vec![unresolved_secret_condition(), body_hmac]);
        let compiled = CompiledConditionSet::new(&set, "conditions", Some(BodyRequirement::fail_close()));
        let mut session = MockPluginSession::new();
        setup_header_mocks(
            &mut session,
            &[
                ("authorization", authorization.as_str()),
                ("date", date.as_str()),
                ("digest", digest),
            ],
        );
        session
            .expect_ensure_request_body()
            .returning(|_| Err(BodyReadError::Timeout));
        session.expect_record_error().returning(|_| ());
        let mut log = PluginLog::new("test");

        assert!(matches!(
            compiled.evaluate_async(&mut session, &mut log, true).await,
            Err(PluginRunningResult::ErrResponse { status: 408, .. })
        ));
    }

    #[tokio::test]
    async fn top_all_of_false_dominates_unresolved_without_reading_body_hmac() {
        let body_hmac = Condition::HmacAuth(HmacMatchConfig {
            validate_request_body: true,
            resolved_credentials: Some(std::collections::HashMap::from([(
                "alice".to_string(),
                HmacCredential {
                    secret: b"body-hmac-secret-at-least-32-bytes".to_vec(),
                    headers: std::collections::HashMap::new(),
                    group: "default".to_string(),
                },
            )])),
            ..Default::default()
        });
        let set = ConditionSet::all_of(vec![
            unresolved_secret_condition(),
            key_match("X-False", &["yes"]),
            body_hmac,
        ]);
        let compiled = CompiledConditionSet::new(&set, "conditions", Some(BodyRequirement::fail_close()));
        let mut session = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-False".to_string(),
            },
            Some("no".to_string()),
        )]);
        let mut log = PluginLog::new("test");

        let eval = compiled
            .evaluate_async(&mut session, &mut log, true)
            .await
            .expect("false evaluation is non-terminal");
        assert_eq!(eval.state, MatchState::False);
        assert_eq!(eval.cause, Some("conditions.allOf[1]:keyMatch:hdr:X-False"));
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
            skip: Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
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
            skip: Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
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
            run: Some(ConditionSet::all_of(vec![
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
            ])),
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
            run: Some(ConditionSet::all_of(vec![
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
            ])),
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
            run: Some(ConditionSet::all_of(vec![
                key_match("X-A", &["yes"]),
                key_match("X-B", &["yes"]),
                key_match("X-C", &["yes"]),
            ])),
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
            vec![
                "run.allOf[0]:keyMatch:hdr:X-A".to_string(),
                "run.allOf[1]:keyMatch:hdr:X-B".to_string(),
            ]
        );

        // OR (skip): first match short-circuits — only that descriptor is pushed.
        let mut skip = PluginConditions {
            skip: Some(ConditionSet::AnyOf {
                any_of: vec![
                    AnyOfClause::Condition(key_match("X-A", &["yes"])),
                    AnyOfClause::Condition(key_match("X-B", &["yes"])),
                ],
            }),
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
        assert_eq!(
            log_entries(&skip_log),
            vec!["skip.anyOf[0]:keyMatch:hdr:X-A".to_string()]
        );
    }

    #[test]
    fn condition_log_stays_inside_tier1_or_records_spill() {
        let mut run = PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![
                key_match("X-A", &["yes"]),
                key_match("X-B", &["yes"]),
                key_match("X-C", &["yes"]),
            ])),
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
                "run.allOf[0]:keyMatch:hdr:X-A".to_string(),
                "run.allOf[1]:keyMatch:hdr:X-B".to_string(),
                "run.allOf[2]:keyMatch:hdr:X-C".to_string(),
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
            skip: Some(ConditionSet::all_of(vec![key_match("X-A", &["yes"])])),
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
            run: Some(ConditionSet::all_of(vec![Condition::DirectPeerIpMatch(ip)])),
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
