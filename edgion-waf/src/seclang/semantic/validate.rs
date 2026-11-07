use super::*;

const NO_INDEX: usize = usize::MAX;

#[derive(Debug, Clone, Copy, Default)]
struct ExceptionCounters {
    raw: usize,
    distinct: usize,
    selector_work: usize,
    comparisons: usize,
    comparison_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExceptionKind {
    Update,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnavailableRulePolicy {
    Reject,
    ExcludeRuleUnits,
}

pub fn validate_semantic_draft<'source>(
    draft: SemanticDraft<'source>,
    context: SemanticValidationContext,
    limits: ValidationLimits,
) -> Result<ValidatedSemanticDocument<'source>, SemanticValidationError> {
    validate_semantic_draft_inner(draft, context, limits, UnavailableRulePolicy::Reject)
}

pub(crate) fn validate_executable_profile<'source>(
    draft: SemanticDraft<'source>,
    context: SemanticValidationContext,
    limits: ValidationLimits,
) -> Result<ValidatedSemanticDocument<'source>, SemanticValidationError> {
    validate_semantic_draft_inner(draft, context, limits, UnavailableRulePolicy::ExcludeRuleUnits)
}

fn validate_semantic_draft_inner<'source>(
    draft: SemanticDraft<'source>,
    context: SemanticValidationContext,
    limits: ValidationLimits,
    unavailable_policy: UnavailableRulePolicy,
) -> Result<ValidatedSemanticDocument<'source>, SemanticValidationError> {
    let rule_units = draft
        .final_rules
        .iter()
        .filter(|rule| !matches!(rule.role, FinalRuleRole::ChainChild { .. }))
        .count();
    if rule_units > limits.max_rule_units {
        return Err(SemanticValidationError::RuleUnitLimit {
            limit: limits.max_rule_units,
        });
    }
    let id_selectors = draft
        .documents
        .iter()
        .try_fold(0_usize, |count, document| {
            count.checked_add(document.id_selector_count())
        })
        .ok_or(SemanticValidationError::IdSelectorLimit {
            limit: limits.max_id_selectors,
        })?;
    if id_selectors > limits.max_id_selectors {
        return Err(SemanticValidationError::IdSelectorLimit {
            limit: limits.max_id_selectors,
        });
    }
    let (exception_count, update_count, removal_count, overlay_bound) =
        validation_exception_shape(&draft, rule_units, limits)?;
    let nodes = draft.final_rules.len();
    let targets = draft.targets.len();
    let distinct_bound = rule_units.min(limits.max_distinct_matches_per_directive);
    let working = validation_working_bytes(
        rule_units,
        nodes,
        targets,
        distinct_bound,
        overlay_bound,
        unavailable_policy == UnavailableRulePolicy::ExcludeRuleUnits,
        limits,
    )?;
    if working > limits.max_working_bytes {
        return Err(SemanticValidationError::WorkingByteLimit {
            limit: limits.max_working_bytes,
        });
    }

    let mut id_index = Vec::new();
    let mut active = Vec::new();
    let mut states = Vec::new();
    let mut overlay_heads = Vec::new();
    let mut overlay_tails = Vec::new();
    let mut final_counts = Vec::new();
    let mut overlays = Vec::new();
    let mut seen = Vec::new();
    let mut affected = Vec::new();
    let mut target_seen = Vec::new();
    let mut regex_targets = Vec::new();
    let mut fold_units = Vec::new();
    let mut compatibility_excluded = Vec::new();
    reserve_validation(&mut id_index, rule_units, "rule ID index")?;
    reserve_validation(&mut active, rule_units, "active rule state")?;
    reserve_validation(&mut states, nodes, "resolved rule states")?;
    reserve_validation(&mut overlay_heads, nodes, "target overlay heads")?;
    reserve_validation(&mut overlay_tails, nodes, "target overlay tails")?;
    reserve_validation(&mut final_counts, nodes, "final target counts")?;
    reserve_validation(&mut overlays, overlay_bound, "target overlays")?;
    reserve_validation(&mut seen, rule_units, "selector generations")?;
    reserve_validation(&mut affected, distinct_bound, "affected rule candidates")?;
    reserve_validation(&mut target_seen, targets, "target requirement generations")?;
    reserve_validation(&mut regex_targets, targets, "target regex candidates")?;
    if unavailable_policy == UnavailableRulePolicy::ExcludeRuleUnits {
        reserve_validation(&mut fold_units, draft.folds.len(), "capability fold attribution")?;
        reserve_validation(&mut compatibility_excluded, rule_units, "compatibility-excluded units")?;
        fold_units.resize(draft.folds.len(), NO_INDEX);
        compatibility_excluded.resize(rule_units, 0_u8);
    }
    active.resize(rule_units, 1_u8);
    overlay_heads.resize(nodes, NO_INDEX);
    overlay_tails.resize(nodes, NO_INDEX);
    seen.resize(rule_units, 0_u32);
    target_seen.resize(targets, 0_u32);

    for (node, rule) in draft.final_rules.iter().copied().enumerate() {
        if !matches!(rule.role, FinalRuleRole::ChainChild { .. }) {
            id_index.push(RuleIdIndexRecord {
                id: draft.folds[rule.fold].id.expect("finalized rule unit has an ID"),
                unit: node,
            });
        }
    }
    id_index.sort_unstable_by_key(|entry| entry.id);
    reject_duplicate_ids(&draft, &id_index)?;
    resolve_rule_states(&draft, context, &mut states)?;

    let mut unit = 0_usize;
    let mut current_unit = NO_INDEX;
    let mut total_final_targets = 0_usize;
    for (node, state) in states.iter_mut().enumerate() {
        if !matches!(draft.final_rules[node].role, FinalRuleRole::ChainChild { .. }) {
            state.unit = Some(unit);
            current_unit = unit;
            unit += 1;
        }
        if !fold_units.is_empty() {
            fold_units[draft.final_rules[node].fold] = current_unit;
        }
        let mapping = semantic_mapping_for_rule(&draft, draft.final_rules[node]);
        let count = mapping.target_count;
        let location = draft.folds[draft.final_rules[node].fold].location;
        if count > limits.max_final_targets_per_rule {
            return Err(SemanticValidationError::FinalTargetLimit {
                limit: limits.max_final_targets_per_rule,
                location,
            });
        }
        total_final_targets =
            total_final_targets
                .checked_add(count)
                .ok_or(SemanticValidationError::FinalTargetReferenceLimit {
                    limit: limits.max_final_target_references,
                    location,
                })?;
        if total_final_targets > limits.max_final_target_references {
            return Err(SemanticValidationError::FinalTargetReferenceLimit {
                limit: limits.max_final_target_references,
                location,
            });
        }
        for target in &draft.targets[mapping.target_start..mapping.target_start + mapping.target_count] {
            validate_target(target, state.phase)?;
        }
        final_counts.push(count);
    }

    let mut counters = ExceptionCounters::default();
    let mut affected_total = 0_usize;
    let mut directive_ordinal = 0_u32;
    for mapping in &draft.directives {
        let syntax = draft.documents[mapping.document_index]
            .get(mapping.directive_index)
            .expect("validated semantic directive mapping");
        let (kind, ids, update_targets) = match syntax.targets() {
            DirectiveTargetView::Update { ids, .. } => (
                ExceptionKind::Update,
                ids,
                Some(mapping.target_start..mapping.target_start + mapping.target_count),
            ),
            DirectiveTargetView::Remove { ids } => (ExceptionKind::Remove, ids, None),
            DirectiveTargetView::None | DirectiveTargetView::Rule { .. } => continue,
        };
        directive_ordinal += 1;
        let location = DirectiveLocation {
            document_index: mapping.document_index,
            directive_index: mapping.directive_index,
        };
        affected.clear();
        let local = resolve_exception_union(
            &draft,
            &id_index,
            &states,
            ids,
            location,
            directive_ordinal,
            &mut seen,
            &mut affected,
            counters,
            limits,
        )?;
        match kind {
            ExceptionKind::Update => {
                if affected
                    .iter()
                    .any(|root| active[states[*root].unit.expect("affected root has unit")] == 0)
                {
                    return Err(SemanticValidationError::InactiveUpdate { location });
                }
            }
            ExceptionKind::Remove => {
                affected.retain(|root| active[states[*root].unit.expect("affected root has unit")] != 0);
                if affected.is_empty() {
                    return Err(SemanticValidationError::EmptyRemoval { location });
                }
            }
        }
        if let Some(target_range) = update_targets {
            let comparison_delta = validate_update_candidate(
                &draft,
                &states,
                &affected,
                target_range.clone(),
                location,
                &final_counts,
                &overlay_heads,
                &overlays,
                counters,
                limits,
                total_final_targets,
            )?;
            let added = target_range
                .len()
                .checked_mul(affected.len())
                .expect("validated update projection remains representable");
            counters.raw = counters
                .raw
                .checked_add(local.raw)
                .expect("validated raw count remains representable");
            counters.distinct = counters
                .distinct
                .checked_add(local.distinct)
                .expect("validated distinct count remains representable");
            counters.selector_work = counters
                .selector_work
                .checked_add(local.selector_work)
                .expect("validated selector work remains representable");
            counters.comparisons = counters
                .comparisons
                .checked_add(comparison_delta.comparisons)
                .expect("validated comparison count remains representable");
            counters.comparison_bytes = counters
                .comparison_bytes
                .checked_add(comparison_delta.comparison_bytes)
                .expect("validated comparison bytes remain representable");
            for &matched in &affected {
                let root = matched;
                for target in target_range.clone() {
                    let index = overlays.len();
                    overlays.push(TargetOverlayEntry { target, next: NO_INDEX });
                    if overlay_heads[root] == NO_INDEX {
                        overlay_heads[root] = index;
                    } else {
                        overlays[overlay_tails[root]].next = index;
                    }
                    overlay_tails[root] = index;
                }
                final_counts[root] = final_counts[root]
                    .checked_add(target_range.len())
                    .expect("validated per-rule target count remains representable");
            }
            total_final_targets = total_final_targets
                .checked_add(added)
                .expect("validated target reference count remains representable");
        } else {
            counters.raw = counters
                .raw
                .checked_add(local.raw)
                .expect("validated raw count remains representable");
            counters.distinct = counters
                .distinct
                .checked_add(local.distinct)
                .expect("validated distinct count remains representable");
            counters.selector_work = counters
                .selector_work
                .checked_add(local.selector_work)
                .expect("validated selector work remains representable");
            for &matched in &affected {
                active[states[matched].unit.expect("affected root has unit")] = 0;
            }
        }
        affected_total = affected_total
            .checked_add(affected.len())
            .ok_or(SemanticValidationError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
            })?;
    }

    if unavailable_policy == UnavailableRulePolicy::Reject {
        if let Some(record) = draft.unavailable.first() {
            return Err(SemanticValidationError::UnavailableCapability {
                capability: record.capability,
                location: record.location,
            });
        }
    } else {
        for record in &draft.unavailable {
            let unit = capability_unit(&draft, &fold_units, record.location).ok_or(
                SemanticValidationError::UnavailableCapability {
                    capability: record.capability,
                    location: record.location,
                },
            )?;
            compatibility_excluded[unit] = 1;
            active[unit] = 0;
        }
    }

    mark_reachable_regex_targets(
        &draft,
        &states,
        &active,
        &overlay_heads,
        &overlays,
        &mut target_seen,
        &mut regex_targets,
    );
    let draft_requirement_count = draft
        .requirements
        .iter()
        .filter(|record| {
            !is_default_requirement(record.requirement)
                && !requirement_is_excluded(&draft, &fold_units, &compatibility_excluded, record.location)
        })
        .count();
    let requirement_count =
        draft_requirement_count
            .checked_add(regex_targets.len())
            .ok_or(SemanticValidationError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
            })?;
    let validation_charge = validation_owned_bytes(
        nodes,
        rule_units,
        total_final_targets,
        update_count,
        removal_count,
        affected_total,
        requirement_count,
        limits,
    )?;
    if validation_charge > limits.max_owned_bytes {
        return Err(SemanticValidationError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
        });
    }
    if draft
        .retained_charged_bytes()
        .checked_add(validation_charge)
        .is_none_or(|value| value > limits.max_retained_bytes)
    {
        return Err(SemanticValidationError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        });
    }

    let mut validated = ValidatedSemanticDocument {
        draft,
        nodes: Vec::new(),
        units: Vec::new(),
        id_index: Vec::new(),
        final_targets: Vec::new(),
        updates: Vec::new(),
        removals: Vec::new(),
        affected_units: Vec::new(),
        requirements: Vec::new(),
        validation_charged_bytes: validation_charge,
    };
    reserve_validation(&mut validated.nodes, nodes, "validated rule nodes")?;
    reserve_validation(&mut validated.units, rule_units, "validated rule units")?;
    reserve_validation(&mut validated.id_index, rule_units, "validated rule ID index")?;
    reserve_validation(
        &mut validated.final_targets,
        total_final_targets,
        "final target references",
    )?;
    reserve_validation(&mut validated.updates, update_count, "applied target updates")?;
    reserve_validation(&mut validated.removals, removal_count, "applied rule removals")?;
    reserve_validation(&mut validated.affected_units, affected_total, "applied exception units")?;
    reserve_validation(&mut validated.requirements, requirement_count, "validated requirements")?;
    commit_validated_nodes(&mut validated, &states, &overlay_heads, &overlays);
    for (node, rule) in validated.draft.final_rules.iter().copied().enumerate() {
        if matches!(rule.role, FinalRuleRole::ChainChild { .. }) {
            continue;
        }
        let fold = validated.draft.folds[rule.fold];
        let unit = states[node].unit.expect("root state retains unit index");
        debug_assert_eq!(unit, validated.units.len());
        validated.units.push(ValidatedRuleUnitRecord {
            id: fold.id.expect("validated unit ID"),
            root: node,
            active: active[unit] != 0,
            compatibility_excluded: compatibility_excluded.get(unit).is_some_and(|value| *value != 0),
        });
    }
    commit_applied_exceptions(
        &mut validated,
        &states,
        &id_index,
        limits,
        &mut active,
        &mut seen,
        &mut affected,
    )?;
    for mut entry in id_index {
        entry.unit = states[entry.unit].unit.expect("ID root maps to a unit");
        validated.id_index.push(entry);
    }
    for (index, record) in validated.draft.requirements.iter().enumerate() {
        if !is_default_requirement(record.requirement)
            && !requirement_is_excluded(&validated.draft, &fold_units, &compatibility_excluded, record.location)
        {
            validated.requirements.push(ValidatedRequirementReference::Draft(index));
        }
    }
    for target in regex_targets {
        validated
            .requirements
            .push(ValidatedRequirementReference::TargetSelectorRegex { target });
    }
    debug_assert_eq!(exception_count, update_count + removal_count);
    Ok(validated)
}

pub(super) fn validation_exception_shape(
    draft: &SemanticDraft<'_>,
    units: usize,
    limits: ValidationLimits,
) -> Result<(usize, usize, usize, usize), SemanticValidationError> {
    let mut exceptions = 0_usize;
    let mut updates = 0_usize;
    let mut removals = 0_usize;
    let mut sum = 0_usize;
    let mut maximum = 0_usize;
    let distinct = units.min(limits.max_distinct_matches_per_directive);
    for document in &draft.documents {
        for directive in document.iter() {
            match directive.targets() {
                DirectiveTargetView::Update { atoms, .. } => {
                    exceptions = exceptions
                        .checked_add(1)
                        .ok_or(SemanticValidationError::ExceptionLimit {
                            limit: limits.max_applied_exceptions,
                        })?;
                    updates += 1;
                    maximum = maximum.max(atoms.len());
                    sum = sum
                        .checked_add(atoms.len().checked_mul(distinct).ok_or(
                            SemanticValidationError::WorkingByteLimit {
                                limit: limits.max_working_bytes,
                            },
                        )?)
                        .ok_or(SemanticValidationError::WorkingByteLimit {
                            limit: limits.max_working_bytes,
                        })?;
                }
                DirectiveTargetView::Remove { .. } => {
                    exceptions = exceptions
                        .checked_add(1)
                        .ok_or(SemanticValidationError::ExceptionLimit {
                            limit: limits.max_applied_exceptions,
                        })?;
                    removals += 1;
                }
                DirectiveTargetView::None | DirectiveTargetView::Rule { .. } => {}
            }
        }
    }
    let limit = limits.max_applied_exceptions.min(u32::MAX as usize);
    if exceptions > limit {
        return Err(SemanticValidationError::ExceptionLimit { limit });
    }
    let cumulative =
        maximum
            .checked_mul(limits.max_distinct_matches)
            .ok_or(SemanticValidationError::WorkingByteLimit {
                limit: limits.max_working_bytes,
            })?;
    Ok((exceptions, updates, removals, sum.min(cumulative)))
}

fn capability_unit(draft: &SemanticDraft<'_>, fold_units: &[usize], location: SemanticLocation) -> Option<usize> {
    let mapping = draft
        .directives
        .binary_search_by_key(&(location.document_index, location.directive_index), |record| {
            (record.document_index, record.directive_index)
        })
        .ok()
        .and_then(|index| draft.directives.get(index))?;
    let fold = mapping.fold?;
    let unit = *fold_units.get(fold)?;
    (unit != NO_INDEX).then_some(unit)
}

fn requirement_is_excluded(
    draft: &SemanticDraft<'_>,
    fold_units: &[usize],
    excluded: &[u8],
    location: SemanticLocation,
) -> bool {
    capability_unit(draft, fold_units, location)
        .and_then(|unit| excluded.get(unit))
        .is_some_and(|value| *value != 0)
}

pub(super) fn validation_working_bytes(
    units: usize,
    rules: usize,
    targets: usize,
    distinct_bound: usize,
    overlay_bound: usize,
    attribute_capabilities: bool,
    limits: ValidationLimits,
) -> Result<usize, SemanticValidationError> {
    let mut total = 0_usize;
    for (count, size) in [
        (units, size_of::<RuleIdIndexRecord>()),
        (units, size_of::<u8>()),
        (rules, size_of::<ResolvedRuleStateRecord>()),
        (rules, 2 * size_of::<usize>()),
        (rules, size_of::<usize>()),
        (overlay_bound, size_of::<TargetOverlayEntry>()),
        (units, size_of::<u32>()),
        (distinct_bound, size_of::<usize>()),
        (targets, size_of::<u32>()),
        (targets, size_of::<usize>()),
        (usize::from(attribute_capabilities) * rules, size_of::<usize>()),
        (usize::from(attribute_capabilities) * units, size_of::<u8>()),
    ] {
        total = total
            .checked_add(
                count
                    .checked_mul(size)
                    .ok_or(SemanticValidationError::WorkingByteLimit {
                        limit: limits.max_working_bytes,
                    })?,
            )
            .ok_or(SemanticValidationError::WorkingByteLimit {
                limit: limits.max_working_bytes,
            })?;
    }
    Ok(total)
}

fn reserve_validation<T>(
    values: &mut Vec<T>,
    count: usize,
    operation: &'static str,
) -> Result<(), SemanticValidationError> {
    #[cfg(test)]
    if VALIDATION_ALLOCATION_FAILURE.with(|failure| failure.get() == Some(operation)) {
        return Err(SemanticValidationError::Allocation { operation });
    }
    values
        .try_reserve_exact(count)
        .map_err(|_| SemanticValidationError::Allocation { operation })
}

fn reject_duplicate_ids(draft: &SemanticDraft<'_>, index: &[RuleIdIndexRecord]) -> Result<(), SemanticValidationError> {
    let mut duplicate = None;
    let mut cursor = 0_usize;
    while cursor < index.len() {
        let mut end = cursor + 1;
        while end < index.len() && index[end].id == index[cursor].id {
            end += 1;
        }
        if end - cursor > 1 {
            let mut first = None;
            let mut second = None;
            for entry in &index[cursor..end] {
                let location = draft.folds[draft.final_rules[entry.unit].fold].location;
                if first.is_none_or(|value| location < value) {
                    second = first;
                    first = Some(location);
                } else if second.is_none_or(|value| location < value) {
                    second = Some(location);
                }
            }
            let candidate = (first.expect("duplicate group"), second.expect("duplicate group"));
            if duplicate.is_none_or(|(_, current)| candidate.1 < current) {
                duplicate = Some(candidate);
            }
        }
        cursor = end;
    }
    if let Some((first, second)) = duplicate {
        Err(SemanticValidationError::DuplicateRuleId { first, second })
    } else {
        Ok(())
    }
}

fn resolve_rule_states(
    draft: &SemanticDraft<'_>,
    context: SemanticValidationContext,
    states: &mut Vec<ResolvedRuleStateRecord>,
) -> Result<(), SemanticValidationError> {
    let mut inherited: Option<ResolvedRuleStateRecord> = None;
    for rule in draft.final_rules.iter().copied() {
        let fold = draft.folds[rule.fold];
        let state = if matches!(rule.role, FinalRuleRole::ChainChild { .. }) {
            let mut value = inherited.expect("validated chain child follows its root");
            value.unit = None;
            value
        } else {
            let phase = match rule.phase {
                FinalPhaseState::Explicit { phase, .. } | FinalPhaseState::Inherited { phase, .. } => phase,
                FinalPhaseState::RequiresDefault { owner } => context
                    .implicit_rule_phase()
                    .ok_or(SemanticValidationError::MissingImplicitPhase { location: owner })?,
            };
            let defaults = context
                .phase_defaults(phase)
                .ok_or(SemanticValidationError::MissingPhaseDefaults {
                    phase,
                    location: fold.location,
                })?;
            let (disruptive, status) = match fold.disruptive.expect("root fold has disruptive behavior") {
                DisruptiveKind::Pass => (ResolvedDisruptive::Pass, None),
                DisruptiveKind::Deny => {
                    let default_status = match defaults.disruptive() {
                        DefaultDisruptive::Pass => None,
                        DefaultDisruptive::Deny { status } => Some(status),
                    };
                    (
                        ResolvedDisruptive::Deny,
                        Some(fold.status.or(default_status).unwrap_or(403)),
                    )
                }
                DisruptiveKind::Block => match defaults.disruptive() {
                    DefaultDisruptive::Pass => {
                        if let Some(action) = fold.status_action {
                            return Err(SemanticValidationError::BlockStatusPass {
                                location: draft.actions[action].location,
                            });
                        }
                        (ResolvedDisruptive::Pass, None)
                    }
                    DefaultDisruptive::Deny { status } => {
                        (ResolvedDisruptive::Deny, Some(fold.status.unwrap_or(status)))
                    }
                },
            };
            ResolvedRuleStateRecord {
                phase,
                disruptive,
                status,
                log: fold.log.unwrap_or(defaults.log()),
                audit_log: fold.audit_log.unwrap_or(defaults.audit_log()),
                unit: None,
            }
        };
        if !matches!(rule.role, FinalRuleRole::ChainChild { .. }) {
            inherited = Some(state);
        }
        states.push(state);
    }
    Ok(())
}

pub(super) fn semantic_mapping_for_rule<'a>(
    draft: &'a SemanticDraft<'_>,
    rule: FinalRuleRecord,
) -> &'a DirectiveSemanticRecord {
    let location = draft.folds[rule.fold].location;
    let index = draft
        .directives
        .binary_search_by_key(&(location.document_index, location.directive_index), |record| {
            (record.document_index, record.directive_index)
        })
        .expect("final rule location has semantic directive mapping");
    &draft.directives[index]
}

fn validate_target(target: &SemanticTarget<'_>, phase: Phase) -> Result<(), SemanticValidationError> {
    let minimum = minimum_collection_phase(target.collection);
    if phase != Phase::Logging && (phase as u8) < (minimum as u8) {
        return Err(SemanticValidationError::CollectionPhase {
            collection: target.collection,
            phase,
            location: target.location,
        });
    }
    if target.selector_kind != SelectorKind::None && !collection_accepts_selector(target.collection) {
        return Err(SemanticValidationError::SelectorNotAllowed {
            collection: target.collection,
            selector: target.selector_kind,
            location: target.location,
        });
    }
    if target.selector_kind == SelectorKind::Path && target.collection != CollectionKind::Xml {
        return Err(SemanticValidationError::PathSelectorCollection {
            location: target.location,
        });
    }
    Ok(())
}

pub(super) fn minimum_collection_phase(collection: CollectionKind) -> Phase {
    match collection {
        CollectionKind::Args
        | CollectionKind::ArgsNames
        | CollectionKind::Files
        | CollectionKind::FilesCombinedSize
        | CollectionKind::FilesNames
        | CollectionKind::MultipartPartHeaders
        | CollectionKind::RequestBody
        | CollectionKind::RequestBodyLength
        | CollectionKind::Xml => Phase::RequestBody,
        CollectionKind::ResponseHeaders | CollectionKind::ResponseStatus => Phase::ResponseHeaders,
        CollectionKind::ResponseBody => Phase::ResponseBody,
        _ => Phase::RequestHeaders,
    }
}

pub(super) fn collection_accepts_selector(collection: CollectionKind) -> bool {
    matches!(
        collection,
        CollectionKind::Args
            | CollectionKind::ArgsGet
            | CollectionKind::ArgsGetNames
            | CollectionKind::ArgsNames
            | CollectionKind::Files
            | CollectionKind::FilesNames
            | CollectionKind::MatchedVars
            | CollectionKind::MultipartPartHeaders
            | CollectionKind::RequestCookies
            | CollectionKind::RequestCookiesNames
            | CollectionKind::RequestHeaders
            | CollectionKind::RequestHeadersNames
            | CollectionKind::ResponseHeaders
            | CollectionKind::Tx
            | CollectionKind::Xml
    )
}

fn is_default_requirement(requirement: CompileRequirement) -> bool {
    matches!(
        requirement,
        CompileRequirement::BlockDefaultDisruptive | CompileRequirement::DefaultPhase
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_exception_union(
    draft: &SemanticDraft<'_>,
    index: &[RuleIdIndexRecord],
    states: &[ResolvedRuleStateRecord],
    selectors: &[IdSelector<'_>],
    directive: DirectiveLocation,
    generation: u32,
    seen: &mut [u32],
    affected: &mut Vec<usize>,
    cumulative: ExceptionCounters,
    limits: ValidationLimits,
) -> Result<ExceptionCounters, SemanticValidationError> {
    let mut local = ExceptionCounters::default();
    let lookup_work = ceil_log2(index.len().saturating_add(1));
    for selector in selectors {
        let position = selector.span().start;
        let (start, end, base_work) = match selector.kind() {
            IdSelectorKind::Single(id) => {
                let start = index.partition_point(|entry| entry.id < id);
                let end = index.partition_point(|entry| entry.id <= id);
                (start, end, lookup_work)
            }
            IdSelectorKind::Range { start, end } => {
                if start > end {
                    return Err(SemanticValidationError::ReversedIdRange { position });
                }
                (
                    index.partition_point(|entry| entry.id < start),
                    index.partition_point(|entry| entry.id <= end),
                    lookup_work.saturating_mul(2),
                )
            }
        };
        if start == end {
            return Err(SemanticValidationError::UnresolvedIdSelector { position });
        }
        for entry in &index[start..end] {
            let location = draft.folds[draft.final_rules[entry.unit].fold].location;
            if location >= directive {
                return Err(SemanticValidationError::ForwardIdSelector { position });
            }
        }
        local.selector_work =
            local
                .selector_work
                .checked_add(base_work)
                .ok_or(SemanticValidationError::SelectorWorkLimit {
                    limit: limits.max_selector_work,
                    position,
                })?;
        check_selector_work(&cumulative, &local, limits, position)?;
        for entry in &index[start..end] {
            local.raw = local
                .raw
                .checked_add(1)
                .ok_or(SemanticValidationError::RawMatchDirectiveLimit {
                    limit: limits.max_raw_matches_per_directive,
                    position,
                })?;
            if local.raw > limits.max_raw_matches_per_directive {
                return Err(SemanticValidationError::RawMatchDirectiveLimit {
                    limit: limits.max_raw_matches_per_directive,
                    position,
                });
            }
            if cumulative
                .raw
                .checked_add(local.raw)
                .is_none_or(|value| value > limits.max_raw_matches)
            {
                return Err(SemanticValidationError::RawMatchLimit {
                    limit: limits.max_raw_matches,
                    position,
                });
            }
            local.selector_work =
                local
                    .selector_work
                    .checked_add(1)
                    .ok_or(SemanticValidationError::SelectorWorkLimit {
                        limit: limits.max_selector_work,
                        position,
                    })?;
            let unit = states[entry.unit].unit.expect("indexed root has unit");
            if seen[unit] != generation {
                local.distinct =
                    local
                        .distinct
                        .checked_add(1)
                        .ok_or(SemanticValidationError::DistinctMatchDirectiveLimit {
                            limit: limits.max_distinct_matches_per_directive,
                            position,
                        })?;
                if local.distinct > limits.max_distinct_matches_per_directive {
                    return Err(SemanticValidationError::DistinctMatchDirectiveLimit {
                        limit: limits.max_distinct_matches_per_directive,
                        position,
                    });
                }
                if cumulative
                    .distinct
                    .checked_add(local.distinct)
                    .is_none_or(|value| value > limits.max_distinct_matches)
                {
                    return Err(SemanticValidationError::DistinctMatchLimit {
                        limit: limits.max_distinct_matches,
                        position,
                    });
                }
                local.selector_work =
                    local
                        .selector_work
                        .checked_add(1)
                        .ok_or(SemanticValidationError::SelectorWorkLimit {
                            limit: limits.max_selector_work,
                            position,
                        })?;
                check_selector_work(&cumulative, &local, limits, position)?;
                seen[unit] = generation;
                affected.push(entry.unit);
            }
            check_selector_work(&cumulative, &local, limits, position)?;
        }
    }
    Ok(local)
}

fn check_selector_work(
    cumulative: &ExceptionCounters,
    local: &ExceptionCounters,
    limits: ValidationLimits,
    position: SourcePosition,
) -> Result<(), SemanticValidationError> {
    if cumulative
        .selector_work
        .checked_add(local.selector_work)
        .is_none_or(|value| value > limits.max_selector_work)
    {
        return Err(SemanticValidationError::SelectorWorkLimit {
            limit: limits.max_selector_work,
            position,
        });
    }
    Ok(())
}

const fn ceil_log2(value: usize) -> usize {
    if value <= 1 {
        0
    } else {
        usize::BITS as usize - (value - 1).leading_zeros() as usize
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_update_candidate(
    draft: &SemanticDraft<'_>,
    states: &[ResolvedRuleStateRecord],
    affected: &[usize],
    targets: Range<usize>,
    directive: DirectiveLocation,
    final_counts: &[usize],
    overlay_heads: &[usize],
    overlays: &[TargetOverlayEntry],
    cumulative: ExceptionCounters,
    limits: ValidationLimits,
    total_final_targets: usize,
) -> Result<ExceptionCounters, SemanticValidationError> {
    let added =
        targets
            .len()
            .checked_mul(affected.len())
            .ok_or(SemanticValidationError::FinalTargetReferenceLimit {
                limit: limits.max_final_target_references,
                location: directive,
            })?;
    if total_final_targets
        .checked_add(added)
        .is_none_or(|value| value > limits.max_final_target_references)
    {
        return Err(SemanticValidationError::FinalTargetReferenceLimit {
            limit: limits.max_final_target_references,
            location: directive,
        });
    }
    for &root in affected {
        if matches!(draft.final_rules[root].role, FinalRuleRole::Action) {
            return Err(SemanticValidationError::SecActionTargetUpdate { location: directive });
        }
        if final_counts[root]
            .checked_add(targets.len())
            .is_none_or(|value| value > limits.max_final_targets_per_rule)
        {
            return Err(SemanticValidationError::FinalTargetLimit {
                limit: limits.max_final_targets_per_rule,
                location: directive,
            });
        }
    }
    let mut delta = ExceptionCounters::default();
    for target_index in targets.clone() {
        let target = &draft.targets[target_index];
        for &root in affected {
            validate_target(target, states[root].phase)?;
            if target.modifier == TargetModifier::Exclude
                && !has_exclusion_base(
                    draft,
                    root,
                    target,
                    targets.start..target_index,
                    overlay_heads,
                    overlays,
                    cumulative,
                    &mut delta,
                    limits,
                )?
            {
                return Err(SemanticValidationError::MissingExclusionBase {
                    location: target.location,
                });
            }
        }
    }
    Ok(delta)
}

#[allow(clippy::too_many_arguments)]
fn has_exclusion_base(
    draft: &SemanticDraft<'_>,
    root: usize,
    candidate: &SemanticTarget<'_>,
    candidate_prefix: Range<usize>,
    overlay_heads: &[usize],
    overlays: &[TargetOverlayEntry],
    cumulative: ExceptionCounters,
    delta: &mut ExceptionCounters,
    limits: ValidationLimits,
) -> Result<bool, SemanticValidationError> {
    let mapping = semantic_mapping_for_rule(draft, draft.final_rules[root]);
    for base in &draft.targets[mapping.target_start..mapping.target_start + mapping.target_count] {
        if target_base_matches(base, candidate, cumulative, delta, limits)? {
            return Ok(true);
        }
    }
    let mut overlay = overlay_heads[root];
    while overlay != NO_INDEX {
        let entry = overlays[overlay];
        if target_base_matches(&draft.targets[entry.target], candidate, cumulative, delta, limits)? {
            return Ok(true);
        }
        overlay = entry.next;
    }
    for index in candidate_prefix {
        if target_base_matches(&draft.targets[index], candidate, cumulative, delta, limits)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn target_base_matches(
    base: &SemanticTarget<'_>,
    candidate: &SemanticTarget<'_>,
    cumulative: ExceptionCounters,
    delta: &mut ExceptionCounters,
    limits: ValidationLimits,
) -> Result<bool, SemanticValidationError> {
    if base.modifier != TargetModifier::None || base.collection != candidate.collection {
        return Ok(false);
    }
    delta.comparisons = delta
        .comparisons
        .checked_add(1)
        .ok_or(SemanticValidationError::TargetComparisonLimit {
            limit: limits.max_target_comparisons,
            location: candidate.location,
        })?;
    if cumulative
        .comparisons
        .checked_add(delta.comparisons)
        .is_none_or(|value| value > limits.max_target_comparisons)
    {
        return Err(SemanticValidationError::TargetComparisonLimit {
            limit: limits.max_target_comparisons,
            location: candidate.location,
        });
    }
    if base.selector_kind == SelectorKind::None {
        return Ok(true);
    }
    if base.selector_kind != candidate.selector_kind {
        return Ok(false);
    }
    let (left, right) = match base.selector_kind {
        SelectorKind::Regex(_) => (
            base.regex_body.expect("regex base body").text(),
            candidate.regex_body.expect("regex candidate body").text(),
        ),
        SelectorKind::Literal | SelectorKind::Path => (
            base.selector.expect("selected base").text(),
            candidate.selector.expect("selected candidate").text(),
        ),
        SelectorKind::None => unreachable!(),
    };
    let bytes = left
        .len()
        .checked_add(right.len())
        .ok_or(SemanticValidationError::TargetComparisonByteLimit {
            limit: limits.max_target_comparison_bytes,
            location: candidate.location,
        })?;
    delta.comparison_bytes =
        delta
            .comparison_bytes
            .checked_add(bytes)
            .ok_or(SemanticValidationError::TargetComparisonByteLimit {
                limit: limits.max_target_comparison_bytes,
                location: candidate.location,
            })?;
    if cumulative
        .comparison_bytes
        .checked_add(delta.comparison_bytes)
        .is_none_or(|value| value > limits.max_target_comparison_bytes)
    {
        return Err(SemanticValidationError::TargetComparisonByteLimit {
            limit: limits.max_target_comparison_bytes,
            location: candidate.location,
        });
    }
    Ok(left == right)
}

#[allow(clippy::too_many_arguments)]
fn mark_reachable_regex_targets(
    draft: &SemanticDraft<'_>,
    states: &[ResolvedRuleStateRecord],
    active: &[u8],
    overlay_heads: &[usize],
    overlays: &[TargetOverlayEntry],
    seen: &mut [u32],
    candidates: &mut Vec<usize>,
) {
    for (root, rule) in draft.final_rules.iter().copied().enumerate() {
        if matches!(rule.role, FinalRuleRole::ChainChild { .. }) {
            continue;
        }
        let unit = states[root].unit.expect("root has unit");
        if active[unit] == 0 {
            continue;
        }
        let mut mark_node = |node: usize| {
            let mapping = semantic_mapping_for_rule(draft, draft.final_rules[node]);
            let target_range = mapping.target_start..mapping.target_start + mapping.target_count;
            for (offset, marked) in seen[target_range.clone()].iter_mut().enumerate() {
                let target = target_range.start + offset;
                if *marked == 0 && matches!(draft.targets[target].selector_kind, SelectorKind::Regex(_)) {
                    *marked = 1;
                    candidates.push(target);
                }
            }
            let mut overlay = overlay_heads[node];
            while overlay != NO_INDEX {
                let entry = overlays[overlay];
                if seen[entry.target] == 0
                    && matches!(draft.targets[entry.target].selector_kind, SelectorKind::Regex(_))
                {
                    seen[entry.target] = 1;
                    candidates.push(entry.target);
                }
                overlay = entry.next;
            }
        };
        if let Some(chain) = rule.chain {
            let chain = draft.chains[chain];
            for &node in &draft.chain_members[chain.member_start..chain.member_start + chain.member_count] {
                mark_node(node);
            }
        } else {
            mark_node(root);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validation_owned_bytes(
    nodes: usize,
    units: usize,
    final_targets: usize,
    updates: usize,
    removals: usize,
    affected: usize,
    requirements: usize,
    limits: ValidationLimits,
) -> Result<usize, SemanticValidationError> {
    let mut total = 0_usize;
    for (count, size) in [
        (nodes, size_of::<ValidatedRuleNodeRecord>()),
        (units, size_of::<ValidatedRuleUnitRecord>()),
        (units, size_of::<RuleIdIndexRecord>()),
        (final_targets, size_of::<usize>()),
        (updates, size_of::<AppliedExceptionRecord>()),
        (removals, size_of::<AppliedExceptionRecord>()),
        (affected, size_of::<usize>()),
        (requirements, size_of::<ValidatedRequirementReference>()),
    ] {
        total = total
            .checked_add(count.checked_mul(size).ok_or(SemanticValidationError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
            })?)
            .ok_or(SemanticValidationError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
            })?;
    }
    Ok(total)
}

fn commit_validated_nodes(
    validated: &mut ValidatedSemanticDocument<'_>,
    states: &[ResolvedRuleStateRecord],
    overlay_heads: &[usize],
    overlays: &[TargetOverlayEntry],
) {
    for node in 0..validated.draft.final_rules.len() {
        let mapping = semantic_mapping_for_rule(&validated.draft, validated.draft.final_rules[node]);
        let target_start = validated.final_targets.len();
        validated
            .final_targets
            .extend(mapping.target_start..mapping.target_start + mapping.target_count);
        let mut overlay = overlay_heads[node];
        while overlay != NO_INDEX {
            let entry = overlays[overlay];
            validated.final_targets.push(entry.target);
            overlay = entry.next;
        }
        validated.nodes.push(ValidatedRuleNodeRecord {
            rule: node,
            state: states[node],
            target_start,
            target_count: validated.final_targets.len() - target_start,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn commit_applied_exceptions(
    validated: &mut ValidatedSemanticDocument<'_>,
    states: &[ResolvedRuleStateRecord],
    id_index: &[RuleIdIndexRecord],
    limits: ValidationLimits,
    active: &mut [u8],
    seen: &mut [u32],
    affected: &mut Vec<usize>,
) -> Result<(), SemanticValidationError> {
    active.fill(1);
    seen.fill(0);
    let mut generation = 0_u32;
    let mut counters = ExceptionCounters::default();
    for (directive_index, mapping) in validated.draft.directives.iter().enumerate() {
        let syntax = validated.draft.documents[mapping.document_index]
            .get(mapping.directive_index)
            .expect("validated exception replay mapping");
        let (kind, ids) = match syntax.targets() {
            DirectiveTargetView::Update { ids, .. } => (ExceptionKind::Update, ids),
            DirectiveTargetView::Remove { ids } => (ExceptionKind::Remove, ids),
            DirectiveTargetView::None | DirectiveTargetView::Rule { .. } => continue,
        };
        generation += 1;
        let location = DirectiveLocation {
            document_index: mapping.document_index,
            directive_index: mapping.directive_index,
        };
        affected.clear();
        let local = resolve_exception_union(
            &validated.draft,
            id_index,
            states,
            ids,
            location,
            generation,
            seen,
            affected,
            counters,
            limits,
        )?;
        match kind {
            ExceptionKind::Update => {
                debug_assert!(affected
                    .iter()
                    .all(|root| active[states[*root].unit.expect("replay root unit")] != 0));
            }
            ExceptionKind::Remove => {
                affected.retain(|root| active[states[*root].unit.expect("replay root unit")] != 0);
            }
        }
        let affected_start = validated.affected_units.len();
        for &root in affected.iter() {
            let unit = states[root].unit.expect("replay root unit");
            validated.affected_units.push(unit);
            if kind == ExceptionKind::Remove {
                active[unit] = 0;
            }
        }
        let record = AppliedExceptionRecord {
            directive: directive_index,
            affected_start,
            affected_count: affected.len(),
        };
        match kind {
            ExceptionKind::Update => validated.updates.push(record),
            ExceptionKind::Remove => validated.removals.push(record),
        }
        counters.raw += local.raw;
        counters.distinct += local.distinct;
        counters.selector_work += local.selector_work;
    }
    Ok(())
}
