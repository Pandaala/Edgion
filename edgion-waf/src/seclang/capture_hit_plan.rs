//! Optional, generation-owned hit detection. Capture extraction stays canonical.
use super::*;
use json_inspection_plan::{CompiledJsonPlan, JsonMemberMode, JsonView};

const MAX_BATCH: usize = 32;
const MAX_BATCH_SOURCE: usize = 64 * 1024;

#[derive(Debug, Default)]
pub(super) struct CaptureHitPlan {
    pub matchers: Vec<CompiledMatcher>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HitBinding {
    pub batch: usize,
    pub pattern: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HitBatch {
    pub matcher: usize,
    pub profile: usize,
    pub start: usize,
    pub count: usize,
}

#[derive(Debug, Default)]
pub(super) struct DomainHits {
    pub bindings: Vec<Option<HitBinding>>,
    pub batches: Vec<HitBatch>,
    pub peers: Vec<usize>,
}

#[derive(Clone, Copy)]
struct Candidate<'a> {
    member: usize,
    source: &'a str,
    location: SemanticLocation,
    key: (usize, usize, bool, usize),
}

#[derive(Clone, Copy)]
struct DomainPeers {
    records: [(usize, usize); MAX_BATCH],
    count: usize,
    profile: usize,
}

struct Staged {
    pool: CaptureHitPlan,
    json: DomainHits,
    xml: DomainHits,
}

fn invariant() -> CompilerError {
    CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    }
}

fn is_invariant(error: &CompilerError) -> bool {
    matches!(
        error,
        CompilerError::PreflightArithmetic
            | CompilerError::PrivateInvariant { .. }
            | CompilerError::PhraseMatcher {
                kind: PhraseMatcherErrorKind::Arithmetic,
                ..
            }
    )
}

// A refused admission never contaminates the attempt's admitted peak. Allocator
// capacity is checked immediately after each fallible reservation.
fn reserve<T>(
    values: &mut Vec<T>,
    count: usize,
    retained: bool,
    work: &mut MatcherCompileWork,
) -> Result<(), CompilerError> {
    let mut next = *work;
    if retained {
        next.retain_compiler_records::<T>(count)?;
    } else {
        next.retain_working_records::<T>(count)?;
    }
    *work = next;
    reserve_exact(values, count, "optional capture hit records")?;
    let extra = values.capacity().checked_sub(count).ok_or_else(invariant)?;
    let mut next = *work;
    if retained {
        next.retain_compiler_records::<T>(extra)?;
    } else {
        next.retain_working_records::<T>(extra)?;
    }
    *work = next;
    Ok(())
}

fn charge(work: &mut CanonicalWork, count: usize, limits: CompilerLimits) -> Result<(), CompilerError> {
    // Charge before executing the work; refused work has not been performed.
    let next = checked_add(work.record_comparisons, count)?;
    if next > limits.max_group_key_record_comparisons {
        return Err(CompilerError::GroupKeyRecordComparisonLimit {
            limit: limits.max_group_key_record_comparisons,
        });
    }
    work.record_comparisons = next;
    Ok(())
}

fn candidate<'a, 'source>(
    generation: &CompiledGeneration,
    unit: super::super::ValidatedRuleUnitView<'a, 'source>,
    member: usize,
    canonical: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<Option<Candidate<'a>>, CompilerError> {
    if !unit.is_active() {
        return Ok(None);
    }
    let root = unit.root();
    let plan = generation.members.get(member).ok_or_else(invariant)?;
    if plan.phase != Phase::RequestBody
        || !matches!(
            generation
                .operator_plans
                .get(member)
                .copied()
                .flatten()
                .map(|p| p.operand),
            Some(CompiledOperatorOperand::Regex(RegexPlanRef::Capture(_)))
        )
    {
        return Ok(None);
    }
    let target_id = plan.target_plan.ok_or_else(invariant)?;
    let targets = generation.target_plans.get(target_id).ok_or_else(invariant)?;
    let end = checked_add(targets.record_start, targets.record_count)?;
    charge(canonical, targets.record_count, limits)?;
    if !generation
        .target_records
        .get(targets.record_start..end)
        .ok_or_else(invariant)?
        .iter()
        .any(|t| {
            t.modifier == TargetModifier::None
                && matches!(
                    t.collection,
                    CollectionKind::Args | CollectionKind::ArgsNames | CollectionKind::Xml
                )
        })
    {
        return Ok(None);
    }
    let operator = root.operator().ok_or_else(invariant)?;
    let super::super::OperatorOperandView::Regex(source) = operator.operand() else {
        return Err(invariant());
    };
    Ok(Some(Candidate {
        member,
        source: source.text(),
        location: operator.location(),
        key: (
            target_id,
            plan.transformation_plan.ok_or_else(invariant)?,
            root.rule().fold().multi_match_action().is_some(),
            plan.runtime_expansion_byte_limit,
        ),
    }))
}

fn domain_peers(
    plan: &CompiledJsonPlan,
    candidates: &[Candidate<'_>],
    view: JsonView,
    hits: &DomainHits,
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<Option<DomainPeers>, CompilerError> {
    let mut peers = [(usize::MAX, 0); MAX_BATCH];
    let mut count = 0;
    let mut profile = None;
    for (pattern, candidate) in candidates.iter().enumerate() {
        charge(work, 1, limits)?;
        let member = plan.members.get(candidate.member).ok_or_else(invariant)?;
        let mode = match view {
            JsonView::Name => member.name,
            JsonView::Value => member.value,
        };
        let JsonMemberMode::StaticFact(group) = mode else {
            return Ok(None);
        };
        let group_profile = plan.groups.get(group).ok_or_else(invariant)?.profile;
        if profile.is_some_and(|p| p != group_profile) || hits.bindings.get(group).is_some_and(Option::is_some) {
            return Ok(None);
        }
        profile = Some(group_profile);
        charge(work, count, limits)?;
        if !peers[..count].iter().any(|&(id, _)| id == group) {
            peers[count] = (group, pattern);
            count += 1;
        }
    }
    Ok((count > 1).then_some(DomainPeers {
        records: peers,
        count,
        profile: profile.ok_or_else(invariant)?,
    }))
}

fn domain_reserve(
    plan: &CompiledJsonPlan,
    roots: usize,
    work: &mut MatcherCompileWork,
) -> Result<DomainHits, CompilerError> {
    let mut hits = DomainHits::default();
    reserve(&mut hits.bindings, plan.groups.len(), true, work)?;
    hits.bindings.resize(plan.groups.len(), None);
    reserve(&mut hits.batches, roots, true, work)?;
    reserve(&mut hits.peers, checked_add(roots, roots)?, true, work)?;
    Ok(hits)
}

fn install_domain(
    hits: &mut DomainHits,
    matcher: usize,
    peers: &[(usize, usize)],
    profile: usize,
) -> Result<(), CompilerError> {
    let batch = hits.batches.len();
    if batch == hits.batches.capacity() || checked_add(hits.peers.len(), peers.len())? > hits.peers.capacity() {
        return Err(invariant());
    }
    let start = hits.peers.len();
    for &(id, pattern) in peers {
        let binding = hits.bindings.get_mut(id).ok_or_else(invariant)?;
        if binding.is_some() {
            return Err(invariant());
        }
        *binding = Some(HitBinding { batch, pattern });
        hits.peers.push(id);
    }
    hits.batches.push(HitBatch {
        matcher,
        profile,
        start,
        count: peers.len(),
    });
    Ok(())
}

fn attempt(
    generation: &CompiledGeneration,
    validated: &ValidatedSemanticDocument<'_>,
    preflight: GroupPatternPreflight,
    work: &mut MatcherCompileWork,
    canonical: &mut CanonicalWork,
) -> Result<Staged, CompilerError> {
    let limits = work.limits;
    let mut count = 0;
    let mut member = 0;
    for unit in validated.rule_units() {
        charge(canonical, 1, limits)?;
        count = checked_add(
            count,
            usize::from(candidate(generation, unit, member, canonical, limits)?.is_some()),
        )?;
        member = checked_add(member, unit.members().len())?;
    }
    if count < 2 {
        return Ok(Staged {
            pool: CaptureHitPlan::default(),
            json: DomainHits::default(),
            xml: DomainHits::default(),
        });
    }
    let mut candidates = Vec::new();
    reserve(&mut candidates, count, false, work)?;
    member = 0;
    for unit in validated.rule_units() {
        charge(canonical, 1, limits)?;
        if let Some(value) = candidate(generation, unit, member, canonical, limits)? {
            candidates.push(value);
        }
        member = checked_add(member, unit.members().len())?;
    }
    let mut comparisons = count
        .checked_mul((usize::BITS - count.leading_zeros()) as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(CompilerError::PreflightArithmetic)?;
    crate::bounded_sort::heapsort_by(&mut candidates, |a, b| {
        comparisons = comparisons
            .checked_sub(1)
            .ok_or(CompilerError::GroupKeyRecordComparisonLimit {
                limit: limits.max_group_key_record_comparisons,
            })?;
        charge(canonical, 1, limits)?;
        Ok((a.key, a.member).cmp(&(b.key, b.member)))
    })?;
    let mut result = Staged {
        pool: CaptureHitPlan::default(),
        json: domain_reserve(&generation.json_inspection, count, work)?,
        xml: domain_reserve(&generation.xml_inspection, count, work)?,
    };
    reserve(&mut result.pool.matchers, count / 2, true, work)?;
    let mut patterns = Vec::new();
    reserve(&mut patterns, MAX_BATCH, false, work)?;
    let mut programs = checked_add(generation.boolean_regexes.len(), generation.capture_regexes.len())?;
    for group in &generation.groups {
        let end = checked_add(group.mapping_start, group.mapping_count)?;
        let mappings = generation
            .pattern_mappings
            .get(group.mapping_start..end)
            .ok_or_else(invariant)?;
        charge(canonical, mappings.len(), limits)?;
        if mappings.iter().any(|mapping| {
            generation.operator_plans[mapping.member].is_some_and(|op| op.kind == super::super::OperatorKind::Rx)
        }) {
            programs = checked_inc(programs)?;
        }
    }
    let mut active_patterns = preflight.active_patterns;
    let mut active_bytes = preflight.active_source_bytes;
    let mut start = 0;
    while start < candidates.len() {
        let mut end = start + 1;
        while end < candidates.len() {
            charge(canonical, 1, limits)?;
            if candidates[end].key != candidates[start].key {
                break;
            }
            end += 1;
        }
        let range = (start, end);
        start = end;
        if !(2..=MAX_BATCH).contains(&(range.1 - range.0)) {
            continue;
        }
        let mut pending = [(0, 0); MAX_BATCH];
        pending[0] = range;
        let mut pending_len = 1;
        while pending_len != 0 {
            pending_len -= 1;
            let (left, right) = pending[pending_len];
            let batch = &candidates[left..right];
            charge(canonical, 1, limits)?;
            let mut source_bytes = 0;
            for candidate in batch {
                charge(canonical, 1, limits)?;
                source_bytes = checked_add(source_bytes, candidate.source.len())?;
            }
            if source_bytes > MAX_BATCH_SOURCE {
                continue;
            }
            let mut domains = [(None, None), (None, None)];
            for (index, (plan, hits)) in [
                (&generation.json_inspection, &result.json),
                (&generation.xml_inspection, &result.xml),
            ]
            .into_iter()
            .enumerate()
            {
                domains[index].0 = domain_peers(plan, batch, JsonView::Name, hits, canonical, limits)?;
                domains[index].1 = domain_peers(plan, batch, JsonView::Value, hits, canonical, limits)?;
            }
            if domains.iter().all(|(name, value)| name.is_none() && value.is_none()) {
                continue;
            }
            let groups = checked_add(generation.groups.len(), checked_inc(result.pool.matchers.len())?)?;
            let next_programs = checked_inc(programs)?;
            let next_patterns = checked_add(active_patterns, batch.len())?;
            let next_bytes = checked_add(active_bytes, source_bytes)?;
            if groups > limits.max_matcher_groups
                || next_programs > limits.max_regex_programs
                || next_patterns > limits.max_matcher_patterns
                || next_bytes > limits.max_matcher_source_bytes
            {
                continue;
            }
            patterns.clear();
            for candidate in batch {
                patterns.push(PatternSpec::Regex(candidate.source));
            }
            let counts = MatcherPatternCounts {
                regexes: patterns.len(),
                ..MatcherPatternCounts::default()
            };
            let bytes = MatcherPatternBytes {
                regexes: source_bytes,
                ..MatcherPatternBytes::default()
            };
            let projection = match MatcherBuildProjection::for_counts(counts, bytes, limits.matcher) {
                Ok(value) => value,
                Err(error) => {
                    let error = map_phrase_matcher_error(error, batch[0].location);
                    if is_invariant(&error) {
                        return Err(error);
                    }
                    continue;
                }
            };
            let mut admitted = *work;
            if let Err(error) = admitted.precheck_matcher_projection(projection) {
                if is_invariant(&error) {
                    return Err(error);
                }
                if matches!(error, CompilerError::PeakByteLimit { .. }) && batch.len() > 2 {
                    let middle = left + batch.len() / 2;
                    // Push the right half first so source-order ownership is stable.
                    for (a, b) in [(middle, right), (left, middle)] {
                        if b - a < 2 {
                            continue;
                        }
                        if pending_len == pending.len() {
                            return Err(invariant());
                        }
                        pending[pending_len] = (a, b);
                        pending_len += 1;
                    }
                }
                continue;
            }
            *work = admitted;
            let mut build = *work;
            let matcher = match build.compile_group_matcher(&patterns, counts, bytes, batch[0].location) {
                Ok(value) => value,
                Err(error) => {
                    if is_invariant(&error) {
                        return Err(error);
                    }
                    continue;
                }
            };
            *work = build;
            let matcher_id = checked_add(generation.groups.len(), result.pool.matchers.len())?;
            result.pool.matchers.push(matcher);
            for (hits, (name, value)) in [&mut result.json, &mut result.xml].into_iter().zip(domains) {
                for peers in name.into_iter().chain(value) {
                    install_domain(hits, matcher_id, &peers.records[..peers.count], peers.profile)?;
                }
            }
            programs = next_programs;
            active_patterns = next_patterns;
            active_bytes = next_bytes;
        }
    }
    work.drop_working_records::<PatternSpec<'_>>(patterns.capacity());
    work.drop_working_records::<Candidate<'_>>(candidates.capacity());
    for hits in [&mut result.json, &mut result.xml] {
        if hits.batches.is_empty() {
            let bytes = hits
                .bindings
                .capacity()
                .checked_mul(size_of::<Option<HitBinding>>())
                .and_then(|b| b.checked_add(hits.batches.capacity().checked_mul(size_of::<HitBatch>())?))
                .and_then(|b| b.checked_add(hits.peers.capacity().checked_mul(size_of::<usize>())?))
                .ok_or(CompilerError::PreflightArithmetic)?;
            work.drop_compiler_bytes(bytes)?;
            *hits = DomainHits::default();
        }
    }
    Ok(result)
}

pub(super) fn compile_optional(
    generation: &mut CompiledGeneration,
    validated: &ValidatedSemanticDocument<'_>,
    preflight: GroupPatternPreflight,
    work: &mut MatcherCompileWork,
    canonical: &mut CanonicalWork,
) -> Result<(), CompilerError> {
    let mut optional = *work;
    match attempt(generation, validated, preflight, &mut optional, canonical) {
        Ok(result) if !result.pool.matchers.is_empty() => {
            generation.capture_hits = result.pool;
            generation.json_inspection.capture_hits = result.json;
            generation.xml_inspection.capture_hits = result.xml;
            *work = optional;
        }
        Ok(_) => work.peak_bytes = work.peak_bytes.max(optional.peak_bytes),
        Err(error) if is_invariant(&error) => return Err(error),
        Err(_) => work.peak_bytes = work.peak_bytes.max(optional.peak_bytes),
    }
    Ok(())
}

impl CompiledGeneration {
    pub(super) fn all_matchers(&self) -> impl Iterator<Item = &CompiledMatcher> {
        self.groups
            .iter()
            .map(|g| &g.matcher)
            .chain(&self.capture_hits.matchers)
    }
    pub(super) fn matcher_at(&self, index: usize) -> Option<&CompiledMatcher> {
        if index < self.groups.len() {
            self.groups.get(index).map(|g| &g.matcher)
        } else {
            self.capture_hits.matchers.get(index - self.groups.len())
        }
    }
    pub(super) fn matcher_count(&self) -> usize {
        self.groups.len() + self.capture_hits.matchers.len()
    }
}
