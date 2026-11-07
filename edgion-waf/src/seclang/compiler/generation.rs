use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ByteRange {
    pub(super) start: usize,
    pub(super) len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompiledSegmentKind {
    Literal(ByteRange),
    Macro(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledTemplateSegment {
    pub(super) kind: CompiledSegmentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledMacroRef {
    pub(super) source: MacroSource,
    pub(super) key: ByteRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledTemplate {
    pub(super) segment_start: usize,
    pub(super) segment_count: usize,
    pub(super) static_bytes: usize,
    pub(super) runtime_byte_limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledSetVar {
    pub(super) collection: CollectionKind,
    pub(super) operation: SetVarOperation,
    pub(super) key: usize,
    pub(super) value: Option<usize>,
    pub(super) constant: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompiledCtlSelector {
    None,
    Literal(ByteRange),
    Path(ByteRange),
    RegexSource(ByteRange),
    Resolved(CompiledTargetSelector),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompiledCtl {
    ForceRequestBodyVariable(OnOff),
    RequestBodyProcessor(RequestBodyProcessor),
    RuleRemoveByTag(ByteRange),
    RuleRemoveTargetByTag {
        tag: ByteRange,
        collection: CollectionKind,
        selector: CompiledCtlSelector,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledTagRuleRef {
    pub(super) tag: ByteRange,
    pub(super) unit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompiledActionOperand {
    None,
    Local(LocalActionValue),
    Transformation(TransformationKind),
    Template(usize),
    Literal(ByteRange),
    SetVar(usize),
    Ctl(CompiledCtl),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledActionPlan {
    pub(super) kind: ActionEnvelopeKind,
    pub(super) operand: CompiledActionOperand,
    pub(super) location: SemanticLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledMemberPlan {
    pub(super) active: bool,
    pub(super) expensive_aggregate_actions: bool,
    pub(super) operator_template: Option<usize>,
    pub(super) target_plan: Option<usize>,
    pub(super) transformation_plan: Option<usize>,
    pub(super) phase: Phase,
    pub(super) disruptive: ResolvedDisruptive,
    pub(super) status: Option<u16>,
    pub(super) log: bool,
    pub(super) audit_log: bool,
    pub(super) action_start: usize,
    pub(super) action_count: usize,
    pub(super) transformation_start: usize,
    pub(super) transformation_count: usize,
    pub(super) runtime_expansion_byte_limit: usize,
    pub(super) skip_after_marker: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledPhrase {
    pub(super) bytes: ByteRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResolvedAsset {
    pub(super) request_id: usize,
    pub(super) phrase_start: usize,
    pub(super) phrase_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RequirementState {
    pub(super) kind: CompileRequirement,
    pub(super) location: SemanticLocation,
    pub(super) discharged: bool,
}

#[derive(Debug)]
pub(super) struct AssetTemplateState<'source> {
    pub(super) compilation_input_bytes: usize,
    pub(super) validated: ValidatedSemanticDocument<'source>,
    pub(super) bytes: Vec<u8>,
    pub(super) asset_phrase_bytes: Vec<u8>,
    pub(super) templates: Vec<CompiledTemplate>,
    pub(super) segments: Vec<CompiledTemplateSegment>,
    pub(super) macros: Vec<CompiledMacroRef>,
    pub(super) actions: Vec<CompiledActionPlan>,
    pub(super) setvars: Vec<CompiledSetVar>,
    pub(super) members: Vec<CompiledMemberPlan>,
    pub(super) transformations: Vec<TransformationKind>,
    pub(super) assets: Vec<ResolvedAsset>,
    pub(super) phrases: Vec<CompiledPhrase>,
    pub(super) requirements: Vec<RequirementState>,
    pub(super) compiler_owned_bytes: usize,
    pub(super) compiler_working_bytes: usize,
    pub(super) peak_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StringRelation {
    BeginsWith,
    Contains,
    EndsWith,
    Equals,
    Within,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IntegerRelation {
    Equal,
    GreaterOrEqual,
    Greater,
    Less,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompiledIntegerOperand {
    Static(i64),
    Template(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledIpNetwork {
    pub(super) address: IpAddr,
    pub(super) prefix: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RegexPlanRef {
    Boolean(usize),
    Capture(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompiledOperatorOperand {
    String {
        relation: StringRelation,
        template: usize,
    },
    Integer {
        relation: IntegerRelation,
        operand: CompiledIntegerOperand,
    },
    IpNetworks {
        start: usize,
        count: usize,
    },
    ByteSet {
        words: [u64; 4],
    },
    ValidateUrlEncoding,
    ValidateUtf8Encoding,
    Unconditional,
    DetectSqli {
        capture: bool,
    },
    DetectXss,
    Regex(RegexPlanRef),
    Matcher {
        group: usize,
        pattern_start: usize,
        pattern_count: usize,
    },
    CapturePhrase(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledOperatorPlan {
    pub(super) kind: OperatorKind,
    pub(super) negated: bool,
    pub(super) operand: CompiledOperatorOperand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledTargetRegexPlan {
    pub(super) identity: SemanticLocation,
    pub(super) program: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RegexCandidate<'source> {
    pub(super) source: &'source str,
    pub(super) location: SemanticLocation,
    pub(super) capture: bool,
    pub(super) active: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct RegexCandidateCounts {
    pub(super) selectors: usize,
    pub(super) active_boolean: usize,
    pub(super) active_capture: usize,
    pub(super) removed_boolean: usize,
    pub(super) removed_capture: usize,
}

impl RegexCandidateCounts {
    pub(super) fn total(self) -> Option<usize> {
        self.selectors
            .checked_add(self.active_boolean)?
            .checked_add(self.active_capture)?
            .checked_add(self.removed_boolean)?
            .checked_add(self.removed_capture)
    }
    pub(super) fn active(self) -> Option<usize> {
        self.selectors.checked_add(self.active_capture)
    }
}

#[derive(Debug)]
pub(super) struct MatcherState<'source> {
    pub(super) asset_templates: AssetTemplateState<'source>,
    pub(super) operator_plans: Vec<Option<CompiledOperatorPlan>>,
    pub(super) ip_networks: Vec<CompiledIpNetwork>,
    pub(super) target_regexes: Vec<CompiledTargetRegexPlan>,
    pub(super) boolean_regexes: Vec<CompiledRegexProgram>,
    pub(super) capture_regexes: Vec<CompiledCaptureRegexProgram>,
    pub(super) canonical_work: CanonicalWork,
    pub(super) compiler_owned_bytes: usize,
    pub(super) matcher_charged_bytes: usize,
    pub(super) peak_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CaptureMode {
    None,
    #[cfg_attr(not(test), expect(dead_code, reason = "capture regex programs remain dedicated"))]
    RegexGroups,
    #[cfg_attr(not(test), expect(dead_code, reason = "capture phrase programs remain dedicated"))]
    PhraseSpans,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ByteView {
    RawBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompiledTargetSelector {
    None,
    Literal(ByteRange),
    Path(ByteRange),
    Regex { style: RegexStyle, program: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledTargetRecord {
    pub(super) collection: CollectionKind,
    pub(super) modifier: TargetModifier,
    pub(super) selector: CompiledTargetSelector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledTargetPlan {
    pub(super) record_start: usize,
    pub(super) record_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledTransformationPlan {
    pub(super) transformation_start: usize,
    pub(super) transformation_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledMatcherGroupKey {
    pub(super) phase: Phase,
    pub(super) target_plan: usize,
    pub(super) transformation_plan: usize,
    pub(super) multi_match: bool,
    pub(super) capture_mode: CaptureMode,
    pub(super) byte_view: ByteView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledPatternMapping {
    pub(super) unit: usize,
    pub(super) member: usize,
    pub(super) local_ordinal: usize,
}

#[derive(Debug)]
pub(super) struct CompiledMatcherGroup {
    pub(super) key: CompiledMatcherGroupKey,
    pub(super) matcher: CompiledMatcher,
    pub(super) mapping_start: usize,
    pub(super) mapping_count: usize,
}

#[derive(Debug)]
pub(super) struct GroupedMatcherState<'source> {
    pub(super) pattern_preflight: GroupPatternPreflight,
    pub(super) canonical_work: CanonicalWork,
    pub(super) matchers: MatcherState<'source>,
    pub(super) target_bytes: Vec<u8>,
    pub(super) target_records: Vec<CompiledTargetRecord>,
    pub(super) target_plans: Vec<CompiledTargetPlan>,
    pub(super) canonical_transformations: Vec<TransformationKind>,
    pub(super) transformation_plans: Vec<CompiledTransformationPlan>,
    pub(super) groups: Vec<CompiledMatcherGroup>,
    pub(super) pattern_mappings: Vec<CompiledPatternMapping>,
    pub(super) capture_phrases: Vec<CompiledCapturePhraseProgram>,
    pub(super) compiler_owned_bytes: usize,
    pub(super) matcher_charged_bytes: usize,
    pub(super) peak_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompiledScheduleEntry {
    RuleUnit(usize),
    MarkerSentinel(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledRuleUnit {
    pub(super) expensive_check: bool,
    pub(super) id: u64,
    pub(super) phase: Phase,
    pub(super) disruptive: ResolvedDisruptive,
    pub(super) status: Option<u16>,
    pub(super) log: bool,
    pub(super) audit_log: bool,
    pub(super) member_start: usize,
    pub(super) member_count: usize,
    pub(super) schedule_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompiledMarker {
    pub(super) schedule_indices: [usize; 5],
}

/// Fully owned immutable SecLang generation ready for transaction execution.
#[derive(Debug)]
#[expect(dead_code, reason = "transaction execution consumes the immutable compiled arenas")]
pub struct CompiledGeneration {
    pub(super) capture_hits: capture_hit_plan::CaptureHitPlan,
    pub(super) has_expensive_checks: bool,
    pub(super) inspection_activation: inspection_activation::CompiledActivation,
    pub(super) json_inspection: json_inspection_plan::CompiledJsonPlan,
    pub(super) xml_inspection: json_inspection_plan::CompiledJsonPlan,
    pub(super) prefix_validation: Result<(), &'static str>,
    pub(super) requires_complete_request_body: bool,
    pub(super) bytes: Vec<u8>,
    pub(super) templates: Vec<CompiledTemplate>,
    pub(super) segments: Vec<CompiledTemplateSegment>,
    pub(super) macros: Vec<CompiledMacroRef>,
    pub(super) actions: Vec<CompiledActionPlan>,
    pub(super) tag_rule_refs: Vec<CompiledTagRuleRef>,
    pub(super) setvars: Vec<CompiledSetVar>,
    pub(super) members: Vec<CompiledMemberPlan>,
    pub(super) operator_plans: Vec<Option<CompiledOperatorPlan>>,
    pub(super) ip_networks: Vec<CompiledIpNetwork>,
    pub(super) target_regexes: Vec<CompiledTargetRegexPlan>,
    pub(super) boolean_regexes: Vec<CompiledRegexProgram>,
    pub(super) capture_regexes: Vec<CompiledCaptureRegexProgram>,
    pub(super) target_bytes: Vec<u8>,
    pub(super) target_records: Vec<CompiledTargetRecord>,
    pub(super) target_plans: Vec<CompiledTargetPlan>,
    pub(super) transformations: Vec<TransformationKind>,
    pub(super) transformation_plans: Vec<CompiledTransformationPlan>,
    pub(super) groups: Vec<CompiledMatcherGroup>,
    pub(super) pattern_mappings: Vec<CompiledPatternMapping>,
    pub(super) capture_phrases: Vec<CompiledCapturePhraseProgram>,
    pub(super) units: Vec<CompiledRuleUnit>,
    pub(super) markers: Vec<CompiledMarker>,
    pub(super) schedules: [Vec<CompiledScheduleEntry>; 5],
    pub(super) compiler_owned_bytes: usize,
    pub(super) matcher_charged_bytes: usize,
    pub(super) peak_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompiledScheduleEntryKind {
    RuleUnit,
    MarkerSentinel,
}

/// The bounded execution lane required by one Phase 2 rule unit.
///
/// This classification is intentionally conservative. A rule is promoted to a
/// streaming lane only when the compiled targets, operator, and
/// transformations have an exact incremental execution model. Callers must not
/// reinterpret `BoundedSnapshot` as permission to retain an unbounded body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestBodyExecutionClass {
    StreamingRaw,
    /// Reserved until structured publication preserves duplicate and ordering semantics.
    StreamingStructured,
    BoundedSnapshot,
    Unsupported,
}

/// Low-cost aggregate of the Phase 2 execution lanes in one generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestBodyExecutionSummary {
    requires_request_body: bool,
    streaming_raw_rule_units: usize,
    streaming_structured_rule_units: usize,
    bounded_snapshot_rule_units: usize,
    unsupported_rule_units: usize,
}

impl RequestBodyExecutionSummary {
    pub const fn streaming_raw_rule_units(self) -> usize {
        self.streaming_raw_rule_units
    }

    pub const fn streaming_structured_rule_units(self) -> usize {
        self.streaming_structured_rule_units
    }

    pub const fn bounded_snapshot_rule_units(self) -> usize {
        self.bounded_snapshot_rule_units
    }

    pub const fn unsupported_rule_units(self) -> usize {
        self.unsupported_rule_units
    }

    /// Whether a phase-two target or macro consumes request-body values.
    /// Execution-lane counts also include body-independent phase-two units.
    pub const fn requires_request_body(self) -> bool {
        self.requires_request_body
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CompiledScheduleEntryView {
    entry: CompiledScheduleEntry,
}

impl CompiledScheduleEntryView {
    pub const fn kind(self) -> CompiledScheduleEntryKind {
        match self.entry {
            CompiledScheduleEntry::RuleUnit(_) => CompiledScheduleEntryKind::RuleUnit,
            CompiledScheduleEntry::MarkerSentinel(_) => CompiledScheduleEntryKind::MarkerSentinel,
        }
    }

    pub const fn ordinal(self) -> usize {
        match self.entry {
            CompiledScheduleEntry::RuleUnit(value) | CompiledScheduleEntry::MarkerSentinel(value) => value,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CompiledRuleUnitView<'generation> {
    generation: &'generation CompiledGeneration,
    index: usize,
}

impl<'generation> CompiledRuleUnitView<'generation> {
    fn record(self) -> CompiledRuleUnit {
        self.generation.units[self.index]
    }

    pub fn id(self) -> u64 {
        self.record().id
    }

    pub fn phase(self) -> Phase {
        self.record().phase
    }

    pub fn disruptive(self) -> ResolvedDisruptive {
        self.record().disruptive
    }

    pub fn status(self) -> Option<u16> {
        self.record().status
    }

    pub fn log(self) -> bool {
        self.record().log
    }

    pub fn audit_log(self) -> bool {
        self.record().audit_log
    }

    /// Return the compiler-selected Phase 2 lane, or `None` for another phase.
    pub fn request_body_execution_class(self) -> Option<RequestBodyExecutionClass> {
        (self.phase() == Phase::RequestBody).then(|| classify_request_body_unit(self.generation, self.record()))
    }

    pub fn schedule_index(self) -> usize {
        self.record().schedule_index
    }

    pub fn members(self) -> impl ExactSizeIterator<Item = CompiledRuleMemberView<'generation>> + 'generation {
        let record = self.record();
        (record.member_start..record.member_start + record.member_count).map(|index| CompiledRuleMemberView {
            generation: self.generation,
            index,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CompiledRuleMemberView<'generation> {
    generation: &'generation CompiledGeneration,
    index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompiledTargetSelectorKind {
    None,
    Literal,
    Path,
    Regex,
}

#[derive(Debug, Clone, Copy)]
pub struct CompiledTargetRecordView<'generation> {
    generation: &'generation CompiledGeneration,
    record: CompiledTargetRecord,
}

#[derive(Debug, Clone, Copy)]
pub struct CompiledTargetPlanView<'generation> {
    generation: &'generation CompiledGeneration,
    plan: CompiledTargetPlan,
}

impl<'generation> CompiledTargetPlanView<'generation> {
    pub fn records(self) -> impl ExactSizeIterator<Item = CompiledTargetRecordView<'generation>> + 'generation {
        let start = self.plan.record_start;
        let records = start
            .checked_add(self.plan.record_count)
            .and_then(|end| self.generation.target_records.get(start..end))
            .unwrap_or_default();
        records.iter().copied().map(|record| CompiledTargetRecordView {
            generation: self.generation,
            record,
        })
    }
}

impl<'generation> CompiledTargetRecordView<'generation> {
    pub const fn collection(self) -> CollectionKind {
        self.record.collection
    }

    pub const fn modifier(self) -> TargetModifier {
        self.record.modifier
    }

    pub const fn selector_kind(self) -> CompiledTargetSelectorKind {
        match self.record.selector {
            CompiledTargetSelector::None => CompiledTargetSelectorKind::None,
            CompiledTargetSelector::Literal(_) => CompiledTargetSelectorKind::Literal,
            CompiledTargetSelector::Path(_) => CompiledTargetSelectorKind::Path,
            CompiledTargetSelector::Regex { .. } => CompiledTargetSelectorKind::Regex,
        }
    }

    pub fn selector_bytes(self) -> Option<&'generation [u8]> {
        let (CompiledTargetSelector::Literal(range) | CompiledTargetSelector::Path(range)) = self.record.selector
        else {
            return None;
        };
        let end = range.start.checked_add(range.len)?;
        self.generation.target_bytes.get(range.start..end)
    }

    pub const fn selector_regex_program_id(self) -> Option<usize> {
        match self.record.selector {
            CompiledTargetSelector::Regex { program, .. } => Some(program),
            _ => None,
        }
    }
}

impl<'generation> CompiledRuleMemberView<'generation> {
    fn record(self) -> CompiledMemberPlan {
        self.generation.members[self.index]
    }

    pub fn phase(self) -> Phase {
        self.record().phase
    }

    pub fn target_plan_id(self) -> Option<usize> {
        self.record().target_plan
    }

    pub fn target_plan(self) -> Option<CompiledTargetPlanView<'generation>> {
        let id = self.record().target_plan?;
        let plan = *self.generation.target_plans.get(id)?;
        Some(CompiledTargetPlanView {
            generation: self.generation,
            plan,
        })
    }

    pub fn transformation_plan_id(self) -> Option<usize> {
        self.record().transformation_plan
    }

    pub fn transformations(self) -> Option<&'generation [TransformationKind]> {
        let id = self.record().transformation_plan?;
        let plan = *self.generation.transformation_plans.get(id)?;
        let end = plan.transformation_start.checked_add(plan.transformation_count)?;
        self.generation.transformations.get(plan.transformation_start..end)
    }

    pub fn operator_kind(self) -> Option<OperatorKind> {
        self.generation.operator_plans[self.index].map(|plan| plan.kind)
    }

    pub fn operator_negated(self) -> Option<bool> {
        self.generation.operator_plans[self.index].map(|plan| plan.negated)
    }

    pub fn action_count(self) -> usize {
        self.record().action_count
    }

    pub fn skip_after_schedule_index(self) -> Option<usize> {
        self.record().skip_after_marker
    }
}

impl CompiledGeneration {
    pub fn rule_unit_count(&self) -> usize {
        self.units.len()
    }

    pub fn marker_count(&self) -> usize {
        self.markers.len()
    }

    pub fn matcher_group_count(&self) -> usize {
        self.groups.len()
    }

    pub fn rule_units(&self) -> impl ExactSizeIterator<Item = CompiledRuleUnitView<'_>> + '_ {
        (0..self.units.len()).map(|index| CompiledRuleUnitView {
            generation: self,
            index,
        })
    }

    pub fn phase_schedule(&self, phase: Phase) -> impl ExactSizeIterator<Item = CompiledScheduleEntryView> + '_ {
        self.schedules[phase_index(phase)]
            .iter()
            .copied()
            .map(|entry| CompiledScheduleEntryView { entry })
    }

    pub fn request_body_execution_summary(&self) -> RequestBodyExecutionSummary {
        let mut summary = RequestBodyExecutionSummary::default();
        for unit in self.rule_units() {
            if unit.record().phase == Phase::RequestBody {
                let unit = unit.record();
                summary.requires_request_body |= self.members[unit.member_start..unit.member_start + unit.member_count]
                    .iter()
                    .any(|member| self.member_requires_request_body(*member));
            }
            match unit.request_body_execution_class() {
                Some(RequestBodyExecutionClass::StreamingRaw) => summary.streaming_raw_rule_units += 1,
                Some(RequestBodyExecutionClass::StreamingStructured) => {
                    summary.streaming_structured_rule_units += 1;
                }
                Some(RequestBodyExecutionClass::BoundedSnapshot) => summary.bounded_snapshot_rule_units += 1,
                Some(RequestBodyExecutionClass::Unsupported) => summary.unsupported_rule_units += 1,
                None => {}
            }
        }
        summary
    }

    /// Validate the complete generation for bounded request-prefix inspection.
    /// This deliberately narrower contract does not follow streaming classification.
    pub fn validate_request_body_prefix(&self) -> Result<(), &'static str> {
        self.prefix_validation
    }

    /// Whether a phase after request headers consumes a complete-body value.
    /// Phase-one ARGS contains query arguments only and does not trigger capture.
    pub const fn requires_complete_request_body(&self) -> bool {
        self.requires_complete_request_body
    }

    pub(super) fn has_complete_request_body_dependency(&self) -> bool {
        self.units.iter().any(|unit| {
            if unit.phase == Phase::RequestHeaders {
                return false;
            }
            self.members[unit.member_start..unit.member_start + unit.member_count]
                .iter()
                .any(|member| self.member_requires_request_body(*member))
        })
    }

    pub(super) fn member_requires_request_body(&self, member: CompiledMemberPlan) -> bool {
        member.target_plan.is_some_and(|id| {
            let plan = self.target_plans[id];
            self.target_records[plan.record_start..plan.record_start + plan.record_count]
                .iter()
                .any(|record| collection_requires_request_body(record.collection))
        }) || self.member_has_body_macro(member)
    }

    fn member_has_body_macro(&self, member: CompiledMemberPlan) -> bool {
        // ARGS macros include parsed body entries after phase one. TX values copied
        // from body targets are covered at the producing member instead.
        member
            .operator_template
            .is_some_and(|id| self.template_has_body_macro(id))
            || self.actions[member.action_start..member.action_start + member.action_count]
                .iter()
                .any(|action| match action.operand {
                    CompiledActionOperand::Template(id) => self.template_has_body_macro(id),
                    CompiledActionOperand::SetVar(id) => {
                        let setvar = self.setvars[id];
                        self.template_has_body_macro(setvar.key)
                            || setvar.value.is_some_and(|id| self.template_has_body_macro(id))
                    }
                    _ => false,
                })
    }

    fn template_has_body_macro(&self, id: usize) -> bool {
        let template = self.templates[id];
        self.segments[template.segment_start..template.segment_start + template.segment_count]
            .iter()
            .any(|segment| {
                matches!(segment.kind, CompiledSegmentKind::Macro(id)
                if self.macros[id].source == MacroSource::Args)
            })
    }

    pub(super) fn check_request_body_prefix(&self) -> Result<(), &'static str> {
        if !self.macros.is_empty() {
            return Err("requestBody.inspection prefix does not support macros; select full");
        }
        for unit in &self.units {
            if unit.member_count != 1 {
                return Err("requestBody.inspection prefix does not support chains; select full");
            }
            let member = self.members[unit.member_start];
            if !prefix_actions_supported(self, member) {
                return Err("requestBody.inspection prefix does not support control or state actions; select full");
            }
            if unit.phase == Phase::RequestBody {
                if self.member_requires_request_body(member) && !is_prefix_body_member(self, unit.member_start) {
                    return Err("requestBody.inspection prefix requires positive raw REQUEST_BODY contains/pm rules; select full");
                }
            } else if unit.phase != Phase::RequestHeaders {
                if let Some(plan) = member.target_plan.map(|id| self.target_plans[id]) {
                    for record in &self.target_records[plan.record_start..plan.record_start + plan.record_count] {
                        if collection_requires_request_body(record.collection) {
                            return Err("requestBody.inspection prefix cannot provide later-phase request body values; select full");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub const fn compiler_owned_bytes(&self) -> usize {
        self.compiler_owned_bytes
    }

    pub const fn matcher_charged_bytes(&self) -> usize {
        self.matcher_charged_bytes
    }

    pub const fn retained_bytes(&self) -> usize {
        self.compiler_owned_bytes + self.matcher_charged_bytes
    }

    pub const fn compilation_peak_bytes(&self) -> usize {
        self.peak_bytes
    }

    pub(crate) fn include_compilation_peak(&mut self, bytes: usize) {
        self.peak_bytes = self.peak_bytes.max(bytes);
    }

    pub(super) fn inspection_plan(&self, domain: InspectionDomain) -> &json_inspection_plan::CompiledJsonPlan {
        match domain {
            InspectionDomain::Json => &self.json_inspection,
            InspectionDomain::Xml => &self.xml_inspection,
        }
    }
    pub fn xml_inspection_unsupported(&self) -> &[XmlInspectionUnsupported] {
        &self.xml_inspection.xml_unsupported
    }

    /// JSON-specific capability findings; admission is configuration-aware.
    pub fn json_inspection_unsupported(&self) -> &[JsonInspectionUnsupported] {
        &self.json_inspection.unsupported
    }
}

pub(super) fn member_group_key(
    plan: CompiledMemberPlan,
    multi_match: bool,
) -> Result<CompiledMatcherGroupKey, CompilerError> {
    Ok(CompiledMatcherGroupKey {
        phase: plan.phase,
        target_plan: plan.target_plan.ok_or(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        })?,
        transformation_plan: plan.transformation_plan.ok_or(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        })?,
        multi_match,
        capture_mode: CaptureMode::None,
        byte_view: ByteView::RawBytes,
    })
}

fn prefix_actions_supported(generation: &CompiledGeneration, member: CompiledMemberPlan) -> bool {
    generation.actions[member.action_start..member.action_start + member.action_count]
        .iter()
        .all(|action| match action.kind {
            ActionEnvelopeKind::Transformation => true,
            ActionEnvelopeKind::Action(kind) => is_streaming_safe_action(kind) || kind == ActionKind::Msg,
        })
}

fn collection_requires_request_body(collection: CollectionKind) -> bool {
    matches!(
        collection,
        CollectionKind::RequestBody
            | CollectionKind::RequestBodyLength
            | CollectionKind::Args
            | CollectionKind::ArgsCombinedSize
            | CollectionKind::ArgsNames
            | CollectionKind::Files
            | CollectionKind::FilesCombinedSize
            | CollectionKind::FilesNames
            | CollectionKind::MultipartPartHeaders
            | CollectionKind::Xml
            | CollectionKind::ReqbodyProcessor
    )
}

fn is_prefix_body_member(generation: &CompiledGeneration, index: usize) -> bool {
    let member = generation.members[index];
    let Some(plan) = member.target_plan.map(|id| generation.target_plans[id]) else {
        return false;
    };
    let records = &generation.target_records[plan.record_start..plan.record_start + plan.record_count];
    if !matches!(
        records,
        [CompiledTargetRecord {
            collection: CollectionKind::RequestBody,
            modifier: TargetModifier::None,
            selector: CompiledTargetSelector::None
        }]
    ) {
        return false;
    }
    if !generation.operator_plans[index]
        .is_some_and(|operator| !operator.negated && matches!(operator.kind, OperatorKind::Contains | OperatorKind::Pm))
    {
        return false;
    }
    member.transformation_plan.is_none_or(|id| {
        let plan = generation.transformation_plans[id];
        generation.transformations[plan.transformation_start..plan.transformation_start + plan.transformation_count]
            .iter()
            .all(|kind| matches!(kind, TransformationKind::None | TransformationKind::Lowercase))
    })
}

fn classify_request_body_unit(generation: &CompiledGeneration, unit: CompiledRuleUnit) -> RequestBodyExecutionClass {
    let mut class = RequestBodyExecutionClass::StreamingRaw;
    let member_end = unit.member_start.saturating_add(unit.member_count);
    let Some(members) = generation.members.get(unit.member_start..member_end) else {
        return RequestBodyExecutionClass::Unsupported;
    };
    for (member_offset, member) in members.iter().enumerate() {
        // Body macros can occur on body-independent targets or on SecAction,
        // which has no target plan. They still require the parsed publication.
        if generation.member_has_body_macro(*member) {
            class = merge_request_body_class(class, RequestBodyExecutionClass::BoundedSnapshot);
        }
        let Some(target_plan_id) = member.target_plan else {
            continue;
        };
        let Some(target_plan) = generation.target_plans.get(target_plan_id).copied() else {
            return RequestBodyExecutionClass::Unsupported;
        };
        let record_end = target_plan.record_start.saturating_add(target_plan.record_count);
        let Some(records) = generation.target_records.get(target_plan.record_start..record_end) else {
            return RequestBodyExecutionClass::Unsupported;
        };
        let mut raw_body_target = false;
        let mut structured_body_target = false;
        for record in records {
            if record.collection == CollectionKind::Xml {
                if let CompiledTargetSelector::Path(range) = record.selector {
                    if !matches!(byte_range(&generation.target_bytes, range), Ok(b"/*" | b"//@*")) {
                        return RequestBodyExecutionClass::Unsupported;
                    }
                }
            }
            match record.collection {
                CollectionKind::RequestBody | CollectionKind::RequestBodyLength => {
                    raw_body_target = true;
                    if record.modifier != TargetModifier::None
                        || !matches!(record.selector, CompiledTargetSelector::None)
                    {
                        class = merge_request_body_class(class, RequestBodyExecutionClass::BoundedSnapshot);
                    }
                }
                CollectionKind::Args
                | CollectionKind::ArgsCombinedSize
                | CollectionKind::ArgsNames
                | CollectionKind::Files
                | CollectionKind::FilesCombinedSize
                | CollectionKind::FilesNames
                | CollectionKind::MultipartPartHeaders
                | CollectionKind::Xml => {
                    structured_body_target = true;
                }
                // This value depends on the processor selected from the real
                // content type (or ctl action). Publishing an empty Raw body
                // at streamed EOS would silently change its semantics.
                CollectionKind::ReqbodyProcessor => {
                    class = merge_request_body_class(class, RequestBodyExecutionClass::BoundedSnapshot);
                }
                CollectionKind::ResponseBody | CollectionKind::ResponseHeaders | CollectionKind::ResponseStatus => {
                    return RequestBodyExecutionClass::Unsupported;
                }
                _ => {}
            }
        }
        if structured_body_target {
            // A structured target alone does not prove incremental equivalence.
            // JSON object members publish in canonical key order (duplicate
            // occurrences grouped under their key), and that traversal order
            // affects captures and actions. Keep complete publication until
            // a processor-specific execution plan proves these semantics exact.
            class = merge_request_body_class(class, RequestBodyExecutionClass::BoundedSnapshot);
        }
        if !raw_body_target {
            continue;
        }
        let member_index = unit.member_start.saturating_add(member_offset);
        if unit.member_count != 1 || !is_streaming_raw_member(generation, member_index) {
            class = merge_request_body_class(class, RequestBodyExecutionClass::BoundedSnapshot);
        }
    }
    class
}

pub(super) fn is_streaming_raw_member(generation: &CompiledGeneration, member_index: usize) -> bool {
    let Some(member) = generation.members.get(member_index).copied() else {
        return false;
    };
    let Some(target_plan) = member
        .target_plan
        .and_then(|plan| generation.target_plans.get(plan))
        .copied()
    else {
        return false;
    };
    let Some(records) = target_plan
        .record_start
        .checked_add(target_plan.record_count)
        .and_then(|end| generation.target_records.get(target_plan.record_start..end))
    else {
        return false;
    };
    if !matches!(
        records,
        [CompiledTargetRecord {
            collection: CollectionKind::RequestBody,
            modifier: TargetModifier::None,
            selector: CompiledTargetSelector::None,
        }]
    ) {
        return false;
    }
    let stream_operator = generation
        .operator_plans
        .get(member_index)
        .and_then(|plan| *plan)
        .is_some_and(|plan| {
            matches!(plan.kind, OperatorKind::Pm | OperatorKind::PmFromFile)
                && matches!(plan.operand, CompiledOperatorOperand::Matcher { .. })
        });
    if !stream_operator {
        return false;
    }
    let stream_transformations = member.transformation_plan.is_none_or(|plan_id| {
        generation
            .transformation_plans
            .get(plan_id)
            .and_then(|plan| {
                let end = plan.transformation_start.checked_add(plan.transformation_count)?;
                generation.transformations.get(plan.transformation_start..end)
            })
            .is_some_and(|transformations| {
                transformations
                    .iter()
                    .all(|kind| matches!(kind, TransformationKind::None | TransformationKind::Lowercase))
            })
    });
    if !stream_transformations {
        return false;
    }
    let Some(actions) = member
        .action_start
        .checked_add(member.action_count)
        .and_then(|end| generation.actions.get(member.action_start..end))
    else {
        return false;
    };
    actions.iter().all(|action| match action.kind {
        ActionEnvelopeKind::Transformation => true,
        ActionEnvelopeKind::Action(kind) => is_streaming_safe_action(kind),
    })
}

/// Actions a streaming raw-body member may carry. The request-prefix path
/// additionally accepts `Msg` (see `prefix_actions_supported`). The asymmetry
/// is preserved as found; whether streaming members should also accept `Msg`
/// is tracked in `tasks/todo/waf-streaming-raw-member-msg-action.md`.
fn is_streaming_safe_action(kind: ActionKind) -> bool {
    matches!(
        kind,
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
    )
}

const fn merge_request_body_class(
    left: RequestBodyExecutionClass,
    right: RequestBodyExecutionClass,
) -> RequestBodyExecutionClass {
    use RequestBodyExecutionClass::{BoundedSnapshot, StreamingRaw, StreamingStructured, Unsupported};
    match (left, right) {
        (Unsupported, _) | (_, Unsupported) => Unsupported,
        (BoundedSnapshot, _) | (_, BoundedSnapshot) => BoundedSnapshot,
        (StreamingStructured, _) | (_, StreamingStructured) => StreamingStructured,
        (StreamingRaw, StreamingRaw) => StreamingRaw,
    }
}
