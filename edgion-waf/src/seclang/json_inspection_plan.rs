//! Immutable fused JSON dispatch. Owns no request values or runtime fact matrix.
use crate::bounded_sort::heapsort_by as sort;
use std::cmp::Ordering;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonInspectionUnsupportedReason {
    DynamicBroadValueOperand,
    DynamicRegexValueOperand,
    StructuralValueSelector,
}

/// Attributable capability finding, without source operands or request payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonInspectionUnsupported {
    pub rule_id: u64,
    /// Absolute compiled member index, including chain children.
    pub member: usize,
    pub reason: JsonInspectionUnsupportedReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JsonView {
    Name,
    Value,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JsonMemberMode {
    None,
    StaticFact(usize),
    SemanticLength,
    NameDynamic,
    NamedDeferred,
    Unsupported,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct JsonMemberPlan {
    pub name: JsonMemberMode,
    pub value: JsonMemberMode,
    pub summary_no_hit: bool,
}
impl Default for JsonMemberPlan {
    fn default() -> Self {
        Self {
            name: JsonMemberMode::None,
            value: JsonMemberMode::None,
            summary_no_hit: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct JsonGroup {
    pub member: usize,
    pub profile: usize,
    pub view: JsonView,
    pub multi_match: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct JsonProfile {
    pub transformation_plan: Option<usize>,
    pub view: JsonView,
    pub multi_match: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum JsonNameRef {
    Target(ByteRange),
    Macro(ByteRange),
}
impl JsonNameRef {
    pub(super) fn bytes(self, generation: &CompiledGeneration) -> Result<&[u8], CompilerError> {
        match self {
            Self::Target(range) => byte_range(&generation.target_bytes, range),
            Self::Macro(range) => byte_range(&generation.bytes, range),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct JsonLiteralRow {
    pub name: JsonNameRef,
    pub group: usize,
}
#[derive(Debug, Clone, Copy)]
pub(super) struct JsonRegexRow {
    pub selector: CompiledTargetSelector,
    pub group: usize,
}

#[derive(Debug, Default)]
pub(super) struct CompiledJsonPlan {
    pub capture_hits: capture_hit_plan::DomainHits,
    pub members: Vec<JsonMemberPlan>,
    pub xml_text: Vec<usize>,
    pub xml_attributes: Vec<usize>,
    pub xml_candidate_text: bool,
    pub xml_candidate_attributes: bool,
    pub xml_unsupported: Vec<super::xml_inspection_plan::XmlInspectionUnsupported>,
    pub groups: Vec<JsonGroup>,
    pub profiles: Vec<JsonProfile>,
    pub broad_names: Vec<usize>,
    pub broad_values: Vec<usize>,
    /// Canonical (profile, group) dispatch, populated only without selector rows.
    pub broad_only_order: Vec<usize>,
    /// Sorted by actual name bytes; equal names are a contiguous dispatch range.
    pub literals: Vec<JsonLiteralRow>,
    pub regexes: Vec<JsonRegexRow>,
    /// Sorted, byte-deduplicated explicit semantic-value retention demands.
    pub named_demands: Vec<JsonNameRef>,
    pub macro_demands: Vec<Option<usize>>,
    pub candidate_literals: Vec<JsonNameRef>,
    pub candidate_regexes: Vec<CompiledTargetSelector>,
    pub unsupported: Vec<JsonInspectionUnsupported>,
    /// Unique compiled Matcher IDs; bounds every profile's stage scan cache.
    pub matcher_scan_capacity: usize,
    pub needs_candidate_names: bool,
    pub needs_all_candidate_names: bool,
    pub needs_argument_count: bool,
    pub needs_combined_size: bool,
    pub needs_raw_source: bool,
}

fn invariant() -> CompilerError {
    CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    }
}

// Growth and allocator capacity, including reallocation overlap, use canonical
// generation/compilation bounds. No temporary uncharged per-rule collections.
fn push<T>(values: &mut Vec<T>, value: T, work: &mut MatcherCompileWork) -> Result<(), CompilerError> {
    if values.len() == values.capacity() {
        let old = values.capacity();
        let target = checked_inc(old)?.max(old.checked_mul(2).ok_or(CompilerError::PreflightArithmetic)?);
        work.retain_compiler_records::<T>(target - old)?;
        work.record_matcher_peak(
            old.checked_mul(size_of::<T>())
                .ok_or(CompilerError::PreflightArithmetic)?,
        )?;
        reserve_exact(values, target - values.len(), "compiled JSON inspection records")?;
        work.retain_compiler_records::<T>(values.capacity().checked_sub(target).ok_or_else(invariant)?)?;
    }
    values.push(value);
    Ok(())
}

fn compare_names(
    a: JsonNameRef,
    b: JsonNameRef,
    generation: &CompiledGeneration,
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<Ordering, CompilerError> {
    work.charge_records(1, limits)?;
    let a = a.bytes(generation)?;
    let b = b.bytes(generation)?;
    for (&a, &b) in a.iter().zip(b) {
        work.charge_comparison_bytes(1, limits)?;
        if a != b {
            return Ok(a.cmp(&b));
        }
    }
    Ok(a.len().cmp(&b.len()))
}

fn transforms(generation: &CompiledGeneration, member: usize) -> Result<&[TransformationKind], CompilerError> {
    let member = generation.members.get(member).ok_or_else(invariant)?;
    let Some(id) = member.transformation_plan else {
        return Ok(&[]);
    };
    let plan = generation.transformation_plans.get(id).ok_or_else(invariant)?;
    generation
        .transformations
        .get(plan.transformation_start..checked_add(plan.transformation_start, plan.transformation_count)?)
        .ok_or_else(invariant)
}
fn multi_match(
    generation: &CompiledGeneration,
    member: CompiledMemberPlan,
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<bool, CompilerError> {
    work.charge_records(member.action_count, limits)?;
    Ok(generation
        .actions
        .get(member.action_start..checked_add(member.action_start, member.action_count)?)
        .ok_or_else(invariant)?
        .iter()
        .any(|action| action.kind == ActionEnvelopeKind::Action(super::super::ActionKind::MultiMatch)))
}
fn template_static(
    generation: &CompiledGeneration,
    id: usize,
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<bool, CompilerError> {
    let template = generation.templates.get(id).ok_or_else(invariant)?;
    for segment in generation
        .segments
        .get(template.segment_start..checked_add(template.segment_start, template.segment_count)?)
        .ok_or_else(invariant)?
    {
        work.charge_records(1, limits)?;
        if matches!(segment.kind, CompiledSegmentKind::Macro(_)) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn group_hash(
    generation: &CompiledGeneration,
    member: usize,
    view: JsonView,
    multi: bool,
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<u64, CompilerError> {
    let operator = generation.operator_plans[member].ok_or_else(invariant)?;
    let mut hash = hash_transformations(transforms(generation, member)?, work, limits)?;
    work.hash_enum_name(&mut hash, operator.kind.as_str(), limits)?;
    for value in [
        u64::from(view == JsonView::Name),
        u64::from(multi),
        u64::from(operator.negated),
        generation.members[member].runtime_expansion_byte_limit as u64,
    ] {
        work.hash_u64(&mut hash, value, limits)?;
    }
    let mut fields = [0u64; 6];
    match operator.operand {
        CompiledOperatorOperand::String { relation, template } => {
            fields[0] = 1;
            fields[1] = relation as u64;
            fields[2] = template as u64;
        }
        CompiledOperatorOperand::Integer { relation, operand } => {
            fields[0] = 2;
            fields[1] = relation as u64;
            match operand {
                CompiledIntegerOperand::Static(value) => {
                    fields[2] = 0;
                    fields[3] = value as u64;
                }
                CompiledIntegerOperand::Template(id) => {
                    fields[2] = 1;
                    fields[3] = id as u64;
                }
            }
        }
        CompiledOperatorOperand::IpNetworks { start, count } => {
            fields[0] = 3;
            fields[1] = start as u64;
            fields[2] = count as u64;
        }
        CompiledOperatorOperand::ByteSet { words } => {
            fields[0] = 4;
            fields[1..5].copy_from_slice(&words);
        }
        CompiledOperatorOperand::ValidateUrlEncoding => fields[0] = 5,
        CompiledOperatorOperand::ValidateUtf8Encoding => fields[0] = 6,
        CompiledOperatorOperand::Unconditional => fields[0] = 7,
        CompiledOperatorOperand::DetectSqli { capture } => {
            fields[0] = 8;
            fields[1] = u64::from(capture);
        }
        CompiledOperatorOperand::DetectXss => fields[0] = 9,
        CompiledOperatorOperand::Regex(RegexPlanRef::Boolean(id)) => {
            fields[0] = 10;
            fields[1] = id as u64;
        }
        CompiledOperatorOperand::Regex(RegexPlanRef::Capture(id)) => {
            fields[0] = 11;
            fields[1] = id as u64;
        }
        CompiledOperatorOperand::Matcher {
            group,
            pattern_start,
            pattern_count,
        } => {
            fields[0] = 12;
            fields[1] = group as u64;
            fields[2] = pattern_start as u64;
            fields[3] = pattern_count as u64;
        }
        CompiledOperatorOperand::CapturePhrase(id) => {
            fields[0] = 13;
            fields[1] = id as u64;
        }
    }
    for field in fields {
        work.hash_u64(&mut hash, field, limits)?;
    }
    Ok(hash)
}

impl CompiledJsonPlan {
    pub(super) fn compile(
        generation: &CompiledGeneration,
        memory: &mut MatcherCompileWork,
        work: &mut CanonicalWork,
    ) -> Result<Self, CompilerError> {
        Self::compile_domain(generation, memory, work, InspectionDomain::Json)
    }
    pub(super) fn compile_domain(
        generation: &CompiledGeneration,
        memory: &mut MatcherCompileWork,
        work: &mut CanonicalWork,
        domain: InspectionDomain,
    ) -> Result<Self, CompilerError> {
        let mut plan = Self::default();
        let mut possible_groups = 0;
        for member in &generation.members {
            work.charge_records(1, memory.limits)?;
            if member.phase == Phase::RequestHeaders {
                continue;
            }
            if let Some(id) = member.target_plan {
                let target = generation.target_plans.get(id).ok_or_else(invariant)?;
                let mut views = [false; 2];
                for record in generation
                    .target_records
                    .get(target.record_start..checked_add(target.record_start, target.record_count)?)
                    .ok_or_else(invariant)?
                {
                    work.charge_records(1, memory.limits)?;
                    if record.modifier == TargetModifier::None {
                        if (domain == InspectionDomain::Json && record.collection == CollectionKind::Args)
                            || (domain == InspectionDomain::Xml && record.collection == CollectionKind::Xml)
                        {
                            views[0] = true;
                        }
                        if domain == InspectionDomain::Json && record.collection == CollectionKind::ArgsNames {
                            views[1] = true;
                        }
                    }
                }
                possible_groups = checked_add(possible_groups, views.iter().filter(|v| **v).count())?;
            }
        }
        let count = canonical_table_slots(possible_groups)?;
        memory.retain_working_records::<CanonicalSlot>(count)?;
        let mut group_slots = Vec::new();
        reserve_exact(&mut group_slots, count, "JSON predicate canonical table")?;
        memory.retain_working_records::<CanonicalSlot>(
            group_slots.capacity().checked_sub(count).ok_or_else(invariant)?,
        )?;
        group_slots.resize(count, CanonicalSlot::EMPTY);
        for _ in &generation.members {
            work.charge_records(1, memory.limits)?;
            push(&mut plan.members, JsonMemberPlan::default(), memory)?;
        }
        work.charge_records(generation.actions.len(), memory.limits)?;
        let controls = generation
            .actions
            .iter()
            .any(|action| action.kind == ActionEnvelopeKind::Action(super::super::ActionKind::Ctl));
        for unit in &generation.units {
            // Phase one sees query arguments only. It cannot require retaining a
            // JSON field that has not been published yet.
            if unit.phase == Phase::RequestHeaders {
                continue;
            }
            for id in unit.member_start..checked_add(unit.member_start, unit.member_count)? {
                work.charge_records(1, memory.limits)?;
                let member = *generation.members.get(id).ok_or_else(invariant)?;
                if domain == InspectionDomain::Json {
                    plan.macros_for_member(generation, member, memory, work)?;
                }
                let member_multi = multi_match(generation, member, work, memory.limits)?;
                work.charge_records(member.action_count, memory.limits)?;
                let member_capture = generation
                    .actions
                    .get(member.action_start..checked_add(member.action_start, member.action_count)?)
                    .ok_or_else(invariant)?
                    .iter()
                    .any(|action| action.kind == ActionEnvelopeKind::Action(super::super::ActionKind::Capture));
                let Some(target_id) = member.target_plan else {
                    continue;
                };
                let targets = generation.target_plans.get(target_id).ok_or_else(invariant)?;
                let records = generation
                    .target_records
                    .get(targets.record_start..checked_add(targets.record_start, targets.record_count)?)
                    .ok_or_else(invariant)?;
                work.charge_records(records.len(), memory.limits)?;
                let explicit_exclusions = records.iter().any(|record| record.modifier == TargetModifier::Exclude);
                for target in records {
                    work.charge_records(1, memory.limits)?;
                    if domain == InspectionDomain::Xml && target.collection == CollectionKind::Xml {
                        if let CompiledTargetSelector::Path(path) = target.selector {
                            if !matches!(byte_range(&generation.target_bytes, path)?, b"/*" | b"//@*") {
                                push(
                                    &mut plan.xml_unsupported,
                                    super::xml_inspection_plan::XmlInspectionUnsupported {
                                        rule_id: unit.id,
                                        member: id,
                                        reason: super::xml_inspection_plan::XmlInspectionUnsupportedReason::Path,
                                    },
                                    memory,
                                )?;
                                continue;
                            }
                        }
                    }
                    if target.modifier == TargetModifier::Exclude {
                        continue;
                    }
                    if target.collection == CollectionKind::RequestBody {
                        plan.needs_raw_source = true;
                    }
                    if target.collection == CollectionKind::ArgsCombinedSize {
                        plan.needs_combined_size = true;
                    }
                    let view = match (domain, target.collection) {
                        (InspectionDomain::Json, CollectionKind::Args)
                        | (InspectionDomain::Xml, CollectionKind::Xml) => JsonView::Value,
                        (InspectionDomain::Json, CollectionKind::ArgsNames) => JsonView::Name,
                        _ => continue,
                    };
                    if target.modifier == TargetModifier::Count {
                        plan.needs_argument_count = true;
                        if domain == InspectionDomain::Xml
                            || controls
                            || explicit_exclusions
                            || target.selector != CompiledTargetSelector::None
                        {
                            plan.require_candidate_domain(target.selector, generation, domain, memory)?;
                        }
                        continue;
                    }
                    plan.require_candidate_domain(target.selector, generation, domain, memory)?;
                    let operator = generation
                        .operator_plans
                        .get(id)
                        .copied()
                        .flatten()
                        .ok_or_else(invariant)?;
                    let is_static = match operator.operand {
                        CompiledOperatorOperand::String { template, .. } => {
                            template_static(generation, template, work, memory.limits)?
                        }
                        CompiledOperatorOperand::Integer {
                            operand: CompiledIntegerOperand::Template(template),
                            ..
                        } => template_static(generation, template, work, memory.limits)?,
                        _ => true,
                    };
                    let mode = if is_static {
                        let group = plan.group(generation, id, view, member_multi, &mut group_slots, memory, work)?;
                        plan.subscribe_domain(group, view, target.selector, generation, domain, memory)?;
                        JsonMemberMode::StaticFact(group)
                    } else if domain == InspectionDomain::Xml {
                        push(
                            &mut plan.xml_unsupported,
                            super::xml_inspection_plan::XmlInspectionUnsupported {
                                rule_id: unit.id,
                                member: id,
                                reason: super::xml_inspection_plan::XmlInspectionUnsupportedReason::DynamicValueOperand,
                            },
                            memory,
                        )?;
                        JsonMemberMode::Unsupported
                    } else if view == JsonView::Name {
                        JsonMemberMode::NameDynamic
                    } else if matches!(
                        transforms(generation, id)?,
                        [TransformationKind::Length] | [TransformationKind::None, TransformationKind::Length]
                    ) && !member_multi
                    {
                        JsonMemberMode::SemanticLength
                    } else {
                        match target.selector {
                            CompiledTargetSelector::Literal(name) => {
                                push(&mut plan.named_demands, JsonNameRef::Target(name), memory)?;
                                JsonMemberMode::NamedDeferred
                            }
                            selector => {
                                let reason = match selector {
                                    CompiledTargetSelector::None => {
                                        JsonInspectionUnsupportedReason::DynamicBroadValueOperand
                                    }
                                    CompiledTargetSelector::Regex { .. } => {
                                        JsonInspectionUnsupportedReason::DynamicRegexValueOperand
                                    }
                                    _ => JsonInspectionUnsupportedReason::StructuralValueSelector,
                                };
                                push(
                                    &mut plan.unsupported,
                                    JsonInspectionUnsupported {
                                        rule_id: unit.id,
                                        member: id,
                                        reason,
                                    },
                                    memory,
                                )?;
                                JsonMemberMode::Unsupported
                            }
                        }
                    };
                    let slot = if view == JsonView::Name {
                        &mut plan.members[id].name
                    } else {
                        &mut plan.members[id].value
                    };
                    if *slot != JsonMemberMode::Unsupported {
                        *slot = mode;
                    }
                    if matches!(mode, JsonMemberMode::StaticFact(_))
                        && unit.member_count == 1
                        && records.len() == 1
                        && target.collection == CollectionKind::Args
                        && target.modifier == TargetModifier::None
                        && target.selector == CompiledTargetSelector::None
                        && !member_multi
                        && !member_capture
                        && !matches!(
                            operator.operand,
                            CompiledOperatorOperand::DetectSqli { .. }
                                | CompiledOperatorOperand::DetectXss
                                | CompiledOperatorOperand::Regex(RegexPlanRef::Capture(_))
                                | CompiledOperatorOperand::CapturePhrase(_)
                        )
                    {
                        let recipe = transforms(generation, id)?;
                        work.charge_records(recipe.len(), memory.limits)?;
                        plan.members[id].summary_no_hit = recipe.iter().all(|transform| {
                            matches!(transform, TransformationKind::None | TransformationKind::Lowercase)
                        });
                    }
                }
            }
        }
        sort(&mut plan.literals, |a, b| {
            Ok(compare_names(a.name, b.name, generation, work, memory.limits)?.then(a.group.cmp(&b.group)))
        })?;
        let mut write = 0;
        for read in 0..plan.literals.len() {
            let row = plan.literals[read];
            if write == 0
                || row.group != plan.literals[write - 1].group
                || compare_names(row.name, plan.literals[write - 1].name, generation, work, memory.limits)?
                    != Ordering::Equal
            {
                plan.literals[write] = row;
                write += 1;
            }
        }
        plan.literals.truncate(write);
        for name in &plan.named_demands {
            push(&mut plan.candidate_literals, *name, memory)?;
        }
        for names in [&mut plan.named_demands, &mut plan.candidate_literals] {
            sort(names, |a, b| compare_names(a, b, generation, work, memory.limits))?;
            let mut write = 0;
            for read in 0..names.len() {
                let name = names[read];
                if write == 0
                    || compare_names(name, names[write - 1], generation, work, memory.limits)? != Ordering::Equal
                {
                    names[write] = name;
                    write += 1;
                }
            }
            names.truncate(write);
        }
        for reference in &generation.macros {
            work.charge_records(1, memory.limits)?;
            let mut demand = None;
            if reference.source == MacroSource::Args {
                let (mut low, mut high) = (0, plan.named_demands.len());
                while low < high {
                    let mid = low + (high - low) / 2;
                    match compare_names(
                        JsonNameRef::Macro(reference.key),
                        plan.named_demands[mid],
                        generation,
                        work,
                        memory.limits,
                    )? {
                        Ordering::Equal => {
                            demand = Some(mid);
                            break;
                        }
                        Ordering::Less => high = mid,
                        Ordering::Greater => low = mid + 1,
                    }
                }
            }
            push(&mut plan.macro_demands, demand, memory)?;
        }
        plan.needs_candidate_names |= !plan.named_demands.is_empty();
        for broad in [
            &mut plan.broad_names,
            &mut plan.broad_values,
            &mut plan.xml_text,
            &mut plan.xml_attributes,
        ] {
            sort(broad, |a, b| {
                work.charge_records(1, memory.limits)?;
                Ok(a.cmp(&b))
            })?;
            broad.dedup();
        }
        work.charge_records(group_slots.len(), memory.limits)?;
        group_slots.fill(CanonicalSlot::EMPTY);
        for group in &mut plan.groups {
            let member = generation.members[group.member];
            let profile = JsonProfile {
                transformation_plan: if transforms(generation, group.member)?.is_empty() {
                    None
                } else {
                    member.transformation_plan
                },
                view: group.view,
                multi_match: group.multi_match,
            };
            let mut hash = FNV1A_OFFSET_BASIS;
            for value in [
                profile.transformation_plan.map_or(u64::MAX, |id| id as u64),
                u64::from(profile.view == JsonView::Name),
                u64::from(profile.multi_match),
            ] {
                work.hash_u64(&mut hash, value, memory.limits)?;
            }
            let mask = group_slots.len().checked_sub(1).ok_or_else(invariant)?;
            let mut position = hash as usize & mask;
            loop {
                work.charge_probe(memory.limits)?;
                let slot = group_slots[position];
                if slot.id == usize::MAX {
                    let id = plan.profiles.len();
                    push(&mut plan.profiles, profile, memory)?;
                    group_slots[position] = CanonicalSlot { hash, id };
                    group.profile = id;
                    break;
                }
                if slot.hash == hash {
                    work.charge_records(1, memory.limits)?;
                    if plan.profiles[slot.id] == profile {
                        group.profile = slot.id;
                        break;
                    }
                }
                position = (position + 1) & mask;
            }
        }
        if plan.literals.is_empty() && plan.regexes.is_empty() {
            for broad in [&mut plan.xml_text, &mut plan.xml_attributes] {
                sort(broad, |a, b| {
                    work.charge_records(1, memory.limits)?;
                    Ok((plan.groups[a].profile, a).cmp(&(plan.groups[b].profile, b)))
                })?;
            }
            for &id in plan.broad_names.iter().chain(&plan.broad_values) {
                work.charge_records(1, memory.limits)?;
                push(&mut plan.broad_only_order, id, memory)?;
            }
            sort(&mut plan.broad_only_order, |a, b| {
                work.charge_records(1, memory.limits)?;
                Ok((plan.groups[a].profile, a).cmp(&(plan.groups[b].profile, b)))
            })?;
        }
        let mut matcher_count = 0;
        for group in &plan.groups {
            work.charge_records(1, memory.limits)?;
            if matches!(
                generation.operator_plans[group.member].ok_or_else(invariant)?.operand,
                CompiledOperatorOperand::Matcher { .. }
            ) {
                matcher_count = checked_inc(matcher_count)?;
            }
        }
        memory.retain_working_records::<usize>(matcher_count)?;
        let mut matcher_ids = Vec::new();
        reserve_exact(&mut matcher_ids, matcher_count, "JSON stage matcher identities")?;
        memory.retain_working_records::<usize>(
            matcher_ids
                .capacity()
                .checked_sub(matcher_count)
                .ok_or_else(invariant)?,
        )?;
        for group in &plan.groups {
            work.charge_records(1, memory.limits)?;
            if let CompiledOperatorOperand::Matcher { group, .. } =
                generation.operator_plans[group.member].ok_or_else(invariant)?.operand
            {
                matcher_ids.push(group);
            }
        }
        sort(&mut matcher_ids, |a, b| {
            work.charge_records(1, memory.limits)?;
            Ok(a.cmp(&b))
        })?;
        work.charge_records(matcher_ids.len(), memory.limits)?;
        matcher_ids.dedup();
        plan.matcher_scan_capacity = matcher_ids.len();
        let matcher_capacity = matcher_ids.capacity();
        drop(matcher_ids);
        memory.drop_working_records::<usize>(matcher_capacity);
        memory.drop_working_records::<CanonicalSlot>(group_slots.capacity());
        Ok(plan)
    }

    /// A sole broad value profile is applicable to every surviving candidate.
    pub(super) fn summary_profile(&self) -> Option<usize> {
        (self.profiles.len() == 1
            && self.profiles[0].view == JsonView::Value
            && !self.profiles[0].multi_match
            && !self.broad_values.is_empty()
            && self.broad_names.is_empty()
            && self.literals.is_empty()
            && self.regexes.is_empty()
            && self.candidate_literals.is_empty()
            && self.candidate_regexes.is_empty()
            && self.named_demands.is_empty())
        .then_some(0)
    }

    #[allow(clippy::too_many_arguments)]
    fn group(
        &mut self,
        generation: &CompiledGeneration,
        member: usize,
        view: JsonView,
        multi: bool,
        slots: &mut [CanonicalSlot],
        memory: &mut MatcherCompileWork,
        work: &mut CanonicalWork,
    ) -> Result<usize, CompilerError> {
        let transformation = transforms(generation, member)?;
        let hash = group_hash(generation, member, view, multi, work, memory.limits)?;
        let mask = slots.len().checked_sub(1).ok_or_else(invariant)?;
        let mut position = hash as usize & mask;
        loop {
            work.charge_probe(memory.limits)?;
            let slot = slots[position];
            if slot.id == usize::MAX {
                break;
            }
            if slot.hash == hash {
                work.charge_records(1, memory.limits)?;
                let group = &self.groups[slot.id];
                if group.view == view
                    && group.multi_match == multi
                    && generation.members[group.member].runtime_expansion_byte_limit
                        == generation.members[member].runtime_expansion_byte_limit
                    && generation.operator_plans[group.member] == generation.operator_plans[member]
                {
                    let existing = transforms(generation, group.member)?;
                    work.charge_records(existing.len().min(transformation.len()), memory.limits)?;
                    if existing == transformation {
                        return Ok(slot.id);
                    }
                }
            }
            position = (position + 1) & mask;
        }
        let id = self.groups.len();
        push(
            &mut self.groups,
            JsonGroup {
                member,
                profile: usize::MAX,
                view,
                multi_match: multi,
            },
            memory,
        )?;
        slots[position] = CanonicalSlot { hash, id };
        Ok(id)
    }

    fn require_candidate_domain(
        &mut self,
        selector: CompiledTargetSelector,
        generation: &CompiledGeneration,
        domain: InspectionDomain,
        memory: &mut MatcherCompileWork,
    ) -> Result<(), CompilerError> {
        if domain == InspectionDomain::Json {
            return self.require_candidate(selector, memory);
        }
        self.needs_candidate_names = true;
        match selector {
            CompiledTargetSelector::None => {
                self.xml_candidate_text = true;
                self.xml_candidate_attributes = true;
            }
            CompiledTargetSelector::Path(path) => match byte_range(&generation.target_bytes, path)? {
                b"/*" => self.xml_candidate_text = true,
                b"//@*" => self.xml_candidate_attributes = true,
                _ => return Err(invariant()),
            },
            CompiledTargetSelector::Literal(name) => {
                push(&mut self.candidate_literals, JsonNameRef::Target(name), memory)?
            }
            selector @ CompiledTargetSelector::Regex { .. } => push(&mut self.candidate_regexes, selector, memory)?,
        }
        Ok(())
    }
    fn subscribe_domain(
        &mut self,
        group: usize,
        view: JsonView,
        selector: CompiledTargetSelector,
        generation: &CompiledGeneration,
        domain: InspectionDomain,
        memory: &mut MatcherCompileWork,
    ) -> Result<(), CompilerError> {
        if domain == InspectionDomain::Json {
            return self.subscribe(group, view, selector, memory);
        }
        match selector {
            CompiledTargetSelector::None => {
                push(&mut self.xml_text, group, memory)?;
                push(&mut self.xml_attributes, group, memory)?;
            }
            CompiledTargetSelector::Path(path) => match byte_range(&generation.target_bytes, path)? {
                b"/*" => push(&mut self.xml_text, group, memory)?,
                b"//@*" => push(&mut self.xml_attributes, group, memory)?,
                _ => return Err(invariant()),
            },
            _ => self.subscribe(group, view, selector, memory)?,
        }
        Ok(())
    }

    fn require_candidate(
        &mut self,
        selector: CompiledTargetSelector,
        memory: &mut MatcherCompileWork,
    ) -> Result<(), CompilerError> {
        self.needs_candidate_names = true;
        match selector {
            CompiledTargetSelector::None => self.needs_all_candidate_names = true,
            CompiledTargetSelector::Literal(name) => {
                push(&mut self.candidate_literals, JsonNameRef::Target(name), memory)?
            }
            selector => push(&mut self.candidate_regexes, selector, memory)?,
        }
        Ok(())
    }

    fn subscribe(
        &mut self,
        group: usize,
        view: JsonView,
        selector: CompiledTargetSelector,
        memory: &mut MatcherCompileWork,
    ) -> Result<(), CompilerError> {
        match selector {
            CompiledTargetSelector::None => push(
                if view == JsonView::Name {
                    &mut self.broad_names
                } else {
                    &mut self.broad_values
                },
                group,
                memory,
            ),
            CompiledTargetSelector::Literal(name) => push(
                &mut self.literals,
                JsonLiteralRow {
                    name: JsonNameRef::Target(name),
                    group,
                },
                memory,
            ),
            selector => push(&mut self.regexes, JsonRegexRow { selector, group }, memory),
        }
    }

    fn template_demands(
        &mut self,
        generation: &CompiledGeneration,
        template: usize,
        memory: &mut MatcherCompileWork,
        work: &mut CanonicalWork,
    ) -> Result<(), CompilerError> {
        let template = generation.templates.get(template).ok_or_else(invariant)?;
        for segment in generation
            .segments
            .get(template.segment_start..checked_add(template.segment_start, template.segment_count)?)
            .ok_or_else(invariant)?
        {
            work.charge_records(1, memory.limits)?;
            if let CompiledSegmentKind::Macro(id) = segment.kind {
                let value = generation.macros.get(id).ok_or_else(invariant)?;
                if value.source == MacroSource::Args {
                    push(&mut self.named_demands, JsonNameRef::Macro(value.key), memory)?;
                }
            }
        }
        Ok(())
    }
    fn macros_for_member(
        &mut self,
        generation: &CompiledGeneration,
        member: CompiledMemberPlan,
        memory: &mut MatcherCompileWork,
        work: &mut CanonicalWork,
    ) -> Result<(), CompilerError> {
        if let Some(template) = member.operator_template {
            self.template_demands(generation, template, memory, work)?;
        }
        for action in generation
            .actions
            .get(member.action_start..checked_add(member.action_start, member.action_count)?)
            .ok_or_else(invariant)?
        {
            work.charge_records(1, memory.limits)?;
            match action.operand {
                CompiledActionOperand::Template(template) => {
                    self.template_demands(generation, template, memory, work)?
                }
                CompiledActionOperand::SetVar(id) => {
                    let setvar = generation.setvars.get(id).ok_or_else(invariant)?;
                    self.template_demands(generation, setvar.key, memory, work)?;
                    if let Some(value) = setvar.value {
                        self.template_demands(generation, value, memory, work)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Conservative byte-comparison charge for both binary-search boundaries.
    /// The collector must charge this against physical work before lookup.
    pub(super) fn literal_lookup_work(&self, name_len: usize) -> Result<usize, CompilerError> {
        let steps = usize::BITS - self.literals.len().leading_zeros();
        checked_inc(name_len)?
            .checked_mul(steps as usize)
            .and_then(|work| work.checked_mul(2))
            .ok_or(CompilerError::PreflightArithmetic)
    }

    /// Lookup visits only matching literal subscribers. The caller first charges
    /// literal_lookup_work; collector stamps then dedup overlap with broad and
    /// regex subscriptions for this scalar. Returned rows still cost dispatch work.
    pub(super) fn literal_matches<'a>(
        &'a self,
        generation: &CompiledGeneration,
        name: &[u8],
    ) -> Result<&'a [JsonLiteralRow], CompilerError> {
        let mut low = 0;
        let mut high = self.literals.len();
        while low < high {
            let mid = low + (high - low) / 2;
            if self.literals[mid].name.bytes(generation)? < name {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let start = low;
        high = self.literals.len();
        while low < high {
            let mid = low + (high - low) / 2;
            if self.literals[mid].name.bytes(generation)? <= name {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        Ok(&self.literals[start..low])
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_inspection_plan_test.rs"]
mod tests;
