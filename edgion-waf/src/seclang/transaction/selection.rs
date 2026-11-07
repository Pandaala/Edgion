use super::*;

pub(super) fn matcher_program_scratch_bytes(generation: &CompiledGeneration) -> Result<usize, RuntimeError> {
    let mut total = 0_usize;
    for matcher in generation.all_matchers() {
        total = total
            .checked_add(
                matcher
                    .required_scratch_bytes()
                    .checked_sub(size_of::<MatchScratch>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .ok_or(RuntimeError::Arithmetic)?;
    }
    for program in &generation.boolean_regexes {
        total = total
            .checked_add(
                program
                    .required_scratch_bytes()
                    .checked_sub(size_of::<RegexBooleanScratch>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .ok_or(RuntimeError::Arithmetic)?;
    }
    for program in &generation.capture_regexes {
        total = total
            .checked_add(
                program
                    .required_scratch_bytes()
                    .checked_sub(size_of::<RegexCaptureScratch>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .ok_or(RuntimeError::Arithmetic)?;
    }
    Ok(total)
}

pub(super) fn matcher_scratch_capacity(
    program_bytes: usize,
    groups: usize,
    booleans: usize,
    captures: usize,
) -> Result<usize, RuntimeError> {
    let mut total = program_bytes;
    for (count, unit) in [
        (groups, size_of::<Option<MatchScratch>>()),
        (booleans, size_of::<Option<RegexBooleanScratch>>()),
        (captures, size_of::<Option<RegexCaptureScratch>>()),
    ] {
        total = total
            .checked_add(count.checked_mul(unit).ok_or(RuntimeError::Arithmetic)?)
            .ok_or(RuntimeError::Arithmetic)?;
    }
    Ok(total)
}
pub(super) fn lazy_group_scratch<'a>(
    slot: &'a mut Option<MatchScratch>,
    program: &crate::matcher::CompiledMatcher,
) -> &'a mut MatchScratch {
    slot.get_or_insert_with(|| {
        #[cfg(test)]
        measurements::record(Event::GroupScratchInitializations, 1);
        program.create_scratch()
    })
}
pub(super) fn lazy_boolean_scratch<'a>(
    slot: &'a mut Option<RegexBooleanScratch>,
    program: &crate::matcher::CompiledRegexProgram,
) -> &'a mut RegexBooleanScratch {
    slot.get_or_insert_with(|| {
        #[cfg(test)]
        measurements::record(Event::BooleanScratchInitializations, 1);
        program.create_scratch()
    })
}
pub(super) fn lazy_capture_scratch<'a>(
    slot: &'a mut Option<RegexCaptureScratch>,
    program: &crate::matcher::CompiledCaptureRegexProgram,
) -> &'a mut RegexCaptureScratch {
    slot.get_or_insert_with(|| {
        #[cfg(test)]
        measurements::record(Event::CaptureScratchInitializations, 1);
        program.create_scratch()
    })
}

pub(super) fn member_has_action(
    generation: &CompiledGeneration,
    member: super::super::CompiledMemberPlan,
    expected: super::super::super::ActionKind,
) -> Result<bool, RuntimeError> {
    let end = member
        .action_start
        .checked_add(member.action_count)
        .ok_or(RuntimeError::Arithmetic)?;
    Ok(generation
        .actions
        .get(member.action_start..end)
        .ok_or(RuntimeError::GenerationInvariant)?
        .iter()
        .any(|action| action.kind == super::super::ActionEnvelopeKind::Action(expected)))
}

#[derive(Clone, Copy)]
pub(super) struct JsonNoHitTail {
    pub(super) query_count: usize,
    pub(super) selected_count: usize,
    pub(super) selection_work: usize,
    pub(super) replay_work: usize,
    pub(super) logical_work: usize,
}

pub(super) struct SelectionParams {
    pub(super) unit_id: usize,
    pub(super) plan_id: usize,
    pub(super) limit: usize,
    pub(super) work_limit: usize,
    pub(super) include_json: bool,
}

/// Read-only selection state plus the selector regex scratch and JSON budget.
/// Built per call from disjoint borrows: the caller may pass a staged `TxState`
/// while `boolean_scratch` and `json_budget` are borrowed from the transaction.
pub(super) struct SelectionCtx<'a> {
    pub(super) generation: &'a CompiledGeneration,
    pub(super) request: &'a RequestState<'a>,
    pub(super) tx: &'a TxState,
    pub(super) response: Option<&'a ResponseState>,
    pub(super) scratch: &'a mut [Option<RegexBooleanScratch>],
    pub(super) json_budget: Option<&'a mut InspectionBudget>,
}

pub(super) fn select_values(
    ctx: &mut SelectionCtx<'_>,
    ctl_work: &mut usize,
    params: SelectionParams,
    selected: &mut Vec<SelectedValue>,
    storage: SelectionStorage,
) -> Result<(), RuntimeError> {
    let (generation, request, tx) = (ctx.generation, ctx.request, ctx.tx);
    let SelectionParams {
        unit_id,
        plan_id,
        limit,
        work_limit,
        include_json,
    } = params;
    let plan = *generation
        .target_plans
        .get(plan_id)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let end = plan
        .record_start
        .checked_add(plan.record_count)
        .ok_or(RuntimeError::Arithmetic)?;
    let records = generation
        .target_records
        .get(plan.record_start..end)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let exclude_start = tx.target_exclusions.partition_point(|entry| entry.unit < unit_id);
    let exclude_end =
        exclude_start + tx.target_exclusions[exclude_start..].partition_point(|entry| entry.unit <= unit_id);
    let exclusions = &tx.target_exclusions[exclude_start..exclude_end];
    for target in records {
        let json_count = if target.modifier == super::super::TargetModifier::Count
            && target.selector == super::super::CompiledTargetSelector::None
            && exclusions.is_empty()
            && matches!(target.collection, CollectionKind::Args | CollectionKind::ArgsNames)
        {
            request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::json)
                .map(JsonPublication::argument_count)
        } else {
            None
        };
        let mut candidates = CandidateCursor::new(target.collection, include_json && json_count.is_none());
        match target.modifier {
            super::super::TargetModifier::None => {
                while let Some(candidate) =
                    candidates.next(ctx.request, ctx.tx, ctx.response, ctx.json_budget.as_deref_mut())?
                {
                    if selector_matches(ctx, target.collection, target.selector, candidate)?
                        && !ctl_excludes_value(ctx, exclusions, candidate, ctl_work, work_limit)?
                    {
                        if selected.len() == limit {
                            return Err(RuntimeError::SelectedValueLimit { limit });
                        }
                        storage.reserve(selected, limit)?;
                        selected.push(candidate);
                    }
                }
            }
            super::super::TargetModifier::Exclude => {
                let mut write = 0_usize;
                for read in 0..selected.len() {
                    let candidate = selected[read];
                    let remove = candidate.collection == target.collection
                        && selector_matches(ctx, target.collection, target.selector, candidate)?;
                    if !remove {
                        selected[write] = candidate;
                        write += 1;
                    }
                }
                selected.truncate(write);
            }
            super::super::TargetModifier::Count => {
                let mut count = json_count.unwrap_or(0);
                while let Some(candidate) =
                    candidates.next(ctx.request, ctx.tx, ctx.response, ctx.json_budget.as_deref_mut())?
                {
                    if selector_matches(ctx, target.collection, target.selector, candidate)?
                        && !ctl_excludes_value(ctx, exclusions, candidate, ctl_work, work_limit)?
                    {
                        count = count.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
                    }
                }
                if selected.len() == limit {
                    return Err(RuntimeError::SelectedValueLimit { limit });
                }
                let (digits, start) = decimal_bytes(count);
                storage.reserve(selected, limit)?;
                selected.push(SelectedValue {
                    collection: target.collection,
                    name: None,
                    value: SelectedBytes::Count {
                        digits,
                        start: u8::try_from(start).map_err(|_| RuntimeError::Arithmetic)?,
                    },
                });
            }
        }
    }
    Ok(())
}

fn ctl_excludes_value(
    ctx: &mut SelectionCtx<'_>,
    exclusions: &[ActiveTargetExclusion],
    candidate: SelectedValue,
    work: &mut usize,
    limit: usize,
) -> Result<bool, RuntimeError> {
    for exclusion in exclusions {
        *work = work.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
        if *work > limit {
            return Err(RuntimeError::OperatorWorkLimit { limit });
        }
        if exclusion.collection != candidate.collection {
            continue;
        }
        *work = work
            .checked_add(
                candidate
                    .name_bytes(ctx.request, ctx.tx, ctx.response, ctx.json_budget.as_deref_mut())?
                    .map_or(1, |name| name.len().saturating_add(1)),
            )
            .ok_or(RuntimeError::Arithmetic)?;
        if *work > limit {
            return Err(RuntimeError::OperatorWorkLimit { limit });
        }
        if selector_matches(ctx, exclusion.collection, exclusion.selector, candidate)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Clone, Copy)]
pub(super) struct SelectionStorage {
    pub(super) capacity_bytes: usize,
    pub(super) retained_limit: usize,
}

impl SelectionStorage {
    /// Simulate complete geometric selection growth with conservative realloc
    /// overlap admission. Near-limit uncertainty falls back to actual selection.
    pub(super) fn summary_capacity(self, mut capacity: usize, count: usize, limit: usize) -> Option<usize> {
        let maximum = limit.min(self.capacity_bytes / size_of::<SelectedValue>());
        while capacity < count {
            if capacity >= maximum {
                return None;
            }
            let target = capacity.checked_mul(2)?.max(4).min(maximum);
            let overlap = capacity.checked_add(target)?.checked_mul(size_of::<SelectedValue>())?;
            if target <= capacity || overlap > self.capacity_bytes {
                return None;
            }
            capacity = target;
        }
        let bytes = capacity.checked_mul(size_of::<SelectedValue>())?;
        (bytes <= self.capacity_bytes).then_some(bytes)
    }

    fn reserve(self, selected: &mut Vec<SelectedValue>, limit: usize) -> Result<(), RuntimeError> {
        if selected.len() < selected.capacity() {
            return Ok(());
        }
        let maximum = limit.min(self.capacity_bytes / size_of::<SelectedValue>());
        if selected.len() >= maximum {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.retained_limit,
            });
        }
        let target = selected.capacity().saturating_mul(2).max(4).min(maximum);
        selected
            .try_reserve_exact(target - selected.len())
            .map_err(|_| RuntimeError::Allocation)?;
        if selected
            .capacity()
            .checked_mul(size_of::<SelectedValue>())
            .ok_or(RuntimeError::Arithmetic)?
            > self.capacity_bytes
        {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.retained_limit,
            });
        }
        Ok(())
    }
}

struct CandidateCursor {
    collection: CollectionKind,
    include_json: bool,
    stage: u8,
    index: usize,
    next_json: Option<CandidateId>,
}

impl CandidateCursor {
    fn new(collection: CollectionKind, include_json: bool) -> Self {
        Self {
            collection,
            include_json,
            stage: 0,
            index: 0,
            next_json: None,
        }
    }

    fn next(
        &mut self,
        request: &RequestState,
        tx: &TxState,
        response: Option<&ResponseState>,
        json_budget: Option<&mut InspectionBudget>,
    ) -> Result<Option<SelectedValue>, RuntimeError> {
        let collection = self.collection;
        let selected = |name, value| {
            Some(SelectedValue {
                collection,
                name,
                value,
            })
        };
        let index = self.index;
        match collection {
            CollectionKind::ReqbodyProcessor if request.body.is_none() && tx.request_body_processor.is_some() => {
                self.index = 1;
                Ok((index == 0).then_some(SelectedValue {
                    collection,
                    name: None,
                    value: SelectedBytes::CtlProcessor,
                }))
            }
            CollectionKind::Tx | CollectionKind::MatchedVars => {
                let count = if collection == CollectionKind::Tx {
                    tx.entries.len()
                } else {
                    tx.matched_vars.len()
                };
                if index == count {
                    return Ok(None);
                }
                self.index += 1;
                Ok(if collection == CollectionKind::Tx {
                    selected(Some(SelectedName::Tx(index)), SelectedBytes::Tx(index))
                } else {
                    selected(
                        Some(SelectedName::MatchedVars(index)),
                        SelectedBytes::MatchedVars(index),
                    )
                })
            }
            CollectionKind::MatchedVar => {
                self.index = 1;
                Ok((index == 0 && tx.matched_var.is_some()).then_some(SelectedValue {
                    collection,
                    name: None,
                    value: SelectedBytes::MatchedVar,
                }))
            }
            CollectionKind::ResponseHeaders | CollectionKind::ResponseStatus => {
                if let Some(response) = response {
                    while let Some(entry) = response.entries.get(self.index) {
                        let index = self.index;
                        self.index += 1;
                        if entry.collection == collection {
                            return Ok(selected(
                                entry.name.as_ref().map(|_| SelectedName::Response(index)),
                                SelectedBytes::Response(index),
                            ));
                        }
                    }
                }
                Ok(None)
            }
            CollectionKind::ResponseBody => {
                self.index = 1;
                Ok(
                    (index == 0 && response.and_then(|response| response.window.as_ref()).is_some()).then_some(
                        SelectedValue {
                            collection,
                            name: None,
                            value: SelectedBytes::ResponseWindow,
                        },
                    ),
                )
            }
            _ => {
                if self.stage == 0 {
                    let overrides = matches!(
                        collection,
                        CollectionKind::ArgsCombinedSize | CollectionKind::ReqbodyProcessor
                    ) && request.body.is_some();
                    let records = request.collection_records(collection)?;
                    if !overrides {
                        if let Some(candidate) = records.get(self.index) {
                            self.index += 1;
                            return Ok(selected(
                                candidate.name.map(SelectedName::Request),
                                SelectedBytes::Request(candidate.value),
                            ));
                        }
                    }
                    self.stage = 1;
                    self.index = 0;
                    self.next_json = request
                        .body
                        .as_ref()
                        .and_then(RequestBodyPublication::structured_results)
                        .and_then(|results| results.receipt)
                        .map(|receipt| receipt.head);
                }
                if self.stage == 1 {
                    if let Some(body) = request.body.as_ref().and_then(RequestBodyPublication::xml) {
                        if self.include_json && collection == CollectionKind::Xml {
                            if let Some(id) = self.next_json {
                                let results = body.results.as_ref().ok_or(RuntimeError::XmlInspectionState)?;
                                if results.domain() != InspectionDomain::Xml {
                                    return Err(RuntimeError::XmlInspectionState);
                                }
                                let candidate = results
                                    .candidate(json_budget.ok_or(RuntimeError::XmlInspectionState)?, id)
                                    .map_err(map_store_error)
                                    .map_err(xml_inspection_runtime::xml_error)?;
                                self.next_json = candidate.next;
                                return match candidate.kind {
                                    CandidateKind::XmlText => Ok(selected(None, SelectedBytes::XmlText(id))),
                                    CandidateKind::XmlAttribute => {
                                        Ok(selected(Some(SelectedName::Xml(id)), SelectedBytes::XmlAttribute(id)))
                                    }
                                    _ => Err(RuntimeError::XmlInspectionState),
                                };
                            }
                        } else if body.metadata_value(collection).is_some() {
                            self.stage = 2;
                            return Ok(selected(None, SelectedBytes::XmlMetadata));
                        }
                    }
                    if let Some(body) = request.body.as_ref().and_then(RequestBodyPublication::json) {
                        if self.include_json && matches!(collection, CollectionKind::Args | CollectionKind::ArgsNames) {
                            if let Some(id) = self.next_json {
                                let results = body.results.as_ref().ok_or(RuntimeError::JsonInspectionState)?;
                                let budget = json_budget.ok_or(RuntimeError::JsonInspectionState)?;
                                self.next_json = results.candidate(budget, id).map_err(map_store_error)?.next;
                                return Ok(selected(Some(SelectedName::Json(id)), SelectedBytes::Json(id)));
                            }
                        } else if body.metadata_value(collection).is_some() {
                            self.stage = 2;
                            return Ok(selected(None, SelectedBytes::JsonMetadata));
                        }
                    }
                    self.stage = 2;
                }
                if let Some(body) = request.body.as_ref().and_then(RequestBodyPublication::materialized) {
                    while let Some(entry) = body.entries.get(self.index) {
                        let index = self.index;
                        self.index += 1;
                        if entry.collection == collection {
                            return Ok(selected(
                                entry.name.as_ref().map(|_| SelectedName::Body(index)),
                                SelectedBytes::Body(index),
                            ));
                        }
                    }
                }
                Ok(None)
            }
        }
    }
}

fn selector_matches(
    ctx: &mut SelectionCtx<'_>,
    collection: CollectionKind,
    selector: super::super::CompiledTargetSelector,
    candidate: SelectedValue,
) -> Result<bool, RuntimeError> {
    let (generation, request, tx, response) = (ctx.generation, ctx.request, ctx.tx, ctx.response);
    match selector {
        super::super::CompiledTargetSelector::None => Ok(true),
        super::super::CompiledTargetSelector::Literal(expected) => {
            let expected = generation_bytes(&generation.target_bytes, expected)?;
            let name = candidate.name_bytes(request, tx, response, ctx.json_budget.as_deref_mut())?;
            let Some(name) = name else {
                return Ok(false);
            };
            Ok(
                if matches!(
                    collection,
                    CollectionKind::RequestHeaders
                        | CollectionKind::RequestHeadersNames
                        | CollectionKind::ResponseHeaders
                        | CollectionKind::Tx
                ) {
                    name.eq_ignore_ascii_case(expected)
                } else {
                    name == expected
                },
            )
        }
        super::super::CompiledTargetSelector::Path(path) => {
            let path = generation_bytes(&generation.target_bytes, path)?;
            if matches!(
                candidate.value,
                SelectedBytes::XmlText(_) | SelectedBytes::XmlAttribute(_)
            ) {
                return match path {
                    b"/*" => Ok(matches!(candidate.value, SelectedBytes::XmlText(_))),
                    b"//@*" => Ok(matches!(candidate.value, SelectedBytes::XmlAttribute(_))),
                    _ => Err(RuntimeError::XmlSelectorUnavailable),
                };
            }
            let SelectedBytes::Body(index) = candidate.value else {
                return Ok(false);
            };
            let entry = request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::materialized)
                .and_then(|body| body.entries.get(index))
                .ok_or(RuntimeError::GenerationInvariant)?;
            if entry.collection != CollectionKind::Xml {
                return Ok(false);
            }
            match path {
                b"/*" => Ok(!entry.xml_attribute),
                b"//@*" => Ok(entry.xml_attribute),
                _ => Err(RuntimeError::XmlSelectorUnavailable),
            }
        }
        super::super::CompiledTargetSelector::Regex { program, .. } => {
            let name = candidate.name_bytes(request, tx, response, ctx.json_budget.as_deref_mut())?;
            let Some(name) = name else {
                return Ok(false);
            };
            let regex = generation
                .boolean_regexes
                .get(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            let slot = ctx.scratch.get_mut(program).ok_or(RuntimeError::GenerationInvariant)?;
            let scratch = lazy_boolean_scratch(slot, regex);
            regex.is_match(name, scratch).map_err(map_regex_scratch_error)
        }
    }
}

pub(super) fn charge_operator_work(work: &mut usize, added: usize, limits: RuntimeLimits) -> Result<(), RuntimeError> {
    *work = work.checked_add(added).ok_or(RuntimeError::Arithmetic)?;
    if *work > limits.max_operator_work_bytes {
        return Err(RuntimeError::OperatorWorkLimit {
            limit: limits.max_operator_work_bytes,
        });
    }
    Ok(())
}
