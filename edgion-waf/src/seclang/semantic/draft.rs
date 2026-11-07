use super::*;

#[derive(Debug)]
pub struct SemanticDraft<'source> {
    pub(super) documents: Vec<SyntaxDocument<'source>>,
    pub(super) directives: Vec<DirectiveSemanticRecord>,
    pub(super) targets: Vec<SemanticTarget<'source>>,
    pub(super) operators: Vec<SemanticOperatorRecord<'source>>,
    pub(super) actions: Vec<SemanticActionRecord<'source>>,
    pub(super) action_operands: Vec<ActionOperandRecord<'source>>,
    pub(super) setvars: Vec<SetVarRecord>,
    pub(super) initcols: Vec<InitColRecord>,
    pub(super) ctls: Vec<CtlRecord<'source>>,
    pub(super) ctl_targets: Vec<CtlTargetRecord<'source>>,
    pub(super) templates: Vec<TemplateRecord<'source>>,
    pub(super) template_segments: Vec<TemplateSegmentRecord>,
    pub(super) expansions: Vec<ExpansionRecord<'source>>,
    pub(super) decoded_operands: Vec<u8>,
    pub(super) operator_operands: Vec<OperatorOperandRecord>,
    pub(super) static_operands: Vec<StaticOperandRecord<'source>>,
    pub(super) list_items: Vec<ListItemRecord>,
    pub(super) networks: Vec<NetworkRecord>,
    pub(super) phrase_count: usize,
    pub(super) asset_count: usize,
    pub(super) structured_action_count: usize,
    pub(super) phrase_bytes: usize,
    pub(super) scanned_operand_bytes: usize,
    pub(super) operand_limits: OperandLimits,
    pub(super) requirements: Vec<RequirementRecord>,
    pub(super) unavailable: Vec<CapabilityRecord>,
    pub(super) folds: Vec<LocalRuleFoldRecord>,
    pub(super) effective_transformations: Vec<usize>,
    pub(super) final_rules: Vec<FinalRuleRecord>,
    pub(super) chains: Vec<ChainRecord>,
    pub(super) chain_members: Vec<usize>,
    pub(super) markers: Vec<MarkerRecord<'source>>,
    pub(super) resolved_skip_afters: Vec<ResolvedSkipAfterRecord>,
    pub(super) semantic_charged_bytes: usize,
    pub(super) inherited_charged_bytes: usize,
}

impl<'source> SemanticDraft<'source> {
    pub fn document_count(&self) -> usize {
        self.documents.len()
    }
    pub fn directive_count(&self) -> usize {
        self.directives.len()
    }
    pub fn directive(&self, index: usize) -> Option<SemanticDirectiveView<'_, 'source>> {
        let record = self.directives.get(index)?;
        let syntax = self.documents.get(record.document_index)?.get(record.directive_index)?;
        let id_selectors = match syntax.targets() {
            DirectiveTargetView::Update { ids, .. } | DirectiveTargetView::Remove { ids } => ids,
            DirectiveTargetView::None | DirectiveTargetView::Rule { .. } => &[],
        };
        let arena = self.template_arena();
        Some(SemanticDirectiveView {
            document_index: record.document_index,
            directive_index: record.directive_index,
            targets: &self.targets[record.target_start..record.target_start + record.target_count],
            id_selectors,
            operator: record.operator.map(|operator| SemanticOperatorView {
                record: &self.operators[operator],
                arena,
            }),
            actions: SemanticActionIter {
                records: self.actions[record.action_start..record.action_start + record.action_count].iter(),
                arena,
            },
            fold: record.fold.map(|index| self.fold_view(index)),
            final_rule: record
                .fold
                .filter(|index| *index < self.final_rules.len())
                .map(|index| self.final_graph_arena().rule(index)),
        })
    }
    pub fn directives(&self) -> impl ExactSizeIterator<Item = SemanticDirectiveView<'_, 'source>> + FusedIterator {
        (0..self.directives.len()).map(|index| self.directive(index).expect("iterator index is bounded"))
    }
    pub fn target_count(&self) -> usize {
        self.targets.len()
    }
    pub fn operator_count(&self) -> usize {
        self.operators.len()
    }
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }
    pub fn semantic_charged_bytes(&self) -> usize {
        self.semantic_charged_bytes
    }
    pub fn inherited_charged_bytes(&self) -> usize {
        self.inherited_charged_bytes
    }
    pub fn retained_charged_bytes(&self) -> usize {
        self.inherited_charged_bytes
            .checked_add(self.semantic_charged_bytes)
            .expect("published semantic retained charge was checked during construction")
    }
    pub fn documents(&self) -> impl ExactSizeIterator<Item = &SyntaxDocument<'source>> {
        self.documents.iter()
    }
    pub fn targets(&self) -> impl ExactSizeIterator<Item = &SemanticTarget<'source>> + FusedIterator {
        self.targets.iter()
    }
    pub fn operators(&self) -> impl ExactSizeIterator<Item = SemanticOperatorView<'_, 'source>> + FusedIterator {
        let arena = self.template_arena();
        self.operators
            .iter()
            .map(move |record| SemanticOperatorView { record, arena })
    }
    pub fn actions(&self) -> SemanticActionIter<'_, 'source> {
        SemanticActionIter {
            records: self.actions.iter(),
            arena: self.template_arena(),
        }
    }
    pub fn requirements(&self) -> impl ExactSizeIterator<Item = &RequirementRecord> + FusedIterator {
        self.requirements.iter()
    }
    pub fn folded_rules(&self) -> impl ExactSizeIterator<Item = LocalRuleFoldView<'_, 'source>> + FusedIterator {
        (0..self.folds.len()).map(|index| self.fold_view(index))
    }
    pub fn final_rules(&self) -> impl ExactSizeIterator<Item = FinalRuleView<'_, 'source>> + FusedIterator {
        let arena = self.final_graph_arena();
        (0..self.final_rules.len()).map(move |index| arena.rule(index))
    }
    pub fn chains(&self) -> impl ExactSizeIterator<Item = ChainView<'_, 'source>> + FusedIterator {
        let arena = self.final_graph_arena();
        (0..self.chains.len()).map(move |index| ChainView { index, arena })
    }
    pub fn markers(&self) -> impl ExactSizeIterator<Item = MarkerView<'_, 'source>> + FusedIterator {
        let arena = self.final_graph_arena();
        (0..self.markers.len()).map(move |index| MarkerView { index, arena })
    }
    pub fn resolved_skip_afters(
        &self,
    ) -> impl ExactSizeIterator<Item = ResolvedSkipAfterView<'_, 'source>> + FusedIterator {
        let arena = self.final_graph_arena();
        (0..self.resolved_skip_afters.len()).map(move |index| ResolvedSkipAfterView { index, arena })
    }
    pub fn unavailable_capabilities(&self) -> impl ExactSizeIterator<Item = &CapabilityRecord> + FusedIterator {
        self.unavailable.iter()
    }
    pub fn template_count(&self) -> usize {
        self.templates.len()
    }
    pub fn template_segment_count(&self) -> usize {
        self.template_segments.len()
    }
    pub fn expansion_count(&self) -> usize {
        self.expansions.len()
    }
    pub fn scanned_operand_bytes(&self) -> usize {
        self.scanned_operand_bytes
    }
    pub fn decoded_operand_bytes(&self) -> usize {
        self.decoded_operands.len()
    }
    pub fn phrase_count(&self) -> usize {
        self.phrase_count
    }
    pub fn external_asset_count(&self) -> usize {
        self.asset_count
    }
    pub fn structured_action_count(&self) -> usize {
        self.structured_action_count
    }
    pub fn ctl_target_count(&self) -> usize {
        self.ctl_targets.len()
    }
    pub fn network_count(&self) -> usize {
        self.networks.len()
    }
    pub fn phrase_bytes(&self) -> usize {
        self.phrase_bytes
    }
    pub const fn operand_limits(&self) -> OperandLimits {
        self.operand_limits
    }

    fn template_arena(&self) -> TemplateArenaView<'_, 'source> {
        TemplateArenaView {
            templates: &self.templates,
            segments: &self.template_segments,
            expansions: &self.expansions,
            decoded: &self.decoded_operands,
            operands: &self.operator_operands,
            static_operands: &self.static_operands,
            list_items: &self.list_items,
            networks: &self.networks,
            action_operands: &self.action_operands,
            setvars: &self.setvars,
            initcols: &self.initcols,
            ctls: &self.ctls,
            ctl_targets: &self.ctl_targets,
        }
    }

    fn fold_view(&self, index: usize) -> LocalRuleFoldView<'_, 'source> {
        LocalRuleFoldView {
            index,
            record: &self.folds[index],
            actions: &self.actions,
            effective: &self.effective_transformations,
            arena: self.template_arena(),
            graph: (!self.final_rules.is_empty()).then(|| self.final_graph_arena()),
        }
    }
    pub(super) fn final_graph_arena(&self) -> FinalGraphArenaView<'_, 'source> {
        FinalGraphArenaView {
            folds: &self.folds,
            actions: &self.actions,
            effective: &self.effective_transformations,
            action_arena: self.template_arena(),
            final_rules: &self.final_rules,
            chains: &self.chains,
            chain_members: &self.chain_members,
            markers: &self.markers,
            resolutions: &self.resolved_skip_afters,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarKind {
    Id,
    Phase,
    Severity,
    Status,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueContract {
    Required,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroErrorKind {
    Unclosed,
    EmptyBody,
    Nested,
    InvalidCharacter,
    UnknownSource,
    MissingKey,
    ForbiddenKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorOperandContract {
    Empty,
    NonEmpty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerOperandErrorKind {
    Invalid,
    Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetErrorKind {
    Absolute,
    Backslash,
    Control,
    Scheme,
    EmptyComponent,
    DotComponent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetVarErrorKind {
    Envelope,
    Collection,
    Operation,
    Key,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitColErrorKind {
    Envelope,
    Collection,
    Key,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtlErrorKind {
    Envelope,
    Command,
    Value,
    Delimiter,
    Collection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldSingletonKind {
    Id,
    Phase,
    Severity,
    Status,
    Msg,
    LogData,
    Ver,
    SkipFamily,
    Chain,
    Capture,
    MultiMatch,
    Disruptive,
    Transformation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldRequiredKind {
    Id,
    Disruptive,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SemanticError {
    #[error("semantic document count exceeds the configured limit of {limit}")]
    DocumentLimit { limit: usize },
    #[error("semantic directive count exceeds the configured limit of {limit}")]
    DirectiveLimit { limit: usize },
    #[error("semantic rule count exceeds the configured limit of {limit}")]
    RuleLimit { limit: usize },
    #[error("semantic node count exceeds the configured limit of {limit}")]
    NodeLimit { limit: usize },
    #[error("local rule fold count exceeds the configured limit of {limit}")]
    FoldLimit { limit: usize },
    #[error("marker count exceeds the configured limit of {limit} at {position:?}")]
    MarkerLimit { limit: usize, position: SourcePosition },
    #[error("skipAfter reference count exceeds the configured limit of {limit} at {position:?}")]
    SkipAfterLimit { limit: usize, position: SourcePosition },
    #[error("marker name exceeds the configured byte limit of {limit} at {position:?}")]
    MarkerNameByteLimit { limit: usize, position: SourcePosition },
    #[error("marker name bytes exceed the configured generation limit of {limit} at {position:?}")]
    MarkerNameTotalLimit { limit: usize, position: SourcePosition },
    #[error("marker registry work exceeds the configured limit of {limit} at {position:?}")]
    MarkerWorkLimit { limit: usize, position: SourcePosition },
    #[error("chain count exceeds the configured limit of {limit} at {position:?}")]
    ChainLimit { limit: usize, position: SourcePosition },
    #[error("chain member count exceeds the configured limit of {limit} at {position:?}")]
    ChainMemberLimit { limit: usize, position: SourcePosition },
    #[error("chain length exceeds the configured limit of {limit} at {position:?}")]
    ChainLengthLimit { limit: usize, position: SourcePosition },
    #[error("effective transformation count exceeds the configured limit of {limit} at {position:?}")]
    EffectiveTransformationLimit { limit: usize, position: SourcePosition },
    #[error("semantic mapping and node metadata requires more than {limit} owned bytes")]
    PreflightOwnedByteLimit { limit: usize },
    #[error("retained syntax and semantic metadata requires more than {limit} bytes")]
    RetainedByteLimit { limit: usize },
    #[error("semantic metadata requires more than {limit} owned bytes at {position:?}")]
    OwnedByteLimit { limit: usize, position: SourcePosition },
    #[error("unknown target collection at {position:?}")]
    UnknownCollection { position: SourcePosition },
    #[error("unknown operator at {position:?}")]
    UnknownOperator { position: SourcePosition },
    #[error("unknown action at {position:?}")]
    UnknownAction { position: SourcePosition },
    #[error("unknown transformation at {position:?}")]
    UnknownTransformation { position: SourcePosition },
    #[error("action has a {actual:?} value but requires a {expected:?} value at {position:?}")]
    ValueContract {
        expected: ValueContract,
        actual: ValueContract,
        position: SourcePosition,
    },
    #[error("invalid {kind:?} scalar at {position:?}")]
    InvalidScalar { kind: ScalarKind, position: SourcePosition },
    #[error("{kind:?} scalar overflows its supported integer at {position:?}")]
    ScalarOverflow { kind: ScalarKind, position: SourcePosition },
    #[error("{kind:?} scalar contains a runtime macro at {position:?}")]
    MacroScalar { kind: ScalarKind, position: SourcePosition },
    #[error("operand exceeds the configured byte limit of {limit} at {position:?}")]
    OperandByteLimit { limit: usize, position: SourcePosition },
    #[error("scanned operands exceed the configured byte limit of {limit} at {position:?}")]
    ScannedOperandByteLimit { limit: usize, position: SourcePosition },
    #[error("template count exceeds the configured limit of {limit} at {position:?}")]
    TemplateLimit { limit: usize, position: SourcePosition },
    #[error("template segment count exceeds the configured limit of {limit} at {position:?}")]
    TemplateSegmentLimit { limit: usize, position: SourcePosition },
    #[error("macro expansion count exceeds the configured limit of {limit} at {position:?}")]
    ExpansionLimit { limit: usize, position: SourcePosition },
    #[error("template expansion count exceeds the configured per-template limit of {limit} at {position:?}")]
    TemplateExpansionLimit { limit: usize, position: SourcePosition },
    #[error("decoded operand bytes exceed the configured limit of {limit} at {position:?}")]
    DecodedOperandByteLimit { limit: usize, position: SourcePosition },
    #[error("invalid macro envelope ({kind:?}) at {position:?}")]
    MacroSyntax {
        kind: MacroErrorKind,
        position: SourcePosition,
    },
    #[error("operator operand violates its {expected:?} presence contract at {position:?}")]
    OperatorOperandPresence {
        expected: OperatorOperandContract,
        position: SourcePosition,
    },
    #[error("invalid strict integer operand ({kind:?}) at {position:?}")]
    IntegerOperand {
        kind: IntegerOperandErrorKind,
        position: SourcePosition,
    },
    #[error("phrase operand uses unsupported syntax at {position:?}")]
    PhraseSyntax { position: SourcePosition },
    #[error("phrase count exceeds the configured per-operator limit of {limit} at {position:?}")]
    OperatorPhraseLimit { limit: usize, position: SourcePosition },
    #[error("phrase count exceeds the configured generation limit of {limit} at {position:?}")]
    PhraseLimit { limit: usize, position: SourcePosition },
    #[error("phrase bytes exceed the configured generation limit of {limit} at {position:?}")]
    PhraseByteLimit { limit: usize, position: SourcePosition },
    #[error("external asset count exceeds the configured per-operator limit of {limit} at {position:?}")]
    OperatorAssetLimit { limit: usize, position: SourcePosition },
    #[error("external asset count exceeds the configured generation limit of {limit} at {position:?}")]
    AssetLimit { limit: usize, position: SourcePosition },
    #[error("external asset handle exceeds the configured byte limit of {limit} at {position:?}")]
    AssetByteLimit { limit: usize, position: SourcePosition },
    #[error("unsafe external asset handle ({kind:?}) at {position:?}")]
    UnsafeAsset {
        kind: AssetErrorKind,
        position: SourcePosition,
    },
    #[error("invalid IP network operand at {position:?}")]
    IpNetworkSyntax { position: SourcePosition },
    #[error("IP network count exceeds the configured per-operator limit of {limit} at {position:?}")]
    OperatorNetworkLimit { limit: usize, position: SourcePosition },
    #[error("IP network count exceeds the configured generation limit of {limit} at {position:?}")]
    NetworkLimit { limit: usize, position: SourcePosition },
    #[error("invalid byte-set member at {position:?}")]
    ByteSetSyntax { position: SourcePosition },
    #[error("byte-set member count exceeds the configured limit of {limit} at {position:?}")]
    ByteRangeMemberLimit { limit: usize, position: SourcePosition },
    #[error("invalid setvar operand ({kind:?}) at {position:?}")]
    SetVarSyntax {
        kind: SetVarErrorKind,
        position: SourcePosition,
    },
    #[error("invalid initcol operand ({kind:?}) at {position:?}")]
    InitColSyntax {
        kind: InitColErrorKind,
        position: SourcePosition,
    },
    #[error("invalid ctl operand ({kind:?}) at {position:?}")]
    CtlSyntax {
        kind: CtlErrorKind,
        position: SourcePosition,
    },
    #[error("invalid ctl target syntax")]
    CtlTarget(#[source] TargetSyntaxError),
    #[error("duplicate local {kind:?} action at {position:?}")]
    FoldDuplicate {
        kind: FoldSingletonKind,
        position: SourcePosition,
    },
    #[error("SecAction forbids local {kind:?} behavior at {position:?}")]
    SecActionForbidden {
        kind: FoldSingletonKind,
        position: SourcePosition,
    },
    #[error("local rule envelope is missing required {kind:?} at {position:?}")]
    FoldMissing {
        kind: FoldRequiredKind,
        position: SourcePosition,
    },
    #[error("local status and pass actions are incompatible at {position:?}")]
    StatusPass { position: SourcePosition },
    #[error("chain action has no immediately adjacent child at {position:?}")]
    DanglingChain { position: SourcePosition },
    #[error("chain child contains forbidden {kind:?} action at {position:?}")]
    ChildAction { kind: ActionKind, position: SourcePosition },
    #[error("marker name contains a forbidden control byte at {position:?}")]
    MarkerControl { position: SourcePosition },
    #[error("duplicate marker at {position:?}; first definition is at {first:?}")]
    DuplicateMarker {
        position: SourcePosition,
        first: DirectiveLocation,
    },
    #[error("skipAfter marker is unresolved at {position:?}")]
    UnresolvedSkipAfter { position: SourcePosition },
    #[error("skipAfter marker is not strictly forward at {position:?}; target is at {target:?}")]
    NonForwardSkipAfter {
        position: SourcePosition,
        target: DirectiveLocation,
    },
    #[error("structured action count exceeds the configured limit of {limit} at {position:?}")]
    StructuredActionLimit { limit: usize, position: SourcePosition },
    #[error("ctl target count exceeds the configured limit of {limit} at {position:?}")]
    CtlTargetLimit { limit: usize, position: SourcePosition },
    #[error("setvar key exceeds the configured byte limit of {limit} at {position:?}")]
    SetVarKeyByteLimit { limit: usize, position: SourcePosition },
    #[error("ctl value exceeds the configured byte limit of {limit} at {position:?}")]
    CtlValueByteLimit { limit: usize, position: SourcePosition },
    #[error("template slice crosses a macro expansion boundary at {position:?}")]
    TemplateSliceInvariant { position: SourcePosition },
    #[error("cannot map operand subrange on logical line {logical_line} at byte offset {logical_offset}")]
    OperandPositionMapping { logical_line: usize, logical_offset: usize },
    #[error("cannot allocate bounded semantic {operation}; reduce the generation or configured limits")]
    Allocation { operation: &'static str },
}

struct Preflight {
    documents: usize,
    inherited: usize,
    directives: usize,
    targets: usize,
    operators: usize,
    actions: usize,
    nodes: usize,
    folds: usize,
    fixed_charge: usize,
}

pub fn analyze_semantic_draft<'source>(
    documents: Vec<SyntaxDocument<'source>>,
    limits: SemanticLimits,
) -> Result<SemanticDraft<'source>, SemanticError> {
    analyze_semantic_draft_inner(documents, limits, true, true)
}

pub(super) fn analyze_semantic_draft_inner<'source>(
    documents: Vec<SyntaxDocument<'source>>,
    limits: SemanticLimits,
    fold: bool,
    graph: bool,
) -> Result<SemanticDraft<'source>, SemanticError> {
    let preflight = preflight(&documents, limits)?;
    let mut normalized_documents = reserve_exact(preflight.documents, "syntax documents")?;
    for document in documents {
        normalized_documents.push(document);
    }
    let mut draft = SemanticDraft {
        documents: normalized_documents,
        directives: reserve_exact(preflight.directives, "directive mappings")?,
        targets: reserve_exact(preflight.targets, "target records")?,
        operators: reserve_exact(preflight.operators, "operator records")?,
        actions: reserve_exact(preflight.actions, "action records")?,
        action_operands: reserve_exact(preflight.actions, "action operands")?,
        setvars: Vec::new(),
        initcols: Vec::new(),
        ctls: Vec::new(),
        ctl_targets: Vec::new(),
        templates: Vec::new(),
        template_segments: Vec::new(),
        expansions: Vec::new(),
        decoded_operands: Vec::new(),
        operator_operands: reserve_exact(preflight.operators, "operator operands")?,
        static_operands: Vec::new(),
        list_items: Vec::new(),
        networks: Vec::new(),
        phrase_count: 0,
        asset_count: 0,
        structured_action_count: 0,
        phrase_bytes: 0,
        scanned_operand_bytes: 0,
        operand_limits: limits.operands,
        requirements: reserve_exact(
            preflight
                .nodes
                .checked_mul(2)
                .and_then(|count| count.checked_add(preflight.folds))
                .ok_or(SemanticError::NodeLimit {
                    limit: limits.max_nodes,
                })?,
            "compile requirements",
        )?,
        unavailable: reserve_exact(preflight.nodes, "unavailable capabilities")?,
        folds: Vec::new(),
        effective_transformations: Vec::new(),
        final_rules: Vec::new(),
        chains: Vec::new(),
        chain_members: Vec::new(),
        markers: Vec::new(),
        resolved_skip_afters: Vec::new(),
        semantic_charged_bytes: preflight.fixed_charge,
        inherited_charged_bytes: preflight.inherited,
    };
    scan_semantics(&mut draft, limits)?;
    if fold {
        fold_local_rules(&mut draft, limits)?;
    }
    if graph {
        finalize_graph(&mut draft, limits)?;
    }
    Ok(draft)
}

pub fn analyze_one_semantic_draft<'source>(
    document: SyntaxDocument<'source>,
    limits: SemanticLimits,
) -> Result<SemanticDraft<'source>, SemanticError> {
    let mut documents = Vec::new();
    documents.try_reserve_exact(1).map_err(|_| SemanticError::Allocation {
        operation: "syntax documents",
    })?;
    documents.push(document);
    analyze_semantic_draft(documents, limits)
}

fn reserve_exact<T>(count: usize, operation: &'static str) -> Result<Vec<T>, SemanticError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| SemanticError::Allocation { operation })?;
    Ok(values)
}

fn preflight(documents: &[SyntaxDocument<'_>], limits: SemanticLimits) -> Result<Preflight, SemanticError> {
    if documents.len() > limits.max_documents {
        return Err(SemanticError::DocumentLimit {
            limit: limits.max_documents,
        });
    }
    let mut inherited = 0_usize;
    let mut directives = 0_usize;
    let mut rules = 0_usize;
    let mut folds = 0_usize;
    let mut targets = 0_usize;
    let mut operators = 0_usize;
    let mut actions = 0_usize;
    for document in documents {
        inherited = checked_add(
            inherited,
            document.retained_charged_bytes(),
            SemanticError::RetainedByteLimit {
                limit: limits.max_retained_bytes,
            },
        )?;
        directives = checked_add(
            directives,
            document.len(),
            SemanticError::DirectiveLimit {
                limit: limits.max_directives,
            },
        )?;
        rules = checked_add(
            rules,
            document
                .iter()
                .filter(|view| matches!(view.directive().directive(), Directive::SecRule { .. }))
                .count(),
            SemanticError::RuleLimit {
                limit: limits.max_rules,
            },
        )?;
        folds = checked_add(
            folds,
            document
                .iter()
                .filter(|view| {
                    matches!(
                        view.directive().directive(),
                        Directive::SecRule { .. } | Directive::SecAction { .. }
                    )
                })
                .count(),
            SemanticError::DirectiveLimit {
                limit: limits.max_directives,
            },
        )?;
        targets = checked_add(
            targets,
            document.target_atom_count(),
            SemanticError::NodeLimit {
                limit: limits.max_nodes,
            },
        )?;
        operators = checked_add(
            operators,
            document.operator_count(),
            SemanticError::NodeLimit {
                limit: limits.max_nodes,
            },
        )?;
        actions = checked_add(
            actions,
            document.action_entry_count(),
            SemanticError::NodeLimit {
                limit: limits.max_nodes,
            },
        )?;
    }
    if directives > limits.max_directives {
        return Err(SemanticError::DirectiveLimit {
            limit: limits.max_directives,
        });
    }
    if rules > limits.max_rules {
        return Err(SemanticError::RuleLimit {
            limit: limits.max_rules,
        });
    }
    let nodes = targets
        .checked_add(operators)
        .and_then(|value| value.checked_add(actions))
        .ok_or(SemanticError::NodeLimit {
            limit: limits.max_nodes,
        })?;
    if nodes > limits.max_nodes {
        return Err(SemanticError::NodeLimit {
            limit: limits.max_nodes,
        });
    }
    let fixed_charge = charged_product::<SyntaxDocument<'_>>(documents.len(), limits.max_owned_bytes)?
        .checked_add(charged_product::<DirectiveSemanticRecord>(
            directives,
            limits.max_owned_bytes,
        )?)
        .and_then(|value| {
            value.checked_add(charged_product::<SemanticTarget<'_>>(targets, limits.max_owned_bytes).ok()?)
        })
        .and_then(|value| {
            value.checked_add(charged_product::<SemanticOperatorRecord<'_>>(operators, limits.max_owned_bytes).ok()?)
        })
        .and_then(|value| {
            value.checked_add(charged_product::<SemanticActionRecord<'_>>(actions, limits.max_owned_bytes).ok()?)
        })
        .and_then(|value| {
            value.checked_add(charged_product::<OperatorOperandRecord>(operators, limits.max_owned_bytes).ok()?)
        })
        .and_then(|value| {
            value.checked_add(charged_product::<ActionOperandRecord<'_>>(actions, limits.max_owned_bytes).ok()?)
        })
        .ok_or(SemanticError::PreflightOwnedByteLimit {
            limit: limits.max_owned_bytes,
        })?;
    if fixed_charge > limits.max_owned_bytes {
        return Err(SemanticError::PreflightOwnedByteLimit {
            limit: limits.max_owned_bytes,
        });
    }
    let retained = inherited
        .checked_add(fixed_charge)
        .ok_or(SemanticError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        })?;
    if retained > limits.max_retained_bytes {
        return Err(SemanticError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        });
    }
    Ok(Preflight {
        documents: documents.len(),
        inherited,
        directives,
        targets,
        operators,
        actions,
        nodes,
        folds,
        fixed_charge,
    })
}

fn charged_product<T>(count: usize, limit: usize) -> Result<usize, SemanticError> {
    size_of::<T>()
        .checked_mul(count)
        .ok_or(SemanticError::PreflightOwnedByteLimit { limit })
}

fn checked_add(value: usize, amount: usize, error: SemanticError) -> Result<usize, SemanticError> {
    value.checked_add(amount).ok_or(error)
}
