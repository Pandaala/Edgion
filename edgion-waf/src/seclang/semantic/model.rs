//! Semantic data model: locations, text and targets, operator/action/template records, views and iterators.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticNodeKind {
    Directive,
    Target,
    Operator,
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticLocation {
    pub(super) document_index: usize,
    pub(super) directive_index: usize,
    pub(super) node_kind: SemanticNodeKind,
    pub(super) node_index: usize,
}

impl SemanticLocation {
    pub const fn document_index(self) -> usize {
        self.document_index
    }
    pub const fn directive_index(self) -> usize {
        self.directive_index
    }
    pub const fn node_kind(self) -> SemanticNodeKind {
        self.node_kind
    }
    pub const fn node_index(self) -> usize {
        self.node_index
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticText<'source> {
    pub(super) text: &'source str,
    pub(super) logical_start: usize,
    pub(super) logical_end: usize,
    pub(super) span: SourceSpan,
}

impl<'source> SemanticText<'source> {
    pub const fn text(self) -> &'source str {
        self.text
    }
    pub fn logical_range(self) -> Range<usize> {
        self.logical_start..self.logical_end
    }
    pub const fn span(self) -> SourceSpan {
        self.span
    }
}

impl<'source> From<SyntaxText<'source>> for SemanticText<'source> {
    fn from(value: SyntaxText<'source>) -> Self {
        let range = value.logical_range();
        Self {
            text: value.text(),
            logical_start: range.start,
            logical_end: range.end,
            span: value.span(),
        }
    }
}

impl<'source> From<ActionText<'source>> for SemanticText<'source> {
    fn from(value: ActionText<'source>) -> Self {
        let range = value.logical_range();
        Self {
            text: value.text(),
            logical_start: range.start,
            logical_end: range.end,
            span: value.span(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticTarget<'source> {
    pub(super) location: SemanticLocation,
    pub(super) collection: CollectionKind,
    pub(super) collection_spelling: SemanticText<'source>,
    pub(super) selector_kind: SelectorKind,
    pub(super) modifier: TargetModifier,
    pub(super) selector: Option<SemanticText<'source>>,
    pub(super) regex_body: Option<SemanticText<'source>>,
    pub(super) span: SourceSpan,
}

impl<'source> SemanticTarget<'source> {
    pub const fn location(self) -> SemanticLocation {
        self.location
    }
    pub const fn collection(self) -> CollectionKind {
        self.collection
    }
    pub const fn collection_spelling(self) -> SemanticText<'source> {
        self.collection_spelling
    }
    pub const fn selector_kind(self) -> SelectorKind {
        self.selector_kind
    }
    pub const fn modifier(self) -> TargetModifier {
        self.modifier
    }
    pub const fn selector(self) -> Option<SemanticText<'source>> {
        self.selector
    }
    pub const fn regex_body(self) -> Option<SemanticText<'source>> {
        self.regex_body
    }
    pub const fn span(self) -> SourceSpan {
        self.span
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SemanticOperatorRecord<'source> {
    pub(super) location: SemanticLocation,
    pub(super) kind: OperatorKind,
    pub(super) spelling: Option<SemanticText<'source>>,
    pub(super) negated: bool,
    pub(super) operand: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SemanticActionRecord<'source> {
    pub(super) location: SemanticLocation,
    pub(super) kind: ActionEnvelopeKind,
    pub(super) name: SemanticText<'source>,
    pub(super) operand: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroSource {
    Tx,
    Args,
    RequestHeaders,
    MatchedVar,
    MatchedVarName,
    RemoteAddr,
    RequestLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OperandStorage<'source> {
    Borrowed(SemanticText<'source>),
    Decoded { start: usize, len: usize, span: SourceSpan },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TemplateRecord<'source> {
    pub(super) storage: OperandStorage<'source>,
    pub(super) segment_start: usize,
    pub(super) segment_count: usize,
    pub(super) expansion_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TemplateSegmentRecordKind {
    Literal,
    Expansion(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TemplateSegmentRecord {
    pub(super) kind: TemplateSegmentRecordKind,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExpansionRecord<'source> {
    pub(super) source: MacroSource,
    pub(super) body: SemanticText<'source>,
    pub(super) source_spelling: SemanticText<'source>,
    pub(super) key: Option<SemanticText<'source>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StaticOperandRecord<'source> {
    pub(super) storage: OperandStorage<'source>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TemplateSliceRecord {
    pub(super) template: usize,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) segment_start: usize,
    pub(super) segment_count: usize,
    pub(super) expansion_count: usize,
    pub(super) span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SetVarRecord {
    pub(super) collection: CollectionKind,
    pub(super) operation: SetVarOperation,
    pub(super) key: TemplateSliceRecord,
    pub(super) value: Option<TemplateSliceRecord>,
    pub(super) constant: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InitColRecord {
    pub(super) collection: InitColCollection,
    pub(super) key: TemplateSliceRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CtlTargetRecord<'source> {
    pub(super) collection: CollectionKind,
    pub(super) collection_spelling: SemanticText<'source>,
    pub(super) selector_kind: SelectorKind,
    pub(super) selector: Option<SemanticText<'source>>,
    pub(super) regex_body: Option<SemanticText<'source>>,
    pub(super) span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CtlRecord<'source> {
    AuditEngine(AuditEngineMode),
    ForceRequestBodyVariable(OnOff),
    RequestBodyProcessor(RequestBodyProcessor),
    RuleRemoveByTag(SemanticText<'source>),
    RuleRemoveTargetByTag { tag: SemanticText<'source>, target: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionOperandRecord<'source> {
    NoArgument,
    Local(LocalActionValue),
    Transformation(TransformationKind),
    Template(usize),
    Literal(SemanticText<'source>),
    SetVar(usize),
    InitCol(usize),
    Ctl(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ListItemRecord {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NetworkRecord {
    pub(super) address: IpAddr,
    pub(super) prefix: u8,
    pub(super) span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OperatorOperandRecord {
    NoArgument,
    Regex(usize),
    StringTemplate(usize),
    IntegerTemplate {
        template: usize,
        constant: Option<i64>,
    },
    PhraseList {
        storage: usize,
        start: usize,
        count: usize,
    },
    ExternalAssets {
        storage: usize,
        start: usize,
        count: usize,
        document: usize,
    },
    IpNetworks {
        storage: usize,
        start: usize,
        count: usize,
    },
    ByteSet {
        storage: usize,
        words: [u64; 4],
        member_count: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateSegmentKind<'view, 'source> {
    Literal,
    Expansion(MacroExpansionView<'view, 'source>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacroExpansionView<'view, 'source> {
    record: &'view ExpansionRecord<'source>,
}

impl<'view, 'source> MacroExpansionView<'view, 'source> {
    pub const fn source(self) -> MacroSource {
        self.record.source
    }
    pub const fn body(self) -> SemanticText<'source> {
        self.record.body
    }
    pub const fn source_spelling(self) -> SemanticText<'source> {
        self.record.source_spelling
    }
    pub const fn key(self) -> Option<SemanticText<'source>> {
        self.record.key
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateSegmentView<'view, 'source> {
    record: &'view TemplateSegmentRecord,
    text: &'view str,
    expansions: &'view [ExpansionRecord<'source>],
}

impl<'view, 'source> TemplateSegmentView<'view, 'source> {
    pub const fn text(self) -> &'view str {
        self.text
    }
    pub const fn span(self) -> SourceSpan {
        self.record.span
    }
    pub fn kind(self) -> TemplateSegmentKind<'view, 'source> {
        match self.record.kind {
            TemplateSegmentRecordKind::Literal => TemplateSegmentKind::Literal,
            TemplateSegmentRecordKind::Expansion(index) => TemplateSegmentKind::Expansion(MacroExpansionView {
                record: &self.expansions[index],
            }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TemplateArenaView<'view, 'source> {
    pub(super) templates: &'view [TemplateRecord<'source>],
    pub(super) segments: &'view [TemplateSegmentRecord],
    pub(super) expansions: &'view [ExpansionRecord<'source>],
    pub(super) decoded: &'view [u8],
    pub(super) operands: &'view [OperatorOperandRecord],
    pub(super) static_operands: &'view [StaticOperandRecord<'source>],
    pub(super) list_items: &'view [ListItemRecord],
    pub(super) networks: &'view [NetworkRecord],
    pub(super) action_operands: &'view [ActionOperandRecord<'source>],
    pub(super) setvars: &'view [SetVarRecord],
    pub(super) initcols: &'view [InitColRecord],
    pub(super) ctls: &'view [CtlRecord<'source>],
    pub(super) ctl_targets: &'view [CtlTargetRecord<'source>],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateSliceSegmentView<'view, 'source> {
    kind: TemplateSegmentKind<'view, 'source>,
    text: &'view str,
    span: SourceSpan,
}

impl<'view, 'source> TemplateSliceSegmentView<'view, 'source> {
    pub const fn kind(self) -> TemplateSegmentKind<'view, 'source> {
        self.kind
    }
    pub const fn text(self) -> &'view str {
        self.text
    }
    pub const fn span(self) -> SourceSpan {
        self.span
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TemplateSliceView<'view, 'source> {
    record: TemplateSliceRecord,
    arena: TemplateArenaView<'view, 'source>,
}

impl<'view, 'source> TemplateSliceView<'view, 'source> {
    pub(crate) const fn template_identity(self) -> usize {
        self.record.template
    }

    pub fn text(self) -> &'view str {
        &self.arena.template(self.record.template).text()[self.record.start..self.record.end]
    }
    pub const fn span(self) -> SourceSpan {
        self.record.span
    }
    pub const fn expansion_count(self) -> usize {
        self.record.expansion_count
    }
    pub fn segments(
        self,
    ) -> impl ExactSizeIterator<Item = TemplateSliceSegmentView<'view, 'source>> + FusedIterator + 'view {
        let text = self.arena.template(self.record.template).text();
        let expansions = self.arena.expansions;
        let slice_start = self.record.start;
        let slice_end = self.record.end;
        let slice_span = self.record.span;
        self.arena.segments[self.record.segment_start..self.record.segment_start + self.record.segment_count]
            .iter()
            .map(move |record| {
                let start = record.start.max(slice_start);
                let end = record.end.min(slice_end);
                let kind = match record.kind {
                    TemplateSegmentRecordKind::Literal => TemplateSegmentKind::Literal,
                    TemplateSegmentRecordKind::Expansion(index) => TemplateSegmentKind::Expansion(MacroExpansionView {
                        record: &expansions[index],
                    }),
                };
                TemplateSliceSegmentView {
                    kind,
                    text: &text[start..end],
                    span: SourceSpan {
                        start: if start == slice_start {
                            slice_span.start
                        } else {
                            record.span.start
                        },
                        end: if end == slice_end {
                            slice_span.end
                        } else {
                            record.span.end
                        },
                    },
                }
            })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SetVarView<'view, 'source> {
    record: &'view SetVarRecord,
    arena: TemplateArenaView<'view, 'source>,
}

impl<'view, 'source> SetVarView<'view, 'source> {
    pub const fn collection(self) -> CollectionKind {
        self.record.collection
    }
    pub const fn operation(self) -> SetVarOperation {
        self.record.operation
    }
    pub fn key(self) -> TemplateSliceView<'view, 'source> {
        TemplateSliceView {
            record: self.record.key,
            arena: self.arena,
        }
    }
    pub fn value(self) -> Option<TemplateSliceView<'view, 'source>> {
        self.record.value.map(|record| TemplateSliceView {
            record,
            arena: self.arena,
        })
    }
    pub const fn constant(self) -> Option<i64> {
        self.record.constant
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InitColView<'view, 'source> {
    record: &'view InitColRecord,
    arena: TemplateArenaView<'view, 'source>,
}

impl<'view, 'source> InitColView<'view, 'source> {
    pub const fn collection(self) -> InitColCollection {
        self.record.collection
    }
    pub fn key(self) -> TemplateSliceView<'view, 'source> {
        TemplateSliceView {
            record: self.record.key,
            arena: self.arena,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CtlTargetView<'view, 'source> {
    record: &'view CtlTargetRecord<'source>,
}

impl<'view, 'source> CtlTargetView<'view, 'source> {
    pub const fn collection(self) -> CollectionKind {
        self.record.collection
    }
    pub const fn collection_spelling(self) -> SemanticText<'source> {
        self.record.collection_spelling
    }
    pub const fn selector_kind(self) -> SelectorKind {
        self.record.selector_kind
    }
    pub const fn selector(self) -> Option<SemanticText<'source>> {
        self.record.selector
    }
    pub const fn regex_body(self) -> Option<SemanticText<'source>> {
        self.record.regex_body
    }
    pub const fn span(self) -> SourceSpan {
        self.record.span
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CtlView<'view, 'source> {
    AuditEngine(AuditEngineMode),
    ForceRequestBodyVariable(OnOff),
    RequestBodyProcessor(RequestBodyProcessor),
    RuleRemoveByTag(SemanticText<'source>),
    RuleRemoveTargetByTag {
        tag: SemanticText<'source>,
        target: CtlTargetView<'view, 'source>,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum ActionOperandView<'view, 'source> {
    NoArgument,
    Local(LocalActionValue),
    Transformation(TransformationKind),
    Template(OperandTemplateView<'view, 'source>),
    Literal(SemanticText<'source>),
    SetVar(SetVarView<'view, 'source>),
    InitCol(InitColView<'view, 'source>),
    Ctl(CtlView<'view, 'source>),
}

impl<'view, 'source> TemplateArenaView<'view, 'source> {
    fn template(self, id: usize) -> OperandTemplateView<'view, 'source> {
        OperandTemplateView {
            index: id,
            record: &self.templates[id],
            arena: self,
        }
    }

    fn text(self, storage: OperandStorage<'source>) -> OperandTextView<'view, 'source> {
        OperandTextView { storage, arena: self }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OperandTemplateView<'view, 'source> {
    index: usize,
    record: &'view TemplateRecord<'source>,
    arena: TemplateArenaView<'view, 'source>,
}

impl<'view, 'source> OperandTemplateView<'view, 'source> {
    pub(crate) const fn identity(self) -> usize {
        self.index
    }
    pub fn text(self) -> &'view str {
        match self.record.storage {
            OperandStorage::Borrowed(text) => text.text(),
            OperandStorage::Decoded { start, len, .. } => std::str::from_utf8(&self.arena.decoded[start..start + len])
                .expect("decoded operator templates preserve UTF-8"),
        }
    }
    pub const fn expansion_count(self) -> usize {
        self.record.expansion_count
    }
    pub fn segments(self) -> impl ExactSizeIterator<Item = TemplateSegmentView<'view, 'source>> + FusedIterator {
        let text = self.text();
        self.arena.segments[self.record.segment_start..self.record.segment_start + self.record.segment_count]
            .iter()
            .map(move |record| TemplateSegmentView {
                record,
                text: &text[record.start..record.end],
                expansions: self.arena.expansions,
            })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticOperatorView<'view, 'source> {
    pub(super) record: &'view SemanticOperatorRecord<'source>,
    pub(super) arena: TemplateArenaView<'view, 'source>,
}

#[derive(Debug, Clone, Copy)]
pub struct OperandTextView<'view, 'source> {
    storage: OperandStorage<'source>,
    arena: TemplateArenaView<'view, 'source>,
}

impl<'view, 'source> OperandTextView<'view, 'source> {
    pub fn text(self) -> &'view str {
        match self.storage {
            OperandStorage::Borrowed(text) => text.text(),
            OperandStorage::Decoded { start, len, .. } => std::str::from_utf8(&self.arena.decoded[start..start + len])
                .expect("decoded operand storage preserves UTF-8"),
        }
    }
    pub const fn span(self) -> SourceSpan {
        match self.storage {
            OperandStorage::Borrowed(text) => text.span(),
            OperandStorage::Decoded { span, .. } => span,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PhraseView<'view> {
    record: &'view ListItemRecord,
    text: &'view str,
}

impl<'view> PhraseView<'view> {
    pub const fn text(self) -> &'view str {
        self.text
    }
    pub const fn span(self) -> SourceSpan {
        self.record.span
    }
}

#[derive(Debug, Clone)]
pub struct PhraseIter<'view> {
    records: std::slice::Iter<'view, ListItemRecord>,
    text: &'view str,
}

impl<'view> Iterator for PhraseIter<'view> {
    type Item = PhraseView<'view>;
    fn next(&mut self) -> Option<Self::Item> {
        self.records.next().map(|record| PhraseView {
            record,
            text: &self.text[record.start..record.end],
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}
impl ExactSizeIterator for PhraseIter<'_> {}
impl FusedIterator for PhraseIter<'_> {}

#[derive(Debug, Clone, Copy)]
pub struct ExternalAssetView<'view> {
    document_index: usize,
    item: PhraseView<'view>,
}

impl<'view> ExternalAssetView<'view> {
    pub const fn document_index(self) -> usize {
        self.document_index
    }
    pub const fn handle(self) -> &'view str {
        self.item.text()
    }
    pub const fn span(self) -> SourceSpan {
        self.item.span()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PhraseListView<'view, 'source> {
    text: OperandTextView<'view, 'source>,
    records: &'view [ListItemRecord],
}

impl<'view, 'source> PhraseListView<'view, 'source> {
    pub const fn text(self) -> OperandTextView<'view, 'source> {
        self.text
    }
    pub fn phrases(self) -> PhraseIter<'view> {
        PhraseIter {
            records: self.records.iter(),
            text: self.text.text(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExternalAssetsView<'view, 'source> {
    text: OperandTextView<'view, 'source>,
    records: &'view [ListItemRecord],
    document_index: usize,
}

impl<'view, 'source> ExternalAssetsView<'view, 'source> {
    pub const fn text(self) -> OperandTextView<'view, 'source> {
        self.text
    }
    pub fn assets(self) -> impl ExactSizeIterator<Item = ExternalAssetView<'view>> + FusedIterator + 'view {
        let document_index = self.document_index;
        PhraseIter {
            records: self.records.iter(),
            text: self.text.text(),
        }
        .map(move |item| ExternalAssetView { document_index, item })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IpNetworkView<'view> {
    record: &'view NetworkRecord,
}

impl IpNetworkView<'_> {
    pub const fn address(self) -> IpAddr {
        self.record.address
    }
    pub const fn prefix(self) -> u8 {
        self.record.prefix
    }
    pub const fn span(self) -> SourceSpan {
        self.record.span
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IpNetworksView<'view, 'source> {
    text: OperandTextView<'view, 'source>,
    records: &'view [NetworkRecord],
}

impl<'view, 'source> IpNetworksView<'view, 'source> {
    pub const fn text(self) -> OperandTextView<'view, 'source> {
        self.text
    }
    pub fn networks(self) -> impl ExactSizeIterator<Item = IpNetworkView<'view>> + FusedIterator + 'view {
        self.records.iter().map(|record| IpNetworkView { record })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ByteSetView<'view, 'source> {
    text: OperandTextView<'view, 'source>,
    words: [u64; 4],
    member_count: usize,
}

impl<'view, 'source> ByteSetView<'view, 'source> {
    pub(crate) const fn words(self) -> [u64; 4] {
        self.words
    }
    pub const fn text(self) -> OperandTextView<'view, 'source> {
        self.text
    }
    pub const fn contains(self, byte: u8) -> bool {
        self.words[byte as usize / 64] & (1u64 << (byte as usize % 64)) != 0
    }
    pub const fn member_count(self) -> usize {
        self.member_count
    }
}

#[derive(Debug, Clone)]
pub enum OperatorOperandView<'view, 'source> {
    NoArgument,
    Regex(OperandTextView<'view, 'source>),
    StringTemplate(OperandTemplateView<'view, 'source>),
    IntegerTemplate {
        template: OperandTemplateView<'view, 'source>,
        constant: Option<i64>,
    },
    PhraseList(PhraseListView<'view, 'source>),
    ExternalAssets(ExternalAssetsView<'view, 'source>),
    IpNetworks(IpNetworksView<'view, 'source>),
    ByteSet(ByteSetView<'view, 'source>),
}

impl<'view, 'source> SemanticOperatorView<'view, 'source> {
    pub const fn location(self) -> SemanticLocation {
        self.record.location
    }
    pub const fn kind(self) -> OperatorKind {
        self.record.kind
    }
    pub const fn spelling(self) -> Option<SemanticText<'source>> {
        self.record.spelling
    }
    pub const fn negated(self) -> bool {
        self.record.negated
    }
    pub fn template(self) -> Option<OperandTemplateView<'view, 'source>> {
        match self.arena.operands[self.record.operand] {
            OperatorOperandRecord::StringTemplate(id) | OperatorOperandRecord::IntegerTemplate { template: id, .. } => {
                Some(self.arena.template(id))
            }
            _ => None,
        }
    }
    pub fn operand(self) -> OperatorOperandView<'view, 'source> {
        match self.arena.operands[self.record.operand] {
            OperatorOperandRecord::NoArgument => OperatorOperandView::NoArgument,
            OperatorOperandRecord::Regex(id) => {
                OperatorOperandView::Regex(self.arena.text(self.arena.static_operands[id].storage))
            }
            OperatorOperandRecord::StringTemplate(id) => OperatorOperandView::StringTemplate(self.arena.template(id)),
            OperatorOperandRecord::IntegerTemplate { template, constant } => OperatorOperandView::IntegerTemplate {
                template: self.arena.template(template),
                constant,
            },
            OperatorOperandRecord::PhraseList { storage, start, count } => {
                OperatorOperandView::PhraseList(PhraseListView {
                    text: self.arena.text(self.arena.static_operands[storage].storage),
                    records: &self.arena.list_items[start..start + count],
                })
            }
            OperatorOperandRecord::ExternalAssets {
                storage,
                start,
                count,
                document,
            } => OperatorOperandView::ExternalAssets(ExternalAssetsView {
                text: self.arena.text(self.arena.static_operands[storage].storage),
                records: &self.arena.list_items[start..start + count],
                document_index: document,
            }),
            OperatorOperandRecord::IpNetworks { storage, start, count } => {
                OperatorOperandView::IpNetworks(IpNetworksView {
                    text: self.arena.text(self.arena.static_operands[storage].storage),
                    records: &self.arena.networks[start..start + count],
                })
            }
            OperatorOperandRecord::ByteSet {
                storage,
                words,
                member_count,
            } => OperatorOperandView::ByteSet(ByteSetView {
                text: self.arena.text(self.arena.static_operands[storage].storage),
                words,
                member_count,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticActionView<'view, 'source> {
    pub(super) record: &'view SemanticActionRecord<'source>,
    pub(super) arena: TemplateArenaView<'view, 'source>,
}

impl<'view, 'source> SemanticActionView<'view, 'source> {
    pub const fn location(self) -> SemanticLocation {
        self.record.location
    }
    pub const fn kind(self) -> ActionEnvelopeKind {
        self.record.kind
    }
    pub const fn name(self) -> SemanticText<'source> {
        self.record.name
    }
    pub fn operand(self) -> ActionOperandView<'view, 'source> {
        match self.arena.action_operands[self.record.operand] {
            ActionOperandRecord::NoArgument => ActionOperandView::NoArgument,
            ActionOperandRecord::Local(value) => ActionOperandView::Local(value),
            ActionOperandRecord::Transformation(value) => ActionOperandView::Transformation(value),
            ActionOperandRecord::Template(template) => ActionOperandView::Template(self.arena.template(template)),
            ActionOperandRecord::Literal(value) => ActionOperandView::Literal(value),
            ActionOperandRecord::SetVar(index) => ActionOperandView::SetVar(SetVarView {
                record: &self.arena.setvars[index],
                arena: self.arena,
            }),
            ActionOperandRecord::InitCol(index) => ActionOperandView::InitCol(InitColView {
                record: &self.arena.initcols[index],
                arena: self.arena,
            }),
            ActionOperandRecord::Ctl(index) => {
                let ctl = match self.arena.ctls[index] {
                    CtlRecord::AuditEngine(value) => CtlView::AuditEngine(value),
                    CtlRecord::ForceRequestBodyVariable(value) => CtlView::ForceRequestBodyVariable(value),
                    CtlRecord::RequestBodyProcessor(value) => CtlView::RequestBodyProcessor(value),
                    CtlRecord::RuleRemoveByTag(value) => CtlView::RuleRemoveByTag(value),
                    CtlRecord::RuleRemoveTargetByTag { tag, target } => CtlView::RuleRemoveTargetByTag {
                        tag,
                        target: CtlTargetView {
                            record: &self.arena.ctl_targets[target],
                        },
                    },
                };
                ActionOperandView::Ctl(ctl)
            }
        }
    }
    pub fn local_value(self) -> LocalActionValue {
        match self.operand() {
            ActionOperandView::NoArgument => LocalActionValue::None,
            ActionOperandView::Local(value) => value,
            _ => LocalActionValue::Unresolved,
        }
    }
    pub fn transformation(self) -> Option<TransformationKind> {
        match self.operand() {
            ActionOperandView::Transformation(value) => Some(value),
            _ => None,
        }
    }
    pub fn template(self) -> Option<OperandTemplateView<'view, 'source>> {
        match self.operand() {
            ActionOperandView::Template(value) => Some(value),
            ActionOperandView::SetVar(value) => Some(self.arena.template(value.record.key.template)),
            ActionOperandView::InitCol(value) => Some(self.arena.template(value.record.key.template)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SemanticActionIter<'view, 'source> {
    pub(super) records: std::slice::Iter<'view, SemanticActionRecord<'source>>,
    pub(super) arena: TemplateArenaView<'view, 'source>,
}

impl<'view, 'source> Iterator for SemanticActionIter<'view, 'source> {
    type Item = SemanticActionView<'view, 'source>;
    fn next(&mut self) -> Option<Self::Item> {
        self.records.next().map(|record| SemanticActionView {
            record,
            arena: self.arena,
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}
impl ExactSizeIterator for SemanticActionIter<'_, '_> {}
impl FusedIterator for SemanticActionIter<'_, '_> {}
