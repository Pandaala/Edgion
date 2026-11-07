use super::*;

pub(super) fn charge_ctl_work(work: &mut usize, amount: usize, limit: usize) -> Result<(), RuntimeError> {
    *work = work.checked_add(amount).ok_or(RuntimeError::Arithmetic)?;
    if *work > limit {
        return Err(RuntimeError::OperatorWorkLimit { limit });
    }
    Ok(())
}

pub(super) fn tagged_units(
    generation: &CompiledGeneration,
    tag: super::super::ByteRange,
) -> Result<&[super::super::CompiledTagRuleRef], RuntimeError> {
    let tag = generation_bytes(&generation.bytes, tag)?;
    let refs = &generation.tag_rule_refs;
    let start = refs.partition_point(|entry| {
        generation_bytes(&generation.bytes, entry.tag).expect("compiled tag reference has a valid range") < tag
    });
    let end = start
        + refs[start..].partition_point(|entry| {
            generation_bytes(&generation.bytes, entry.tag).expect("compiled tag reference has a valid range") <= tag
        });
    Ok(&refs[start..end])
}

// Shared by ordinary selected values and the upcoming provisional JSON
// collector. Context supplies existing programs and scratch, never a second
// operator implementation. Matched identity construction runs only on a match.
// It is also the operator context: `evaluate_operator` and capture extraction
// read the same generation, request/tx state, scratch slices and stage cache.
pub(super) struct SingleInputContext<'a> {
    pub(super) generation: &'a CompiledGeneration,
    pub(super) request: &'a RequestState<'a>,
    pub(super) tx: &'a TxState,
    pub(super) group_scratch: &'a mut [Option<MatchScratch>],
    pub(super) boolean_scratch: &'a mut [Option<RegexBooleanScratch>],
    pub(super) capture_scratch: &'a mut [Option<RegexCaptureScratch>],
    pub(super) scan_cache: Option<&'a mut StageScanCache>,
    pub(super) json_budget: Option<&'a mut InspectionBudget>,
    /// Borrowed-transformation proof for one ordinary request-range input.
    /// Consumed by the first `evaluate_single_input` call; `None` elsewhere.
    pub(super) transform_memo: Option<(&'a mut BorrowedTransformMemo, BorrowedTransformKey)>,
}

pub(super) struct SingleInputPlan<'a> {
    pub(super) operator: super::super::CompiledOperatorPlan,
    pub(super) transformations: &'a [super::super::super::TransformationKind],
    pub(super) multi_match: bool,
    pub(super) runtime_expansion_byte_limit: usize,
    pub(super) limits: RuntimeLimits,
    pub(super) evaluation_mode: EvaluationMode,
}

pub(super) fn evaluate_single_input(
    context: &mut SingleInputContext<'_>,
    plan: SingleInputPlan<'_>,
    value: &[u8],
    accounting: &mut OrderedStageAccounting<'_>,
    matched_count: usize,
    make_matched: impl Fn(&[u8], Option<&mut InspectionBudget>) -> Result<(Vec<u8>, Vec<u8>), RuntimeError>,
) -> Result<Option<MemberMatch>, RuntimeError> {
    let transform_limits = TransformLimits {
        max_output_bytes: plan.limits.max_transformation_output_bytes,
        max_work_bytes: plan.limits.max_transformation_work_bytes,
    };
    if !plan.multi_match {
        let transformed = match context.transform_memo.take() {
            Some((memo, key)) if memo.key == Some(key) => Cow::Borrowed(value),
            Some((memo, key)) => {
                memo.clear();
                let transformed = apply_transformations_borrowed(value, plan.transformations, transform_limits)
                    .map_err(map_transform_error)?;
                // Borrowed proves complete validation and identity for every step.
                // Local transform limits are part of the key; downstream charges
                // still execute below even when a subsequent call reuses this proof.
                if matches!(&transformed, Cow::Borrowed(_)) {
                    memo.key = Some(key);
                }
                transformed
            }
            None => apply_transformations_borrowed(value, plan.transformations, transform_limits)
                .map_err(map_transform_error)?,
        };
        return evaluate_transformed_input(
            context,
            &plan,
            &transformed,
            value.len(),
            accounting,
            matched_count,
            &make_matched,
        );
    }
    let transformed_values =
        apply_transformations_multimatch(value, plan.transformations, transform_limits).map_err(map_transform_error)?;
    for transformed in transformed_values {
        if let Some(matched) = evaluate_transformed_input(
            context,
            &plan,
            &transformed,
            value.len(),
            accounting,
            matched_count,
            &make_matched,
        )? {
            return Ok(Some(matched));
        }
    }
    Ok(None)
}

pub(super) fn evaluate_transformed_input(
    context: &mut SingleInputContext<'_>,
    plan: &SingleInputPlan<'_>,
    transformed: &[u8],
    original_bytes: usize,
    accounting: &mut OrderedStageAccounting<'_>,
    matched_count: usize,
    make_matched: impl FnOnce(&[u8], Option<&mut InspectionBudget>) -> Result<(Vec<u8>, Vec<u8>), RuntimeError>,
) -> Result<Option<MemberMatch>, RuntimeError> {
    accounting.original_bytes = original_bytes;
    let captures = inspect_predicate_stage(context, plan, transformed, accounting).map_err(|error| match error {
        PredicateStageFailure::Semantic { error, .. }
        | PredicateStageFailure::Physical(error)
        | PredicateStageFailure::Observer(error) => error,
    })?;
    captures
        .map(|captures| {
            let (value, name) = make_matched(transformed, context.json_budget.as_deref_mut())?;
            finish_member_match(captures, value, name, matched_count, accounting.work, plan.limits)
        })
        .transpose()
}
pub(super) fn finish_member_match(
    captures: Vec<CaptureEntry>,
    value: impl Into<MatchValue>,
    name: impl Into<MatchValue>,
    matched_count: usize,
    work: &mut usize,
    limits: RuntimeLimits,
) -> Result<MemberMatch, RuntimeError> {
    if matched_count == limits.max_operator_evaluations {
        return Err(RuntimeError::OperatorEvaluationLimit {
            limit: limits.max_operator_evaluations,
        });
    }
    let matched = MemberMatch {
        captures,
        matched_var: Some(value.into()),
        matched_var_name: Some(name.into()),
    };
    charge_operator_work(work, member_match_bytes(&matched)?, limits)?;
    Ok(matched)
}

#[derive(Clone, Copy)]
pub(super) enum PredicateCheckpoint {
    DetectorInput,
    Operator,
    Captures,
}

pub(super) enum PredicateStageFailure<E> {
    Semantic {
        checkpoint: PredicateCheckpoint,
        error: RuntimeError,
    },
    Physical(RuntimeError),
    Observer(E),
}

impl<E> PredicateStageFailure<E> {
    pub(super) fn at(checkpoint: PredicateCheckpoint, error: RuntimeError) -> Self {
        if matches!(error, RuntimeError::Allocation | RuntimeError::Arithmetic) {
            Self::Physical(error)
        } else {
            Self::Semantic { checkpoint, error }
        }
    }
}

impl<E> From<RuntimeError> for PredicateStageFailure<E> {
    fn from(error: RuntimeError) -> Self {
        Self::at(PredicateCheckpoint::Captures, error)
    }
}

/// Observer failures are physical inspection failures. Predicate/capture errors
/// retain their semantic checkpoint so a provisional collector can defer them.
pub(super) trait PredicateStageObserver {
    type Error;
    type CaptureValue: StageCapturePayload;
    type CaptureKey: StageCaptureKey;
    fn observes_physical_work(&self) -> bool {
        false
    }
    fn operator_work(&mut self, _work: usize) -> Result<(), Self::Error> {
        Ok(())
    }
    fn charge_stage(&mut self, transformed_bytes: usize, detector_work: usize) -> Result<(), Self::Error>;
    fn before_captures(&mut self, transformed_bytes: usize) -> Result<(), Self::Error>;
    fn captures(&mut self, captures: &[CaptureEntry<Self::CaptureValue, Self::CaptureKey>]) -> Result<(), Self::Error>;
    fn admit_capture_allocation(&mut self, current_bytes: usize, required_bytes: usize) -> Result<(), Self::Error>;
    fn capture_capacity(&mut self, actual_bytes: usize) -> Result<(), Self::Error>;
}
type PredicateStageResult<O> = Result<
    Option<Vec<CaptureEntry<<O as PredicateStageObserver>::CaptureValue, <O as PredicateStageObserver>::CaptureKey>>>,
    PredicateStageFailure<<O as PredicateStageObserver>::Error>,
>;

/// Per-unit stage accounting shared by every input of one member evaluation.
/// `original_bytes` describes the current input and is set by each stage
/// entry point before it charges a stage.
pub(super) struct OrderedStageAccounting<'a> {
    pub(super) original_bytes: usize,
    pub(super) limits: RuntimeLimits,
    pub(super) work: &'a mut usize,
    pub(super) detector_work_bytes: &'a mut usize,
}

impl<'a> OrderedStageAccounting<'a> {
    pub(super) fn new(limits: RuntimeLimits, work: &'a mut usize, detector_work_bytes: &'a mut usize) -> Self {
        Self {
            original_bytes: 0,
            limits,
            work,
            detector_work_bytes,
        }
    }
}

impl PredicateStageObserver for OrderedStageAccounting<'_> {
    type Error = RuntimeError;
    type CaptureValue = MatchValue;
    type CaptureKey = MatchValue;
    fn charge_stage(&mut self, transformed_bytes: usize, detector_work: usize) -> Result<(), RuntimeError> {
        if detector_work != 0 {
            *self.detector_work_bytes = self
                .detector_work_bytes
                .checked_add(detector_work)
                .ok_or(RuntimeError::Arithmetic)?;
            if *self.detector_work_bytes > self.limits.max_operator_work_bytes {
                return Err(RuntimeError::OperatorWorkLimit {
                    limit: self.limits.max_operator_work_bytes,
                });
            }
        }
        charge_operator_work(
            self.work,
            self.original_bytes
                .checked_add(transformed_bytes)
                .and_then(|bytes| bytes.checked_add(detector_work))
                .ok_or(RuntimeError::Arithmetic)?,
            self.limits,
        )
    }
    fn before_captures(&mut self, _transformed_bytes: usize) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn captures(&mut self, _captures: &[CaptureEntry]) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn admit_capture_allocation(&mut self, _current_bytes: usize, _required_bytes: usize) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn capture_capacity(&mut self, _actual_bytes: usize) -> Result<(), RuntimeError> {
        Ok(())
    }
}

pub(super) fn detector_work_for_stage(
    plan: super::super::CompiledOperatorPlan,
    len: usize,
) -> Result<usize, RuntimeError> {
    match plan.operand {
        super::super::CompiledOperatorOperand::DetectSqli { .. } => {
            crate::injection::work_cost(crate::injection::Kind::Sqli, len)
        }
        super::super::CompiledOperatorOperand::DetectXss => {
            crate::injection::work_cost(crate::injection::Kind::Xss, len)
        }
        _ => Some(0),
    }
    .ok_or(RuntimeError::DetectorInputLimit {
        limit: crate::injection::MAX_INPUT_BYTES,
    })
}

fn inspect_predicate_stage<O: PredicateStageObserver>(
    context: &mut SingleInputContext<'_>,
    plan: &SingleInputPlan<'_>,
    transformed: &[u8],
    observer: &mut O,
) -> PredicateStageResult<O> {
    let mut captures = CaptureBuilder::default();
    if inspect_predicate_stage_into(context, plan, transformed, observer, &mut captures)? {
        Ok(Some(captures.entries))
    } else {
        Ok(None)
    }
}

fn inspect_predicate_stage_into<O: PredicateStageObserver>(
    context: &mut SingleInputContext<'_>,
    plan: &SingleInputPlan<'_>,
    transformed: &[u8],
    observer: &mut O,
    captures: &mut CaptureBuilder<O::CaptureValue, O::CaptureKey>,
) -> Result<bool, PredicateStageFailure<O::Error>> {
    inspect_predicate_stage_with_hit_matcher(context, plan, transformed, observer, captures, None)
}

pub(super) fn inspect_predicate_stage_with_hit_matcher<O: PredicateStageObserver>(
    context: &mut SingleInputContext<'_>,
    plan: &SingleInputPlan<'_>,
    transformed: &[u8],
    observer: &mut O,
    captures: &mut CaptureBuilder<O::CaptureValue, O::CaptureKey>,
    hit_matcher: Option<super::super::CompiledOperatorPlan>,
) -> Result<bool, PredicateStageFailure<O::Error>> {
    if !captures.entries.is_empty() || captures.semantic_bytes != 0 || captures.heap_bytes != 0 {
        return Err(RuntimeError::GenerationInvariant.into());
    }
    if hit_matcher.is_some()
        && (!matches!(
            plan.operator.operand,
            super::super::CompiledOperatorOperand::Regex(super::super::RegexPlanRef::Capture(_))
        ) || !matches!(
            hit_matcher.map(|p| p.operand),
            Some(super::super::CompiledOperatorOperand::Matcher { pattern_count: 1, .. })
        ))
    {
        return Err(RuntimeError::GenerationInvariant.into());
    }
    let detection = hit_matcher.unwrap_or(plan.operator);
    let detector_work = detector_work_for_stage(plan.operator, transformed.len())
        .map_err(|error| PredicateStageFailure::at(PredicateCheckpoint::DetectorInput, error))?;
    observer
        .charge_stage(transformed.len(), detector_work)
        .map_err(PredicateStageFailure::Observer)?;
    if observer.observes_physical_work() {
        let work =
            physical_operator_work(context, detection, transformed.len()).map_err(PredicateStageFailure::Physical)?;
        observer.operator_work(work).map_err(PredicateStageFailure::Observer)?;
    }
    let (positive, capture) = evaluate_operator(
        context,
        detection,
        transformed,
        plan.runtime_expansion_byte_limit,
        plan.evaluation_mode,
    )
    .map_err(|error| PredicateStageFailure::at(PredicateCheckpoint::Operator, error))?;
    if positive == plan.operator.negated {
        return Ok(false);
    }
    if !plan.operator.negated {
        observer
            .before_captures(transformed.len())
            .map_err(PredicateStageFailure::Observer)?;
        extract_captures_into(
            context,
            plan.operator,
            transformed,
            plan.limits,
            capture,
            observer,
            captures,
        )?;
    }
    observer
        .captures(&captures.entries)
        .map_err(PredicateStageFailure::Observer)?;
    Ok(true)
}

pub(super) fn extract_captures_into<O: PredicateStageObserver>(
    context: &mut SingleInputContext<'_>,
    plan: super::super::CompiledOperatorPlan,
    input: &[u8],
    limits: RuntimeLimits,
    capture: Option<OperatorCapture>,
    observer: &mut O,
    captures: &mut CaptureBuilder<O::CaptureValue, O::CaptureKey>,
) -> Result<(), PredicateStageFailure<O::Error>> {
    if !matches!(
        plan.operand,
        super::super::CompiledOperatorOperand::Regex(super::super::RegexPlanRef::Capture(_))
            | super::super::CompiledOperatorOperand::CapturePhrase(_)
            | super::super::CompiledOperatorOperand::DetectSqli { capture: true }
    ) {
        return Ok(());
    }
    let bound = match plan.operand {
        super::super::CompiledOperatorOperand::Regex(super::super::RegexPlanRef::Capture(program)) => context
            .generation
            .capture_regexes
            .get(program)
            .ok_or(RuntimeError::GenerationInvariant)?
            .capture_len()
            .checked_mul(2)
            .ok_or(RuntimeError::Arithmetic)?,
        // Each regex group may produce a numeric key and a named alias;
        // phrase and SQLi fingerprints produce only capture 0.
        _ => 1,
    };
    let slots = bound.min(limits.max_capture_entries).min(64);
    let initial = slots
        .checked_mul(size_of::<CaptureEntry<O::CaptureValue, O::CaptureKey>>())
        .ok_or(RuntimeError::Arithmetic)?;
    if slots > captures.entries.capacity() {
        let current = captures.retained_bytes()?;
        observer
            .admit_capture_allocation(current, current.checked_add(initial).ok_or(RuntimeError::Arithmetic)?)
            .map_err(PredicateStageFailure::Observer)?;
        captures
            .entries
            .try_reserve_exact(slots)
            .map_err(|_| RuntimeError::Allocation)?;
    }
    observer
        .capture_capacity(captures.retained_bytes()?)
        .map_err(PredicateStageFailure::Observer)?;
    match plan.operand {
        super::super::CompiledOperatorOperand::Regex(super::super::RegexPlanRef::Capture(program)) => {
            let regex = context
                .generation
                .capture_regexes
                .get(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            let slot = context
                .capture_scratch
                .get_mut(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            let scratch = lazy_capture_scratch(slot, regex);
            let results = regex.captures(input, scratch).map_err(map_regex_scratch_error)?;
            for index in 0..regex.capture_len() {
                let Some(range) = results.group(index) else {
                    continue;
                };
                let value = CaptureSource::InputRange {
                    input,
                    start: range.start,
                    len: range
                        .end
                        .checked_sub(range.start)
                        .ok_or(RuntimeError::GenerationInvariant)?,
                };
                let (digits, start) = decimal_bytes(index);
                push_capture(
                    captures,
                    CaptureKeySource::Numeric {
                        index,
                        bytes: &digits[start..],
                    },
                    value,
                    limits,
                    observer,
                )?;
                if let Some(name) = regex.capture_name(index) {
                    push_capture(
                        captures,
                        CaptureKeySource::Named {
                            program,
                            group: index,
                            bytes: name.as_bytes(),
                        },
                        value,
                        limits,
                        observer,
                    )?;
                }
            }
        }
        super::super::CompiledOperatorOperand::CapturePhrase(_) => {
            let Some(OperatorCapture::Phrase(matched)) = capture else {
                return Err(RuntimeError::GenerationInvariant.into());
            };
            let value = CaptureSource::InputRange {
                input,
                start: matched.start(),
                len: matched
                    .end()
                    .checked_sub(matched.start())
                    .ok_or(RuntimeError::GenerationInvariant)?,
            };
            push_capture(
                captures,
                CaptureKeySource::Numeric { index: 0, bytes: b"0" },
                value,
                limits,
                observer,
            )?;
        }
        super::super::CompiledOperatorOperand::DetectSqli { capture: true } => {
            let Some(OperatorCapture::Fingerprint(fingerprint)) = capture else {
                return Err(RuntimeError::GenerationInvariant.into());
            };
            push_capture(
                captures,
                CaptureKeySource::Numeric { index: 0, bytes: b"0" },
                CaptureSource::Fingerprint(fingerprint),
                limits,
                observer,
            )?;
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn push_capture<O: PredicateStageObserver>(
    captures: &mut CaptureBuilder<O::CaptureValue, O::CaptureKey>,
    key_source: CaptureKeySource<'_>,
    source: CaptureSource<'_>,
    limits: RuntimeLimits,
    observer: &mut O,
) -> Result<(), PredicateStageFailure<O::Error>> {
    let value_len = source.bytes()?.len();
    let key_len = key_source.bytes().len();
    let key_allocation = O::CaptureKey::allocation_bytes(key_source);
    let allocation_bytes = O::CaptureValue::allocation_bytes(source)?;
    if captures.entries.len() == limits.max_capture_entries {
        return Err(RuntimeError::CaptureEntryLimit {
            limit: limits.max_capture_entries,
        }
        .into());
    }
    let actual = captures
        .semantic_bytes
        .checked_add(key_len)
        .and_then(|total| total.checked_add(value_len))
        .ok_or(RuntimeError::Arithmetic)?;
    if actual > limits.max_capture_bytes {
        return Err(RuntimeError::CaptureByteLimit {
            limit: limits.max_capture_bytes,
        }
        .into());
    }
    let retained = captures.retained_bytes()?;
    let extra_slot = if captures.entries.len() == captures.entries.capacity() {
        size_of::<CaptureEntry<O::CaptureValue, O::CaptureKey>>()
    } else {
        0
    };
    let required = retained
        .checked_add(extra_slot)
        .and_then(|bytes| bytes.checked_add(key_allocation))
        .and_then(|bytes| bytes.checked_add(allocation_bytes))
        .ok_or(RuntimeError::Arithmetic)?;
    let peak = if extra_slot != 0 {
        required
            .checked_add(
                captures
                    .entries
                    .capacity()
                    .checked_mul(size_of::<CaptureEntry<O::CaptureValue, O::CaptureKey>>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .ok_or(RuntimeError::Arithmetic)?
    } else {
        required
    };
    observer
        .admit_capture_allocation(retained, peak)
        .map_err(PredicateStageFailure::Observer)?;
    captures
        .entries
        .try_reserve_exact(1)
        .map_err(|_| RuntimeError::Allocation)?;
    let descriptors = captures.retained_bytes()?;
    observer
        .capture_capacity(descriptors)
        .map_err(PredicateStageFailure::Observer)?;
    observer
        .admit_capture_allocation(
            descriptors,
            descriptors
                .checked_add(key_allocation)
                .ok_or(RuntimeError::Arithmetic)?,
        )
        .map_err(PredicateStageFailure::Observer)?;
    let key = O::CaptureKey::from_source(key_source)?;
    #[cfg(test)]
    measurements::record(Event::TemporaryCaptureCopyBytes, key_allocation);
    let with_key = descriptors
        .checked_add(key.heap_capacity())
        .ok_or(RuntimeError::Arithmetic)?;
    observer
        .capture_capacity(with_key)
        .map_err(PredicateStageFailure::Observer)?;
    observer
        .admit_capture_allocation(
            with_key,
            with_key.checked_add(allocation_bytes).ok_or(RuntimeError::Arithmetic)?,
        )
        .map_err(PredicateStageFailure::Observer)?;
    let value = O::CaptureValue::from_source(source)?;
    #[cfg(test)]
    measurements::record(Event::TemporaryCaptureCopyBytes, allocation_bytes);
    observer
        .capture_capacity(
            with_key
                .checked_add(value.heap_capacity())
                .ok_or(RuntimeError::Arithmetic)?,
        )
        .map_err(PredicateStageFailure::Observer)?;
    let heap_bytes = captures
        .heap_bytes
        .checked_add(key.heap_capacity())
        .and_then(|bytes| bytes.checked_add(value.heap_capacity()))
        .ok_or(RuntimeError::Arithmetic)?;
    captures.entries.push(CaptureEntry { key, value });
    captures.semantic_bytes = actual;
    captures.heap_bytes = heap_bytes;
    Ok(())
}

pub(super) fn matched_variable(
    request: &RequestState,
    tx: &TxState,
    response: Option<&ResponseState>,
    selected: SelectedValue,
    transformed: &[u8],
    budget: Option<&mut InspectionBudget>,
) -> Result<(Vec<u8>, Vec<u8>), RuntimeError> {
    let value = owned_copy(transformed)?;
    let selected_name = selected.name_bytes(request, tx, response, budget)?;
    let name = matched_variable_name(selected.collection, selected_name)?;
    Ok((value, name))
}

pub(super) fn matched_variable_name(
    collection: CollectionKind,
    selected_name: Option<&[u8]>,
) -> Result<Vec<u8>, RuntimeError> {
    let collection = collection.as_str().as_bytes();
    let name_len = selected_name.map_or(0, |name| name.len().saturating_add(1));
    let capacity = collection.len().checked_add(name_len).ok_or(RuntimeError::Arithmetic)?;
    let mut name = Vec::new();
    name.try_reserve_exact(capacity).map_err(|_| RuntimeError::Allocation)?;
    name.extend_from_slice(collection);
    if let Some(selected_name) = selected_name {
        name.push(b':');
        name.extend_from_slice(selected_name);
    }
    Ok(name)
}

pub(super) fn apply_member_match(
    staged: &mut TxState,
    capture_keys: &mut Vec<MatchValue>,
    matched: MemberMatch,
    limits: RuntimeLimits,
    request: &RequestState,
    mut json_budget: Option<&mut InspectionBudget>,
) -> Result<(), RuntimeError> {
    if let (Some(value), Some(name)) = (&matched.matched_var, &matched.matched_var_name) {
        if staged.matched_vars.len() == limits.max_tx_variables {
            return Err(RuntimeError::TxVariableLimit {
                limit: limits.max_tx_variables,
            });
        }
        staged
            .matched_vars
            .try_reserve(1)
            .map_err(|_| RuntimeError::Allocation)?;
        let entry = TxEntry {
            key: name.try_clone()?,
            value: value.try_clone()?,
        };
        let payload = tx_payload_bytes(entry.key.len(), entry.value.len())?;
        let total = tx_payload_delta(staged.matched_vars_payload_bytes, 0, payload)?;
        staged.matched_vars.push(entry);
        staged.matched_vars_payload_bytes = total;
    }
    staged.matched_var = matched.matched_var;
    staged.matched_var_name = matched.matched_var_name;
    staged.refresh_retained_bytes();
    if staged.retained_bytes > limits.max_tx_bytes {
        return Err(RuntimeError::TxByteLimit {
            limit: limits.max_tx_bytes,
        });
    }
    if matched.captures.is_empty() {
        return Ok(());
    }
    for key in capture_keys.iter() {
        staged.delete(
            key.bytes(request, json_budget.as_deref_mut())?,
            request,
            json_budget.as_deref_mut(),
        )?;
    }
    capture_keys.clear();
    capture_keys
        .try_reserve_exact(matched.captures.len())
        .map_err(|_| RuntimeError::Allocation)?;
    for capture in matched.captures {
        let stored_key = capture.key.try_clone()?;
        staged.set(capture.key, capture.value, limits, request, json_budget.as_deref_mut())?;
        capture_keys.push(stored_key);
    }
    Ok(())
}

pub(super) fn try_clone_keys(values: &[MatchValue]) -> Result<Vec<MatchValue>, RuntimeError> {
    let mut cloned = Vec::new();
    cloned
        .try_reserve_exact(values.len())
        .map_err(|_| RuntimeError::Allocation)?;
    for value in values {
        cloned.push(value.try_clone()?);
    }
    Ok(cloned)
}

pub(super) fn chain_branch_bytes(branch: &ChainBranch) -> Result<usize, RuntimeError> {
    branch.capture_keys.iter().try_fold(
        branch
            .capture_keys
            .len()
            .checked_mul(size_of::<MatchValue>())
            .and_then(|value| value.checked_add(branch.tx.retained_bytes))
            .ok_or(RuntimeError::Arithmetic)?,
        |total, key| total.checked_add(key.len()).ok_or(RuntimeError::Arithmetic),
    )
}

pub(super) fn member_match_bytes(matched: &MemberMatch) -> Result<usize, RuntimeError> {
    let capture_bytes = matched.captures.iter().try_fold(
        matched
            .captures
            .len()
            .checked_mul(size_of::<CaptureEntry>())
            .ok_or(RuntimeError::Arithmetic)?,
        |total, capture| {
            total
                .checked_add(capture.key.len())
                .and_then(|value| value.checked_add(capture.value.len()))
                .ok_or(RuntimeError::Arithmetic)
        },
    )?;
    capture_bytes
        .checked_add(matched.matched_var.as_ref().map_or(0, MatchValue::len))
        .and_then(|value| value.checked_add(matched.matched_var_name.as_ref().map_or(0, MatchValue::len)))
        .and_then(|value| value.checked_add(size_of::<MemberMatch>()))
        .ok_or(RuntimeError::Arithmetic)
}

pub(super) fn chain_branches_bytes(branches: &[ChainBranch]) -> Result<usize, RuntimeError> {
    branches.iter().try_fold(
        branches
            .len()
            .checked_mul(size_of::<ChainBranch>())
            .ok_or(RuntimeError::Arithmetic)?,
        |total, branch| {
            total
                .checked_add(chain_branch_bytes(branch)?)
                .ok_or(RuntimeError::Arithmetic)
        },
    )
}
