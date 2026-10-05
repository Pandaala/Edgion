//! Internal multi-rule facts for a future structured executor. This module does
//! not schedule rules, publish variables, or commit transaction state.

use super::super::{
    ActionEnvelopeKind, CompiledActionOperand, CompiledOperatorOperand, CompiledSegmentKind, CompiledTargetSelector,
    TransformationKind,
};
use super::*;
use crate::seclang::{
    ActionKind, JsonBodyAdapter, JsonBodyConfig, JsonBodyError, JsonBodySummary, JsonDocumentEvent,
    JsonDocumentSinkError, JsonStreamInput,
};

/// Admission caps for the immutable generation-owned subscriber plan.
#[derive(Debug, Clone, Copy)]
pub struct JsonPlanLimits {
    pub max_subscribers: usize,
    /// Maximum allocated descriptor bytes of the shared immutable generation plan.
    /// This is admission against shared capacity, not per-request ownership.
    pub max_retained_bytes: usize,
}
/// Bounds for provisional scalar/subscriber facts, including overwritten values.
#[derive(Debug, Clone, Copy)]
pub struct JsonFactLimits {
    pub max_facts: usize,
    /// Aggregate allocated fact and selected-query-header capacities in bytes.
    /// Metadata sharing can change admission and growth-copy charges without
    /// changing canonical runtime work. Fixed owner structs are excluded.
    pub max_retained_bytes: usize,
    /// Speculative selection, transformation, matching and arena-growth work only.
    /// Replay traversal is separately bounded by event/fact and subscriber caps.
    pub max_work: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonExecutionLimit {
    Subscribers,
    Facts,
    RetainedBytes,
    Work,
}
/// Payload-free structured execution failures. Only unsupported initial admission
/// permits choosing ordinary publication; resource failures are not fallback signals.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum JsonExecutionError {
    /// Canonical publication or scheduled rule execution failure.
    #[error(transparent)]
    Runtime(RuntimeError),
    /// Independent source/document failure, including explicit abandonment.
    #[error(transparent)]
    Body(JsonBodyError),
    /// The generation, processor or transaction controls cannot use this lane.
    #[error("structured JSON execution is unsupported")]
    Unsupported,
    /// An independent shared-plan or provisional-fact budget was exhausted.
    #[error("structured JSON execution exceeded {kind:?} limit {limit}")]
    Limit { kind: JsonExecutionLimit, limit: usize },
    #[error("structured JSON execution allocation failed")]
    Allocation,
    #[error("structured JSON execution accounting overflowed")]
    Arithmetic,
    #[error("structured JSON execution is in an invalid lifecycle state")]
    State,
    #[error("structured JSON execution is already complete")]
    AlreadyComplete,
}
impl From<RuntimeError> for JsonExecutionError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct JsonFactUsage {
    pub facts: usize,
    pub retained_bytes: usize,
    pub work: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct JsonRuleOutcome {
    pub selected_count: usize,
    pub match_count: usize,
    /// Logical staged TX size. A nonmatch does not instruct the caller to commit it.
    pub final_tx_bytes: usize,
}

use super::super::json_plan::JsonSubscriberDescriptor as Subscriber;
pub(super) struct JsonSubscriberPlan<'g> {
    generation: &'g CompiledGeneration,
    subscribers: &'g [Subscriber],
}
#[derive(Clone, Copy)]
struct SubscriberView<'g> {
    generation: &'g CompiledGeneration,
    subscribers: &'g [Subscriber],
}
impl JsonSubscriberPlan<'_> {
    fn view(&self) -> SubscriberView<'_> {
        SubscriberView {
            generation: self.generation,
            subscribers: self.subscribers,
        }
    }
}
impl<'g> JsonSubscriberPlan<'g> {
    /// Borrow the immutable capability and check request admission caps in O(1).
    /// Unsupported capability is resolved before request caps; eligible empty
    /// plans are distinct from unsupported generations and require zero bytes.
    pub fn for_generation(
        generation: &'g CompiledGeneration,
        limits: JsonPlanLimits,
    ) -> Result<Self, JsonExecutionError> {
        let subscribers = generation
            .json_capability
            .subscribers()
            .ok_or(JsonExecutionError::Unsupported)?;
        if subscribers.len() > limits.max_subscribers {
            return Err(JsonExecutionError::Limit {
                kind: JsonExecutionLimit::Subscribers,
                limit: limits.max_subscribers,
            });
        }
        if generation.json_capability.retained_bytes() > limits.max_retained_bytes {
            return Err(JsonExecutionError::Limit {
                kind: JsonExecutionLimit::RetainedBytes,
                limit: limits.max_retained_bytes,
            });
        }
        Ok(Self {
            generation,
            subscribers,
        })
    }
    pub fn len(&self) -> usize {
        self.subscribers.len()
    }
    pub fn member_id(&self, index: usize) -> Option<usize> {
        self.subscribers.get(index).map(|value| value.member)
    }
}

fn transformations(
    generation: &CompiledGeneration,
    member: usize,
) -> Result<&[TransformationKind], JsonExecutionError> {
    super::super::json_plan::transformations(generation, member).map_err(|_| JsonExecutionError::State)
}
fn reserve<T>(
    values: &mut Vec<T>,
    count: usize,
    bytes: usize,
    kind: JsonExecutionLimit,
    total_bytes: usize,
) -> Result<(), JsonExecutionError> {
    if values.len() >= count {
        return Err(JsonExecutionError::Limit { kind, limit: count });
    }
    if values.len() == values.capacity() {
        let required = values.len().checked_add(1).ok_or(JsonExecutionError::Arithmetic)?;
        if required
            .checked_mul(size_of::<T>())
            .ok_or(JsonExecutionError::Arithmetic)?
            > bytes
        {
            return Err(JsonExecutionError::Limit {
                kind: JsonExecutionLimit::RetainedBytes,
                limit: total_bytes,
            });
        }
        let target = required
            .max(values.capacity().saturating_mul(2))
            .min(count)
            .min(bytes / size_of::<T>().max(1));
        values
            .try_reserve_exact(target - values.len())
            .map_err(|_| JsonExecutionError::Allocation)?;
        if values
            .capacity()
            .checked_mul(size_of::<T>())
            .ok_or(JsonExecutionError::Arithmetic)?
            > bytes
        {
            return Err(JsonExecutionError::Limit {
                kind: JsonExecutionLimit::RetainedBytes,
                limit: total_bytes,
            });
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct JsonFactRange {
    start: usize,
    end: usize,
}
#[derive(Clone, Copy)]
struct Fact {
    subscriber: usize,
    transformed_bytes: usize,
    outcome: FactOutcome,
}

#[derive(Clone, Copy)]
enum FactOutcome {
    Positive,
    Negative,
    OutputLimit,
    WorkLimit,
}

impl Fact {
    fn transformed(&self) -> Result<usize, RuntimeError> {
        match self.outcome {
            FactOutcome::Positive | FactOutcome::Negative => Ok(self.transformed_bytes),
            FactOutcome::OutputLimit => Err(RuntimeError::Transformation {
                kind: RuntimeTransformError::OutputLimit,
            }),
            FactOutcome::WorkLimit => Err(RuntimeError::Transformation {
                kind: RuntimeTransformError::WorkLimit,
            }),
        }
    }

    // Call only after transformed() validates this fact, or after a complete
    // validation pass over the same immutable ordered facts.
    fn positive(&self) -> bool {
        matches!(self.outcome, FactOutcome::Positive)
    }
}

#[derive(Clone, Copy)]
struct QueryHeader {
    end: usize,
    path_bytes: usize,
    value_bytes: usize,
}

enum ScalarInput<'a> {
    Body(&'a [u8], &'a [u8]),
    Query(&'a [u8], &'a [u8]),
}

struct Facts {
    values: Vec<Fact>,
    query_headers: Vec<QueryHeader>,
    limits: JsonFactLimits,
    work: usize,
}
impl Facts {
    fn retained_bytes(&self) -> usize {
        // Both element types are nonzero-sized. Each Vec allocation is at most
        // isize::MAX bytes, so the two allocated byte counts fit in usize.
        self.values.capacity() * size_of::<Fact>() + self.query_headers.capacity() * size_of::<QueryHeader>()
    }

    fn charge(&mut self, work: usize) -> Result<(), JsonExecutionError> {
        self.work = self.work.checked_add(work).ok_or(JsonExecutionError::Arithmetic)?;
        if self.work > self.limits.max_work {
            return Err(JsonExecutionError::Limit {
                kind: JsonExecutionLimit::Work,
                limit: self.limits.max_work,
            });
        }
        Ok(())
    }
    fn evaluate(
        &mut self,
        plan: SubscriberView<'_>,
        scratch: &mut [MatchScratch],
        runtime: RuntimeLimits,
        mode: EvaluationMode,
        name: &[u8],
        value: &[u8],
    ) -> Result<JsonFactRange, JsonExecutionError> {
        self.evaluate_with_reuse::<true>(plan, scratch, runtime, mode, ScalarInput::Body(name, value))
    }
    fn evaluate_with_reuse<const REUSE: bool>(
        &mut self,
        plan: SubscriberView<'_>,
        scratch: &mut [MatchScratch],
        runtime: RuntimeLimits,
        mode: EvaluationMode,
        scalar: ScalarInput<'_>,
    ) -> Result<JsonFactRange, JsonExecutionError> {
        let (name, value, query) = match scalar {
            ScalarInput::Body(name, value) => (name, value, false),
            ScalarInput::Query(name, value) => (name, value, true),
        };
        let start = self.values.len();
        // Group identity includes canonical targets and transformations, phase,
        // capture/multimatch mode and byte view. Within this one scalar call,
        // equal groups therefore use identical input and transform limits. Only
        // the most recent group is reused; no payload or cache survives the call.
        let mut last_group: Option<(usize, Result<usize, RuntimeError>)> = None;
        for (index, subscriber) in plan.subscribers.iter().enumerate() {
            self.charge(name.len().checked_add(1).ok_or(JsonExecutionError::Arithmetic)?)?;
            if let CompiledTargetSelector::Literal(range) = subscriber.selector {
                if generation_bytes(&plan.generation.target_bytes, range)? != name {
                    continue;
                }
            }
            let input = if subscriber.collection == CollectionKind::ArgsNames {
                name
            } else {
                value
            };
            let transforms = transformations(plan.generation, subscriber.member)?;
            let cost = input
                .len()
                .checked_mul(
                    transforms
                        .len()
                        .checked_mul(2)
                        .and_then(|count| count.checked_add(2))
                        .ok_or(JsonExecutionError::Arithmetic)?,
                )
                .ok_or(JsonExecutionError::Arithmetic)?;
            self.charge(cost)?;
            // Reserve before speculative predicate work. Every provisional fact,
            // including an overwritten subtree, consumes this independent budget.
            if self.values.len() == self.values.capacity() {
                self.charge(
                    self.values
                        .len()
                        .checked_mul(size_of::<Fact>())
                        .ok_or(JsonExecutionError::Arithmetic)?,
                )?;
            }
            if self.values.len() >= self.limits.max_facts {
                return Err(JsonExecutionError::Limit {
                    kind: JsonExecutionLimit::Facts,
                    limit: self.limits.max_facts,
                });
            }
            let byte_error = JsonExecutionError::Limit {
                kind: JsonExecutionLimit::RetainedBytes,
                limit: self.limits.max_retained_bytes,
            };
            if query && self.values.len() == start {
                // Reserve one header only for a selected query. Protect the
                // first fact's minimum space before either arena overallocates:
                // three single-fact queries must fit exactly six records.
                let fact_minimum = self
                    .values
                    .len()
                    .checked_add(1)
                    .ok_or(JsonExecutionError::Arithmetic)?
                    .max(self.values.capacity())
                    .checked_mul(size_of::<Fact>())
                    .ok_or(JsonExecutionError::Arithmetic)?;
                let header_bytes = self
                    .limits
                    .max_retained_bytes
                    .checked_sub(fact_minimum)
                    .ok_or(byte_error)?;
                if self.query_headers.len() == self.query_headers.capacity() {
                    self.charge(
                        self.query_headers
                            .len()
                            .checked_mul(size_of::<QueryHeader>())
                            .ok_or(JsonExecutionError::Arithmetic)?,
                    )?;
                }
                reserve(
                    &mut self.query_headers,
                    self.limits.max_facts,
                    header_bytes,
                    JsonExecutionLimit::Facts,
                    self.limits.max_retained_bytes,
                )?;
            }
            let header_bytes = self
                .query_headers
                .capacity()
                .checked_mul(size_of::<QueryHeader>())
                .ok_or(JsonExecutionError::Arithmetic)?;
            let fact_bytes = self
                .limits
                .max_retained_bytes
                .checked_sub(header_bytes)
                .ok_or(byte_error)?;
            reserve(
                &mut self.values,
                self.limits.max_facts,
                fact_bytes,
                JsonExecutionLimit::Facts,
                self.limits.max_retained_bytes,
            )?;
            let operator = plan.generation.operator_plans[subscriber.member].ok_or(JsonExecutionError::State)?;
            let CompiledOperatorOperand::Matcher {
                group,
                pattern_start,
                pattern_count,
            } = operator.operand
            else {
                return Err(JsonExecutionError::State);
            };
            let cached = last_group.filter(|(previous, _)| REUSE && *previous == group);
            let mut transformed_bytes = None;
            let transformed = match cached {
                Some((_, result)) => result,
                None => apply_transformations(
                    input,
                    transforms,
                    TransformLimits {
                        max_output_bytes: runtime.max_transformation_output_bytes,
                        max_work_bytes: runtime.max_transformation_work_bytes,
                    },
                )
                .map(|bytes| {
                    let len = bytes.len();
                    transformed_bytes = Some(bytes);
                    len
                })
                .map_err(map_transform_error),
            };
            let mut positive = false;
            let transformed = match transformed {
                Ok(len) => {
                    let patterns = plan
                        .generation
                        .groups
                        .get(group)
                        .ok_or(JsonExecutionError::State)?
                        .matcher
                        .pattern_count();
                    // Charge each subscriber exactly as before, even when the
                    // actual transform and scan work can be reused.
                    self.charge(
                        len.checked_add(1)
                            .and_then(|bytes| bytes.checked_mul(patterns.checked_add(1)?))
                            .ok_or(JsonExecutionError::Arithmetic)?,
                    )?;
                    positive = if let Some(bytes) = transformed_bytes {
                        evaluate_matcher_operand(
                            plan.generation,
                            scratch,
                            group,
                            pattern_start,
                            pattern_count,
                            &bytes,
                            mode,
                        )?
                    } else {
                        let end = pattern_start
                            .checked_add(pattern_count)
                            .ok_or(RuntimeError::Arithmetic)?;
                        scratch
                            .get(group)
                            .ok_or(RuntimeError::GenerationInvariant)?
                            .contains_any_in_range(patterns, pattern_start..end)
                            .ok_or(RuntimeError::GenerationInvariant)?
                    };
                    Ok(len)
                }
                Err(
                    error @ RuntimeError::Transformation {
                        kind: RuntimeTransformError::OutputLimit | RuntimeTransformError::WorkLimit,
                    },
                ) => Err(error),
                Err(error) => return Err(error.into()),
            };
            last_group = Some((group, transformed));
            let _name_bytes = subscriber
                .collection
                .as_str()
                .len()
                .checked_add(1)
                .and_then(|v| v.checked_add(name.len()))
                .ok_or(JsonExecutionError::Arithmetic)?;
            // Only transformation limits may be deferred. Keep those two tags
            // rather than every unrelated RuntimeError variant in each record.
            // Both lengths remain explicit; no length-preserving-transform
            // assumption is needed by replay or a future eligible transform.
            let (transformed_bytes, outcome) = match transformed {
                Ok(bytes) => (
                    bytes,
                    if positive {
                        FactOutcome::Positive
                    } else {
                        FactOutcome::Negative
                    },
                ),
                Err(RuntimeError::Transformation {
                    kind: RuntimeTransformError::OutputLimit,
                }) => (0, FactOutcome::OutputLimit),
                Err(RuntimeError::Transformation {
                    kind: RuntimeTransformError::WorkLimit,
                }) => (0, FactOutcome::WorkLimit),
                Err(error) => return Err(error.into()),
            };
            self.values.push(Fact {
                subscriber: index,
                transformed_bytes,
                outcome,
            });
        }
        let end = self.values.len();
        if query && end != start {
            self.query_headers.push(QueryHeader {
                end,
                path_bytes: name.len(),
                value_bytes: value.len(),
            });
        }
        Ok(JsonFactRange { start, end })
    }
}

/// Owns the adapter and its private fact ranges, preventing cross-session ranges.
/// The caller supplies canonical query name/value pairs before body input and
/// the matching head ARGS combined size in the adapter configuration. Scratch
/// must belong to the same generation as the plan. The body limits and head
/// summary must agree with the canonical request/runtime configuration. Scratch
/// belongs to the transaction. Actual arena budgets are separate from replayed
/// logical runtime costs. Runtime predicate limits are deferred; allocation and
/// inconsistent matcher state are immediate independent failures.
pub(super) struct JsonSubscriberSession<'a, 'g, 'scratch> {
    plan: &'a JsonSubscriberPlan<'g>,
    scratch: &'scratch mut [MatchScratch],
    core: JsonCollectionCore,
}
impl<'a, 'g, 'scratch> JsonSubscriberSession<'a, 'g, 'scratch> {
    pub fn new(
        plan: &'a JsonSubscriberPlan<'g>,
        scratch: &'scratch mut [MatchScratch],
        runtime: RuntimeLimits,
        mode: EvaluationMode,
        config: JsonBodyConfig,
        limits: JsonFactLimits,
    ) -> Self {
        Self {
            plan,
            scratch,
            core: JsonCollectionCore::new(runtime, mode, config, limits),
        }
    }
    pub fn usage(&self) -> JsonFactUsage {
        self.core.usage()
    }
    pub fn summary(&self) -> Option<JsonBodySummary> {
        self.core.summary()
    }
    pub fn add_query(&mut self, name: &[u8], value: &[u8]) -> Result<(), JsonExecutionError> {
        self.core.add_query(self.plan.view(), self.scratch, name, value)
    }
    pub fn feed(&mut self, input: JsonStreamInput<'_>) -> Result<(), JsonExecutionError> {
        self.core.feed(self.plan.view(), self.scratch, input)
    }
    pub fn replay_rule(&self, subscriber: usize, base: usize) -> Result<JsonRuleOutcome, JsonExecutionError> {
        self.core.replay_rule(self.plan.view(), subscriber, base)
    }
}

/// Persistent state has no generation or matcher-scratch borrow. Its owner
/// supplies those references only for the duration of one synchronous callback.
pub(super) struct JsonOwnedCollector {
    core: JsonCollectionCore,
}
impl JsonOwnedCollector {
    pub fn new(
        _plan: JsonSubscriberPlan<'_>,
        runtime: RuntimeLimits,
        mode: EvaluationMode,
        config: JsonBodyConfig,
        limits: JsonFactLimits,
    ) -> Self {
        Self {
            core: JsonCollectionCore::new(runtime, mode, config, limits),
        }
    }
    pub fn add_query(
        &mut self,
        generation: &CompiledGeneration,
        scratch: &mut [MatchScratch],
        name: &[u8],
        value: &[u8],
    ) -> Result<(), JsonExecutionError> {
        self.core.add_query(
            SubscriberView {
                generation,
                subscribers: generation
                    .json_capability
                    .subscribers()
                    .ok_or(JsonExecutionError::State)?,
            },
            scratch,
            name,
            value,
        )
    }
    pub fn feed(
        &mut self,
        generation: &CompiledGeneration,
        scratch: &mut [MatchScratch],
        input: JsonStreamInput<'_>,
    ) -> Result<(), JsonExecutionError> {
        self.core.feed(
            SubscriberView {
                generation,
                subscribers: generation
                    .json_capability
                    .subscribers()
                    .ok_or(JsonExecutionError::State)?,
            },
            scratch,
            input,
        )
    }
    pub fn into_completed(self) -> Result<JsonCompletedFacts, JsonExecutionError> {
        let detached = self.core.into_completed()?;
        Ok(JsonCompletedFacts {
            runtime: detached.runtime,
            adapter: detached.adapter,
            facts: detached.facts,
        })
    }
}

struct JsonCollectionCore {
    runtime: RuntimeLimits,
    mode: EvaluationMode,
    adapter: Option<JsonBodyAdapter<JsonFactRange>>,
    facts: Facts,
    started: bool,
    complete: bool,
    error: Option<JsonExecutionError>,
}
impl JsonCollectionCore {
    pub fn new(runtime: RuntimeLimits, mode: EvaluationMode, config: JsonBodyConfig, limits: JsonFactLimits) -> Self {
        Self {
            runtime,
            mode,
            adapter: Some(JsonBodyAdapter::new(config)),
            facts: Facts {
                values: Vec::new(),
                query_headers: Vec::new(),
                limits,
                work: 0,
            },
            started: false,
            complete: false,
            error: None,
        }
    }
    fn fail(&mut self, error: JsonExecutionError) -> JsonExecutionError {
        self.error = Some(error);
        self.adapter = None;
        self.facts.values = Vec::new();
        self.facts.query_headers = Vec::new();
        error
    }
    pub fn usage(&self) -> JsonFactUsage {
        JsonFactUsage {
            facts: self.facts.values.len(),
            retained_bytes: self.facts.retained_bytes(),
            work: self.facts.work,
        }
    }
    pub fn summary(&self) -> Option<JsonBodySummary> {
        self.adapter.as_ref()?.summary()
    }
    pub fn add_query(
        &mut self,
        plan: SubscriberView<'_>,
        scratch: &mut [MatchScratch],
        name: &[u8],
        value: &[u8],
    ) -> Result<(), JsonExecutionError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.complete {
            return Err(JsonExecutionError::AlreadyComplete);
        }
        if self.started {
            return Err(self.fail(JsonExecutionError::State));
        }
        match self.facts.evaluate_with_reuse::<true>(
            plan,
            scratch,
            self.runtime,
            self.mode,
            ScalarInput::Query(name, value),
        ) {
            Ok(_) => Ok(()),
            Err(error) => Err(self.fail(error)),
        }
    }
    pub fn feed(
        &mut self,
        plan: SubscriberView<'_>,
        scratch: &mut [MatchScratch],
        input: JsonStreamInput<'_>,
    ) -> Result<(), JsonExecutionError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.complete {
            return Err(JsonExecutionError::AlreadyComplete);
        }
        self.started = true;
        let mut fact_error = None;
        let result = self
            .adapter
            .as_mut()
            .ok_or(JsonExecutionError::State)?
            .feed(input, |name, value| {
                self.facts
                    .evaluate(plan, scratch, self.runtime, self.mode, name.as_bytes(), value)
                    .map_err(|error| {
                        fact_error = Some(error);
                        JsonDocumentSinkError
                    })
            });
        if let Err(error) = result {
            return Err(self.fail(fact_error.unwrap_or(JsonExecutionError::Body(error))));
        }
        if matches!(input, JsonStreamInput::Complete) {
            self.complete = true;
        }
        Ok(())
    }
    pub fn replay_rule(
        &self,
        plan: SubscriberView<'_>,
        subscriber: usize,
        nonmatch_tx_bytes: usize,
    ) -> Result<JsonRuleOutcome, JsonExecutionError> {
        self.view(plan).replay_rule(subscriber, nonmatch_tx_bytes)
    }
    fn view<'s>(&'s self, plan: SubscriberView<'s>) -> ReplayView<'s> {
        ReplayView {
            generation: plan.generation,
            subscribers: plan.subscribers,
            runtime: self.runtime,
            adapter: self.adapter.as_ref(),
            facts: &self.facts,
            complete: self.complete,
            error: self.error,
        }
    }
    pub fn into_completed(self) -> Result<JsonDetachedFacts, JsonExecutionError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if !self.complete {
            return Err(JsonExecutionError::State);
        }
        Ok(JsonDetachedFacts {
            runtime: self.runtime,
            adapter: self.adapter.ok_or(JsonExecutionError::State)?,
            facts: self.facts,
        })
    }
}

/// Owned state after dropping the mutable matcher scratch borrow.
struct JsonDetachedFacts {
    runtime: RuntimeLimits,
    adapter: JsonBodyAdapter<JsonFactRange>,
    facts: Facts,
}
/// Attached only by the transaction that collected the query and body facts.
/// Contains no generation or scratch borrow, so failed scheduled phases can retry.
pub(super) struct JsonCompletedFacts {
    runtime: RuntimeLimits,
    adapter: JsonBodyAdapter<JsonFactRange>,
    facts: Facts,
}
impl JsonCompletedFacts {
    pub fn summary(&self) -> Result<JsonBodySummary, RuntimeError> {
        self.adapter.summary().ok_or(RuntimeError::GenerationInvariant)
    }
    pub fn replay_subscriber(
        &self,
        generation: &CompiledGeneration,
        subscriber: usize,
        base: usize,
    ) -> Result<JsonRuleOutcome, RuntimeError> {
        let subscribers = generation
            .json_capability
            .subscribers()
            .ok_or(RuntimeError::GenerationInvariant)?;
        ReplayView {
            generation,
            subscribers,
            runtime: self.runtime,
            adapter: Some(&self.adapter),
            facts: &self.facts,
            complete: true,
            error: None,
        }
        .replay_rule(subscriber, base)
        .map_err(|error| match error {
            JsonExecutionError::Runtime(error) => error,
            _ => RuntimeError::GenerationInvariant,
        })
    }
}

/// Explicit, independent structured-execution budgets; no production defaults
/// are selected. Canonical body and rule limits come from the transaction.
/// These caps account for distinct owners and do not replace a transport body cap.
#[derive(Debug, Clone, Copy)]
pub struct JsonExecutionLimits {
    pub lexical: crate::seclang::JsonStreamLimits,
    pub document: crate::seclang::JsonDocumentLimits,
    pub plan: JsonPlanLimits,
    pub facts: JsonFactLimits,
}

struct ReplayView<'s> {
    generation: &'s CompiledGeneration,
    subscribers: &'s [Subscriber],
    runtime: RuntimeLimits,
    adapter: Option<&'s JsonBodyAdapter<JsonFactRange>>,
    facts: &'s Facts,
    complete: bool,
    error: Option<JsonExecutionError>,
}
struct ReplayFact<'a> {
    fact: &'a Fact,
    collection: CollectionKind,
    path_bytes: usize,
    value_bytes: usize,
}
impl ReplayFact<'_> {
    fn input_bytes(&self) -> usize {
        if self.collection == CollectionKind::ArgsNames {
            self.path_bytes
        } else {
            self.value_bytes
        }
    }
    fn name_bytes(&self) -> Result<usize, RuntimeError> {
        // The identical checked operation ran before this fact was appended.
        self.collection
            .as_str()
            .len()
            .checked_add(1)
            .and_then(|prefix| prefix.checked_add(self.path_bytes))
            .ok_or(RuntimeError::Arithmetic)
    }
}

impl ReplayView<'_> {
    fn ordered(&self, subscriber: usize) -> Result<impl Iterator<Item = ReplayFact<'_>>, JsonExecutionError> {
        let adapter = self.adapter.as_ref().ok_or(JsonExecutionError::State)?;
        let collection = self
            .subscribers
            .get(subscriber)
            .ok_or(JsonExecutionError::State)?
            .collection;
        let mut query_start = 0;
        let query_ranges = self.facts.query_headers.iter().map(move |header| {
            let start = query_start;
            query_start = header.end;
            (
                JsonFactRange { start, end: header.end },
                header.path_bytes,
                header.value_bytes,
            )
        });
        let body_ranges = adapter.events().into_iter().flatten().filter_map(|event| match event {
            JsonDocumentEvent::Scalar {
                result,
                path_bytes,
                value_bytes,
            } => Some((result, path_bytes, value_bytes)),
            _ => None,
        });
        // Scalar event lengths equal the sink's original path/value lengths.
        // Query headers preserve the same metadata for ephemeral query inputs.
        // Subscriber IDs remain sorted within each private contiguous range.
        Ok(query_ranges
            .chain(body_ranges)
            .filter_map(move |(range, path_bytes, value_bytes)| {
                let facts = &self.facts.values[range.start..range.end];
                facts
                    .binary_search_by_key(&subscriber, |fact| fact.subscriber)
                    .ok()
                    .map(|index| ReplayFact {
                        fact: &facts[index],
                        collection,
                        path_bytes,
                        value_bytes,
                    })
            }))
    }
    /// Independent evaluation of one reached rule; no scheduler or TX commit.
    /// `nonmatch_tx_bytes` is the retained size after clearing inherited matches,
    /// including ordinary TX entries, captures and controls. A zero-match outcome
    /// leaves the caller's original committed state untouched.
    /// This does not check whole-request retention or collection entries, scheduled
    /// unit counts, findings or interventions. The future transaction integration
    /// must enforce those checks and commit rules independently.
    /// A deferred rule error leaves facts available for differential inspection.
    /// Three passes each take O(events + scalar_ranges * log(subscribers + 1));
    /// they do not scan every subscriber fact for each rule. Event/fact and plan
    /// subscriber caps bound traversal, independently of speculative `max_work`.
    /// Replayed operator work retains the ordinary runtime accounting model.
    pub fn replay_rule(
        &self,
        subscriber: usize,
        nonmatch_tx_bytes: usize,
    ) -> Result<JsonRuleOutcome, JsonExecutionError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if !self.complete {
            return Err(JsonExecutionError::State);
        }
        let member_id = self
            .subscribers
            .get(subscriber)
            .map(|subscriber| subscriber.member)
            .ok_or(JsonExecutionError::State)?;
        let mut selected_count = 0usize;
        for _ in self.ordered(subscriber)? {
            if selected_count == self.runtime.max_selected_values {
                return Err(RuntimeError::SelectedValueLimit {
                    limit: self.runtime.max_selected_values,
                }
                .into());
            }
            selected_count += 1;
        }
        let operator = self.generation.operator_plans[member_id].ok_or(JsonExecutionError::State)?;
        let mut work = 0usize;
        let mut match_count = 0usize;
        for fact in self.ordered(subscriber)? {
            let transformed = fact.fact.transformed()?;
            charge_operator_work(
                &mut work,
                fact.input_bytes()
                    .checked_add(transformed)
                    .ok_or(RuntimeError::Arithmetic)?,
                self.runtime,
            )?;
            if fact.fact.positive() != operator.negated {
                if match_count == self.runtime.max_operator_evaluations {
                    return Err(RuntimeError::OperatorEvaluationLimit {
                        limit: self.runtime.max_operator_evaluations,
                    }
                    .into());
                }
                let bytes = fact
                    .name_bytes()?
                    .checked_add(transformed)
                    .and_then(|v| v.checked_add(size_of::<MemberMatch>()))
                    .ok_or(RuntimeError::Arithmetic)?;
                charge_operator_work(&mut work, bytes, self.runtime)?;
                match_count += 1;
            }
        }
        let mut matches = 0usize;
        let mut lengths = 0usize;
        let mut final_tx_bytes = nonmatch_tx_bytes;
        // The preceding pass validated every transformation in this identical,
        // immutable ordered set. Polarity needs no repeated error reconstruction.
        for fact in self.ordered(subscriber)? {
            if fact.fact.positive() == operator.negated {
                continue;
            }
            if matches == self.runtime.max_tx_variables {
                return Err(RuntimeError::TxVariableLimit {
                    limit: self.runtime.max_tx_variables,
                }
                .into());
            }
            let len = fact
                .name_bytes()?
                .checked_add(fact.fact.transformed()?)
                .ok_or(RuntimeError::Arithmetic)?;
            lengths = lengths.checked_add(len).ok_or(RuntimeError::Arithmetic)?;
            matches += 1;
            final_tx_bytes = matches
                .checked_mul(size_of::<TxEntry>())
                .and_then(|v| v.checked_add(lengths))
                .and_then(|v| v.checked_add(len))
                .and_then(|v| v.checked_add(nonmatch_tx_bytes))
                .ok_or(RuntimeError::Arithmetic)?;
            if final_tx_bytes > self.runtime.max_tx_bytes {
                return Err(RuntimeError::TxByteLimit {
                    limit: self.runtime.max_tx_bytes,
                }
                .into());
            }
        }
        if match_count != 0 {
            validate_static_metadata(self.generation, member_id)?;
        }
        Ok(JsonRuleOutcome {
            selected_count,
            match_count,
            final_tx_bytes,
        })
    }
}
fn validate_static_metadata(generation: &CompiledGeneration, id: usize) -> Result<(), RuntimeError> {
    let member = generation.members.get(id).ok_or(RuntimeError::GenerationInvariant)?;
    for action in &generation.actions[member.action_start..member.action_start + member.action_count] {
        if !matches!(
            action.kind,
            ActionEnvelopeKind::Action(ActionKind::Msg | ActionKind::LogData)
        ) {
            continue;
        }
        let CompiledActionOperand::Template(id) = action.operand else {
            return Err(RuntimeError::GenerationInvariant);
        };
        let template = generation.templates.get(id).ok_or(RuntimeError::GenerationInvariant)?;
        let mut length = 0usize;
        for segment in &generation.segments[template.segment_start..template.segment_start + template.segment_count] {
            let CompiledSegmentKind::Literal(range) = segment.kind else {
                return Err(RuntimeError::GenerationInvariant);
            };
            length = checked_runtime_append(
                length,
                range.len,
                template.runtime_byte_limit.min(member.runtime_expansion_byte_limit),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_rules_test.rs"]
mod tests;
