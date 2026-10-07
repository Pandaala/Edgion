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

struct JsonCaptureScratch<'generation> {
    generation: GenerationHandle<'generation>,
    builder: CaptureBuilder<StageCaptureValue, JsonCaptureKey>,
}
impl JsonCaptureScratch<'_> {
    fn validate_generation(&self, generation: &CompiledGeneration) -> Collected<()> {
        if !std::ptr::eq(&*self.generation, generation) {
            return Err(CollectorError::State);
        }
        Ok(())
    }
    fn bytes(&self) -> Collected<usize> {
        self.builder.retained_bytes().map_err(Into::into)
    }
    fn release(&mut self) {
        self.builder = CaptureBuilder::default();
    }
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
        if !store.is_group_active(id)? {
            #[cfg(test)]
            measurements::record(Event::AvoidedPredicateGroups, 1);
            return Ok(());
        }
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

pub(super) struct JsonCollectorState<'generation> {
    store: ResultStore<'generation>,
    runtime: RuntimeLimits,
    mode: EvaluationMode,
    transform: TransformScratch,
    dispatch: DispatchScratch,
    captures: JsonCaptureScratch<'generation>,
    failure: Option<CollectorError>,
    xml_receipt: Option<Receipt>,
}

/// The context is borrowed only while one checker feed invokes its sink.
pub(super) struct JsonCollector<'call, 'generation> {
    state: JsonCollectorState<'generation>,
    context: SingleInputContext<'call>,
}
impl<'call, 'generation> JsonCollector<'call, 'generation> {
    pub(super) fn new(
        generation: GenerationHandle<'generation>,
        context: SingleInputContext<'call>,
        runtime: RuntimeLimits,
        mode: EvaluationMode,
        limits: ResultLimits,
    ) -> Collected<Self> {
        Self::new_domain(
            generation,
            context,
            runtime,
            mode,
            limits,
            super::super::InspectionDomain::Json,
        )
    }
    pub(super) fn new_domain(
        generation: GenerationHandle<'generation>,
        context: SingleInputContext<'call>,
        runtime: RuntimeLimits,
        mode: EvaluationMode,
        limits: ResultLimits,
        domain: super::super::InspectionDomain,
    ) -> Collected<Self> {
        if !std::ptr::eq(&*generation, context.generation)
            || (!generation.inspection_plan(domain).unsupported.is_empty()
                || !generation.inspection_plan(domain).xml_unsupported.is_empty())
        {
            return Err(CollectorError::State);
        }
        let group_count = generation.inspection_plan(domain).groups.len();
        let profile_count = generation.inspection_plan(domain).profiles.len();
        let scan_capacity = generation.inspection_plan(domain).matcher_scan_capacity;
        let capture_generation = generation.clone();
        let mut state = JsonCollectorState {
            store: ResultStore::new_domain(generation, runtime, mode, limits, domain)?,
            runtime,
            mode,
            transform: TransformScratch::default(),
            dispatch: DispatchScratch::default(),
            captures: JsonCaptureScratch {
                generation: capture_generation,
                builder: CaptureBuilder::default(),
            },
            failure: None,
            xml_receipt: None,
        };
        build_activation(&context, &mut state.store, domain)?;
        // Each allocation admits its old/new overlap and reports allocator capacity.
        let mut total = 0;
        reserve_scratch(&mut state.dispatch.groups, group_count, &mut total, &mut state.store)?;
        reserve_scratch(
            &mut state.dispatch.profiles,
            profile_count,
            &mut total,
            &mut state.store,
        )?;
        reserve_scratch(&mut state.dispatch.stamps, group_count, &mut total, &mut state.store)?;
        reserve_scratch(
            &mut state.dispatch.cache.scanned,
            scan_capacity,
            &mut total,
            &mut state.store,
        )?;
        state.store.charge_work(group_count)?;
        state.dispatch.stamps.resize(group_count, 0);
        Ok(Self { state, context })
    }
    pub(super) fn resume(
        state: JsonCollectorState<'generation>,
        context: SingleInputContext<'call>,
    ) -> Collected<Self> {
        state.captures.validate_generation(context.generation)?;
        Ok(Self { state, context })
    }
    pub(super) fn into_state(self) -> JsonCollectorState<'generation> {
        self.state
    }
    pub(super) fn failure(&self) -> Option<CollectorError> {
        self.state.failure
    }
    pub(super) fn into_abandoned_budget(self) -> Collected<InspectionBudget> {
        self.state.into_abandoned_budget()
    }
    pub(super) fn into_sealed(self) -> Collected<(SealedResults<'generation>, InspectionBudget)> {
        self.state.into_sealed()
    }
    fn sink_result<T>(&mut self, result: Collected<T>) -> Result<T, JsonCheckerSinkError> {
        result.map_err(|error| {
            self.state.failure.get_or_insert(error);
            JsonCheckerSinkError
        })
    }
    fn collect_scalar(&mut self, name: &[u8], value: &[u8], kind: JsonScalarKind) -> Collected<Option<Receipt>> {
        self.collect_value(name, value, kind, None)
    }
    fn collect_value(
        &mut self,
        name: &[u8],
        value: &[u8],
        kind: JsonScalarKind,
        xml_attribute: Option<bool>,
    ) -> Collected<Option<Receipt>> {
        if let Some(error) = self.state.failure {
            return Err(error);
        }
        #[cfg(test)]
        let dispatch_work_before = self.store.usage().work;
        let generation = self.context.generation;
        let plan = generation.inspection_plan(self.state.store.domain());
        let deferred = exact_name_match(generation, &plan.named_demands, name, &mut self.state.store)?;
        let mut retain = deferred
            || plan.needs_all_candidate_names
            || match xml_attribute {
                Some(true) => plan.xml_candidate_attributes,
                Some(false) => plan.xml_candidate_text,
                None => false,
            };
        if !retain && xml_attribute != Some(false) {
            retain = exact_name_match(generation, &plan.candidate_literals, name, &mut self.state.store)?;
        }
        if !retain && xml_attribute != Some(false) {
            for &selector in &plan.candidate_regexes {
                if match_name(
                    generation,
                    self.context.boolean_scratch,
                    selector,
                    name,
                    &mut self.state.store,
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
        self.state.dispatch.epoch = self.state.dispatch.epoch.checked_add(1).ok_or(StoreError::Arithmetic)?;
        self.state.store.charge_work(
            self.state
                .dispatch
                .groups
                .len()
                .checked_add(self.state.dispatch.profiles.len())
                .ok_or(StoreError::Arithmetic)?,
        )?;
        self.state.dispatch.groups.clear();
        self.state.dispatch.profiles.clear();
        let broad_only = plan.literals.is_empty() && plan.regexes.is_empty();
        if broad_only {
            let order = match xml_attribute {
                Some(true) => &plan.xml_attributes,
                Some(false) => &plan.xml_text,
                None => &plan.broad_only_order,
            };
            for &id in order {
                self.state.dispatch.add(id, &mut self.state.store)?;
            }
        } else {
            let xml_broad = match xml_attribute {
                Some(true) => &plan.xml_attributes[..],
                Some(false) => &plan.xml_text[..],
                None => &[],
            };
            for &id in xml_broad
                .iter()
                .chain(plan.broad_names.iter())
                .chain(&plan.broad_values)
            {
                self.state.dispatch.add(id, &mut self.state.store)?;
            }
            self.state.store.charge_work(
                plan.literal_lookup_work(name.len())
                    .map_err(|_| CollectorError::State)?,
            )?;
            for row in plan
                .literal_matches(generation, name)
                .map_err(|_| CollectorError::State)?
            {
                if xml_attribute != Some(false) {
                    self.state.dispatch.add(row.group, &mut self.state.store)?;
                }
            }
            for row in &plan.regexes {
                if xml_attribute != Some(false)
                    && match_name(
                        generation,
                        self.context.boolean_scratch,
                        row.selector,
                        name,
                        &mut self.state.store,
                    )?
                {
                    self.state.dispatch.add(row.group, &mut self.state.store)?;
                }
            }
            let n = self.state.dispatch.groups.len();
            let sort_work = n
                .checked_mul((usize::BITS - n.leading_zeros()) as usize)
                .and_then(|v| v.checked_mul(3))
                .ok_or(StoreError::Arithmetic)?;
            self.state.store.charge_work(sort_work)?;
            self.state
                .dispatch
                .groups
                .sort_unstable_by_key(|entry| plan.groups[entry.id].profile);
        }
        let n = self.state.dispatch.groups.len();
        #[cfg(test)]
        measurements::record(Event::DispatchWork, self.store.usage().work - dispatch_work_before);
        let candidate = if let Some(attribute) = xml_attribute {
            self.store.begin_xml_candidate(attribute.then_some(name), value.len())?
        } else {
            self.store
                .begin_candidate(name, value.len(), kind, deferred.then_some(value))?
        };
        let mut start = 0;
        while start < n {
            let profile_id = plan.groups[self.state.dispatch.groups[start].id].profile;
            let mut end = start + 1;
            while end < n && plan.groups[self.state.dispatch.groups[end].id].profile == profile_id {
                end += 1;
            }
            self.state.store.charge_work(end - start)?;
            let profile = plan.profiles.get(profile_id).ok_or(CollectorError::State)?;
            let transformations = transformations(generation, profile.transformation_plan)?;
            let source = match profile.view {
                JsonView::Name => name,
                JsonView::Value => value,
            };
            let fixed_scratch = self.state.dispatch.bytes()?;
            let mut observer = ProfileObserver {
                store: &mut self.state.store,
                context: &mut self.context,
                groups: &mut self.state.dispatch.groups[start..end],
                cache: &mut self.state.dispatch.cache,
                captures: &mut self.state.captures,
                candidate,
                profile: profile_id,
                runtime: self.state.runtime,
                mode: self.state.mode,
                fixed_scratch,
                transform_bytes: self.state.transform.retained_bytes().map_err(map_transform_error)?,
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
                    max_output_bytes: self.state.runtime.max_transformation_output_bytes,
                    max_work_bytes: self.state.runtime.max_transformation_work_bytes,
                },
                &mut self.state.transform,
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
            self.state.dispatch.profiles.push(profile_id);
            start = end;
        }
        let proof = self
            .state
            .store
            .completion_proof(candidate, &self.state.dispatch.profiles)?;
        self.state.store.complete_candidate(proof).map(Some).map_err(Into::into)
    }
}
impl<'generation> std::ops::Deref for JsonCollector<'_, 'generation> {
    type Target = JsonCollectorState<'generation>;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
impl std::ops::DerefMut for JsonCollector<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}
impl<'generation> JsonCollectorState<'generation> {
    pub(super) fn usage(&self) -> super::json_inspection::StoreUsage {
        self.store.usage_snapshot()
    }
    pub(super) fn into_abandoned_budget(self) -> Collected<InspectionBudget> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let Self {
            mut store,
            transform,
            dispatch,
            captures,
            ..
        } = self;
        drop(transform);
        drop(dispatch);
        drop(captures);
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
            captures,
            ..
        } = self;
        drop(transform);
        drop(dispatch);
        drop(captures);
        let (sealed, mut budget) = store.into_sealed()?;
        budget.release_scratch();
        Ok((sealed, budget))
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
        let result = self.state.store.concat(left, right).map_err(Into::into);
        self.sink_result(result)
    }
    fn discard(&mut self, receipt: Receipt) -> Result<(), JsonCheckerSinkError> {
        let result = self.state.store.discard(receipt).map_err(Into::into);
        self.sink_result(result)
    }
    fn seal(&mut self, receipt: Option<Receipt>, summary: JsonSemanticSummary) -> Result<(), JsonCheckerSinkError> {
        self.state.captures.release();
        let result = (|| {
            let scratch = self
                .dispatch
                .bytes()?
                .checked_add(self.state.transform.retained_bytes().map_err(map_transform_error)?)
                .ok_or(StoreError::Arithmetic)?;
            self.state.store.scratch_capacity(scratch)?;
            self.state.store.seal(receipt, summary).map_err(Into::into)
        })();
        self.sink_result(result)
    }
    fn abandon(&mut self) {
        self.state.transform = TransformScratch::default();
        self.state.dispatch = DispatchScratch::default();
        self.state.captures.release();
        self.state.store.release_scratch();
        self.state.store.abandon();
    }
}

impl crate::seclang::XmlStreamSink for JsonCollector<'_, '_> {
    fn value(&mut self, name: Option<&[u8]>, value: &[u8]) -> Result<(), crate::seclang::XmlStreamSinkError> {
        let result = (|| {
            let receipt = self.collect_value(
                name.unwrap_or_default(),
                value,
                JsonScalarKind::String,
                Some(name.is_some()),
            )?;
            self.state.xml_receipt = self.state.store.concat(self.state.xml_receipt, receipt)?;
            Ok(())
        })();
        result.map_err(|error: CollectorError| {
            self.state.failure.get_or_insert(error);
            crate::seclang::XmlStreamSinkError
        })
    }
}
impl JsonCollector<'_, '_> {
    pub(super) fn abandon_xml(&mut self) {
        JsonCheckerSink::abandon(self);
        self.state.xml_receipt = None;
    }
    pub(super) fn seal_xml(&mut self, summary: crate::seclang::XmlSemanticSummary) -> Collected<()> {
        self.state.captures.release();
        let scratch = self
            .dispatch
            .bytes()?
            .checked_add(self.state.transform.retained_bytes().map_err(map_transform_error)?)
            .ok_or(StoreError::Arithmetic)?;
        self.state.store.scratch_capacity(scratch)?;
        self.state
            .store
            .seal_xml(self.state.xml_receipt, summary)
            .map_err(Into::into)
    }
}

/// The snapshot is built once after Phase1, before any semantic-value dispatch.
fn build_activation(
    context: &SingleInputContext<'_>,
    store: &mut ResultStore<'_>,
    domain: InspectionDomain,
) -> Collected<()> {
    let generation = context.generation;
    let policy_prunes = !context.tx.check_policy.expensive_checks && generation.has_expensive_checks;
    #[cfg(test)]
    if context.tx.force_unpruned_inspection {
        if policy_prunes {
            store.initialize_activation(generation.inspection_plan(domain).groups.len())?;
            for (unit_id, unit) in generation.units.iter().enumerate() {
                store.charge_work(1)?;
                if !unit.expensive_check {
                    activate_consumers(generation, unit_id, domain, store)?;
                }
            }
        }
        return Ok(());
    }
    let proof = &generation.inspection_activation;
    if !proof.has_gate && context.tx.removed_units.is_empty() && !policy_prunes {
        return Ok(());
    }
    let mut reachable = Vec::new();
    let mut scratch = 0;
    reserve_scratch(&mut reachable, proof.steps.len(), &mut scratch, store)?;
    store.charge_work(proof.steps.len())?;
    reachable.resize(proof.steps.len(), false);
    if let Some(first) = reachable.first_mut() {
        *first = true;
    }
    store.initialize_activation(generation.inspection_plan(domain).groups.len())?;
    for (ordinal, step) in proof.steps.iter().enumerate() {
        store.charge_work(1)?;
        if !reachable[ordinal] {
            continue;
        }
        let next = ordinal.checked_add(1).ok_or(StoreError::Arithmetic)?;
        let Some(unit_id) = step.unit else {
            if let Some(slot) = reachable.get_mut(next) {
                *slot = true;
            }
            continue;
        };
        store.charge_work((usize::BITS - context.tx.removed_units.len().leading_zeros()) as usize + 1)?;
        if context.tx.removed_units.binary_search(&unit_id).is_ok()
            || (policy_prunes && generation.units[unit_id].expensive_check)
        {
            if let Some(slot) = reachable.get_mut(next) {
                *slot = true;
            }
            continue;
        }
        activate_consumers(generation, unit_id, domain, store)?;
        let mut known = None;
        if let Some(gate) = step.gate {
            let mut excluded = false;
            for exclusion in &context.tx.target_exclusions {
                store.charge_work(1)?;
                excluded |= exclusion.unit == unit_id && exclusion.collection == CollectionKind::Tx;
            }
            if !excluded {
                let key = generation_bytes(&generation.target_bytes, gate.key)?;
                for entry in &context.tx.entries {
                    store.charge_work(1)?;
                    let candidate = entry.key.observed_bytes(context.request)?;
                    store.charge_work(candidate.len().min(key.len()))?;
                    if candidate.eq_ignore_ascii_case(key) {
                        let value = entry.value.observed_bytes(context.request)?;
                        // UTF8 validation, syntax and integer parsing each walk
                        // the seed at most once. Invalid seeds stay unknown.
                        store.charge_work(value.len().checked_mul(3).ok_or(StoreError::Arithmetic)?)?;
                        known = parse_i64(value)
                            .ok()
                            .map(|actual| integer_relation(gate.relation, actual, gate.expected) ^ gate.negated);
                        break;
                    }
                }
            }
        }
        if known != Some(true) {
            if let Some(slot) = reachable.get_mut(next) {
                *slot = true;
            }
        }
        if known != Some(false) {
            if step.matched < next || step.matched > proof.steps.len() {
                return Err(CollectorError::State);
            }
            if let Some(slot) = reachable.get_mut(step.matched) {
                *slot = true;
            }
        }
    }
    // A later-phase or chain consumer owns the same static fact regardless of
    // the representative group's member. Phase1 has no body subscriptions.
    for (unit_id, unit) in generation.units.iter().enumerate() {
        store.charge_work(1)?;
        if matches!(
            unit.phase,
            Phase::ResponseHeaders | Phase::ResponseBody | Phase::Logging
        ) {
            activate_consumers(generation, unit_id, domain, store)?;
        }
    }
    drop(reachable);
    store.scratch_capacity(0)?;
    Ok(())
}
fn activate_consumers(
    generation: &CompiledGeneration,
    unit_id: usize,
    domain: InspectionDomain,
    store: &mut ResultStore<'_>,
) -> Collected<()> {
    use super::super::json_inspection_plan::JsonMemberMode;
    let unit = generation.units.get(unit_id).ok_or(CollectorError::State)?;
    let end = unit
        .member_start
        .checked_add(unit.member_count)
        .ok_or(StoreError::Arithmetic)?;
    for member in generation
        .inspection_plan(domain)
        .members
        .get(unit.member_start..end)
        .ok_or(CollectorError::State)?
    {
        store.charge_work(1)?;
        for mode in [member.name, member.value] {
            store.charge_work(1)?;
            if let JsonMemberMode::StaticFact(group) = mode {
                store.activate_group(group)?;
            }
        }
    }
    Ok(())
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
    scratch: &mut [Option<RegexBooleanScratch>],
    selector: super::super::CompiledTargetSelector,
    name: &[u8],
    store: &mut ResultStore<'_>,
) -> Collected<bool> {
    store.charge_work(name.len().checked_add(1).ok_or(StoreError::Arithmetic)?)?;
    match selector {
        super::super::CompiledTargetSelector::Regex { program, .. } => {
            let regex = generation.boolean_regexes.get(program).ok_or(CollectorError::State)?;
            let slot = scratch.get_mut(program).ok_or(CollectorError::State)?;
            let scratch = lazy_boolean_scratch(slot, regex);
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
    captures: &'a mut JsonCaptureScratch<'generation>,
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
        self.store.admit_scratch(
            self.fixed_scratch
                .checked_add(self.captures.bytes()?)
                .and_then(|bytes| bytes.checked_add(required))
                .ok_or(StoreError::Arithmetic)?,
        )?;
        Ok(())
    }
    fn scratch_capacity(&mut self, actual: usize) -> Collected<()> {
        self.transform_bytes = actual;
        self.store.scratch_capacity(
            self.fixed_scratch
                .checked_add(self.captures.bytes()?)
                .and_then(|bytes| bytes.checked_add(actual))
                .ok_or(StoreError::Arithmetic)?,
        )?;
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
                .inspection_plan(self.store.domain())
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
            };
            #[cfg(test)]
            measurements::record(Event::OperatorStages, 1);
            let result = (|| {
                self.captures.validate_generation(generation)?;
                let result =
                    inspect_predicate_stage_into(&mut context, &plan, value, &mut observer, &mut self.captures.builder);
                match result {
                    Ok(true) => {
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
                        for entry in &self.captures.builder.entries {
                            entry
                                .key
                                .with_bytes(&self.captures.generation, |key| match &entry.value {
                                    StageCaptureValue::InputRange { start, len } => observer
                                        .store
                                        .add_capture_range(outcome, key, *start, *len)
                                        .map_err(CollectorError::from),
                                    StageCaptureValue::Fingerprint(value) => observer
                                        .store
                                        .add_capture(outcome, key, value.as_bytes())
                                        .map_err(CollectorError::from),
                                })?;
                        }
                        run.terminal = true;
                    }
                    Ok(false) => {}
                    Err(PredicateStageFailure::Semantic { checkpoint, error }) => {
                        self.captures.builder.clear();
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
                Ok(())
            })();
            // Clear partial extraction/transfer on every exit without resolving
            // stale ranges. Retained descriptor capacity stays fully charged.
            self.captures.builder.clear();
            result?;
            observer
                .store
                .scratch_capacity(base.checked_add(self.captures.bytes()?).ok_or(StoreError::Arithmetic)?)?;
        }
        Ok(())
    }
}
struct PredicateObserver<'a, 'generation> {
    store: &'a mut ResultStore<'generation>,
    base: usize,
}
impl PredicateStageObserver for PredicateObserver<'_, '_> {
    type Error = CollectorError;
    type CaptureValue = StageCaptureValue;
    type CaptureKey = JsonCaptureKey;
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
    fn captures(&mut self, _: &[CaptureEntry<Self::CaptureValue, Self::CaptureKey>]) -> Collected<()> {
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
        self.store
            .scratch_capacity(self.base.checked_add(actual).ok_or(StoreError::Arithmetic)?)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_inspection_collector_test.rs"]
mod tests;
