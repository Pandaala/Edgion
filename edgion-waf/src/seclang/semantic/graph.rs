//! Rule graph: directive locations, final rule roles, chain and marker records and their views.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DirectiveLocation {
    pub(super) document_index: usize,
    pub(super) directive_index: usize,
}

impl DirectiveLocation {
    pub const fn document_index(self) -> usize {
        self.document_index
    }
    pub const fn directive_index(self) -> usize {
        self.directive_index
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SemanticValidationError {
    #[error("rule-unit count exceeds the configured limit of {limit}")]
    RuleUnitLimit { limit: usize },
    #[error("rule-ID selector count exceeds the configured limit of {limit}")]
    IdSelectorLimit { limit: usize },
    #[error("applied exception count exceeds the configured limit of {limit}")]
    ExceptionLimit { limit: usize },
    #[error("temporary validation work exceeds the configured byte limit of {limit}")]
    WorkingByteLimit { limit: usize },
    #[error("duplicate global rule ID at {second:?}; first definition is at {first:?}")]
    DuplicateRuleId {
        first: DirectiveLocation,
        second: DirectiveLocation,
    },
    #[error("rule at {location:?} requires an implicit phase")]
    MissingImplicitPhase { location: DirectiveLocation },
    #[error("rule at {location:?} requires defaults for {phase:?}")]
    MissingPhaseDefaults { phase: Phase, location: DirectiveLocation },
    #[error("block resolved to pass while retaining status at {location:?}")]
    BlockStatusPass { location: SemanticLocation },
    #[error("collection {collection:?} is unavailable in {phase:?} at {location:?}")]
    CollectionPhase {
        collection: CollectionKind,
        phase: Phase,
        location: SemanticLocation,
    },
    #[error("collection {collection:?} forbids {selector:?} selectors at {location:?}")]
    SelectorNotAllowed {
        collection: CollectionKind,
        selector: SelectorKind,
        location: SemanticLocation,
    },
    #[error("path selector is valid only for XML at {location:?}")]
    PathSelectorCollection { location: SemanticLocation },
    #[error("rule-ID range is reversed at {position:?}")]
    ReversedIdRange { position: SourcePosition },
    #[error("rule-ID selector is unresolved at {position:?}")]
    UnresolvedIdSelector { position: SourcePosition },
    #[error("rule-ID selector references a later rule at {position:?}")]
    ForwardIdSelector { position: SourcePosition },
    #[error("raw exception matches exceed the per-directive limit of {limit} at {position:?}")]
    RawMatchDirectiveLimit { limit: usize, position: SourcePosition },
    #[error("distinct exception matches exceed the per-directive limit of {limit} at {position:?}")]
    DistinctMatchDirectiveLimit { limit: usize, position: SourcePosition },
    #[error("raw exception matches exceed the generation limit of {limit} at {position:?}")]
    RawMatchLimit { limit: usize, position: SourcePosition },
    #[error("distinct exception matches exceed the generation limit of {limit} at {position:?}")]
    DistinctMatchLimit { limit: usize, position: SourcePosition },
    #[error("rule-ID selector work exceeds the configured limit of {limit} at {position:?}")]
    SelectorWorkLimit { limit: usize, position: SourcePosition },
    #[error("target update addresses an inactive rule at {location:?}")]
    InactiveUpdate { location: DirectiveLocation },
    #[error("rule removal has no active match at {location:?}")]
    EmptyRemoval { location: DirectiveLocation },
    #[error("target update addresses a SecAction at {location:?}")]
    SecActionTargetUpdate { location: DirectiveLocation },
    #[error("final target count exceeds the per-rule limit of {limit} at {location:?}")]
    FinalTargetLimit { limit: usize, location: DirectiveLocation },
    #[error("final target references exceed the generation limit of {limit} at {location:?}")]
    FinalTargetReferenceLimit { limit: usize, location: DirectiveLocation },
    #[error("target exclusion has no positive base at {location:?}")]
    MissingExclusionBase { location: SemanticLocation },
    #[error("target comparisons exceed the configured limit of {limit} at {location:?}")]
    TargetComparisonLimit { limit: usize, location: SemanticLocation },
    #[error("target comparison bytes exceed the configured limit of {limit} at {location:?}")]
    TargetComparisonByteLimit { limit: usize, location: SemanticLocation },
    #[error("unavailable capability {capability:?} remains at {location:?}")]
    UnavailableCapability {
        capability: UnavailableCapability,
        location: SemanticLocation,
    },
    #[error("validated semantic metadata exceeds the configured owned-byte limit of {limit}")]
    OwnedByteLimit { limit: usize },
    #[error("validated semantic document exceeds the configured retained-byte limit of {limit}")]
    RetainedByteLimit { limit: usize },
    #[error("cannot allocate bounded semantic validation {operation}")]
    Allocation { operation: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalRuleEnvelope {
    Rule,
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldActionPresence {
    Absent,
    PresentEmpty,
    PresentNonempty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionalRuleRole {
    Root,
    Child {
        predecessor: DirectiveLocation,
        root: DirectiveLocation,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisruptiveKind {
    Block,
    Deny,
    Pass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalRuleRole {
    Action,
    StandaloneRule,
    ChainRoot {
        root: DirectiveLocation,
    },
    ChainChild {
        root: DirectiveLocation,
        predecessor: DirectiveLocation,
        ordinal: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalPhaseState {
    Explicit { phase: Phase, source: DirectiveLocation },
    Inherited { phase: Phase, root: DirectiveLocation },
    RequiresDefault { owner: DirectiveLocation },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FinalRuleRecord {
    pub(super) fold: usize,
    pub(super) role: FinalRuleRole,
    pub(super) phase: FinalPhaseState,
    pub(super) chain: Option<usize>,
    pub(super) resolution: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ChainRecord {
    pub(super) root: DirectiveLocation,
    pub(super) phase: FinalPhaseState,
    pub(super) member_start: usize,
    pub(super) member_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MarkerRecord<'source> {
    pub(super) location: DirectiveLocation,
    pub(super) name: &'source str,
    pub(super) span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResolvedSkipAfterRecord {
    pub(super) referrer: usize,
    pub(super) action: usize,
    pub(super) marker: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LocalRuleFoldRecord {
    pub(super) location: DirectiveLocation,
    pub(super) envelope: LocalRuleEnvelope,
    pub(super) role: Option<ProvisionalRuleRole>,
    pub(super) presence: FoldActionPresence,
    pub(super) action_start: usize,
    pub(super) action_count: usize,
    pub(super) id: Option<u64>,
    pub(super) phase: Option<Phase>,
    pub(super) disruptive: Option<DisruptiveKind>,
    pub(super) status: Option<u16>,
    pub(super) id_action: Option<usize>,
    pub(super) phase_action: Option<usize>,
    pub(super) disruptive_action: Option<usize>,
    pub(super) status_action: Option<usize>,
    pub(super) chain_action: Option<usize>,
    pub(super) skip_action: Option<usize>,
    pub(super) capture_action: Option<usize>,
    pub(super) multi_match_action: Option<usize>,
    pub(super) reset_action: Option<usize>,
    pub(super) log: Option<bool>,
    pub(super) audit_log: Option<bool>,
    pub(super) effective_start: usize,
    pub(super) effective_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct LocalRuleFoldView<'view, 'source> {
    pub(super) index: usize,
    pub(super) record: &'view LocalRuleFoldRecord,
    pub(super) actions: &'view [SemanticActionRecord<'source>],
    pub(super) effective: &'view [usize],
    pub(super) arena: TemplateArenaView<'view, 'source>,
    pub(super) graph: Option<FinalGraphArenaView<'view, 'source>>,
}

impl<'view, 'source> LocalRuleFoldView<'view, 'source> {
    pub const fn location(self) -> DirectiveLocation {
        self.record.location
    }
    pub const fn envelope(self) -> LocalRuleEnvelope {
        self.record.envelope
    }
    pub const fn role(self) -> Option<ProvisionalRuleRole> {
        self.record.role
    }
    pub const fn action_presence(self) -> FoldActionPresence {
        self.record.presence
    }
    pub fn actions(self) -> SemanticActionIter<'view, 'source> {
        SemanticActionIter {
            records: self.actions[self.record.action_start..self.record.action_start + self.record.action_count].iter(),
            arena: self.arena,
        }
    }
    pub const fn id(self) -> Option<u64> {
        self.record.id
    }
    pub const fn phase(self) -> Option<Phase> {
        self.record.phase
    }
    pub const fn disruptive(self) -> Option<DisruptiveKind> {
        self.record.disruptive
    }
    pub const fn status(self) -> Option<u16> {
        self.record.status
    }
    pub const fn log_override(self) -> Option<bool> {
        self.record.log
    }
    pub const fn audit_log_override(self) -> Option<bool> {
        self.record.audit_log
    }
    pub const fn reset(self) -> bool {
        self.record.reset_action.is_some()
    }
    pub fn id_action(self) -> Option<SemanticActionView<'view, 'source>> {
        self.action(self.record.id_action)
    }
    pub fn phase_action(self) -> Option<SemanticActionView<'view, 'source>> {
        self.action(self.record.phase_action)
    }
    pub fn disruptive_action(self) -> Option<SemanticActionView<'view, 'source>> {
        self.action(self.record.disruptive_action)
    }
    pub fn status_action(self) -> Option<SemanticActionView<'view, 'source>> {
        self.action(self.record.status_action)
    }
    pub fn chain_action(self) -> Option<SemanticActionView<'view, 'source>> {
        self.action(self.record.chain_action)
    }
    pub fn skip_family_action(self) -> Option<SemanticActionView<'view, 'source>> {
        self.action(self.record.skip_action)
    }
    pub fn capture_action(self) -> Option<SemanticActionView<'view, 'source>> {
        self.action(self.record.capture_action)
    }
    pub fn multi_match_action(self) -> Option<SemanticActionView<'view, 'source>> {
        self.action(self.record.multi_match_action)
    }
    pub fn reset_action(self) -> Option<SemanticActionView<'view, 'source>> {
        self.action(self.record.reset_action)
    }
    pub fn effective_transformations(
        self,
    ) -> impl ExactSizeIterator<Item = SemanticActionView<'view, 'source>> + FusedIterator + 'view {
        let actions = self.actions;
        let arena = self.arena;
        self.effective[self.record.effective_start..self.record.effective_start + self.record.effective_count]
            .iter()
            .map(move |&index| SemanticActionView {
                record: &actions[index],
                arena,
            })
    }
    pub fn final_rule(self) -> Option<FinalRuleView<'view, 'source>> {
        let graph = self.graph?;
        Some(FinalRuleView {
            index: self.index,
            arena: graph,
        })
    }
    fn action(self, index: Option<usize>) -> Option<SemanticActionView<'view, 'source>> {
        index.map(|index| SemanticActionView {
            record: &self.actions[index],
            arena: self.arena,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct FinalGraphArenaView<'view, 'source> {
    pub(super) folds: &'view [LocalRuleFoldRecord],
    pub(super) actions: &'view [SemanticActionRecord<'source>],
    pub(super) effective: &'view [usize],
    pub(super) action_arena: TemplateArenaView<'view, 'source>,
    pub(super) final_rules: &'view [FinalRuleRecord],
    pub(super) chains: &'view [ChainRecord],
    pub(super) chain_members: &'view [usize],
    pub(super) markers: &'view [MarkerRecord<'source>],
    pub(super) resolutions: &'view [ResolvedSkipAfterRecord],
}

impl<'view, 'source> FinalGraphArenaView<'view, 'source> {
    pub(super) fn rule(self, index: usize) -> FinalRuleView<'view, 'source> {
        FinalRuleView { index, arena: self }
    }
    fn fold(self, index: usize) -> LocalRuleFoldView<'view, 'source> {
        LocalRuleFoldView {
            index,
            record: &self.folds[index],
            actions: self.actions,
            effective: self.effective,
            arena: self.action_arena,
            graph: Some(self),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FinalRuleView<'view, 'source> {
    index: usize,
    arena: FinalGraphArenaView<'view, 'source>,
}
impl<'view, 'source> FinalRuleView<'view, 'source> {
    fn record(self) -> &'view FinalRuleRecord {
        &self.arena.final_rules[self.index]
    }
    pub fn location(self) -> DirectiveLocation {
        self.fold().location()
    }
    pub fn fold(self) -> LocalRuleFoldView<'view, 'source> {
        self.arena.fold(self.record().fold)
    }
    pub fn role(self) -> FinalRuleRole {
        self.record().role
    }
    pub fn phase(self) -> FinalPhaseState {
        self.record().phase
    }
    pub fn chain(self) -> Option<ChainView<'view, 'source>> {
        self.record().chain.map(|index| ChainView {
            index,
            arena: self.arena,
        })
    }
    pub fn resolved_skip_after(self) -> Option<ResolvedSkipAfterView<'view, 'source>> {
        self.record().resolution.map(|index| ResolvedSkipAfterView {
            index,
            arena: self.arena,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChainView<'view, 'source> {
    pub(super) index: usize,
    pub(super) arena: FinalGraphArenaView<'view, 'source>,
}
impl<'view, 'source> ChainView<'view, 'source> {
    fn record(self) -> &'view ChainRecord {
        &self.arena.chains[self.index]
    }
    pub fn root(self) -> DirectiveLocation {
        self.record().root
    }
    pub fn phase(self) -> FinalPhaseState {
        self.record().phase
    }
    pub fn members(self) -> impl ExactSizeIterator<Item = ChainMemberView<'view, 'source>> + FusedIterator + 'view {
        let record = self.record();
        let arena = self.arena;
        arena.chain_members[record.member_start..record.member_start + record.member_count]
            .iter()
            .enumerate()
            .map(move |(ordinal, &index)| ChainMemberView {
                ordinal,
                rule: arena.rule(index),
            })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChainMemberView<'view, 'source> {
    ordinal: usize,
    rule: FinalRuleView<'view, 'source>,
}
impl<'view, 'source> ChainMemberView<'view, 'source> {
    pub const fn ordinal(self) -> usize {
        self.ordinal
    }
    pub const fn rule(self) -> FinalRuleView<'view, 'source> {
        self.rule
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MarkerView<'view, 'source> {
    pub(super) index: usize,
    pub(super) arena: FinalGraphArenaView<'view, 'source>,
}
impl<'view, 'source> MarkerView<'view, 'source> {
    fn record(self) -> &'view MarkerRecord<'source> {
        &self.arena.markers[self.index]
    }
    pub fn location(self) -> DirectiveLocation {
        self.record().location
    }
    pub fn name(self) -> &'source str {
        self.record().name
    }
    pub fn span(self) -> SourceSpan {
        self.record().span
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedSkipAfterView<'view, 'source> {
    pub(super) index: usize,
    pub(super) arena: FinalGraphArenaView<'view, 'source>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedDisruptive {
    Pass,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResolvedRuleStateRecord {
    pub(super) phase: Phase,
    pub(super) disruptive: ResolvedDisruptive,
    pub(super) status: Option<u16>,
    pub(super) log: bool,
    pub(super) audit_log: bool,
    pub(super) unit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ValidatedRuleNodeRecord {
    pub(super) rule: usize,
    pub(super) state: ResolvedRuleStateRecord,
    pub(super) target_start: usize,
    pub(super) target_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ValidatedRuleUnitRecord {
    pub(super) id: u64,
    pub(super) root: usize,
    pub(super) active: bool,
    pub(super) compatibility_excluded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RuleIdIndexRecord {
    pub(super) id: u64,
    pub(super) unit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TargetOverlayEntry {
    pub(super) target: usize,
    pub(super) next: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AppliedExceptionRecord {
    pub(super) directive: usize,
    pub(super) affected_start: usize,
    pub(super) affected_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ValidatedRequirementReference {
    Draft(usize),
    TargetSelectorRegex { target: usize },
}

#[derive(Debug)]
pub struct ValidatedSemanticDocument<'source> {
    pub(super) draft: SemanticDraft<'source>,
    pub(super) nodes: Vec<ValidatedRuleNodeRecord>,
    pub(super) units: Vec<ValidatedRuleUnitRecord>,
    pub(super) id_index: Vec<RuleIdIndexRecord>,
    pub(super) final_targets: Vec<usize>,
    pub(super) updates: Vec<AppliedExceptionRecord>,
    pub(super) removals: Vec<AppliedExceptionRecord>,
    pub(super) affected_units: Vec<usize>,
    pub(super) requirements: Vec<ValidatedRequirementReference>,
    pub(super) validation_charged_bytes: usize,
}

impl<'source> ValidatedSemanticDocument<'source> {
    pub(crate) fn selection_target_scratch_bytes(&self) -> usize {
        self.draft.targets.len()
    }
    /// Remove activity only after validating the complete original rule graph.
    pub(crate) fn apply_selection_mask(
        &mut self,
        mask: &[bool],
        work: &mut CanonicalWork,
        limits: CompilerLimits,
    ) -> Result<(), CompilerError> {
        if mask.len() != self.units.len() {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            });
        }
        let mut seen = Vec::new();
        seen.try_reserve_exact(self.draft.targets.len())
            .map_err(|_| CompilerError::Allocation {
                operation: "selected target reachability",
            })?;
        seen.resize(self.draft.targets.len(), false);
        for (unit, keep) in self.units.iter_mut().zip(mask) {
            work.charge_records(1, limits)?;
            unit.active &= *keep;
        }
        for unit in self.active_rule_units() {
            for member in unit.members() {
                let node = member.record();
                for target in &self.final_targets[node.target_start..node.target_start + node.target_count] {
                    work.charge_records(1, limits)?;
                    seen[*target] = true;
                }
            }
        }
        // Draft requirements remain strict. Only active target-regex reachability changes.
        for _ in &self.requirements {
            work.charge_records(1, limits)?;
        }
        self.requirements.retain(|requirement| match requirement {
            ValidatedRequirementReference::Draft(_) => true,
            ValidatedRequirementReference::TargetSelectorRegex { target } => seen[*target],
        });
        // retain preserves the allocation; do not reduce validation_charged_bytes.
        Ok(())
    }

    pub fn rule_units(&self) -> impl ExactSizeIterator<Item = ValidatedRuleUnitView<'_, 'source>> + FusedIterator {
        (0..self.units.len()).map(|index| ValidatedRuleUnitView { index, owner: self })
    }

    /// Reuse the validated ID index for bounded inspection pagination without an ID copy.
    pub fn rule_units_by_id(
        &self,
    ) -> impl ExactSizeIterator<Item = ValidatedRuleUnitView<'_, 'source>> + FusedIterator {
        self.id_index.iter().map(|entry| ValidatedRuleUnitView {
            index: entry.unit,
            owner: self,
        })
    }

    pub fn active_rule_units(&self) -> ActiveRuleUnitIter<'_, 'source> {
        ActiveRuleUnitIter {
            owner: self,
            index: 0,
            remaining: self.units.iter().filter(|unit| unit.active).count(),
        }
    }

    pub fn rule_by_id(&self, id: u64) -> Option<ValidatedRuleUnitView<'_, 'source>> {
        let index = self.id_index.binary_search_by_key(&id, |entry| entry.id).ok()?;
        let unit = self.id_index[index].unit;
        self.units[unit].active.then_some(ValidatedRuleUnitView {
            index: unit,
            owner: self,
        })
    }

    pub fn rule_nodes(&self) -> impl ExactSizeIterator<Item = ValidatedRuleNodeView<'_, 'source>> + FusedIterator {
        (0..self.nodes.len()).map(|index| ValidatedRuleNodeView { index, owner: self })
    }

    pub fn applied_target_updates(
        &self,
    ) -> impl ExactSizeIterator<Item = AppliedTargetUpdateView<'_, 'source>> + FusedIterator {
        (0..self.updates.len()).map(|index| AppliedTargetUpdateView { index, owner: self })
    }

    pub fn applied_rule_removals(
        &self,
    ) -> impl ExactSizeIterator<Item = AppliedRuleRemovalView<'_, 'source>> + FusedIterator {
        (0..self.removals.len()).map(|index| AppliedRuleRemovalView { index, owner: self })
    }

    pub fn requirements(&self) -> impl ExactSizeIterator<Item = ValidatedRequirementView<'_, 'source>> + FusedIterator {
        (0..self.requirements.len()).map(|index| ValidatedRequirementView { index, owner: self })
    }

    pub fn markers(&self) -> impl ExactSizeIterator<Item = ValidatedMarkerView<'_, 'source>> + FusedIterator {
        (0..self.draft.markers.len()).map(|index| ValidatedMarkerView { index, owner: self })
    }

    pub const fn validation_charged_bytes(&self) -> usize {
        self.validation_charged_bytes
    }

    pub fn retained_charged_bytes(&self) -> usize {
        self.draft.retained_charged_bytes() + self.validation_charged_bytes
    }

    fn node(&self, index: usize) -> ValidatedRuleNodeView<'_, 'source> {
        ValidatedRuleNodeView { index, owner: self }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedRuleStateView<'view> {
    record: &'view ResolvedRuleStateRecord,
}

impl ResolvedRuleStateView<'_> {
    pub const fn phase(self) -> Phase {
        self.record.phase
    }
    pub const fn disruptive(self) -> ResolvedDisruptive {
        self.record.disruptive
    }
    pub const fn status(self) -> Option<u16> {
        self.record.status
    }
    pub const fn log(self) -> bool {
        self.record.log
    }
    pub const fn audit_log(self) -> bool {
        self.record.audit_log
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ValidatedRuleNodeView<'view, 'source> {
    index: usize,
    owner: &'view ValidatedSemanticDocument<'source>,
}

impl<'view, 'source> ValidatedRuleNodeView<'view, 'source> {
    fn record(self) -> &'view ValidatedRuleNodeRecord {
        &self.owner.nodes[self.index]
    }
    pub fn rule(self) -> FinalRuleView<'view, 'source> {
        self.owner.draft.final_graph_arena().rule(self.record().rule)
    }
    pub fn role(self) -> FinalRuleRole {
        self.rule().role()
    }
    pub fn resolved_state(self) -> ResolvedRuleStateView<'view> {
        ResolvedRuleStateView {
            record: &self.record().state,
        }
    }
    pub fn final_targets(
        self,
    ) -> impl ExactSizeIterator<Item = &'view SemanticTarget<'source>> + FusedIterator + 'view {
        let record = self.record();
        self.owner.final_targets[record.target_start..record.target_start + record.target_count]
            .iter()
            .map(|index| &self.owner.draft.targets[*index])
    }
    pub fn source_directive(self) -> SemanticDirectiveView<'view, 'source> {
        let rule = self.owner.draft.final_rules[self.record().rule];
        let mapping = semantic_mapping_for_rule(&self.owner.draft, rule);
        let index = self
            .owner
            .draft
            .directives
            .binary_search_by_key(&(mapping.document_index, mapping.directive_index), |record| {
                (record.document_index, record.directive_index)
            })
            .expect("validated rule directive remains mapped");
        self.owner
            .draft
            .directive(index)
            .expect("validated rule directive remains present")
    }
    pub fn operator(self) -> Option<SemanticOperatorView<'view, 'source>> {
        self.source_directive().operator()
    }
    pub fn actions(self) -> SemanticActionIter<'view, 'source> {
        self.rule().fold().actions()
    }
    pub fn effective_transformations(
        self,
    ) -> impl ExactSizeIterator<Item = SemanticActionView<'view, 'source>> + FusedIterator + 'view {
        self.rule().fold().effective_transformations()
    }
    pub fn skip_after_marker(self) -> Option<ValidatedMarkerView<'view, 'source>> {
        let rule = self.owner.draft.final_rules[self.record().rule];
        let resolution = rule.resolution?;
        let marker = self.owner.draft.resolved_skip_afters[resolution].marker;
        Some(ValidatedMarkerView {
            index: marker,
            owner: self.owner,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ValidatedMarkerView<'view, 'source> {
    index: usize,
    owner: &'view ValidatedSemanticDocument<'source>,
}

impl ValidatedMarkerView<'_, '_> {
    pub const fn ordinal(self) -> usize {
        self.index
    }
    pub fn location(self) -> DirectiveLocation {
        self.owner.draft.markers[self.index].location
    }
    pub fn span(self) -> SourceSpan {
        self.owner.draft.markers[self.index].span
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ValidatedRuleUnitView<'view, 'source> {
    index: usize,
    owner: &'view ValidatedSemanticDocument<'source>,
}

impl<'view, 'source> ValidatedRuleUnitView<'view, 'source> {
    pub const fn ordinal(self) -> usize {
        self.index
    }
    fn record(self) -> &'view ValidatedRuleUnitRecord {
        &self.owner.units[self.index]
    }
    pub fn id(self) -> u64 {
        self.record().id
    }
    pub fn root(self) -> ValidatedRuleNodeView<'view, 'source> {
        self.owner.node(self.record().root)
    }
    pub fn is_active(self) -> bool {
        self.record().active
    }
    pub fn is_compatibility_excluded(self) -> bool {
        self.record().compatibility_excluded
    }
    pub fn members(
        self,
    ) -> impl ExactSizeIterator<Item = ValidatedRuleNodeView<'view, 'source>> + FusedIterator + 'view {
        let owner = self.owner;
        let root = self.record().root;
        let rule = owner.draft.final_rules[root];
        // A chained unit walks its chain-member slice; an unchained unit yields only its root node.
        let (members, positions): (Option<&'view [usize]>, Range<usize>) = match rule.chain {
            Some(chain) => {
                let chain = owner.draft.chains[chain];
                let members = &owner.draft.chain_members[chain.member_start..chain.member_start + chain.member_count];
                (Some(members), 0..members.len())
            }
            None => (None, root..root + 1),
        };
        positions.map(move |position| owner.node(members.map_or(position, |members| members[position])))
    }
}

#[derive(Debug, Clone)]
pub struct ActiveRuleUnitIter<'view, 'source> {
    owner: &'view ValidatedSemanticDocument<'source>,
    index: usize,
    remaining: usize,
}

impl<'view, 'source> Iterator for ActiveRuleUnitIter<'view, 'source> {
    type Item = ValidatedRuleUnitView<'view, 'source>;
    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.owner.units.len() {
            let index = self.index;
            self.index += 1;
            if self.owner.units[index].active {
                self.remaining -= 1;
                return Some(ValidatedRuleUnitView {
                    index,
                    owner: self.owner,
                });
            }
        }
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for ActiveRuleUnitIter<'_, '_> {}
impl FusedIterator for ActiveRuleUnitIter<'_, '_> {}

#[derive(Debug, Clone, Copy)]
pub struct AppliedTargetUpdateView<'view, 'source> {
    index: usize,
    owner: &'view ValidatedSemanticDocument<'source>,
}

#[derive(Debug, Clone, Copy)]
pub struct AppliedRuleRemovalView<'view, 'source> {
    index: usize,
    owner: &'view ValidatedSemanticDocument<'source>,
}

macro_rules! applied_view {
    ($name:ident, $field:ident) => {
        impl<'view, 'source> $name<'view, 'source> {
            fn record(self) -> &'view AppliedExceptionRecord {
                &self.owner.$field[self.index]
            }
            pub fn source_directive(self) -> SemanticDirectiveView<'view, 'source> {
                self.owner
                    .draft
                    .directive(self.record().directive)
                    .expect("validated exception directive remains present")
            }
            pub fn affected_units(
                self,
            ) -> impl ExactSizeIterator<Item = ValidatedRuleUnitView<'view, 'source>> + FusedIterator + 'view {
                let record = self.record();
                let owner = self.owner;
                owner.affected_units[record.affected_start..record.affected_start + record.affected_count]
                    .iter()
                    .map(move |&index| ValidatedRuleUnitView { index, owner })
            }
        }
    };
}
applied_view!(AppliedTargetUpdateView, updates);
applied_view!(AppliedRuleRemovalView, removals);

#[derive(Debug, Clone, Copy)]
pub struct ValidatedRequirementView<'view, 'source> {
    index: usize,
    owner: &'view ValidatedSemanticDocument<'source>,
}

impl<'view, 'source> ValidatedRequirementView<'view, 'source> {
    pub fn kind(self) -> CompileRequirement {
        match self.owner.requirements[self.index] {
            ValidatedRequirementReference::Draft(index) => self.owner.draft.requirements[index].requirement,
            ValidatedRequirementReference::TargetSelectorRegex { .. } => {
                CompileRequirement::TargetSelectorRegexCompilation
            }
        }
    }
    pub fn location(self) -> SemanticLocation {
        match self.owner.requirements[self.index] {
            ValidatedRequirementReference::Draft(index) => self.owner.draft.requirements[index].location,
            ValidatedRequirementReference::TargetSelectorRegex { target } => self.owner.draft.targets[target].location,
        }
    }
    pub fn target(self) -> Option<&'view SemanticTarget<'source>> {
        match self.owner.requirements[self.index] {
            ValidatedRequirementReference::Draft(_) => None,
            ValidatedRequirementReference::TargetSelectorRegex { target } => Some(&self.owner.draft.targets[target]),
        }
    }
}
impl<'view, 'source> ResolvedSkipAfterView<'view, 'source> {
    fn record(self) -> &'view ResolvedSkipAfterRecord {
        &self.arena.resolutions[self.index]
    }
    pub fn referrer(self) -> DirectiveLocation {
        self.arena.rule(self.record().referrer).location()
    }
    pub fn source_action(self) -> SemanticActionView<'view, 'source> {
        let record = self.record();
        SemanticActionView {
            record: &self.arena.actions[record.action],
            arena: self.arena.action_arena,
        }
    }
    pub fn target(self) -> MarkerView<'view, 'source> {
        MarkerView {
            index: self.record().marker,
            arena: self.arena,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequirementRecord {
    pub(super) location: SemanticLocation,
    pub(super) requirement: CompileRequirement,
}

impl RequirementRecord {
    pub const fn location(self) -> SemanticLocation {
        self.location
    }
    pub const fn requirement(self) -> CompileRequirement {
        self.requirement
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityRecord {
    pub(super) location: SemanticLocation,
    pub(super) capability: UnavailableCapability,
}

impl CapabilityRecord {
    pub const fn location(self) -> SemanticLocation {
        self.location
    }
    pub const fn capability(self) -> UnavailableCapability {
        self.capability
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct DirectiveSemanticRecord {
    pub(super) document_index: usize,
    pub(super) directive_index: usize,
    pub(super) target_start: usize,
    pub(super) target_count: usize,
    pub(super) operator: Option<usize>,
    pub(super) action_start: usize,
    pub(super) action_count: usize,
    pub(super) fold: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct SemanticDirectiveView<'view, 'source> {
    pub(super) document_index: usize,
    pub(super) directive_index: usize,
    pub(super) targets: &'view [SemanticTarget<'source>],
    pub(super) id_selectors: &'view [IdSelector<'source>],
    pub(super) operator: Option<SemanticOperatorView<'view, 'source>>,
    pub(super) actions: SemanticActionIter<'view, 'source>,
    pub(super) fold: Option<LocalRuleFoldView<'view, 'source>>,
    pub(super) final_rule: Option<FinalRuleView<'view, 'source>>,
}

impl<'view, 'source> SemanticDirectiveView<'view, 'source> {
    pub const fn document_index(&self) -> usize {
        self.document_index
    }
    pub const fn directive_index(&self) -> usize {
        self.directive_index
    }
    pub const fn targets(&self) -> &'view [SemanticTarget<'source>] {
        self.targets
    }
    pub const fn id_selectors(&self) -> &'view [IdSelector<'source>] {
        self.id_selectors
    }
    pub const fn operator(&self) -> Option<SemanticOperatorView<'view, 'source>> {
        self.operator
    }
    pub fn actions(&self) -> SemanticActionIter<'view, 'source> {
        self.actions.clone()
    }
    pub const fn local_rule_fold(&self) -> Option<LocalRuleFoldView<'view, 'source>> {
        self.fold
    }
    pub const fn final_rule(&self) -> Option<FinalRuleView<'view, 'source>> {
        self.final_rule
    }
}
