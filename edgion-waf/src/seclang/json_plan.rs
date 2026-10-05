//! Immutable structured JSON subscriber capability owned by a compiled generation.
//! Unsupported execution shapes remain valid ordinary WAF configurations.
use super::*;
use crate::seclang::ActionKind;

#[derive(Debug, Clone, Copy)]
pub(super) struct JsonSubscriberDescriptor {
    pub member: usize,
    pub collection: CollectionKind,
    pub selector: CompiledTargetSelector,
}
#[derive(Debug)]
pub(super) enum JsonCapability {
    Unsupported,
    Eligible(Vec<JsonSubscriberDescriptor>),
}
impl JsonCapability {
    pub(super) fn subscribers(&self) -> Option<&[JsonSubscriberDescriptor]> {
        match self {
            Self::Unsupported => None,
            Self::Eligible(values) => Some(values),
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        match self {
            Self::Unsupported => 0,
            Self::Eligible(values) => values.capacity() * size_of::<JsonSubscriberDescriptor>(),
        }
    }
    pub(super) fn compile(
        generation: &CompiledGeneration,
        work: &mut MatcherCompileWork,
    ) -> Result<Self, CompilerError> {
        let mut count = 0usize;
        match visit(generation, |_| {
            count = checked_inc(count).map_err(PlanError::Compiler)?;
            Ok(())
        }) {
            Ok(()) => {}
            Err(PlanError::Unsupported) => return Ok(Self::Unsupported),
            Err(PlanError::Compiler(error)) => return Err(error),
        }
        work.retain_compiler_records::<JsonSubscriberDescriptor>(count)?;
        let mut values = Vec::new();
        if count != 0 {
            reserve_exact(&mut values, count, "compiled JSON subscriber plan")?;
        }
        let excess = values.capacity().checked_sub(count).ok_or_else(invalid_plan)?;
        work.retain_compiler_records::<JsonSubscriberDescriptor>(excess)?;
        visit(generation, |descriptor| {
            values.push(descriptor);
            Ok(())
        })
        .map_err(|error| match error {
            PlanError::Unsupported => invalid_plan(),
            PlanError::Compiler(error) => error,
        })?;
        Ok(Self::Eligible(values))
    }
}
enum PlanError {
    Unsupported,
    Compiler(CompilerError),
}
fn invalid_plan() -> CompilerError {
    CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    }
}
fn visit(
    generation: &CompiledGeneration,
    mut emit: impl FnMut(JsonSubscriberDescriptor) -> Result<(), PlanError>,
) -> Result<(), PlanError> {
    if generation
        .actions
        .iter()
        .any(|action| matches!(action.kind, ActionEnvelopeKind::Action(ActionKind::Ctl)))
    {
        return Err(PlanError::Unsupported);
    }
    for unit in generation.units.iter().filter(|unit| {
        matches!(
            unit.phase,
            Phase::ResponseHeaders | Phase::ResponseBody | Phase::Logging
        )
    }) {
        let end = unit
            .member_start
            .checked_add(unit.member_count)
            .ok_or(PlanError::Compiler(CompilerError::PreflightArithmetic))?;
        for member in generation
            .members
            .get(unit.member_start..end)
            .ok_or(PlanError::Compiler(invalid_plan()))?
        {
            // Include operator and action macros, not only direct body targets.
            if generation.member_requires_request_body(*member) {
                return Err(PlanError::Unsupported);
            }
        }
    }
    for unit in generation.units.iter().filter(|unit| unit.phase == Phase::RequestBody) {
        let end = unit
            .member_start
            .checked_add(unit.member_count)
            .ok_or(PlanError::Compiler(CompilerError::PreflightArithmetic))?;
        let members = generation
            .members
            .get(unit.member_start..end)
            .ok_or(PlanError::Compiler(invalid_plan()))?;
        // Ordinary units keep their canonical evaluator and schedule position.
        // A chain qualifies only when every member is independent of the body.
        if members
            .iter()
            .all(|member| !generation.member_requires_request_body(*member))
        {
            continue;
        }
        if unit.member_count != 1 {
            return Err(PlanError::Unsupported);
        }
        let member = generation
            .members
            .get(unit.member_start)
            .ok_or(PlanError::Compiler(invalid_plan()))?;
        // Replay retains match accounting without payload bytes. Its own
        // templates must therefore remain static even when ordinary units use
        // dynamic templates through the canonical evaluator.
        if let Some(template) = member.operator_template {
            require_static_template(generation, template)?;
        }
        let plan = member
            .target_plan
            .and_then(|id| generation.target_plans.get(id))
            .ok_or(PlanError::Unsupported)?;
        let records = generation
            .target_records
            .get(
                plan.record_start
                    ..plan
                        .record_start
                        .checked_add(plan.record_count)
                        .ok_or(PlanError::Compiler(CompilerError::PreflightArithmetic))?,
            )
            .ok_or(PlanError::Compiler(invalid_plan()))?;
        let [target] = records else {
            return Err(PlanError::Unsupported);
        };
        if !matches!(target.collection, CollectionKind::Args | CollectionKind::ArgsNames)
            || target.modifier != TargetModifier::None
            || !matches!(
                target.selector,
                CompiledTargetSelector::None | CompiledTargetSelector::Literal(_)
            )
        {
            return Err(PlanError::Unsupported);
        }
        let operator = generation
            .operator_plans
            .get(unit.member_start)
            .copied()
            .flatten()
            .ok_or(PlanError::Unsupported)?;
        if !matches!(
            operator.kind,
            crate::seclang::OperatorKind::Pm | crate::seclang::OperatorKind::PmFromFile
        ) || !matches!(operator.operand, CompiledOperatorOperand::Matcher { .. })
        {
            return Err(PlanError::Unsupported);
        }
        if transformations(generation, unit.member_start)
            .map_err(PlanError::Compiler)?
            .iter()
            .any(|kind| !matches!(kind, TransformationKind::None | TransformationKind::Lowercase))
        {
            return Err(PlanError::Unsupported);
        }
        let actions = generation
            .actions
            .get(
                member.action_start
                    ..member
                        .action_start
                        .checked_add(member.action_count)
                        .ok_or(PlanError::Compiler(CompilerError::PreflightArithmetic))?,
            )
            .ok_or(PlanError::Compiler(invalid_plan()))?;
        for action in actions {
            if !matches!(
                action.kind,
                ActionEnvelopeKind::Transformation
                    | ActionEnvelopeKind::Action(
                        ActionKind::AuditLog
                            | ActionKind::Block
                            | ActionKind::Deny
                            | ActionKind::Id
                            | ActionKind::Log
                            | ActionKind::NoAuditLog
                            | ActionKind::NoLog
                            | ActionKind::Pass
                            | ActionKind::Phase
                            | ActionKind::Severity
                            | ActionKind::Status
                            | ActionKind::Tag
                            | ActionKind::Ver
                            | ActionKind::Msg
                            | ActionKind::LogData
                    )
            ) {
                return Err(PlanError::Unsupported);
            }
            if let CompiledActionOperand::Template(template) = action.operand {
                require_static_template(generation, template)?;
            }
        }
        emit(JsonSubscriberDescriptor {
            member: unit.member_start,
            collection: target.collection,
            selector: target.selector,
        })?;
    }
    Ok(())
}

fn require_static_template(generation: &CompiledGeneration, id: usize) -> Result<(), PlanError> {
    let template = generation
        .templates
        .get(id)
        .ok_or(PlanError::Compiler(invalid_plan()))?;
    let end = template
        .segment_start
        .checked_add(template.segment_count)
        .ok_or(PlanError::Compiler(CompilerError::PreflightArithmetic))?;
    let segments = generation
        .segments
        .get(template.segment_start..end)
        .ok_or(PlanError::Compiler(invalid_plan()))?;
    if segments
        .iter()
        .any(|segment| matches!(segment.kind, CompiledSegmentKind::Macro(_)))
    {
        return Err(PlanError::Unsupported);
    }
    Ok(())
}

pub(super) fn transformations(
    generation: &CompiledGeneration,
    member: usize,
) -> Result<&[TransformationKind], CompilerError> {
    let member = generation.members.get(member).ok_or(invalid_plan())?;
    let Some(id) = member.transformation_plan else {
        return Ok(&[]);
    };
    let plan = generation.transformation_plans.get(id).ok_or(invalid_plan())?;
    generation
        .transformations
        .get(
            plan.transformation_start
                ..plan
                    .transformation_start
                    .checked_add(plan.transformation_count)
                    .ok_or(CompilerError::PreflightArithmetic)?,
        )
        .ok_or(invalid_plan())
}
