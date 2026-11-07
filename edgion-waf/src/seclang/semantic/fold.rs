use super::*;

#[derive(Debug, Clone, Copy)]
struct FoldAdjacency {
    location: DirectiveLocation,
    root: DirectiveLocation,
    chains: bool,
}

#[derive(Debug, Clone, Copy)]
struct FoldInspection {
    record: LocalRuleFoldRecord,
    default_phase: bool,
    effective_excess_position: Option<SourcePosition>,
}

#[derive(Debug, Clone, Copy)]
struct FoldActionArena<'view, 'source> {
    actions: &'view [SemanticActionRecord<'source>],
    operands: &'view [ActionOperandRecord<'source>],
    effective_base: usize,
    effective_limit: usize,
}

pub(super) fn fold_local_rules(draft: &mut SemanticDraft<'_>, limits: SemanticLimits) -> Result<(), SemanticError> {
    let fold_count = draft
        .documents
        .iter()
        .map(|document| {
            document
                .iter()
                .filter(|view| {
                    matches!(
                        view.directive().directive(),
                        Directive::SecRule { .. } | Directive::SecAction { .. }
                    )
                })
                .count()
        })
        .try_fold(0_usize, usize::checked_add)
        .ok_or(SemanticError::FoldLimit {
            limit: limits.fold.max_folded_rules,
        })?;
    if fold_count > limits.fold.max_folded_rules {
        return Err(SemanticError::FoldLimit {
            limit: limits.fold.max_folded_rules,
        });
    }

    let mut effective_count = 0_usize;
    let mut effective_excess = None;
    let mut default_count = 0_usize;
    let mut previous = None;
    let mut mapping_index = 0_usize;
    let mut final_position = SourcePosition {
        line: 1,
        column: 1,
        byte_offset: 0,
    };
    for (document_index, document) in draft.documents.iter().enumerate() {
        for directive_index in 0..document.len() {
            let syntax = document.get(directive_index).expect("bounded fold scan");
            final_position = syntax.directive().span().end;
            let mapping = &draft.directives[mapping_index];
            mapping_index += 1;
            let Some(inspection) = inspect_fold(
                FoldActionArena {
                    actions: &draft.actions,
                    operands: &draft.action_operands,
                    effective_base: effective_count,
                    effective_limit: limits.fold.max_effective_transformations,
                },
                syntax,
                mapping,
                document_index,
                directive_index,
                previous,
            )?
            else {
                previous = None;
                continue;
            };
            if effective_excess.is_none() {
                effective_excess = inspection.effective_excess_position;
            }
            match effective_count.checked_add(inspection.record.effective_count) {
                Some(next) => {
                    effective_count = next;
                }
                None => {
                    effective_count = usize::MAX;
                    effective_excess.get_or_insert(final_position);
                }
            }
            default_count += usize::from(inspection.default_phase);
            previous = adjacency(inspection.record);
        }
    }

    if let Some(position) = effective_excess {
        return Err(SemanticError::EffectiveTransformationLimit {
            limit: limits.fold.max_effective_transformations,
            position,
        });
    }

    let dynamic_charge = size_of::<LocalRuleFoldRecord>()
        .checked_mul(fold_count)
        .and_then(|value| value.checked_add(size_of::<usize>().checked_mul(effective_count)?))
        .and_then(|value| value.checked_add(size_of::<RequirementRecord>().checked_mul(default_count)?))
        .ok_or(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position: final_position,
        })?;
    let projected_semantic =
        draft
            .semantic_charged_bytes
            .checked_add(dynamic_charge)
            .ok_or(SemanticError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
                position: final_position,
            })?;
    if projected_semantic > limits.max_owned_bytes {
        return Err(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position: final_position,
        });
    }
    let projected_retained =
        draft
            .inherited_charged_bytes
            .checked_add(projected_semantic)
            .ok_or(SemanticError::RetainedByteLimit {
                limit: limits.max_retained_bytes,
            })?;
    if projected_retained > limits.max_retained_bytes {
        return Err(SemanticError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        });
    }

    draft
        .folds
        .try_reserve_exact(fold_count)
        .map_err(|_| SemanticError::Allocation {
            operation: "local rule folds",
        })?;
    draft
        .effective_transformations
        .try_reserve_exact(effective_count)
        .map_err(|_| SemanticError::Allocation {
            operation: "effective transformations",
        })?;
    draft
        .requirements
        .try_reserve_exact(default_count)
        .map_err(|_| SemanticError::Allocation {
            operation: "default phase requirements",
        })?;

    previous = None;
    mapping_index = 0;
    for (document_index, document) in draft.documents.iter().enumerate() {
        for directive_index in 0..document.len() {
            let syntax = document.get(directive_index).expect("bounded fold commit");
            let mapping = &draft.directives[mapping_index];
            let effective_start = draft.effective_transformations.len();
            let Some(mut inspection) = inspect_fold(
                FoldActionArena {
                    actions: &draft.actions,
                    operands: &draft.action_operands,
                    effective_base: 0,
                    effective_limit: usize::MAX,
                },
                syntax,
                mapping,
                document_index,
                directive_index,
                previous,
            )?
            else {
                previous = None;
                mapping_index += 1;
                continue;
            };
            inspection.record.effective_start = effective_start;
            // Inspection validates every action; commit only the admitted final suffix.
            let surviving_start = inspection
                .record
                .reset_action
                .map_or(mapping.action_start, |index| index + 1);
            let action_end = mapping.action_start + mapping.action_count;
            for index in surviving_start..action_end {
                if matches!(draft.actions[index].kind, ActionEnvelopeKind::Transformation) {
                    draft.effective_transformations.push(index);
                }
            }
            let fold_index = draft.folds.len();
            draft.directives[mapping_index].fold = Some(fold_index);
            if inspection.default_phase {
                draft.requirements.push(RequirementRecord {
                    location: location(
                        document_index,
                        directive_index,
                        SemanticNodeKind::Directive,
                        mapping_index,
                    ),
                    requirement: CompileRequirement::DefaultPhase,
                });
            }
            previous = adjacency(inspection.record);
            draft.folds.push(inspection.record);
            mapping_index += 1;
        }
    }
    draft.semantic_charged_bytes = projected_semantic;
    Ok(())
}

impl SemanticActionRecord<'_> {
    fn transformation(&self, operands: &[ActionOperandRecord<'_>]) -> TransformationKind {
        match operands[self.operand] {
            ActionOperandRecord::Transformation(kind) => kind,
            _ => unreachable!("classified transformation action must retain a transformation operand"),
        }
    }
}

fn adjacency(record: LocalRuleFoldRecord) -> Option<FoldAdjacency> {
    let role = record.role?;
    let root = match role {
        ProvisionalRuleRole::Root => record.location,
        ProvisionalRuleRole::Child { root, .. } => root,
    };
    Some(FoldAdjacency {
        location: record.location,
        root,
        chains: record.chain_action.is_some(),
    })
}

fn inspect_fold(
    arena: FoldActionArena<'_, '_>,
    syntax: super::super::SyntaxDirectiveView<'_, '_>,
    mapping: &DirectiveSemanticRecord,
    document_index: usize,
    directive_index: usize,
    previous: Option<FoldAdjacency>,
) -> Result<Option<FoldInspection>, SemanticError> {
    let envelope = match syntax.directive().directive() {
        Directive::SecRule { .. } => LocalRuleEnvelope::Rule,
        Directive::SecAction { .. } => LocalRuleEnvelope::Action,
        _ => return Ok(None),
    };
    let location = DirectiveLocation {
        document_index,
        directive_index,
    };
    let role = match envelope {
        LocalRuleEnvelope::Action => None,
        LocalRuleEnvelope::Rule => Some(match previous.filter(|value| value.chains) {
            Some(value) => ProvisionalRuleRole::Child {
                predecessor: value.location,
                root: value.root,
            },
            None => ProvisionalRuleRole::Root,
        }),
    };
    let presence = match syntax.actions() {
        None => FoldActionPresence::Absent,
        Some(list) if list.entries().is_empty() => FoldActionPresence::PresentEmpty,
        Some(_) => FoldActionPresence::PresentNonempty,
    };
    let anchor = syntax
        .actions()
        .map_or(syntax.directive().span().end, |list| list.span().end);
    let mut record = LocalRuleFoldRecord {
        location,
        envelope,
        role,
        presence,
        action_start: mapping.action_start,
        action_count: mapping.action_count,
        id: None,
        phase: None,
        disruptive: None,
        status: None,
        id_action: None,
        phase_action: None,
        disruptive_action: None,
        status_action: None,
        chain_action: None,
        skip_action: None,
        capture_action: None,
        multi_match_action: None,
        reset_action: None,
        log: None,
        audit_log: None,
        effective_start: 0,
        effective_count: 0,
    };
    let mut severity = false;
    let mut msg = false;
    let mut logdata = false;
    let mut ver = false;
    let mut effective_after_reset = 0_usize;
    let mut effective_excess_position = None;
    let mut duplicate = None;
    let mut forbidden = None;
    let mut status_pass = None;
    let action_end = mapping.action_start + mapping.action_count;
    for (relative_index, action) in arena.actions[mapping.action_start..action_end].iter().enumerate() {
        let index = mapping.action_start + relative_index;
        let position = action.name.span().start;
        match action.kind {
            ActionEnvelopeKind::Transformation => {
                let transformation = action.transformation(arena.operands);
                if transformation == TransformationKind::None {
                    record.reset_action = Some(index);
                    effective_after_reset = 0;
                    effective_excess_position = None;
                } else {
                    if envelope == LocalRuleEnvelope::Action {
                        forbidden.get_or_insert((FoldSingletonKind::Transformation, position));
                    }
                    effective_after_reset += 1;
                    if effective_excess_position.is_none()
                        && arena
                            .effective_base
                            .checked_add(effective_after_reset)
                            .is_none_or(|count| count > arena.effective_limit)
                    {
                        effective_excess_position = Some(position);
                    }
                }
            }
            ActionEnvelopeKind::Action(kind) => match kind {
                ActionKind::Id => {
                    set_singleton(
                        &mut record.id_action,
                        index,
                        FoldSingletonKind::Id,
                        position,
                        &mut duplicate,
                    );
                    record.id = match arena.operands[action.operand] {
                        ActionOperandRecord::Local(LocalActionValue::Id(value)) => Some(value),
                        _ => None,
                    };
                }
                ActionKind::Phase => {
                    set_singleton(
                        &mut record.phase_action,
                        index,
                        FoldSingletonKind::Phase,
                        position,
                        &mut duplicate,
                    );
                    record.phase = match arena.operands[action.operand] {
                        ActionOperandRecord::Local(LocalActionValue::Phase(value)) => Some(value),
                        _ => None,
                    };
                }
                ActionKind::Severity => {
                    set_flag(&mut severity, FoldSingletonKind::Severity, position, &mut duplicate);
                }
                ActionKind::Status => {
                    set_singleton(
                        &mut record.status_action,
                        index,
                        FoldSingletonKind::Status,
                        position,
                        &mut duplicate,
                    );
                    if record.disruptive == Some(DisruptiveKind::Pass) {
                        status_pass.get_or_insert(position);
                    }
                    record.status = match arena.operands[action.operand] {
                        ActionOperandRecord::Local(LocalActionValue::Status(value)) => Some(value),
                        _ => None,
                    };
                }
                ActionKind::Msg => set_flag(&mut msg, FoldSingletonKind::Msg, position, &mut duplicate),
                ActionKind::LogData => {
                    set_flag(&mut logdata, FoldSingletonKind::LogData, position, &mut duplicate);
                }
                ActionKind::Ver => set_flag(&mut ver, FoldSingletonKind::Ver, position, &mut duplicate),
                ActionKind::Skip | ActionKind::SkipAfter => {
                    set_singleton(
                        &mut record.skip_action,
                        index,
                        FoldSingletonKind::SkipFamily,
                        position,
                        &mut duplicate,
                    );
                }
                ActionKind::Block | ActionKind::Deny | ActionKind::Pass => {
                    set_singleton(
                        &mut record.disruptive_action,
                        index,
                        FoldSingletonKind::Disruptive,
                        position,
                        &mut duplicate,
                    );
                    let disruptive = match kind {
                        ActionKind::Block => DisruptiveKind::Block,
                        ActionKind::Deny => DisruptiveKind::Deny,
                        ActionKind::Pass => DisruptiveKind::Pass,
                        _ => unreachable!("the enclosing arm admits only Block, Deny and Pass"),
                    };
                    if disruptive == DisruptiveKind::Pass && record.status.is_some() {
                        status_pass.get_or_insert(position);
                    }
                    record.disruptive = Some(disruptive);
                }
                ActionKind::Chain => {
                    set_singleton(
                        &mut record.chain_action,
                        index,
                        FoldSingletonKind::Chain,
                        position,
                        &mut duplicate,
                    );
                    collect_forbidden(envelope, FoldSingletonKind::Chain, position, &mut forbidden);
                }
                ActionKind::Capture => {
                    set_singleton(
                        &mut record.capture_action,
                        index,
                        FoldSingletonKind::Capture,
                        position,
                        &mut duplicate,
                    );
                    collect_forbidden(envelope, FoldSingletonKind::Capture, position, &mut forbidden);
                }
                ActionKind::MultiMatch => {
                    set_singleton(
                        &mut record.multi_match_action,
                        index,
                        FoldSingletonKind::MultiMatch,
                        position,
                        &mut duplicate,
                    );
                    collect_forbidden(envelope, FoldSingletonKind::MultiMatch, position, &mut forbidden);
                }
                ActionKind::Log => {
                    record.log = Some(true);
                    record.audit_log = Some(true);
                }
                ActionKind::NoLog => {
                    record.log = Some(false);
                    record.audit_log = Some(false);
                }
                ActionKind::AuditLog => record.audit_log = Some(true),
                ActionKind::NoAuditLog => record.audit_log = Some(false),
                ActionKind::Tag | ActionKind::SetVar | ActionKind::Ctl | ActionKind::InitCol => {}
            },
        }
    }
    record.effective_count = effective_after_reset;
    let root_or_action = role == Some(ProvisionalRuleRole::Root) || envelope == LocalRuleEnvelope::Action;
    if let Some((kind, position)) = duplicate {
        return Err(SemanticError::FoldDuplicate { kind, position });
    }
    if let Some((kind, position)) = forbidden {
        return Err(SemanticError::SecActionForbidden { kind, position });
    }
    if root_or_action && record.id.is_none() {
        return Err(SemanticError::FoldMissing {
            kind: FoldRequiredKind::Id,
            position: anchor,
        });
    }
    if root_or_action && record.disruptive.is_none() {
        return Err(SemanticError::FoldMissing {
            kind: FoldRequiredKind::Disruptive,
            position: anchor,
        });
    }
    if let Some(position) = status_pass {
        return Err(SemanticError::StatusPass { position });
    }
    Ok(Some(FoldInspection {
        default_phase: root_or_action && record.phase.is_none(),
        effective_excess_position,
        record,
    }))
}

fn set_singleton(
    slot: &mut Option<usize>,
    index: usize,
    kind: FoldSingletonKind,
    position: SourcePosition,
    duplicate: &mut Option<(FoldSingletonKind, SourcePosition)>,
) {
    if slot.is_some() {
        duplicate.get_or_insert((kind, position));
    } else {
        *slot = Some(index);
    }
}

fn set_flag(
    slot: &mut bool,
    kind: FoldSingletonKind,
    position: SourcePosition,
    duplicate: &mut Option<(FoldSingletonKind, SourcePosition)>,
) {
    if *slot {
        duplicate.get_or_insert((kind, position));
    } else {
        *slot = true;
    }
}

fn collect_forbidden(
    envelope: LocalRuleEnvelope,
    kind: FoldSingletonKind,
    position: SourcePosition,
    forbidden: &mut Option<(FoldSingletonKind, SourcePosition)>,
) {
    if envelope == LocalRuleEnvelope::Action {
        forbidden.get_or_insert((kind, position));
    }
}

#[derive(Debug, Clone, Copy)]
struct MarkerCandidate {
    location: DirectiveLocation,
    index: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ChainProjection {
    pub(super) chain_count: usize,
    pub(super) chain_members: usize,
    limit_position: SourcePosition,
    chain_count_excess: Option<SourcePosition>,
    chain_member_excess: Option<SourcePosition>,
    chain_length_excess: Option<SourcePosition>,
    #[cfg(test)]
    pub(super) inspected_folds: usize,
}

pub(super) fn project_chains(
    folds: &[LocalRuleFoldRecord],
    actions: &[SemanticActionRecord<'_>],
    limits: GraphLimits,
) -> ChainProjection {
    let mut projection = ChainProjection {
        chain_count: 0,
        chain_members: 0,
        limit_position: SourcePosition {
            line: 1,
            column: 1,
            byte_offset: 0,
        },
        chain_count_excess: None,
        chain_member_excess: None,
        chain_length_excess: None,
        #[cfg(test)]
        inspected_folds: 0,
    };
    let mut active: Option<(DirectiveLocation, DirectiveLocation, usize, SourcePosition)> = None;
    for fold in folds.iter().copied() {
        #[cfg(test)]
        {
            projection.inspected_folds += 1;
        }
        match fold.role {
            Some(ProvisionalRuleRole::Root) => {
                let Some(chain_action) = fold.chain_action else {
                    active = None;
                    continue;
                };
                let position = actions[chain_action].name.span().start;
                projection.chain_count = projection.chain_count.saturating_add(1);
                projection.chain_members = projection.chain_members.saturating_add(1);
                projection.limit_position = position;
                if projection.chain_count > limits.max_chains && projection.chain_count_excess.is_none() {
                    projection.chain_count_excess = Some(position);
                }
                if projection.chain_members > limits.max_chain_members && projection.chain_member_excess.is_none() {
                    projection.chain_member_excess = Some(position);
                }
                if limits.max_members_per_chain == 0 && projection.chain_length_excess.is_none() {
                    projection.chain_length_excess = Some(position);
                }
                active = Some((fold.location, fold.location, 1, position));
            }
            Some(ProvisionalRuleRole::Child { root, predecessor }) => {
                let Some((active_root, active_predecessor, length, position)) = active else {
                    continue;
                };
                if active_root != root || active_predecessor != predecessor {
                    active = None;
                    continue;
                }
                let length = length.saturating_add(1);
                projection.chain_members = projection.chain_members.saturating_add(1);
                if projection.chain_members > limits.max_chain_members && projection.chain_member_excess.is_none() {
                    projection.chain_member_excess = Some(position);
                }
                if length > limits.max_members_per_chain && projection.chain_length_excess.is_none() {
                    projection.chain_length_excess = Some(position);
                }
                active = fold
                    .chain_action
                    .map(|_| (active_root, fold.location, length, position));
            }
            None => active = None,
        }
    }
    projection
}

pub(super) fn finalize_graph(draft: &mut SemanticDraft<'_>, limits: SemanticLimits) -> Result<(), SemanticError> {
    let mut marker_count = 0_usize;
    let mut ref_count = 0_usize;
    let mut marker_bytes = 0_usize;
    let mut reference_bytes = 0_usize;
    let mut marker_count_excess = None;
    let mut ref_count_excess = None;
    let mut marker_bytes_excess = None;
    let mut marker_name_excess = None;
    let mut work_excess = None;
    let mut work = 0_usize;
    let mut mapping_index = 0_usize;
    for document in &draft.documents {
        for directive_index in 0..document.len() {
            let syntax = document.get(directive_index).expect("bounded graph precount");
            if let Directive::SecMarker { marker } = syntax.directive().directive() {
                marker_count = marker_count.saturating_add(1);
                if marker_count > limits.graph.max_markers && marker_count_excess.is_none() {
                    marker_count_excess = Some(marker.span().start);
                }
                marker_bytes = marker_bytes.saturating_add(marker.value().len());
                if marker.value().len() > limits.graph.max_marker_name_bytes && marker_name_excess.is_none() {
                    marker_name_excess = Some(marker.span().start);
                }
                if marker_bytes > limits.graph.max_marker_name_bytes_total && marker_bytes_excess.is_none() {
                    marker_bytes_excess = Some(marker.span().start);
                }
                let contribution = MARKER_WORK_ENTRY_CHARGE.saturating_add(marker.value().len());
                work = work.saturating_add(contribution);
                if work > limits.graph.max_registry_work_bytes && work_excess.is_none() {
                    work_excess = Some(marker.span().start);
                }
            }
            let fold_mapping = draft.directives[mapping_index].fold;
            mapping_index += 1;
            let Some(fold_index) = fold_mapping else {
                continue;
            };
            let fold = draft.folds[fold_index];
            if matches!(fold.role, Some(ProvisionalRuleRole::Child { .. })) {
                continue;
            }
            if let Some((action, literal)) = skip_after(draft, fold) {
                ref_count = ref_count.saturating_add(1);
                if ref_count > limits.graph.max_skip_after_refs && ref_count_excess.is_none() {
                    ref_count_excess = Some(draft.actions[action].name.span().start);
                }
                reference_bytes = reference_bytes.saturating_add(literal.text().len());
                work = work.saturating_add(literal.text().len());
                if work > limits.graph.max_registry_work_bytes && work_excess.is_none() {
                    work_excess = Some(literal.span().start);
                }
            }
        }
    }
    if let Some(position) = marker_count_excess {
        return Err(SemanticError::MarkerLimit {
            limit: limits.graph.max_markers,
            position,
        });
    }
    if let Some(position) = ref_count_excess {
        return Err(SemanticError::SkipAfterLimit {
            limit: limits.graph.max_skip_after_refs,
            position,
        });
    }
    if let Some(position) = marker_name_excess {
        return Err(SemanticError::MarkerNameByteLimit {
            limit: limits.graph.max_marker_name_bytes,
            position,
        });
    }
    if let Some(position) = marker_bytes_excess {
        return Err(SemanticError::MarkerNameTotalLimit {
            limit: limits.graph.max_marker_name_bytes_total,
            position,
        });
    }
    if let Some(position) = work_excess {
        return Err(SemanticError::MarkerWorkLimit {
            limit: limits.graph.max_registry_work_bytes,
            position,
        });
    }
    let expected_work = marker_count
        .checked_mul(MARKER_WORK_ENTRY_CHARGE)
        .and_then(|value| value.checked_add(marker_bytes))
        .and_then(|value| value.checked_add(reference_bytes))
        .ok_or(SemanticError::MarkerWorkLimit {
            limit: limits.graph.max_registry_work_bytes,
            position: SourcePosition {
                line: 1,
                column: 1,
                byte_offset: 0,
            },
        })?;
    debug_assert_eq!(work, expected_work);

    let mut registry = HashMap::with_hasher(RandomState::new());
    registry
        .try_reserve(marker_count)
        .map_err(|_| SemanticError::Allocation {
            operation: "marker registry",
        })?;
    let mut structural: Option<(DirectiveLocation, usize, SemanticError)> = None;
    let mut duplicate: Option<(DirectiveLocation, SemanticError)> = None;
    let mut marker_index = 0_usize;
    for (document_index, document) in draft.documents.iter().enumerate() {
        for directive_index in 0..document.len() {
            let location = DirectiveLocation {
                document_index,
                directive_index,
            };
            let syntax = document.get(directive_index).expect("bounded graph validation");
            if let Directive::SecMarker { marker } = syntax.directive().directive() {
                let invalid = if marker
                    .value()
                    .as_bytes()
                    .iter()
                    .any(|byte| *byte == 0 || (*byte < 0x20 && *byte != b'\t') || *byte == 0x7f)
                {
                    Some(SemanticError::MarkerControl {
                        position: marker.span().start,
                    })
                } else {
                    None
                };
                if let Some(error) = invalid {
                    retain_earliest_source(&mut structural, location, marker.span().start, error);
                }
                match registry.entry(marker.value()) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(MarkerCandidate {
                            location,
                            index: marker_index,
                        });
                    }
                    std::collections::hash_map::Entry::Occupied(entry) => retain_earliest(
                        &mut duplicate,
                        location,
                        SemanticError::DuplicateMarker {
                            position: marker.span().start,
                            first: entry.get().location,
                        },
                    ),
                }
                marker_index += 1;
            }
        }
    }

    for (index, fold) in draft.folds.iter().copied().enumerate() {
        if let Some(chain_action) = fold.chain_action {
            let expected_root = match fold.role {
                Some(ProvisionalRuleRole::Root) => fold.location,
                Some(ProvisionalRuleRole::Child { root, .. }) => root,
                None => fold.location,
            };
            let next_is_child = draft.folds.get(index + 1).is_some_and(|next| {
                matches!(next.role, Some(ProvisionalRuleRole::Child { predecessor, root }) if predecessor == fold.location && root == expected_root)
            });
            if !next_is_child {
                let position = draft.actions[chain_action].name.span().start;
                retain_earliest_source(
                    &mut structural,
                    fold.location,
                    position,
                    SemanticError::DanglingChain { position },
                );
            }
        }
        if matches!(fold.role, Some(ProvisionalRuleRole::Child { .. })) {
            for action in &draft.actions[fold.action_start..fold.action_start + fold.action_count] {
                if let ActionEnvelopeKind::Action(kind) = action.kind {
                    if is_forbidden_child(kind) {
                        let position = action.name.span().start;
                        retain_earliest_source(
                            &mut structural,
                            fold.location,
                            position,
                            SemanticError::ChildAction { kind, position },
                        );
                        break;
                    }
                }
            }
        }
    }
    let chains = project_chains(&draft.folds, &draft.actions, limits.graph);
    if let Some((_, _, error)) = structural {
        return Err(error);
    }
    if let Some((_, error)) = duplicate {
        return Err(error);
    }

    let mut reference: Option<(DirectiveLocation, SemanticError)> = None;
    for (index, fold) in draft.folds.iter().copied().enumerate() {
        if matches!(fold.role, Some(ProvisionalRuleRole::Child { .. })) {
            continue;
        }
        let Some((action, literal)) = skip_after(draft, fold) else {
            continue;
        };
        match registry.get(literal.text()) {
            None => retain_earliest(
                &mut reference,
                fold.location,
                SemanticError::UnresolvedSkipAfter {
                    position: literal.span().start,
                },
            ),
            Some(marker) => {
                let terminal = terminal_location(&draft.folds, index);
                if marker.location <= terminal {
                    retain_earliest(
                        &mut reference,
                        fold.location,
                        SemanticError::NonForwardSkipAfter {
                            position: literal.span().start,
                            target: marker.location,
                        },
                    );
                }
            }
        }
        let _ = action;
    }
    if let Some((_, error)) = reference {
        return Err(error);
    }
    if let Some(position) = chains.chain_count_excess {
        return Err(SemanticError::ChainLimit {
            limit: limits.graph.max_chains,
            position,
        });
    }
    if let Some(position) = chains.chain_member_excess {
        return Err(SemanticError::ChainMemberLimit {
            limit: limits.graph.max_chain_members,
            position,
        });
    }
    if let Some(position) = chains.chain_length_excess {
        return Err(SemanticError::ChainLengthLimit {
            limit: limits.graph.max_members_per_chain,
            position,
        });
    }

    let dynamic = size_of::<FinalRuleRecord>()
        .checked_mul(draft.folds.len())
        .and_then(|value| value.checked_add(size_of::<ChainRecord>().checked_mul(chains.chain_count)?))
        .and_then(|value| value.checked_add(size_of::<usize>().checked_mul(chains.chain_members)?))
        .and_then(|value| value.checked_add(size_of::<MarkerRecord<'_>>().checked_mul(marker_count)?))
        .and_then(|value| value.checked_add(size_of::<ResolvedSkipAfterRecord>().checked_mul(ref_count)?))
        .ok_or(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position: chains.limit_position,
        })?;
    let projected = draft
        .semantic_charged_bytes
        .checked_add(dynamic)
        .ok_or(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position: chains.limit_position,
        })?;
    if projected > limits.max_owned_bytes {
        return Err(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position: chains.limit_position,
        });
    }
    if draft
        .inherited_charged_bytes
        .checked_add(projected)
        .is_none_or(|value| value > limits.max_retained_bytes)
    {
        return Err(SemanticError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        });
    }
    reserve_more(&mut draft.final_rules, draft.folds.len(), "final rule graph")?;
    reserve_more(&mut draft.chains, chains.chain_count, "chains")?;
    reserve_more(&mut draft.chain_members, chains.chain_members, "chain members")?;
    reserve_more(&mut draft.markers, marker_count, "markers")?;
    reserve_more(
        &mut draft.resolved_skip_afters,
        ref_count,
        "resolved skipAfter references",
    )?;

    for (document_index, document) in draft.documents.iter().enumerate() {
        for directive_index in 0..document.len() {
            let syntax = document.get(directive_index).expect("bounded marker commit");
            if let Directive::SecMarker { marker } = syntax.directive().directive() {
                draft.markers.push(MarkerRecord {
                    location: DirectiveLocation {
                        document_index,
                        directive_index,
                    },
                    name: marker.value(),
                    span: marker.span(),
                });
            }
        }
    }
    commit_final_graph(draft, &registry);
    draft.semantic_charged_bytes = projected;
    Ok(())
}

fn reserve_more<T>(values: &mut Vec<T>, count: usize, operation: &'static str) -> Result<(), SemanticError> {
    values
        .try_reserve_exact(count)
        .map_err(|_| SemanticError::Allocation { operation })
}
fn retain_earliest(
    slot: &mut Option<(DirectiveLocation, SemanticError)>,
    location: DirectiveLocation,
    error: SemanticError,
) {
    if slot.as_ref().is_none_or(|(current, _)| location < *current) {
        *slot = Some((location, error));
    }
}
fn retain_earliest_source(
    slot: &mut Option<(DirectiveLocation, usize, SemanticError)>,
    location: DirectiveLocation,
    position: SourcePosition,
    error: SemanticError,
) {
    let key = (location, position.byte_offset);
    if slot
        .as_ref()
        .is_none_or(|(current, byte_offset, _)| key < (*current, *byte_offset))
    {
        *slot = Some((location, position.byte_offset, error));
    }
}
fn is_forbidden_child(kind: ActionKind) -> bool {
    matches!(
        kind,
        ActionKind::Id
            | ActionKind::Phase
            | ActionKind::Block
            | ActionKind::Deny
            | ActionKind::Pass
            | ActionKind::Skip
            | ActionKind::SkipAfter
            | ActionKind::Status
            | ActionKind::Severity
            | ActionKind::Msg
            | ActionKind::LogData
            | ActionKind::Tag
            | ActionKind::Ver
    )
}
fn skip_after<'source>(
    draft: &SemanticDraft<'source>,
    fold: LocalRuleFoldRecord,
) -> Option<(usize, SemanticText<'source>)> {
    let index = fold.skip_action?;
    if draft.actions[index].kind != ActionEnvelopeKind::Action(ActionKind::SkipAfter) {
        return None;
    }
    match draft.action_operands[draft.actions[index].operand] {
        ActionOperandRecord::Literal(value) => Some((index, value)),
        _ => None,
    }
}
fn terminal_location(folds: &[LocalRuleFoldRecord], root: usize) -> DirectiveLocation {
    let mut cursor = root;
    while folds[cursor].chain_action.is_some() && cursor + 1 < folds.len() {
        cursor += 1;
    }
    folds[cursor].location
}

fn phase_for_root(fold: LocalRuleFoldRecord) -> FinalPhaseState {
    match fold.phase {
        Some(phase) => FinalPhaseState::Explicit {
            phase,
            source: fold.location,
        },
        None => FinalPhaseState::RequiresDefault { owner: fold.location },
    }
}

fn commit_final_graph(draft: &mut SemanticDraft<'_>, registry: &HashMap<&str, MarkerCandidate, RandomState>) {
    let mut active: Option<(usize, DirectiveLocation, FinalPhaseState, usize)> = None;
    for index in 0..draft.folds.len() {
        let fold = draft.folds[index];
        let (role, phase, chain) = match fold.envelope {
            LocalRuleEnvelope::Action => (FinalRuleRole::Action, phase_for_root(fold), None),
            LocalRuleEnvelope::Rule => match fold.role.expect("rule fold has provisional role") {
                ProvisionalRuleRole::Root if fold.chain_action.is_none() => {
                    (FinalRuleRole::StandaloneRule, phase_for_root(fold), None)
                }
                ProvisionalRuleRole::Root => {
                    let phase = phase_for_root(fold);
                    let chain_index = draft.chains.len();
                    let count = chain_length(&draft.folds, index);
                    draft.chains.push(ChainRecord {
                        root: fold.location,
                        phase,
                        member_start: draft.chain_members.len(),
                        member_count: count,
                    });
                    draft.chain_members.push(index);
                    active = Some((chain_index, fold.location, phase, 1));
                    (
                        FinalRuleRole::ChainRoot { root: fold.location },
                        phase,
                        Some(chain_index),
                    )
                }
                ProvisionalRuleRole::Child { root, predecessor } => {
                    let (chain_index, _, root_phase, ordinal) =
                        active.expect("validated child belongs to active chain");
                    draft.chain_members.push(index);
                    let phase = match root_phase {
                        FinalPhaseState::Explicit { phase, .. } => FinalPhaseState::Inherited { phase, root },
                        FinalPhaseState::RequiresDefault { owner } => FinalPhaseState::RequiresDefault { owner },
                        FinalPhaseState::Inherited { .. } => {
                            unreachable!("only chain children inherit a phase; a root never does")
                        }
                    };
                    if fold.chain_action.is_some() {
                        active = Some((chain_index, root, root_phase, ordinal + 1));
                    } else {
                        active = None;
                    }
                    (
                        FinalRuleRole::ChainChild {
                            root,
                            predecessor,
                            ordinal,
                        },
                        phase,
                        Some(chain_index),
                    )
                }
            },
        };
        let resolution = if !matches!(fold.role, Some(ProvisionalRuleRole::Child { .. })) {
            skip_after(draft, fold).map(|(action, literal)| {
                let marker = registry[literal.text()].index;
                let resolution = draft.resolved_skip_afters.len();
                draft.resolved_skip_afters.push(ResolvedSkipAfterRecord {
                    referrer: index,
                    action,
                    marker,
                });
                resolution
            })
        } else {
            None
        };
        draft.final_rules.push(FinalRuleRecord {
            fold: index,
            role,
            phase,
            chain,
            resolution,
        });
    }
}
fn chain_length(folds: &[LocalRuleFoldRecord], root: usize) -> usize {
    let mut cursor = root;
    let mut count = 1;
    while folds[cursor].chain_action.is_some() {
        cursor += 1;
        count += 1;
    }
    count
}

pub(super) fn location(
    document: usize,
    directive: usize,
    node_kind: SemanticNodeKind,
    node_index: usize,
) -> SemanticLocation {
    SemanticLocation {
        document_index: document,
        directive_index: directive,
        node_kind,
        node_index,
    }
}
