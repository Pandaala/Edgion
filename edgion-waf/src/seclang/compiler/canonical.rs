//! Canonical target and pattern planning, slot layout and regex program interning.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CandidateTargetSelector {
    None,
    Literal(ByteRange),
    Path(ByteRange),
    Regex { style: RegexStyle, program: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CandidateTargetRecord {
    pub(super) collection: CollectionKind,
    pub(super) modifier: TargetModifier,
    pub(super) selector: CandidateTargetSelector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CandidateTargetPlan {
    pub(super) record_start: usize,
    pub(super) record_count: usize,
}

/// One candidate target plan with the record and selector-byte tables it indexes.
#[derive(Debug, Clone, Copy)]
pub(super) struct TargetPlanRef<'a> {
    pub(super) plan: CandidateTargetPlan,
    pub(super) records: &'a [CandidateTargetRecord],
    pub(super) bytes: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CanonicalSlot {
    pub(super) hash: u64,
    pub(super) id: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CandidatePatternKind {
    Phrase,
    Regex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CandidatePattern {
    pub(super) group: usize,
    pub(super) kind: CandidatePatternKind,
    pub(super) bytes: ByteRange,
    pub(super) mapping: CompiledPatternMapping,
    pub(super) next: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MemberMatcherBinding {
    pub(super) member: usize,
    pub(super) group: usize,
    pub(super) pattern_start: usize,
    pub(super) pattern_count: usize,
    pub(super) kind: OperatorKind,
    pub(super) negated: bool,
}

pub(super) const fn phase_index(phase: Phase) -> usize {
    match phase {
        Phase::RequestHeaders => 0,
        Phase::RequestBody => 1,
        Phase::ResponseHeaders => 2,
        Phase::ResponseBody => 3,
        Phase::Logging => 4,
    }
}

impl CanonicalSlot {
    pub(super) const EMPTY: Self = Self {
        hash: 0,
        id: usize::MAX,
    };

    const fn is_empty(self) -> bool {
        self.id == usize::MAX
    }
}

#[derive(Clone, Copy)]
pub(super) struct RegexProgramSource<'source> {
    source: &'source str,
    ascii_case_insensitive: bool,
}

/// Compilation-local identity for exact source and byte compilation profile.
/// Sources borrow the validated arena; no source copy survives publication.
pub(super) struct RegexProgramInterner<'source> {
    pub(super) slots: Vec<CanonicalSlot>,
    pub(super) sources: Vec<RegexProgramSource<'source>>,
}
impl<'source> RegexProgramInterner<'source> {
    pub(super) fn new(occurrences: usize, memory: &mut MatcherCompileWork) -> Result<Self, CompilerError> {
        // A single occurrence cannot share a program, so it needs no index.
        let references = if occurrences > 1 { occurrences } else { 0 };
        let count = canonical_table_slots(references)?;
        memory.retain_working_records::<CanonicalSlot>(count)?;
        let mut slots = Vec::new();
        reserve_exact(&mut slots, count, "regex canonical table")?;
        memory.retain_working_records::<CanonicalSlot>(
            slots
                .capacity()
                .checked_sub(count)
                .ok_or(CompilerError::PreflightArithmetic)?,
        )?;
        slots.resize(count, CanonicalSlot::EMPTY);
        memory.retain_working_records::<RegexProgramSource<'_>>(references)?;
        let mut sources = Vec::new();
        reserve_exact(&mut sources, references, "regex source references")?;
        memory.retain_working_records::<RegexProgramSource<'_>>(
            sources
                .capacity()
                .checked_sub(references)
                .ok_or(CompilerError::PreflightArithmetic)?,
        )?;
        Ok(Self { slots, sources })
    }

    pub(super) fn compile(
        &mut self,
        source: &'source str,
        location: SemanticLocation,
        programs: &mut Vec<CompiledCaptureRegexProgram>,
        memory: &mut MatcherCompileWork,
        work: &mut CanonicalWork,
    ) -> Result<usize, CompilerError> {
        self.compile_with(source, false, programs, memory, work, |memory| {
            memory.compile_capture_regex(source, location, true)
        })
    }

    pub(super) fn compile_with<P>(
        &mut self,
        source: &'source str,
        ascii_case_insensitive: bool,
        programs: &mut Vec<P>,
        memory: &mut MatcherCompileWork,
        work: &mut CanonicalWork,
        build: impl FnOnce(&mut MatcherCompileWork) -> Result<P, CompilerError>,
    ) -> Result<usize, CompilerError> {
        let mut hash = FNV1A_OFFSET_BASIS;
        let position = if self.slots.is_empty() {
            None
        } else {
            work.hash_u64(&mut hash, u64::from(ascii_case_insensitive), memory.limits)?;
            work.hash_bytes(&mut hash, source.as_bytes(), memory.limits)?;
            let mask = self
                .slots
                .len()
                .checked_sub(1)
                .ok_or(CompilerError::PreflightArithmetic)?;
            let mut position = hash as usize & mask;
            loop {
                work.charge_probe(memory.limits)?;
                let slot = self.slots[position];
                if slot.is_empty() {
                    break Some(position);
                }
                if slot.hash == hash {
                    work.charge_records(1, memory.limits)?;
                    let previous = self.sources.get(slot.id).ok_or(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    })?;
                    let mut equal = previous.ascii_case_insensitive == ascii_case_insensitive
                        && previous.source.len() == source.len();
                    if equal {
                        for (&a, &b) in previous.source.as_bytes().iter().zip(source.as_bytes()) {
                            work.charge_comparison_bytes(1, memory.limits)?;
                            if a != b {
                                equal = false;
                                break;
                            }
                        }
                    }
                    if equal {
                        return Ok(slot.id);
                    }
                }
                position = (position + 1) & mask;
            }
        };
        if programs.len() == programs.capacity()
            || (position.is_some() && self.sources.len() == self.sources.capacity())
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            });
        }
        let program = build(memory)?;
        let id = programs.len();
        programs.push(program);
        if let Some(position) = position {
            self.sources.push(RegexProgramSource {
                source,
                ascii_case_insensitive,
            });
            self.slots[position] = CanonicalSlot { hash, id };
        }
        Ok(id)
    }

    pub(super) fn release(self, memory: &mut MatcherCompileWork) {
        let slots = self.slots.capacity();
        let sources = self.sources.capacity();
        drop(self);
        memory.drop_working_records::<CanonicalSlot>(slots);
        memory.drop_working_records::<RegexProgramSource<'_>>(sources);
    }
}

pub(super) const FNV1A_OFFSET_BASIS: u64 = 14_695_981_039_346_656_037;
const FNV1A_PRIME: u64 = 1_099_511_628_211;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct CanonicalWork {
    pub(super) probes: usize,
    pub(super) record_comparisons: usize,
    pub(super) comparison_bytes: usize,
    pub(super) hash_bytes: usize,
}

impl CanonicalWork {
    pub(super) fn charge_probe(&mut self, limits: CompilerLimits) -> Result<(), CompilerError> {
        self.probes = checked_inc(self.probes)?;
        if self.probes > limits.max_group_table_probes {
            return Err(CompilerError::GroupTableProbeLimit {
                limit: limits.max_group_table_probes,
            });
        }
        Ok(())
    }

    pub(crate) fn charge_records(&mut self, count: usize, limits: CompilerLimits) -> Result<(), CompilerError> {
        self.record_comparisons = checked_add(self.record_comparisons, count)?;
        if self.record_comparisons > limits.max_group_key_record_comparisons {
            return Err(CompilerError::GroupKeyRecordComparisonLimit {
                limit: limits.max_group_key_record_comparisons,
            });
        }
        Ok(())
    }

    pub(crate) fn charge_comparison_bytes(
        &mut self,
        count: usize,
        limits: CompilerLimits,
    ) -> Result<(), CompilerError> {
        self.comparison_bytes = checked_add(self.comparison_bytes, count)?;
        if self.comparison_bytes > limits.max_group_key_comparison_bytes {
            return Err(CompilerError::GroupKeyComparisonByteLimit {
                limit: limits.max_group_key_comparison_bytes,
            });
        }
        Ok(())
    }

    pub(super) fn hash_bytes(
        &mut self,
        hash: &mut u64,
        bytes: &[u8],
        limits: CompilerLimits,
    ) -> Result<(), CompilerError> {
        self.hash_bytes = checked_add(self.hash_bytes, bytes.len())?;
        if self.hash_bytes > limits.max_group_key_hash_bytes {
            return Err(CompilerError::GroupKeyHashByteLimit {
                limit: limits.max_group_key_hash_bytes,
            });
        }
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(FNV1A_PRIME);
        }
        Ok(())
    }

    pub(super) fn hash_u64(&mut self, hash: &mut u64, value: u64, limits: CompilerLimits) -> Result<(), CompilerError> {
        self.hash_bytes(hash, &value.to_le_bytes(), limits)
    }

    pub(super) fn hash_enum_name(
        &mut self,
        hash: &mut u64,
        value: &str,
        limits: CompilerLimits,
    ) -> Result<(), CompilerError> {
        self.hash_u64(
            hash,
            u64::try_from(value.len()).map_err(|_| CompilerError::PreflightArithmetic)?,
            limits,
        )?;
        self.hash_bytes(hash, value.as_bytes(), limits)
    }
}

pub(super) fn canonical_table_slots(candidates: usize) -> Result<usize, CompilerError> {
    if candidates == 0 {
        return Ok(0);
    }
    candidates
        .checked_mul(2)
        .and_then(|count| count.max(2).checked_next_power_of_two())
        .ok_or(CompilerError::PreflightArithmetic)
}

pub(super) fn byte_range(bytes: &[u8], range: ByteRange) -> Result<&[u8], CompilerError> {
    let end = range
        .start
        .checked_add(range.len)
        .ok_or(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        })?;
    bytes.get(range.start..end).ok_or(CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    })
}

fn modifier_code(modifier: TargetModifier) -> u64 {
    match modifier {
        TargetModifier::None => 0,
        TargetModifier::Exclude => 1,
        TargetModifier::Count => 2,
    }
}

fn selector_code(selector: CandidateTargetSelector) -> u64 {
    match selector {
        CandidateTargetSelector::None => 0,
        CandidateTargetSelector::Literal(_) => 1,
        CandidateTargetSelector::Path(_) => 2,
        CandidateTargetSelector::Regex { .. } => 3,
    }
}

fn regex_style_code(style: RegexStyle) -> u64 {
    match style {
        RegexStyle::Bare => 0,
        RegexStyle::SingleQuoted => 1,
    }
}

pub(super) fn hash_target_plan(
    candidate: TargetPlanRef<'_>,
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<u64, CompilerError> {
    let TargetPlanRef { plan, records, bytes } = candidate;
    let mut hash = FNV1A_OFFSET_BASIS;
    work.hash_u64(
        &mut hash,
        u64::try_from(plan.record_count).map_err(|_| CompilerError::PreflightArithmetic)?,
        limits,
    )?;
    let end = plan
        .record_start
        .checked_add(plan.record_count)
        .ok_or(CompilerError::PreflightArithmetic)?;
    let plan_records = records
        .get(plan.record_start..end)
        .ok_or(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        })?;
    for record in plan_records {
        work.hash_enum_name(&mut hash, record.collection.as_str(), limits)?;
        work.hash_u64(&mut hash, modifier_code(record.modifier), limits)?;
        work.hash_u64(&mut hash, selector_code(record.selector), limits)?;
        match record.selector {
            CandidateTargetSelector::None => {}
            CandidateTargetSelector::Literal(range) | CandidateTargetSelector::Path(range) => {
                let value = byte_range(bytes, range)?;
                work.hash_u64(
                    &mut hash,
                    u64::try_from(value.len()).map_err(|_| CompilerError::PreflightArithmetic)?,
                    limits,
                )?;
                work.hash_bytes(&mut hash, value, limits)?;
            }
            CandidateTargetSelector::Regex { style, program } => {
                work.hash_u64(&mut hash, regex_style_code(style), limits)?;
                work.hash_u64(
                    &mut hash,
                    u64::try_from(program).map_err(|_| CompilerError::PreflightArithmetic)?,
                    limits,
                )?;
            }
        }
    }
    Ok(hash)
}

fn target_plan_equal(
    candidate: TargetPlanRef<'_>,
    canonical: CompiledTargetPlan,
    canonical_records: &[CompiledTargetRecord],
    canonical_bytes: &[u8],
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<bool, CompilerError> {
    let TargetPlanRef {
        plan: candidate,
        records: candidate_records,
        bytes: candidate_bytes,
    } = candidate;
    if candidate.record_count != canonical.record_count {
        return Ok(false);
    }
    let candidate_end = candidate
        .record_start
        .checked_add(candidate.record_count)
        .ok_or(CompilerError::PreflightArithmetic)?;
    let canonical_end = canonical
        .record_start
        .checked_add(canonical.record_count)
        .ok_or(CompilerError::PreflightArithmetic)?;
    let lhs = candidate_records
        .get(candidate.record_start..candidate_end)
        .ok_or(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        })?;
    let rhs = canonical_records
        .get(canonical.record_start..canonical_end)
        .ok_or(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        })?;
    for (lhs, rhs) in lhs.iter().zip(rhs) {
        work.charge_records(1, limits)?;
        if lhs.collection != rhs.collection || lhs.modifier != rhs.modifier {
            return Ok(false);
        }
        let equal = match (lhs.selector, rhs.selector) {
            (CandidateTargetSelector::None, CompiledTargetSelector::None) => true,
            (CandidateTargetSelector::Literal(left), CompiledTargetSelector::Literal(right))
            | (CandidateTargetSelector::Path(left), CompiledTargetSelector::Path(right)) => {
                let left = byte_range(candidate_bytes, left)?;
                let right = byte_range(canonical_bytes, right)?;
                if left.len() != right.len() {
                    false
                } else {
                    work.charge_comparison_bytes(left.len(), limits)?;
                    left == right
                }
            }
            (
                CandidateTargetSelector::Regex {
                    style: left_style,
                    program: left_program,
                },
                CompiledTargetSelector::Regex {
                    style: right_style,
                    program: right_program,
                },
            ) => left_style == right_style && left_program == right_program,
            _ => false,
        };
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}

fn copy_target_plan(
    candidate: TargetPlanRef<'_>,
    target_records: &mut Vec<CompiledTargetRecord>,
    target_bytes: &mut Vec<u8>,
    target_plans: &mut Vec<CompiledTargetPlan>,
    work: &mut MatcherCompileWork,
) -> Result<usize, CompilerError> {
    let TargetPlanRef {
        plan: candidate,
        records: candidate_records,
        bytes: candidate_bytes,
    } = candidate;
    let mut other_body_dependency = false;
    let mut reads_body_error = false;
    let record_start = target_records.len();
    let end = candidate
        .record_start
        .checked_add(candidate.record_count)
        .ok_or(CompilerError::PreflightArithmetic)?;
    for record in candidate_records
        .get(candidate.record_start..end)
        .ok_or(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        })?
    {
        let selector = match record.selector {
            CandidateTargetSelector::None => CompiledTargetSelector::None,
            CandidateTargetSelector::Literal(range) | CandidateTargetSelector::Path(range) => {
                let source = byte_range(candidate_bytes, range)?;
                let start = target_bytes.len();
                work.reserve_canonical(target_bytes, source.len(), "canonical target bytes")?;
                target_bytes.extend_from_slice(source);
                let owned = ByteRange {
                    start,
                    len: source.len(),
                };
                match record.selector {
                    CandidateTargetSelector::Literal(_) => CompiledTargetSelector::Literal(owned),
                    CandidateTargetSelector::Path(_) => CompiledTargetSelector::Path(owned),
                    _ => unreachable!("the enclosing arm admits only Literal and Path selectors"),
                }
            }
            CandidateTargetSelector::Regex { style, program } => CompiledTargetSelector::Regex { style, program },
        };
        work.reserve_canonical(target_records, 1, "canonical target records")?;
        if record.collection == CollectionKind::ReqbodyError {
            // The scalar has no selectors. A whole-target exclusion removes all
            // earlier reads (including counts), and a later read adds it back.
            reads_body_error = record.modifier != TargetModifier::Exclude;
        } else {
            other_body_dependency |= generation::collection_requires_request_body(record.collection);
        }
        target_records.push(CompiledTargetRecord {
            collection: record.collection,
            modifier: record.modifier,
            selector,
        });
    }
    work.reserve_canonical(target_plans, 1, "canonical target plans")?;
    let id = target_plans.len();
    target_plans.push(CompiledTargetPlan {
        requires_request_body: other_body_dependency || reads_body_error,
        reads_body_error,
        record_start,
        record_count: candidate.record_count,
    });
    Ok(id)
}

pub(super) fn hash_transformations(
    values: &[TransformationKind],
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<u64, CompilerError> {
    let mut hash = FNV1A_OFFSET_BASIS;
    work.hash_u64(
        &mut hash,
        u64::try_from(values.len()).map_err(|_| CompilerError::PreflightArithmetic)?,
        limits,
    )?;
    for value in values {
        work.hash_enum_name(&mut hash, value.as_str(), limits)?;
    }
    Ok(hash)
}

fn transformation_plan_equal(
    candidate: &[TransformationKind],
    canonical: CompiledTransformationPlan,
    values: &[TransformationKind],
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<bool, CompilerError> {
    if candidate.len() != canonical.transformation_count {
        return Ok(false);
    }
    let end = canonical
        .transformation_start
        .checked_add(canonical.transformation_count)
        .ok_or(CompilerError::PreflightArithmetic)?;
    let canonical_values = values
        .get(canonical.transformation_start..end)
        .ok_or(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        })?;
    for (left, right) in candidate.iter().zip(canonical_values) {
        work.charge_records(1, limits)?;
        if left != right {
            return Ok(false);
        }
    }
    Ok(true)
}

// Independent outputs: the three canonical tables are built before their
// owning GroupedMatcherState exists, beside two separate work accounts.
#[allow(clippy::too_many_arguments)]
pub(super) fn insert_target_plan(
    candidate: TargetPlanRef<'_>,
    hash: u64,
    slots: &mut [CanonicalSlot],
    target_records: &mut Vec<CompiledTargetRecord>,
    target_bytes: &mut Vec<u8>,
    target_plans: &mut Vec<CompiledTargetPlan>,
    canonical_work: &mut CanonicalWork,
    compile_work: &mut MatcherCompileWork,
) -> Result<usize, CompilerError> {
    let mask = slots.len().checked_sub(1).ok_or(CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    })?;
    let mut index = (hash as usize) & mask;
    loop {
        canonical_work.charge_probe(compile_work.limits)?;
        let slot = slots[index];
        if slot.is_empty() {
            let id = copy_target_plan(candidate, target_records, target_bytes, target_plans, compile_work)?;
            slots[index] = CanonicalSlot { hash, id };
            return Ok(id);
        }
        if slot.hash == hash
            && target_plan_equal(
                candidate,
                target_plans[slot.id],
                target_records,
                target_bytes,
                canonical_work,
                compile_work.limits,
            )?
        {
            return Ok(slot.id);
        }
        index = (index + 1) & mask;
    }
}

pub(super) fn insert_transformation_plan(
    candidate: &[TransformationKind],
    hash: u64,
    slots: &mut [CanonicalSlot],
    values: &mut Vec<TransformationKind>,
    plans: &mut Vec<CompiledTransformationPlan>,
    canonical_work: &mut CanonicalWork,
    compile_work: &mut MatcherCompileWork,
) -> Result<usize, CompilerError> {
    let mask = slots.len().checked_sub(1).ok_or(CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    })?;
    let mut index = (hash as usize) & mask;
    loop {
        canonical_work.charge_probe(compile_work.limits)?;
        let slot = slots[index];
        if slot.is_empty() {
            let transformation_start = values.len();
            compile_work.reserve_canonical(values, candidate.len(), "canonical transformations")?;
            values.extend_from_slice(candidate);
            compile_work.reserve_canonical(plans, 1, "canonical transformation plans")?;
            let id = plans.len();
            plans.push(CompiledTransformationPlan {
                transformation_start,
                transformation_count: candidate.len(),
            });
            slots[index] = CanonicalSlot { hash, id };
            return Ok(id);
        }
        if slot.hash == hash
            && transformation_plan_equal(candidate, plans[slot.id], values, canonical_work, compile_work.limits)?
        {
            return Ok(slot.id);
        }
        index = (index + 1) & mask;
    }
}

fn phase_code(phase: Phase) -> u64 {
    match phase {
        Phase::RequestHeaders => 1,
        Phase::RequestBody => 2,
        Phase::ResponseHeaders => 3,
        Phase::ResponseBody => 4,
        Phase::Logging => 5,
    }
}

fn capture_mode_code(mode: CaptureMode) -> u64 {
    match mode {
        CaptureMode::None => 0,
        CaptureMode::RegexGroups => 1,
        CaptureMode::PhraseSpans => 2,
    }
}

fn byte_view_code(view: ByteView) -> u64 {
    match view {
        ByteView::RawBytes => 0,
    }
}

pub(super) fn hash_group_key(
    key: CompiledMatcherGroupKey,
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<u64, CompilerError> {
    let mut hash = FNV1A_OFFSET_BASIS;
    for value in [
        phase_code(key.phase),
        u64::try_from(key.target_plan).map_err(|_| CompilerError::PreflightArithmetic)?,
        u64::try_from(key.transformation_plan).map_err(|_| CompilerError::PreflightArithmetic)?,
        u64::from(key.multi_match),
        capture_mode_code(key.capture_mode),
        byte_view_code(key.byte_view),
    ] {
        work.hash_u64(&mut hash, value, limits)?;
    }
    Ok(hash)
}

pub(super) fn insert_group_key(
    key: CompiledMatcherGroupKey,
    hash: u64,
    slots: &mut [CanonicalSlot],
    keys: &mut Vec<CompiledMatcherGroupKey>,
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<usize, CompilerError> {
    let mask = slots.len().checked_sub(1).ok_or(CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    })?;
    let mut index = (hash as usize) & mask;
    loop {
        work.charge_probe(limits)?;
        let slot = slots[index];
        if slot.is_empty() {
            if keys.len() >= limits.max_matcher_groups {
                return Err(CompilerError::MatcherGroupLimit {
                    actual: keys.len().saturating_add(1),
                    limit: limits.max_matcher_groups,
                });
            }
            reserve_one(keys, "matcher group keys")?;
            let id = keys.len();
            keys.push(key);
            slots[index] = CanonicalSlot { hash, id };
            return Ok(id);
        }
        if slot.hash == hash {
            work.charge_records(1, limits)?;
            if keys[slot.id] == key {
                return Ok(slot.id);
            }
        }
        index = (index + 1) & mask;
    }
}
