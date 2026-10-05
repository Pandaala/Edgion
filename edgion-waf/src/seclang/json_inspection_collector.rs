//! Fused provisional inspection. No scalar event tape or default value storage.
#[cfg(test)]
use crate::seclang::measurements::{self, Event};

use super::super::json_inspection_plan::JsonView;
use super::json_inspection::{
    CandidateId, CheckpointError, InspectionBudget, Receipt, ResultLimits, ResultStore, SealedResults, StoreError,
};
use super::*;
use crate::seclang::transform::{
    walk_transformations, TransformCheckpoint, TransformObserver, TransformScratch, TransformWalkError,
};
use crate::seclang::{JsonCheckerSink, JsonCheckerSinkError, JsonScalarKind, JsonSemanticSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CollectorError {
    Store(StoreError),
    Runtime(RuntimeError),
    State,
}
impl From<StoreError> for CollectorError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
impl From<RuntimeError> for CollectorError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}
type Collected<T> = Result<T, CollectorError>;

/// Persisted checkpoint encoding shared with canonical replay. `step` remains a
/// transformation index for validation errors, an operator ordinal otherwise.
#[derive(Clone, Copy)]
#[repr(usize)]
pub(super) enum StoredCheckpoint {
    Initial,
    StepInput,
    StepOutput,
    StepComplete,
    DetectorInput,
    Operator,
    Captures,
}
impl StoredCheckpoint {
    fn predicate(value: PredicateCheckpoint) -> Self {
        match value {
            PredicateCheckpoint::DetectorInput => Self::DetectorInput,
            PredicateCheckpoint::Operator => Self::Operator,
            PredicateCheckpoint::Captures => Self::Captures,
        }
    }
}
#[derive(Clone, Copy)]
struct GroupRun {
    id: usize,
    terminal: bool,
}
#[derive(Default)]
struct DispatchScratch {
    groups: Vec<GroupRun>,
    profiles: Vec<usize>,
    stamps: Vec<usize>,
    epoch: usize,
    cache: StageScanCache,
}
impl DispatchScratch {
    fn bytes(&self) -> Collected<usize> {
        self.groups
            .capacity()
            .checked_mul(size_of::<GroupRun>())
            .and_then(|v| v.checked_add(self.profiles.capacity().checked_mul(size_of::<usize>())?))
            .and_then(|v| v.checked_add(self.stamps.capacity().checked_mul(size_of::<usize>())?))
            .and_then(|v| v.checked_add(self.cache.scanned.capacity().checked_mul(size_of::<usize>())?))
            .ok_or(CollectorError::Store(StoreError::Arithmetic))
    }
    fn add(&mut self, id: usize, store: &mut ResultStore<'_>) -> Collected<()> {
        store.charge_work(1)?;
        let stamp = self.stamps.get_mut(id).ok_or(CollectorError::State)?;
        if *stamp != self.epoch {
            *stamp = self.epoch;
            if self.groups.len() == self.groups.capacity() {
                return Err(CollectorError::State);
            }
            self.groups.push(GroupRun { id, terminal: false });
            #[cfg(test)]
            measurements::record(Event::DispatchGroups, 1);
        }
        Ok(())
    }
}

pub(super) struct JsonCollector<'call, 'generation> {
    store: ResultStore<'generation>,
    context: SingleInputContext<'call>,
    runtime: RuntimeLimits,
    mode: EvaluationMode,
    transform: TransformScratch,
    dispatch: DispatchScratch,
    failure: Option<CollectorError>,
}
impl<'call, 'generation> JsonCollector<'call, 'generation> {
    pub(super) fn new(
        generation: GenerationHandle<'generation>,
        context: SingleInputContext<'call>,
        runtime: RuntimeLimits,
        mode: EvaluationMode,
        limits: ResultLimits,
    ) -> Collected<Self> {
        if !std::ptr::eq(&*generation, context.generation) || !generation.json_inspection.unsupported.is_empty() {
            return Err(CollectorError::State);
        }
        let group_count = generation.json_inspection.groups.len();
        let profile_count = generation.json_inspection.profiles.len();
        let mut result = Self {
            store: ResultStore::new(generation, runtime, mode, limits)?,
            context,
            runtime,
            mode,
            transform: TransformScratch::default(),
            dispatch: DispatchScratch::default(),
            failure: None,
        };
        // Each allocation admits its old/new overlap and reports allocator capacity.
        let mut total = 0;
        reserve_scratch(&mut result.dispatch.groups, group_count, &mut total, &mut result.store)?;
        reserve_scratch(
            &mut result.dispatch.profiles,
            profile_count,
            &mut total,
            &mut result.store,
        )?;
        reserve_scratch(&mut result.dispatch.stamps, group_count, &mut total, &mut result.store)?;
        reserve_scratch(
            &mut result.dispatch.cache.scanned,
            group_count,
            &mut total,
            &mut result.store,
        )?;
        result.store.charge_work(group_count)?;
        result.dispatch.stamps.resize(group_count, 0);
        Ok(result)
    }
    pub(super) fn failure(&self) -> Option<CollectorError> {
        self.failure
    }
    pub(super) fn into_abandoned_budget(self) -> Collected<InspectionBudget> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let Self {
            mut store,
            transform,
            dispatch,
            ..
        } = self;
        drop(transform);
        drop(dispatch);
        store.release_scratch();
        store.into_abandoned_budget().map_err(Into::into)
    }
    pub(super) fn into_sealed(self) -> Collected<(SealedResults<'generation>, InspectionBudget)> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let Self {
            store,
            transform,
            dispatch,
            ..
        } = self;
        drop(transform);
        drop(dispatch);
        let (sealed, mut budget) = store.into_sealed()?;
        budget.release_scratch();
        Ok((sealed, budget))
    }
    fn sink_result<T>(&mut self, result: Collected<T>) -> Result<T, JsonCheckerSinkError> {
        result.map_err(|error| {
            self.failure.get_or_insert(error);
            JsonCheckerSinkError
        })
    }
    fn collect_scalar(&mut self, name: &[u8], value: &[u8], kind: JsonScalarKind) -> Collected<Option<Receipt>> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        #[cfg(test)]
        let dispatch_work_before = self.store.usage().work;
        let generation = self.context.generation;
        let plan = &generation.json_inspection;
        let deferred = exact_name_match(generation, &plan.named_demands, name, &mut self.store)?;
        let mut retain = deferred || plan.needs_all_candidate_names;
        if !retain {
            retain = exact_name_match(generation, &plan.candidate_literals, name, &mut self.store)?;
        }
        if !retain {
            for &selector in &plan.candidate_regexes {
                if match_name(
                    generation,
                    self.context.boolean_scratch,
                    selector,
                    name,
                    &mut self.store,
                )? {
                    retain = true;
                    break;
                }
            }
        }
        if !retain {
            #[cfg(test)]
            measurements::record(Event::DispatchWork, self.store.usage().work - dispatch_work_before);
            return Ok(None);
        }
        self.dispatch.epoch = self.dispatch.epoch.checked_add(1).ok_or(StoreError::Arithmetic)?;
        self.store.charge_work(
            self.dispatch
                .groups
                .len()
                .checked_add(self.dispatch.profiles.len())
                .ok_or(StoreError::Arithmetic)?,
        )?;
        self.dispatch.groups.clear();
        self.dispatch.profiles.clear();
        let broad_only = plan.literals.is_empty() && plan.regexes.is_empty();
        if broad_only {
            for &id in &plan.broad_only_order {
                self.dispatch.add(id, &mut self.store)?;
            }
        } else {
            for &id in plan.broad_names.iter().chain(&plan.broad_values) {
                self.dispatch.add(id, &mut self.store)?;
            }
            self.store.charge_work(
                plan.literal_lookup_work(name.len())
                    .map_err(|_| CollectorError::State)?,
            )?;
            for row in plan
                .literal_matches(generation, name)
                .map_err(|_| CollectorError::State)?
            {
                self.dispatch.add(row.group, &mut self.store)?;
            }
            for row in &plan.regexes {
                if match_name(
                    generation,
                    self.context.boolean_scratch,
                    row.selector,
                    name,
                    &mut self.store,
                )? {
                    self.dispatch.add(row.group, &mut self.store)?;
                }
            }
            let n = self.dispatch.groups.len();
            let sort_work = n
                .checked_mul((usize::BITS - n.leading_zeros()) as usize)
                .and_then(|v| v.checked_mul(3))
                .ok_or(StoreError::Arithmetic)?;
            self.store.charge_work(sort_work)?;
            self.dispatch
                .groups
                .sort_unstable_by_key(|entry| plan.groups[entry.id].profile);
        }
        let n = self.dispatch.groups.len();
        #[cfg(test)]
        measurements::record(Event::DispatchWork, self.store.usage().work - dispatch_work_before);
        let candidate = self
            .store
            .begin_candidate(name, value.len(), kind, deferred.then_some(value))?;
        let mut start = 0;
        while start < n {
            let profile_id = plan.groups[self.dispatch.groups[start].id].profile;
            let mut end = start + 1;
            while end < n && plan.groups[self.dispatch.groups[end].id].profile == profile_id {
                end += 1;
            }
            self.store.charge_work(end - start)?;
            let profile = plan.profiles.get(profile_id).ok_or(CollectorError::State)?;
            let transformations = transformations(generation, profile.transformation_plan)?;
            let source = match profile.view {
                JsonView::Name => name,
                JsonView::Value => value,
            };
            let fixed_scratch = self.dispatch.bytes()?;
            let mut observer = ProfileObserver {
                store: &mut self.store,
                context: &mut self.context,
                groups: &mut self.dispatch.groups[start..end],
                cache: &mut self.dispatch.cache,
                candidate,
                profile: profile_id,
                runtime: self.runtime,
                mode: self.mode,
                fixed_scratch,
                transform_bytes: self.transform.retained_bytes().map_err(map_transform_error)?,
                last: StoredCheckpoint::Initial,
                step: 0,
            };
            #[cfg(test)]
            measurements::record(Event::ProfileWalks, 1);
            let walked = walk_transformations(
                source,
                transformations,
                profile.multi_match,
                TransformLimits {
                    max_output_bytes: self.runtime.max_transformation_output_bytes,
                    max_work_bytes: self.runtime.max_transformation_work_bytes,
                },
                &mut self.transform,
                &mut observer,
            );
            match walked {
                Ok(_) => observer.store.complete_profile(candidate, profile_id)?,
                Err(TransformWalkError::Observer(error)) => return Err(error),
                Err(TransformWalkError::Transformation(
                    error @ (TransformError::Allocation | TransformError::Arithmetic),
                )) => return Err(CollectorError::Runtime(map_transform_error(error))),
                Err(TransformWalkError::Transformation(error)) => observer.store.record_transform_error(
                    candidate,
                    profile_id,
                    CheckpointError {
                        step: observer.step,
                        checkpoint: observer.last as usize,
                        error: map_transform_error(error),
                    },
                )?,
            }
            self.dispatch.profiles.push(profile_id);
            start = end;
        }
        let proof = self.store.completion_proof(candidate, &self.dispatch.profiles)?;
        self.store.complete_candidate(proof).map(Some).map_err(Into::into)
    }
}
impl JsonCheckerSink for JsonCollector<'_, '_> {
    type Receipt = Receipt;
    fn scalar(
        &mut self,
        name: &[u8],
        value: &[u8],
        kind: JsonScalarKind,
    ) -> Result<Option<Receipt>, JsonCheckerSinkError> {
        let result = self.collect_scalar(name, value, kind);
        self.sink_result(result)
    }
    fn concat(
        &mut self,
        left: Option<Receipt>,
        right: Option<Receipt>,
    ) -> Result<Option<Receipt>, JsonCheckerSinkError> {
        let result = self.store.concat(left, right).map_err(Into::into);
        self.sink_result(result)
    }
    fn discard(&mut self, receipt: Receipt) -> Result<(), JsonCheckerSinkError> {
        let result = self.store.discard(receipt).map_err(Into::into);
        self.sink_result(result)
    }
    fn seal(&mut self, receipt: Option<Receipt>, summary: JsonSemanticSummary) -> Result<(), JsonCheckerSinkError> {
        let result = self.store.seal(receipt, summary).map_err(Into::into);
        self.sink_result(result)
    }
    fn abandon(&mut self) {
        self.transform = TransformScratch::default();
        self.dispatch = DispatchScratch::default();
        self.store.release_scratch();
        self.store.abandon();
    }
}

fn reserve_scratch<T>(
    values: &mut Vec<T>,
    needed: usize,
    total: &mut usize,
    store: &mut ResultStore<'_>,
) -> Collected<()> {
    let old = values
        .capacity()
        .checked_mul(size_of::<T>())
        .ok_or(StoreError::Arithmetic)?;
    let requested = needed.checked_mul(size_of::<T>()).ok_or(StoreError::Arithmetic)?;
    store.admit_scratch(total.checked_add(requested).ok_or(StoreError::Arithmetic)?)?;
    store.charge_work(requested.checked_add(old).ok_or(StoreError::Arithmetic)?)?;
    values
        .try_reserve_exact(needed.saturating_sub(values.len()))
        .map_err(|_| StoreError::Allocation)?;
    *total = total
        .checked_sub(old)
        .and_then(|v| v.checked_add(values.capacity().checked_mul(size_of::<T>())?))
        .ok_or(StoreError::Arithmetic)?;
    store.scratch_capacity(*total)?;
    Ok(())
}
fn exact_name_match(
    generation: &CompiledGeneration,
    refs: &[super::super::json_inspection_plan::JsonNameRef],
    name: &[u8],
    store: &mut ResultStore<'_>,
) -> Collected<bool> {
    let (mut low, mut high) = (0, refs.len());
    while low < high {
        store.charge_work(name.len().checked_add(1).ok_or(StoreError::Arithmetic)?)?;
        let mid = low + (high - low) / 2;
        let key = refs[mid].bytes(generation).map_err(|_| CollectorError::State)?;
        match key.cmp(name) {
            std::cmp::Ordering::Equal => return Ok(true),
            std::cmp::Ordering::Less => low = mid + 1,
            std::cmp::Ordering::Greater => high = mid,
        }
    }
    Ok(false)
}
fn match_name(
    generation: &CompiledGeneration,
    scratch: &mut [RegexBooleanScratch],
    selector: super::super::CompiledTargetSelector,
    name: &[u8],
    store: &mut ResultStore<'_>,
) -> Collected<bool> {
    store.charge_work(name.len().checked_add(1).ok_or(StoreError::Arithmetic)?)?;
    match selector {
        super::super::CompiledTargetSelector::Regex { program, .. } => {
            let regex = generation.boolean_regexes.get(program).ok_or(CollectorError::State)?;
            let scratch = scratch.get_mut(program).ok_or(CollectorError::State)?;
            regex
                .is_match(name, scratch)
                .map_err(map_regex_scratch_error)
                .map_err(Into::into)
        }
        _ => Err(CollectorError::State),
    }
}
fn transformations(
    generation: &CompiledGeneration,
    id: Option<usize>,
) -> Collected<&[crate::seclang::TransformationKind]> {
    let Some(id) = id else {
        return Ok(&[]);
    };
    let plan = generation.transformation_plans.get(id).ok_or(CollectorError::State)?;
    let end = plan
        .transformation_start
        .checked_add(plan.transformation_count)
        .ok_or(StoreError::Arithmetic)?;
    generation
        .transformations
        .get(plan.transformation_start..end)
        .ok_or(CollectorError::State)
}

struct ProfileObserver<'a, 'call, 'generation> {
    store: &'a mut ResultStore<'generation>,
    context: &'a mut SingleInputContext<'call>,
    groups: &'a mut [GroupRun],
    cache: &'a mut StageScanCache,
    candidate: CandidateId,
    profile: usize,
    runtime: RuntimeLimits,
    mode: EvaluationMode,
    fixed_scratch: usize,
    transform_bytes: usize,
    last: StoredCheckpoint,
    step: usize,
}
impl TransformObserver for ProfileObserver<'_, '_, '_> {
    type Error = CollectorError;
    fn admit_scratch(&mut self, required: usize) -> Collected<()> {
        self.store
            .admit_scratch(self.fixed_scratch.checked_add(required).ok_or(StoreError::Arithmetic)?)?;
        Ok(())
    }
    fn scratch_capacity(&mut self, actual: usize) -> Collected<()> {
        self.transform_bytes = actual;
        self.store
            .scratch_capacity(self.fixed_scratch.checked_add(actual).ok_or(StoreError::Arithmetic)?)?;
        Ok(())
    }
    fn checkpoint(&mut self, point: TransformCheckpoint) -> Collected<()> {
        match point {
            TransformCheckpoint::Initial { input_bytes } => {
                self.last = StoredCheckpoint::Initial;
                self.store
                    .charge_work(input_bytes.checked_add(1).ok_or(StoreError::Arithmetic)?)?;
                #[cfg(test)]
                measurements::record(Event::TransformWork, input_bytes + 1);
            }
            TransformCheckpoint::StepInput { step, input_bytes, .. } => {
                self.step = step;
                self.last = StoredCheckpoint::StepInput;
                self.store
                    .charge_work(input_bytes.checked_add(1).ok_or(StoreError::Arithmetic)?)?;
                #[cfg(test)]
                measurements::record(Event::TransformWork, input_bytes + 1);
            }
            TransformCheckpoint::StepOutput {
                step,
                output_bytes,
                changed,
                ..
            } => {
                self.step = step;
                self.last = StoredCheckpoint::StepOutput;
                self.store
                    .charge_work(output_bytes.checked_add(1).ok_or(StoreError::Arithmetic)?)?;
                #[cfg(test)]
                measurements::record(Event::TransformWork, output_bytes + 1);
                if changed {
                    self.store
                        .record_changed_step(self.candidate, self.profile, step, output_bytes)?;
                }
            }
            TransformCheckpoint::StepComplete { step, .. } => {
                #[cfg(test)]
                measurements::record(Event::TransformSteps, 1);
                self.step = step;
                self.last = StoredCheckpoint::StepComplete;
            }
        }
        Ok(())
    }
    fn stage(&mut self, ordinal: usize, value: &[u8]) -> Collected<()> {
        self.store.charge_work(self.cache.scanned.len())?;
        self.cache.scanned.clear();
        let generation = self.context.generation;
        let share_stage = self.groups.len() > 1;
        let mut stage_match = None;
        for run in &mut *self.groups {
            self.store.charge_work(1)?;
            if run.terminal {
                continue;
            }
            let group = generation
                .json_inspection
                .groups
                .get(run.id)
                .ok_or(CollectorError::State)?;
            let member = generation.members.get(group.member).ok_or(CollectorError::State)?;
            let operator = generation
                .operator_plans
                .get(group.member)
                .copied()
                .flatten()
                .ok_or(CollectorError::State)?;
            let plan = SingleInputPlan {
                operator,
                transformations: &[],
                multi_match: group.multi_match,
                runtime_expansion_byte_limit: member.runtime_expansion_byte_limit,
                limits: self.runtime,
                evaluation_mode: self.mode,
            };
            let mut context = SingleInputContext {
                generation,
                request: self.context.request,
                tx: self.context.tx,
                group_scratch: self.context.group_scratch,
                boolean_scratch: self.context.boolean_scratch,
                capture_scratch: self.context.capture_scratch,
                scan_cache: Some(self.cache),
                json_budget: None,
            };
            let base = self
                .fixed_scratch
                .checked_add(self.transform_bytes)
                .ok_or(StoreError::Arithmetic)?;
            let mut observer = PredicateObserver {
                store: self.store,
                base,
                capture_bytes: 0,
            };
            #[cfg(test)]
            measurements::record(Event::OperatorStages, 1);
            let result = inspect_predicate_stage(&mut context, &plan, value, &mut observer);
            match result {
                Ok(Some(captures)) => {
                    let outcome = if let Some(token) = stage_match {
                        observer
                            .store
                            .record_shared_match(self.candidate, run.id, ordinal, token)?
                    } else {
                        let outcome = observer.store.record_match(self.candidate, run.id, ordinal, value)?;
                        if share_stage {
                            stage_match = Some(observer.store.stage_match_token(outcome)?);
                        }
                        outcome
                    };
                    for entry in &captures {
                        observer.store.add_capture(outcome, &entry.key, &entry.value)?;
                    }
                    drop(captures);
                    run.terminal = true;
                }
                Ok(None) => {}
                Err(PredicateStageFailure::Semantic { checkpoint, error }) => {
                    observer.store.record_semantic_error(
                        self.candidate,
                        run.id,
                        ordinal,
                        CheckpointError {
                            step: ordinal,
                            checkpoint: StoredCheckpoint::predicate(checkpoint) as usize,
                            error,
                        },
                    )?;
                    run.terminal = true;
                }
                Err(PredicateStageFailure::Physical(error)) => return Err(error.into()),
                Err(PredicateStageFailure::Observer(error)) => return Err(error),
            }
            // The canonical helper's temporary capture vector is now dropped,
            // including partial capture allocation on semantic error.
            observer.store.scratch_capacity(base)?;
        }
        Ok(())
    }
}
struct PredicateObserver<'a, 'generation> {
    store: &'a mut ResultStore<'generation>,
    base: usize,
    capture_bytes: usize,
}
impl PredicateStageObserver for PredicateObserver<'_, '_> {
    type Error = CollectorError;
    fn observes_physical_work(&self) -> bool {
        true
    }
    fn operator_work(&mut self, work: usize) -> Collected<()> {
        self.store.charge_work(work)?;
        #[cfg(test)]
        measurements::record(Event::OperatorWork, work);
        Ok(())
    }
    fn charge_stage(&mut self, _: usize, detector: usize) -> Collected<()> {
        self.store.charge_work(detector)?;
        #[cfg(test)]
        measurements::record(Event::DetectorWork, detector);
        Ok(())
    }
    fn before_captures(&mut self, bytes: usize) -> Collected<()> {
        self.store.charge_work(bytes)?;
        Ok(())
    }
    fn captures(&mut self, _: &[CaptureEntry]) -> Collected<()> {
        Ok(())
    }
    fn admit_capture_allocation(&mut self, current: usize, required: usize) -> Collected<()> {
        self.store
            .admit_scratch(self.base.checked_add(required).ok_or(StoreError::Arithmetic)?)?;
        self.store.charge_work(
            current
                .checked_add(required.saturating_sub(current))
                .ok_or(StoreError::Arithmetic)?,
        )?;
        Ok(())
    }
    fn capture_capacity(&mut self, actual: usize) -> Collected<()> {
        self.capture_bytes = actual;
        self.store
            .scratch_capacity(self.base.checked_add(actual).ok_or(StoreError::Arithmetic)?)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_inspection_collector_test.rs"]
mod tests;
