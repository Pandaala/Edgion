//! Conservative forward reachability proof. It never executes transaction effects.
use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) struct StaticGate {
    pub key: ByteRange,
    pub relation: IntegerRelation,
    pub expected: i64,
    pub negated: bool,
}
#[derive(Debug, Clone, Copy)]
pub(super) struct ActivationStep {
    pub unit: Option<usize>,
    pub matched: usize,
    pub gate: Option<StaticGate>,
}
#[derive(Debug, Default)]
pub(super) struct CompiledActivation {
    pub steps: Vec<ActivationStep>,
    pub has_gate: bool,
}

/// Shared source-order successor: first skipAfter wins, otherwise last skip.
/// Markers do not consume skip counts; removed units still do.
pub(super) fn successor<E>(
    generation: &CompiledGeneration,
    unit: CompiledRuleUnit,
    schedule_index: usize,
    current: usize,
    mut charge: impl FnMut(usize) -> Result<(), E>,
    invalid: impl Fn() -> E,
) -> Result<usize, E> {
    let end = unit.member_start.checked_add(unit.member_count).ok_or_else(&invalid)?;
    let members = generation.members.get(unit.member_start..end).ok_or_else(&invalid)?;
    let schedule = generation.schedules.get(schedule_index).ok_or_else(&invalid)?;
    let mut target = None;
    for member in members {
        charge(1)?;
        if let Some(marker) = member.skip_after_marker {
            target = Some(marker.checked_add(1).ok_or_else(&invalid)?);
            break;
        }
    }
    if let Some(target) = target {
        return if target >= current && target <= schedule.len() {
            Ok(target)
        } else {
            Err(invalid())
        };
    }
    let mut skip = None;
    for member in members {
        charge(1)?;
        let end = member
            .action_start
            .checked_add(member.action_count)
            .ok_or_else(&invalid)?;
        for action in generation.actions.get(member.action_start..end).ok_or_else(&invalid)? {
            charge(1)?;
            if let CompiledActionOperand::Local(LocalActionValue::Skip(count)) = action.operand {
                skip = Some(usize::try_from(count).map_err(|_| invalid())?);
            }
        }
    }
    let Some(mut remaining) = skip else {
        return Ok(current);
    };
    let mut cursor = current;
    while cursor < schedule.len() && remaining != 0 {
        charge(1)?;
        if matches!(schedule[cursor], CompiledScheduleEntry::RuleUnit(_)) {
            remaining -= 1;
        }
        cursor += 1;
    }
    Ok(cursor)
}

fn invalid() -> CompilerError {
    CompilerError::PrivateInvariant {
        kind: CompilerInvariant::ScheduleOrder,
    }
}
fn comparison(a: &[u8], b: &[u8], work: &mut CanonicalWork, limits: CompilerLimits) -> Result<bool, CompilerError> {
    work.charge_records(1, limits)?;
    work.charge_comparison_bytes(a.len().min(b.len()), limits)?;
    Ok(a.eq_ignore_ascii_case(b))
}

impl CompiledActivation {
    pub(super) fn compile(
        g: &CompiledGeneration,
        memory: &mut MatcherCompileWork,
        work: &mut CanonicalWork,
    ) -> Result<Self, CompilerError> {
        let limits = memory.limits;
        let schedule = &g.schedules[1];
        let mut plan = Self::default();
        memory.retain_compiler_records::<ActivationStep>(schedule.len())?;
        reserve_exact(&mut plan.steps, schedule.len(), "inspection activation schedule")?;
        memory.retain_compiler_records::<ActivationStep>(
            plan.steps.capacity().checked_sub(schedule.len()).ok_or_else(invalid)?,
        )?;
        // A unit removal can disable the gate; a TX target removal can
        // disable its sole selector. Other collections cannot affect a TX
        // gate, and their consumers remain conservatively reachable.
        let mut removal = false;
        for unit in &g.units {
            work.charge_records(1, limits)?;
            if unit.phase != Phase::RequestBody {
                continue;
            }
            for member in g
                .members
                .get(unit.member_start..checked_add(unit.member_start, unit.member_count)?)
                .ok_or_else(invalid)?
            {
                work.charge_records(1, limits)?;
                for action in g
                    .actions
                    .get(member.action_start..checked_add(member.action_start, member.action_count)?)
                    .ok_or_else(invalid)?
                {
                    work.charge_records(1, limits)?;
                    removal |= matches!(
                        action.operand,
                        CompiledActionOperand::Ctl(
                            CompiledCtl::RuleRemoveByTag(_)
                                | CompiledCtl::RuleRemoveTargetByTag {
                                    collection: CollectionKind::Tx,
                                    ..
                                }
                        )
                    );
                }
            }
        }
        for (ordinal, entry) in schedule.iter().enumerate() {
            work.charge_records(1, limits)?;
            let next = checked_inc(ordinal)?;
            let (unit, matched, gate) = match *entry {
                CompiledScheduleEntry::MarkerSentinel(_) => (None, next, None),
                CompiledScheduleEntry::RuleUnit(id) => {
                    let unit = *g.units.get(id).ok_or_else(invalid)?;
                    let matched = successor(g, unit, 1, next, |n| work.charge_records(n, limits), invalid)?;
                    let gate = if removal {
                        None
                    } else {
                        eligible(g, unit, work, limits)?
                    };
                    let gate = match gate {
                        Some(gate) if stable(g, gate.key, work, limits)? => Some(gate),
                        _ => None,
                    };
                    (Some(id), matched, gate)
                }
            };
            plan.has_gate |= gate.is_some();
            plan.steps.push(ActivationStep { unit, matched, gate });
        }
        Ok(plan)
    }
}
fn eligible(
    g: &CompiledGeneration,
    unit: CompiledRuleUnit,
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<Option<StaticGate>, CompilerError> {
    if unit.member_count != 1 {
        return Ok(None);
    }
    let member = *g.members.get(unit.member_start).ok_or_else(invalid)?;
    if !member.active || member.skip_after_marker.is_none() {
        return Ok(None);
    }
    let Some(target) = member.target_plan.and_then(|id| g.target_plans.get(id)) else {
        return Ok(None);
    };
    if target.record_count != 1 {
        return Ok(None);
    }
    let record = g.target_records.get(target.record_start).ok_or_else(invalid)?;
    let CompiledTargetSelector::Literal(key) = record.selector else {
        return Ok(None);
    };
    if record.collection != CollectionKind::Tx || record.modifier != TargetModifier::None {
        return Ok(None);
    }
    let transformations = match member.transformation_plan {
        None => &[][..],
        Some(id) => {
            let plan = g.transformation_plans.get(id).ok_or_else(invalid)?;
            g.transformations
                .get(plan.transformation_start..checked_add(plan.transformation_start, plan.transformation_count)?)
                .ok_or_else(invalid)?
        }
    };
    for transform in transformations {
        work.charge_records(1, limits)?;
        if *transform != TransformationKind::None {
            return Ok(None);
        }
    }
    for action in g
        .actions
        .get(member.action_start..checked_add(member.action_start, member.action_count)?)
        .ok_or_else(invalid)?
    {
        work.charge_records(1, limits)?;
        if matches!(
            action.kind,
            ActionEnvelopeKind::Action(super::super::ActionKind::Capture)
        ) {
            return Ok(None);
        }
    }
    let Some(operator) = g.operator_plans.get(unit.member_start).ok_or_else(invalid)? else {
        return Ok(None);
    };
    let CompiledOperatorOperand::Integer {
        relation,
        operand: CompiledIntegerOperand::Static(expected),
    } = operator.operand
    else {
        return Ok(None);
    };
    Ok(Some(StaticGate {
        key,
        relation,
        expected,
        negated: operator.negated,
    }))
}

/// A dynamic template cannot alias a key if any fixed prefix/suffix disagrees.
/// Interior literals are deliberately ignored: proving them requires expansion.
fn template_aliases(
    g: &CompiledGeneration,
    template: usize,
    key: &[u8],
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<bool, CompilerError> {
    let template = g.templates.get(template).ok_or_else(invalid)?;
    let segments = g
        .segments
        .get(template.segment_start..checked_add(template.segment_start, template.segment_count)?)
        .ok_or_else(invalid)?;
    let mut prefix = 0;
    let mut dynamic = false;
    for segment in segments {
        work.charge_records(1, limits)?;
        match segment.kind {
            CompiledSegmentKind::Macro(_) => {
                dynamic = true;
                break;
            }
            CompiledSegmentKind::Literal(range) => {
                let bytes = byte_range(&g.bytes, range)?;
                let end = checked_add(prefix, bytes.len())?;
                let Some(slice) = key.get(prefix..end) else {
                    return Ok(false);
                };
                if !comparison(bytes, slice, work, limits)? {
                    return Ok(false);
                }
                prefix = end;
            }
        }
    }
    if !dynamic {
        return Ok(prefix == key.len());
    }
    let mut suffix = key.len();
    for segment in segments.iter().rev() {
        work.charge_records(1, limits)?;
        match segment.kind {
            CompiledSegmentKind::Macro(_) => break,
            CompiledSegmentKind::Literal(range) => {
                let bytes = byte_range(&g.bytes, range)?;
                let Some(start) = suffix.checked_sub(bytes.len()) else {
                    return Ok(false);
                };
                if !comparison(bytes, &key[start..suffix], work, limits)? {
                    return Ok(false);
                }
                suffix = start;
            }
        }
    }
    Ok(prefix <= suffix)
}
fn stable(
    g: &CompiledGeneration,
    key: ByteRange,
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<bool, CompilerError> {
    let key = byte_range(&g.target_bytes, key)?;
    for unit in &g.units {
        work.charge_records(1, limits)?;
        if unit.phase != Phase::RequestBody {
            continue;
        }
        for member in g
            .members
            .get(unit.member_start..checked_add(unit.member_start, unit.member_count)?)
            .ok_or_else(invalid)?
        {
            work.charge_records(1, limits)?;
            for action in g
                .actions
                .get(member.action_start..checked_add(member.action_start, member.action_count)?)
                .ok_or_else(invalid)?
            {
                work.charge_records(1, limits)?;
                if let CompiledActionOperand::SetVar(id) = action.operand {
                    let setvar = g.setvars.get(id).ok_or_else(invalid)?;
                    if setvar.collection == CollectionKind::Tx && template_aliases(g, setvar.key, key, work, limits)? {
                        return Ok(false);
                    }
                }
            }
        }
    }
    // Every generation alias is excluded, including aliases written in Phase1
    // and later deleted by any capturing match during Phase2.
    for program in &g.capture_regexes {
        work.charge_records(1, limits)?;
        for index in 0..program.capture_len() {
            work.charge_records(1, limits)?;
            let (digits, start) = crate::seclang::decimal_bytes(index);
            if comparison(key, &digits[start..], work, limits)? {
                return Ok(false);
            }
            if let Some(name) = program.capture_name(index) {
                if comparison(key, name.as_bytes(), work, limits)? {
                    return Ok(false);
                }
            }
        }
    }
    for operator in &g.operator_plans {
        work.charge_records(1, limits)?;
        if operator.is_some_and(|operator| {
            matches!(
                operator.operand,
                CompiledOperatorOperand::CapturePhrase(_) | CompiledOperatorOperand::DetectSqli { capture: true }
            )
        }) && comparison(key, b"0", work, limits)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}
#[cfg(test)]
#[path = "../../tests/unit/seclang/inspection_activation_test.rs"]
mod tests;
