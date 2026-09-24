use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::iter::FusedIterator;
use std::mem::size_of;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Range;

use thiserror::Error;

use super::{
    ActionEntryKind, ActionText, Directive, DirectiveTargetView, IdSelector, IdSelectorKind, OperatorNameView,
    SelectorKind, SourcePosition, SourceSpan, SyntaxDocument, SyntaxText, TargetModifier,
};

pub const DEFAULT_MAX_SEMANTIC_DOCUMENTS: usize = 1_024;
pub const DEFAULT_MAX_SEMANTIC_DIRECTIVES: usize = 200_000;
pub const DEFAULT_MAX_SEMANTIC_RULES: usize = 100_000;
pub const DEFAULT_MAX_SEMANTIC_NODES: usize = 500_000;
pub const DEFAULT_MAX_SEMANTIC_OWNED_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_SEMANTIC_RETAINED_BYTES: usize = 256 * 1024 * 1024;
pub const DEFAULT_MAX_FOLDED_RULES: usize = 200_000;
pub const DEFAULT_MAX_EFFECTIVE_TRANSFORMATIONS: usize = 200_000;
pub const MARKER_WORK_ENTRY_CHARGE: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphLimits {
    pub max_chains: usize,
    pub max_chain_members: usize,
    pub max_members_per_chain: usize,
    pub max_markers: usize,
    pub max_marker_name_bytes: usize,
    pub max_marker_name_bytes_total: usize,
    pub max_skip_after_refs: usize,
    pub max_registry_work_bytes: usize,
}

impl Default for GraphLimits {
    fn default() -> Self {
        Self {
            max_chains: 100_000,
            max_chain_members: 100_000,
            max_members_per_chain: 1_024,
            max_markers: 100_000,
            max_marker_name_bytes: 4 * 1024,
            max_marker_name_bytes_total: 16 * 1024 * 1024,
            max_skip_after_refs: 200_000,
            max_registry_work_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldLimits {
    pub max_folded_rules: usize,
    pub max_effective_transformations: usize,
}

impl Default for FoldLimits {
    fn default() -> Self {
        Self {
            max_folded_rules: DEFAULT_MAX_FOLDED_RULES,
            max_effective_transformations: DEFAULT_MAX_EFFECTIVE_TRANSFORMATIONS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperandLimits {
    pub max_operand_bytes: usize,
    pub max_scanned_bytes: usize,
    pub max_templates: usize,
    pub max_segments: usize,
    pub max_expansions: usize,
    pub max_expansions_per_template: usize,
    pub max_decoded_bytes: usize,
    pub max_runtime_expansion_bytes: usize,
    pub max_rule_expansion_bytes: usize,
    pub max_phrases_per_operator: usize,
    pub max_phrases: usize,
    pub max_phrase_bytes: usize,
    pub max_assets_per_operator: usize,
    pub max_assets: usize,
    pub max_asset_bytes: usize,
    pub max_networks_per_operator: usize,
    pub max_networks: usize,
    pub max_byte_range_members: usize,
    pub max_structured_actions: usize,
    pub max_ctl_targets: usize,
    pub max_setvar_key_bytes: usize,
    pub max_ctl_value_bytes: usize,
}

impl Default for OperandLimits {
    fn default() -> Self {
        Self {
            max_operand_bytes: 1024 * 1024,
            max_scanned_bytes: 64 * 1024 * 1024,
            max_templates: 500_000,
            max_segments: 1_000_000,
            max_expansions: 500_000,
            max_expansions_per_template: 1_024,
            max_decoded_bytes: 16 * 1024 * 1024,
            max_runtime_expansion_bytes: 64 * 1024,
            max_rule_expansion_bytes: 1024 * 1024,
            max_phrases_per_operator: 65_536,
            max_phrases: 500_000,
            max_phrase_bytes: 16 * 1024 * 1024,
            max_assets_per_operator: 64,
            max_assets: 65_536,
            max_asset_bytes: 4 * 1024,
            max_networks_per_operator: 4_096,
            max_networks: 65_536,
            max_byte_range_members: 1_024,
            max_structured_actions: 200_000,
            max_ctl_targets: 200_000,
            max_setvar_key_bytes: 4 * 1024,
            max_ctl_value_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticLimits {
    pub max_documents: usize,
    pub max_directives: usize,
    pub max_rules: usize,
    pub max_nodes: usize,
    pub max_owned_bytes: usize,
    pub max_retained_bytes: usize,
    pub operands: OperandLimits,
    pub fold: FoldLimits,
    pub graph: GraphLimits,
}

impl Default for SemanticLimits {
    fn default() -> Self {
        Self {
            max_documents: DEFAULT_MAX_SEMANTIC_DOCUMENTS,
            max_directives: DEFAULT_MAX_SEMANTIC_DIRECTIVES,
            max_rules: DEFAULT_MAX_SEMANTIC_RULES,
            max_nodes: DEFAULT_MAX_SEMANTIC_NODES,
            max_owned_bytes: DEFAULT_MAX_SEMANTIC_OWNED_BYTES,
            max_retained_bytes: DEFAULT_MAX_SEMANTIC_RETAINED_BYTES,
            operands: OperandLimits::default(),
            fold: FoldLimits::default(),
            graph: GraphLimits::default(),
        }
    }
}

macro_rules! registry_enum {
    ($name:ident { $($variant:ident => $spelling:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $spelling),+ }
            }

            fn lookup_ascii_insensitive(value: &[u8]) -> Option<Self> {
                $(if value.eq_ignore_ascii_case($spelling.as_bytes()) { return Some(Self::$variant); })+
                None
            }

        }
    };
}

registry_enum!(CollectionKind {
    Args => "ARGS", ArgsCombinedSize => "ARGS_COMBINED_SIZE", ArgsGet => "ARGS_GET",
    ArgsGetNames => "ARGS_GET_NAMES", ArgsNames => "ARGS_NAMES", Files => "FILES",
    FilesCombinedSize => "FILES_COMBINED_SIZE", FilesNames => "FILES_NAMES", MatchedVar => "MATCHED_VAR",
    MatchedVars => "MATCHED_VARS", MultipartPartHeaders => "MULTIPART_PART_HEADERS", QueryString => "QUERY_STRING",
    RemoteAddr => "REMOTE_ADDR", ReqbodyProcessor => "REQBODY_PROCESSOR", RequestBasename => "REQUEST_BASENAME",
    RequestBody => "REQUEST_BODY", RequestBodyLength => "REQUEST_BODY_LENGTH", RequestCookies => "REQUEST_COOKIES",
    RequestCookiesNames => "REQUEST_COOKIES_NAMES", RequestFilename => "REQUEST_FILENAME",
    RequestHeaders => "REQUEST_HEADERS", RequestHeadersNames => "REQUEST_HEADERS_NAMES", RequestLine => "REQUEST_LINE",
    RequestMethod => "REQUEST_METHOD", RequestProtocol => "REQUEST_PROTOCOL", RequestUri => "REQUEST_URI",
    RequestUriRaw => "REQUEST_URI_RAW", ResponseBody => "RESPONSE_BODY", ResponseHeaders => "RESPONSE_HEADERS",
    ResponseStatus => "RESPONSE_STATUS", Tx => "TX", UniqueId => "UNIQUE_ID", Xml => "XML",
});

registry_enum!(OperatorKind {
    BeginsWith => "beginsWith", Contains => "contains", DetectSqli => "detectSQLi", DetectXss => "detectXSS",
    EndsWith => "endsWith", Eq => "eq", Ge => "ge", Gt => "gt", IpMatch => "ipMatch", Lt => "lt", Pm => "pm",
    PmFromFile => "pmFromFile", Rx => "rx", Streq => "streq", UnconditionalMatch => "unconditionalMatch",
    ValidateByteRange => "validateByteRange", ValidateUrlEncoding => "validateUrlEncoding",
    ValidateUtf8Encoding => "validateUtf8Encoding", Within => "within",
});

registry_enum!(ActionKind {
    AuditLog => "auditlog", Block => "block", Capture => "capture", Chain => "chain", Ctl => "ctl", Deny => "deny",
    Id => "id", InitCol => "initcol", Log => "log", LogData => "logdata", Msg => "msg", MultiMatch => "multiMatch",
    NoAuditLog => "noauditlog", NoLog => "nolog", Pass => "pass", Phase => "phase", SetVar => "setvar",
    Severity => "severity", Skip => "skip", SkipAfter => "skipAfter", Status => "status", Tag => "tag", Ver => "ver",
});

registry_enum!(TransformationKind {
    Base64Decode => "base64Decode", CmdLine => "cmdLine", CompressWhitespace => "compressWhitespace",
    CssDecode => "cssDecode", EscapeSeqDecode => "escapeSeqDecode", HexEncode => "hexEncode",
    HtmlEntityDecode => "htmlEntityDecode", JsDecode => "jsDecode", Length => "length", Lowercase => "lowercase",
    None => "none", NormalizePath => "normalizePath", NormalizePathWin => "normalizePathWin",
    RemoveCommentsChar => "removeCommentsChar", RemoveNulls => "removeNulls", RemoveWhitespace => "removeWhitespace",
    ReplaceComments => "replaceComments", Sha1 => "sha1", UrlDecodeUni => "urlDecodeUni",
    Utf8ToUnicode => "utf8toUnicode",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionEnvelopeKind {
    Action(ActionKind),
    Transformation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    RequestHeaders = 1,
    RequestBody = 2,
    ResponseHeaders = 3,
    ResponseBody = 4,
    Logging = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalActionValue {
    None,
    Id(u64),
    Phase(Phase),
    Severity(Severity),
    Status(u16),
    Skip(u32),
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetVarOperation {
    Assign,
    Add,
    Subtract,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitColCollection {
    Global,
    Ip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEngineMode {
    On,
    Off,
    RelevantOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnOff {
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestBodyProcessor {
    UrlEncoded,
    Multipart,
    Json,
    Xml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileRequirement {
    RegexCompilation,
    TargetSelectorRegexCompilation,
    PhraseMatcherCompilation,
    ExternalPhraseAsset,
    RuntimeMacro,
    BlockDefaultDisruptive,
    TransactionSetVar,
    DefaultPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableCapability {
    FullResponseBody,
    Ctl,
    InitCol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultDisruptive {
    Pass,
    Deny { status: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseDefaults {
    phase: Phase,
    disruptive: DefaultDisruptive,
    log: bool,
    audit_log: bool,
}

impl PhaseDefaults {
    pub const fn pass(phase: Phase, log: bool, audit_log: bool) -> Self {
        Self {
            phase,
            disruptive: DefaultDisruptive::Pass,
            log,
            audit_log,
        }
    }

    pub fn try_deny(phase: Phase, status: u16, log: bool, audit_log: bool) -> Result<Self, DefaultsContextError> {
        if !(200..=599).contains(&status) {
            return Err(DefaultsContextError::InvalidStatus { phase });
        }
        Ok(Self {
            phase,
            disruptive: DefaultDisruptive::Deny { status },
            log,
            audit_log,
        })
    }

    pub const fn phase(self) -> Phase {
        self.phase
    }
    pub const fn disruptive(self) -> DefaultDisruptive {
        self.disruptive
    }
    pub const fn log(self) -> bool {
        self.log
    }
    pub const fn audit_log(self) -> bool {
        self.audit_log
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DefaultsContextError {
    #[error("phase defaults contain duplicate {phase:?} entries")]
    DuplicatePhase { phase: Phase },
    #[error("phase defaults contain an invalid status for {phase:?}")]
    InvalidStatus { phase: Phase },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticValidationContext {
    implicit_rule_phase: Option<Phase>,
    defaults: [Option<PhaseDefaults>; 5],
}

impl SemanticValidationContext {
    pub fn try_new(
        implicit_rule_phase: Option<Phase>,
        defaults: &[PhaseDefaults],
    ) -> Result<Self, DefaultsContextError> {
        let mut result = Self {
            implicit_rule_phase,
            defaults: [None; 5],
        };
        for value in defaults.iter().copied() {
            let slot = &mut result.defaults[phase_index(value.phase())];
            if slot.is_some() {
                return Err(DefaultsContextError::DuplicatePhase { phase: value.phase() });
            }
            if let DefaultDisruptive::Deny { status } = value.disruptive() {
                if !(200..=599).contains(&status) {
                    return Err(DefaultsContextError::InvalidStatus { phase: value.phase() });
                }
            }
            *slot = Some(value);
        }
        Ok(result)
    }

    pub const fn implicit_rule_phase(self) -> Option<Phase> {
        self.implicit_rule_phase
    }

    pub const fn phase_defaults(self, phase: Phase) -> Option<PhaseDefaults> {
        self.defaults[phase_index(phase)]
    }
}

const fn phase_index(phase: Phase) -> usize {
    phase as usize - 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationLimits {
    pub max_rule_units: usize,
    pub max_id_selectors: usize,
    pub max_raw_matches_per_directive: usize,
    pub max_distinct_matches_per_directive: usize,
    pub max_raw_matches: usize,
    pub max_distinct_matches: usize,
    pub max_selector_work: usize,
    pub max_final_targets_per_rule: usize,
    pub max_final_target_references: usize,
    pub max_applied_exceptions: usize,
    pub max_target_comparisons: usize,
    pub max_target_comparison_bytes: usize,
    pub max_owned_bytes: usize,
    pub max_working_bytes: usize,
    pub max_retained_bytes: usize,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_rule_units: 100_000,
            max_id_selectors: 200_000,
            max_raw_matches_per_directive: 100_000,
            max_distinct_matches_per_directive: 100_000,
            max_raw_matches: 500_000,
            max_distinct_matches: 500_000,
            max_selector_work: 2_000_000,
            max_final_targets_per_rule: 1_024,
            max_final_target_references: 1_000_000,
            max_applied_exceptions: 200_000,
            max_target_comparisons: 10_000_000,
            max_target_comparison_bytes: 256 * 1024 * 1024,
            max_owned_bytes: 64 * 1024 * 1024,
            max_working_bytes: 32 * 1024 * 1024,
            max_retained_bytes: DEFAULT_MAX_SEMANTIC_RETAINED_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticNodeKind {
    Directive,
    Target,
    Operator,
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticLocation {
    document_index: usize,
    directive_index: usize,
    node_kind: SemanticNodeKind,
    node_index: usize,
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
    text: &'source str,
    logical_start: usize,
    logical_end: usize,
    span: SourceSpan,
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
    location: SemanticLocation,
    collection: CollectionKind,
    collection_spelling: SemanticText<'source>,
    selector_kind: SelectorKind,
    modifier: TargetModifier,
    selector: Option<SemanticText<'source>>,
    regex_body: Option<SemanticText<'source>>,
    span: SourceSpan,
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
struct SemanticOperatorRecord<'source> {
    location: SemanticLocation,
    kind: OperatorKind,
    spelling: Option<SemanticText<'source>>,
    negated: bool,
    operand: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SemanticActionRecord<'source> {
    location: SemanticLocation,
    kind: ActionEnvelopeKind,
    name: SemanticText<'source>,
    operand: usize,
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
enum OperandStorage<'source> {
    Borrowed(SemanticText<'source>),
    Decoded { start: usize, len: usize, span: SourceSpan },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TemplateRecord<'source> {
    storage: OperandStorage<'source>,
    segment_start: usize,
    segment_count: usize,
    expansion_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateSegmentRecordKind {
    Literal,
    Expansion(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TemplateSegmentRecord {
    kind: TemplateSegmentRecordKind,
    start: usize,
    end: usize,
    span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExpansionRecord<'source> {
    source: MacroSource,
    body: SemanticText<'source>,
    source_spelling: SemanticText<'source>,
    key: Option<SemanticText<'source>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StaticOperandRecord<'source> {
    storage: OperandStorage<'source>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TemplateSliceRecord {
    template: usize,
    start: usize,
    end: usize,
    segment_start: usize,
    segment_count: usize,
    expansion_count: usize,
    span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SetVarRecord {
    collection: CollectionKind,
    operation: SetVarOperation,
    key: TemplateSliceRecord,
    value: Option<TemplateSliceRecord>,
    constant: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InitColRecord {
    collection: InitColCollection,
    key: TemplateSliceRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CtlTargetRecord<'source> {
    collection: CollectionKind,
    collection_spelling: SemanticText<'source>,
    selector_kind: SelectorKind,
    selector: Option<SemanticText<'source>>,
    regex_body: Option<SemanticText<'source>>,
    span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CtlRecord<'source> {
    AuditEngine(AuditEngineMode),
    ForceRequestBodyVariable(OnOff),
    RequestBodyProcessor(RequestBodyProcessor),
    RuleRemoveByTag(SemanticText<'source>),
    RuleRemoveTargetByTag { tag: SemanticText<'source>, target: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionOperandRecord<'source> {
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
struct ListItemRecord {
    start: usize,
    end: usize,
    span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NetworkRecord {
    address: IpAddr,
    prefix: u8,
    span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperatorOperandRecord {
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
struct TemplateArenaView<'view, 'source> {
    templates: &'view [TemplateRecord<'source>],
    segments: &'view [TemplateSegmentRecord],
    expansions: &'view [ExpansionRecord<'source>],
    decoded: &'view [u8],
    operands: &'view [OperatorOperandRecord],
    static_operands: &'view [StaticOperandRecord<'source>],
    list_items: &'view [ListItemRecord],
    networks: &'view [NetworkRecord],
    action_operands: &'view [ActionOperandRecord<'source>],
    setvars: &'view [SetVarRecord],
    initcols: &'view [InitColRecord],
    ctls: &'view [CtlRecord<'source>],
    ctl_targets: &'view [CtlTargetRecord<'source>],
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

#[derive(Debug, Clone)]
pub struct TemplateSliceSegmentIter<'view, 'source> {
    records: std::slice::Iter<'view, TemplateSegmentRecord>,
    text: &'view str,
    expansions: &'view [ExpansionRecord<'source>],
    slice_start: usize,
    slice_end: usize,
    slice_span: SourceSpan,
}

impl<'view, 'source> Iterator for TemplateSliceSegmentIter<'view, 'source> {
    type Item = TemplateSliceSegmentView<'view, 'source>;

    fn next(&mut self) -> Option<Self::Item> {
        let record = self.records.next()?;
        let start = record.start.max(self.slice_start);
        let end = record.end.min(self.slice_end);
        let kind = match record.kind {
            TemplateSegmentRecordKind::Literal => TemplateSegmentKind::Literal,
            TemplateSegmentRecordKind::Expansion(index) => TemplateSegmentKind::Expansion(MacroExpansionView {
                record: &self.expansions[index],
            }),
        };
        Some(TemplateSliceSegmentView {
            kind,
            text: &self.text[start..end],
            span: SourceSpan {
                start: if start == self.slice_start {
                    self.slice_span.start
                } else {
                    record.span.start
                },
                end: if end == self.slice_end {
                    self.slice_span.end
                } else {
                    record.span.end
                },
            },
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}

impl ExactSizeIterator for TemplateSliceSegmentIter<'_, '_> {}
impl FusedIterator for TemplateSliceSegmentIter<'_, '_> {}

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
    pub fn segments(self) -> TemplateSliceSegmentIter<'view, 'source> {
        TemplateSliceSegmentIter {
            records: self.arena.segments
                [self.record.segment_start..self.record.segment_start + self.record.segment_count]
                .iter(),
            text: self.arena.template(self.record.template).text(),
            expansions: self.arena.expansions,
            slice_start: self.record.start,
            slice_end: self.record.end,
            slice_span: self.record.span,
        }
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
    record: &'view SemanticOperatorRecord<'source>,
    arena: TemplateArenaView<'view, 'source>,
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

#[derive(Debug, Clone)]
pub struct ExternalAssetIter<'view> {
    inner: PhraseIter<'view>,
    document_index: usize,
}

impl<'view> Iterator for ExternalAssetIter<'view> {
    type Item = ExternalAssetView<'view>;
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|item| ExternalAssetView {
            document_index: self.document_index,
            item,
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}
impl ExactSizeIterator for ExternalAssetIter<'_> {}
impl FusedIterator for ExternalAssetIter<'_> {}

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
    pub fn assets(self) -> ExternalAssetIter<'view> {
        ExternalAssetIter {
            inner: PhraseIter {
                records: self.records.iter(),
                text: self.text.text(),
            },
            document_index: self.document_index,
        }
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

#[derive(Debug, Clone)]
pub struct IpNetworkIter<'view> {
    records: std::slice::Iter<'view, NetworkRecord>,
}

impl<'view> Iterator for IpNetworkIter<'view> {
    type Item = IpNetworkView<'view>;
    fn next(&mut self) -> Option<Self::Item> {
        self.records.next().map(|record| IpNetworkView { record })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}
impl ExactSizeIterator for IpNetworkIter<'_> {}
impl FusedIterator for IpNetworkIter<'_> {}

#[derive(Debug, Clone, Copy)]
pub struct IpNetworksView<'view, 'source> {
    text: OperandTextView<'view, 'source>,
    records: &'view [NetworkRecord],
}

impl<'view, 'source> IpNetworksView<'view, 'source> {
    pub const fn text(self) -> OperandTextView<'view, 'source> {
        self.text
    }
    pub fn networks(self) -> IpNetworkIter<'view> {
        IpNetworkIter {
            records: self.records.iter(),
        }
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
    record: &'view SemanticActionRecord<'source>,
    arena: TemplateArenaView<'view, 'source>,
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
    records: std::slice::Iter<'view, SemanticActionRecord<'source>>,
    arena: TemplateArenaView<'view, 'source>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DirectiveLocation {
    document_index: usize,
    directive_index: usize,
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
struct FinalRuleRecord {
    fold: usize,
    role: FinalRuleRole,
    phase: FinalPhaseState,
    chain: Option<usize>,
    resolution: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChainRecord {
    root: DirectiveLocation,
    phase: FinalPhaseState,
    member_start: usize,
    member_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MarkerRecord<'source> {
    location: DirectiveLocation,
    name: &'source str,
    span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedSkipAfterRecord {
    referrer: usize,
    action: usize,
    marker: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalRuleFoldRecord {
    location: DirectiveLocation,
    envelope: LocalRuleEnvelope,
    role: Option<ProvisionalRuleRole>,
    presence: FoldActionPresence,
    action_start: usize,
    action_count: usize,
    id: Option<u64>,
    phase: Option<Phase>,
    disruptive: Option<DisruptiveKind>,
    status: Option<u16>,
    id_action: Option<usize>,
    phase_action: Option<usize>,
    disruptive_action: Option<usize>,
    status_action: Option<usize>,
    chain_action: Option<usize>,
    skip_action: Option<usize>,
    capture_action: Option<usize>,
    multi_match_action: Option<usize>,
    reset_action: Option<usize>,
    log: Option<bool>,
    audit_log: Option<bool>,
    effective_start: usize,
    effective_count: usize,
}

#[derive(Debug, Clone)]
pub struct EffectiveTransformationIter<'view, 'source> {
    indices: std::slice::Iter<'view, usize>,
    actions: &'view [SemanticActionRecord<'source>],
    arena: TemplateArenaView<'view, 'source>,
}

impl<'view, 'source> Iterator for EffectiveTransformationIter<'view, 'source> {
    type Item = SemanticActionView<'view, 'source>;

    fn next(&mut self) -> Option<Self::Item> {
        let index = *self.indices.next()?;
        Some(SemanticActionView {
            record: &self.actions[index],
            arena: self.arena,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.indices.size_hint()
    }
}

impl ExactSizeIterator for EffectiveTransformationIter<'_, '_> {}
impl FusedIterator for EffectiveTransformationIter<'_, '_> {}

#[derive(Debug, Clone, Copy)]
pub struct LocalRuleFoldView<'view, 'source> {
    index: usize,
    record: &'view LocalRuleFoldRecord,
    actions: &'view [SemanticActionRecord<'source>],
    effective: &'view [usize],
    arena: TemplateArenaView<'view, 'source>,
    graph: Option<FinalGraphArenaView<'view, 'source>>,
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
    pub fn effective_transformations(self) -> EffectiveTransformationIter<'view, 'source> {
        EffectiveTransformationIter {
            indices: self.effective
                [self.record.effective_start..self.record.effective_start + self.record.effective_count]
                .iter(),
            actions: self.actions,
            arena: self.arena,
        }
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
struct FinalGraphArenaView<'view, 'source> {
    folds: &'view [LocalRuleFoldRecord],
    actions: &'view [SemanticActionRecord<'source>],
    effective: &'view [usize],
    action_arena: TemplateArenaView<'view, 'source>,
    final_rules: &'view [FinalRuleRecord],
    chains: &'view [ChainRecord],
    chain_members: &'view [usize],
    markers: &'view [MarkerRecord<'source>],
    resolutions: &'view [ResolvedSkipAfterRecord],
}

impl<'view, 'source> FinalGraphArenaView<'view, 'source> {
    fn rule(self, index: usize) -> FinalRuleView<'view, 'source> {
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
    index: usize,
    arena: FinalGraphArenaView<'view, 'source>,
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
    pub fn members(self) -> ChainMemberIter<'view, 'source> {
        let record = self.record();
        ChainMemberIter {
            ordinal: 0,
            indices: self.arena.chain_members[record.member_start..record.member_start + record.member_count].iter(),
            arena: self.arena,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChainMemberIter<'view, 'source> {
    ordinal: usize,
    indices: std::slice::Iter<'view, usize>,
    arena: FinalGraphArenaView<'view, 'source>,
}
impl<'view, 'source> Iterator for ChainMemberIter<'view, 'source> {
    type Item = ChainMemberView<'view, 'source>;
    fn next(&mut self) -> Option<Self::Item> {
        let rule = self.arena.rule(*self.indices.next()?);
        let ordinal = self.ordinal;
        self.ordinal += 1;
        Some(ChainMemberView { ordinal, rule })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.indices.size_hint()
    }
}
impl ExactSizeIterator for ChainMemberIter<'_, '_> {}
impl FusedIterator for ChainMemberIter<'_, '_> {}

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
    index: usize,
    arena: FinalGraphArenaView<'view, 'source>,
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
    index: usize,
    arena: FinalGraphArenaView<'view, 'source>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedDisruptive {
    Pass,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedRuleStateRecord {
    phase: Phase,
    disruptive: ResolvedDisruptive,
    status: Option<u16>,
    log: bool,
    audit_log: bool,
    unit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValidatedRuleNodeRecord {
    rule: usize,
    state: ResolvedRuleStateRecord,
    target_start: usize,
    target_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValidatedRuleUnitRecord {
    id: u64,
    root: usize,
    active: bool,
    compatibility_excluded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuleIdIndexRecord {
    id: u64,
    unit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TargetOverlayEntry {
    target: usize,
    next: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AppliedExceptionRecord {
    directive: usize,
    affected_start: usize,
    affected_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidatedRequirementReference {
    Draft(usize),
    TargetSelectorRegex { target: usize },
}

#[derive(Debug)]
pub struct ValidatedSemanticDocument<'source> {
    draft: SemanticDraft<'source>,
    nodes: Vec<ValidatedRuleNodeRecord>,
    units: Vec<ValidatedRuleUnitRecord>,
    id_index: Vec<RuleIdIndexRecord>,
    final_targets: Vec<usize>,
    updates: Vec<AppliedExceptionRecord>,
    removals: Vec<AppliedExceptionRecord>,
    affected_units: Vec<usize>,
    requirements: Vec<ValidatedRequirementReference>,
    validation_charged_bytes: usize,
}

impl<'source> ValidatedSemanticDocument<'source> {
    pub fn rule_units(&self) -> impl ExactSizeIterator<Item = ValidatedRuleUnitView<'_, 'source>> + FusedIterator {
        (0..self.units.len()).map(|index| ValidatedRuleUnitView { index, owner: self })
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
    pub fn effective_transformations(self) -> EffectiveTransformationIter<'view, 'source> {
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
    pub fn members(self) -> ValidatedRuleNodeIter<'view, 'source> {
        let root = self.record().root;
        let rule = self.owner.draft.final_rules[root];
        let indices = match rule.chain {
            Some(chain) => NodeIndexIter::Slice(
                self.owner.draft.chain_members[self.owner.draft.chains[chain].member_start
                    ..self.owner.draft.chains[chain].member_start + self.owner.draft.chains[chain].member_count]
                    .iter(),
            ),
            None => NodeIndexIter::Range(root..root + 1),
        };
        ValidatedRuleNodeIter {
            owner: self.owner,
            indices,
        }
    }
}

#[derive(Debug, Clone)]
enum NodeIndexIter<'view> {
    Slice(std::slice::Iter<'view, usize>),
    Range(Range<usize>),
}

#[derive(Debug, Clone)]
pub struct ValidatedRuleNodeIter<'view, 'source> {
    owner: &'view ValidatedSemanticDocument<'source>,
    indices: NodeIndexIter<'view>,
}

impl<'view, 'source> Iterator for ValidatedRuleNodeIter<'view, 'source> {
    type Item = ValidatedRuleNodeView<'view, 'source>;
    fn next(&mut self) -> Option<Self::Item> {
        let index = match &mut self.indices {
            NodeIndexIter::Slice(values) => *values.next()?,
            NodeIndexIter::Range(values) => values.next()?,
        };
        Some(self.owner.node(index))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.indices {
            NodeIndexIter::Slice(values) => values.size_hint(),
            NodeIndexIter::Range(values) => values.size_hint(),
        }
    }
}
impl ExactSizeIterator for ValidatedRuleNodeIter<'_, '_> {}
impl FusedIterator for ValidatedRuleNodeIter<'_, '_> {}

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
            pub fn affected_units(self) -> AppliedAffectedUnitIter<'view, 'source> {
                let record = self.record();
                AppliedAffectedUnitIter {
                    owner: self.owner,
                    indices: self.owner.affected_units
                        [record.affected_start..record.affected_start + record.affected_count]
                        .iter(),
                }
            }
        }
    };
}
applied_view!(AppliedTargetUpdateView, updates);
applied_view!(AppliedRuleRemovalView, removals);

#[derive(Debug, Clone)]
pub struct AppliedAffectedUnitIter<'view, 'source> {
    owner: &'view ValidatedSemanticDocument<'source>,
    indices: std::slice::Iter<'view, usize>,
}

impl<'view, 'source> Iterator for AppliedAffectedUnitIter<'view, 'source> {
    type Item = ValidatedRuleUnitView<'view, 'source>;
    fn next(&mut self) -> Option<Self::Item> {
        Some(ValidatedRuleUnitView {
            index: *self.indices.next()?,
            owner: self.owner,
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.indices.size_hint()
    }
}
impl ExactSizeIterator for AppliedAffectedUnitIter<'_, '_> {}
impl FusedIterator for AppliedAffectedUnitIter<'_, '_> {}

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
    location: SemanticLocation,
    requirement: CompileRequirement,
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
    location: SemanticLocation,
    capability: UnavailableCapability,
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
struct DirectiveSemanticRecord {
    document_index: usize,
    directive_index: usize,
    target_start: usize,
    target_count: usize,
    operator: Option<usize>,
    action_start: usize,
    action_count: usize,
    fold: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct SemanticDirectiveView<'view, 'source> {
    document_index: usize,
    directive_index: usize,
    targets: &'view [SemanticTarget<'source>],
    id_selectors: &'view [IdSelector<'source>],
    operator: Option<SemanticOperatorView<'view, 'source>>,
    actions: SemanticActionIter<'view, 'source>,
    fold: Option<LocalRuleFoldView<'view, 'source>>,
    final_rule: Option<FinalRuleView<'view, 'source>>,
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

#[derive(Debug)]
pub struct SemanticDraft<'source> {
    documents: Vec<SyntaxDocument<'source>>,
    directives: Vec<DirectiveSemanticRecord>,
    targets: Vec<SemanticTarget<'source>>,
    operators: Vec<SemanticOperatorRecord<'source>>,
    actions: Vec<SemanticActionRecord<'source>>,
    action_operands: Vec<ActionOperandRecord<'source>>,
    setvars: Vec<SetVarRecord>,
    initcols: Vec<InitColRecord>,
    ctls: Vec<CtlRecord<'source>>,
    ctl_targets: Vec<CtlTargetRecord<'source>>,
    templates: Vec<TemplateRecord<'source>>,
    template_segments: Vec<TemplateSegmentRecord>,
    expansions: Vec<ExpansionRecord<'source>>,
    decoded_operands: Vec<u8>,
    operator_operands: Vec<OperatorOperandRecord>,
    static_operands: Vec<StaticOperandRecord<'source>>,
    list_items: Vec<ListItemRecord>,
    networks: Vec<NetworkRecord>,
    phrase_count: usize,
    asset_count: usize,
    structured_action_count: usize,
    phrase_bytes: usize,
    scanned_operand_bytes: usize,
    operand_limits: OperandLimits,
    requirements: Vec<RequirementRecord>,
    unavailable: Vec<CapabilityRecord>,
    folds: Vec<LocalRuleFoldRecord>,
    effective_transformations: Vec<usize>,
    final_rules: Vec<FinalRuleRecord>,
    chains: Vec<ChainRecord>,
    chain_members: Vec<usize>,
    markers: Vec<MarkerRecord<'source>>,
    resolved_skip_afters: Vec<ResolvedSkipAfterRecord>,
    semantic_charged_bytes: usize,
    inherited_charged_bytes: usize,
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
    fn final_graph_arena(&self) -> FinalGraphArenaView<'_, 'source> {
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
    CtlTarget(#[source] super::TargetSyntaxError),
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

fn analyze_semantic_draft_inner<'source>(
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

#[derive(Debug, Clone)]
struct MacroScan {
    start: usize,
    end: usize,
    body: Range<usize>,
    source: Range<usize>,
    key: Option<Range<usize>>,
    kind: MacroSource,
}

#[derive(Debug, Clone, Copy)]
struct TemplateShape {
    segments: usize,
    expansions: usize,
}

#[derive(Debug, Clone, Copy)]
struct MacroParseError {
    kind: MacroErrorKind,
    offset: usize,
}

const MACRO_SOURCES: [(&[u8], MacroSource, bool); 7] = [
    (b"TX", MacroSource::Tx, true),
    (b"ARGS", MacroSource::Args, true),
    (b"REQUEST_HEADERS", MacroSource::RequestHeaders, true),
    (b"MATCHED_VAR", MacroSource::MatchedVar, false),
    (b"MATCHED_VAR_NAME", MacroSource::MatchedVarName, false),
    (b"REMOTE_ADDR", MacroSource::RemoteAddr, false),
    (b"REQUEST_LINE", MacroSource::RequestLine, false),
];

fn walk_template<I, F>(bytes: I, mut on_macro: F) -> Result<TemplateShape, MacroParseError>
where
    I: IntoIterator<Item = u8>,
    F: FnMut(MacroScan),
{
    let mut bytes = bytes.into_iter().peekable();
    let mut offset = 0usize;
    let mut literal_start = 0usize;
    let mut segments = 0usize;
    let mut expansions = 0usize;
    while let Some(byte) = bytes.next() {
        let current = offset;
        offset += 1;
        if byte != b'%' || bytes.peek() != Some(&b'{') {
            continue;
        }
        bytes.next();
        offset += 1;
        let body_start = offset;
        let mut source_end = None;
        let mut key_start = None;
        let mut candidates = [true; 7];
        let mut source_offset = 0usize;
        let close = loop {
            let Some(part) = bytes.next() else {
                return Err(MacroParseError {
                    kind: MacroErrorKind::Unclosed,
                    offset: current,
                });
            };
            let part_offset = offset;
            offset += 1;
            if part == b'}' {
                break part_offset;
            }
            if part == b'{' || (part == b'%' && bytes.peek() == Some(&b'{')) {
                return Err(MacroParseError {
                    kind: MacroErrorKind::Nested,
                    offset: current,
                });
            }
            if !part.is_ascii_alphanumeric() && !matches!(part, b'_' | b'.' | b'[' | b']' | b'-') {
                return Err(MacroParseError {
                    kind: MacroErrorKind::InvalidCharacter,
                    offset: current,
                });
            }
            if source_end.is_none() && part == b'.' {
                source_end = Some(part_offset);
                key_start = Some(offset);
            } else if source_end.is_none() {
                for (candidate, (name, _, _)) in candidates.iter_mut().zip(MACRO_SOURCES) {
                    *candidate &= name
                        .get(source_offset)
                        .is_some_and(|expected| expected.eq_ignore_ascii_case(&part));
                }
                source_offset += 1;
            }
        };
        if close == body_start {
            return Err(MacroParseError {
                kind: MacroErrorKind::EmptyBody,
                offset: current,
            });
        }
        let source_end = source_end.unwrap_or(close);
        let Some((_, kind, requires_key)) = MACRO_SOURCES.into_iter().enumerate().find_map(|(index, value)| {
            (candidates[index] && value.0.len() == source_end - body_start).then_some(value)
        }) else {
            return Err(MacroParseError {
                kind: MacroErrorKind::UnknownSource,
                offset: body_start,
            });
        };
        let key = key_start.map(|start| start..close);
        if requires_key && key.as_ref().is_none_or(Range::is_empty) {
            return Err(MacroParseError {
                kind: MacroErrorKind::MissingKey,
                offset: key_start.unwrap_or(source_end),
            });
        }
        if !requires_key && key.is_some() {
            return Err(MacroParseError {
                kind: MacroErrorKind::ForbiddenKey,
                offset: key_start.unwrap_or(source_end),
            });
        }
        if current > literal_start {
            segments += 1;
        }
        expansions += 1;
        segments += 1;
        on_macro(MacroScan {
            start: current,
            end: offset,
            body: body_start..close,
            source: body_start..source_end,
            key,
            kind,
        });
        literal_start = offset;
    }
    if offset > literal_start {
        segments += 1;
    }
    Ok(TemplateShape { segments, expansions })
}

fn operator_macro_capable(kind: OperatorKind) -> bool {
    matches!(
        kind,
        OperatorKind::BeginsWith
            | OperatorKind::Contains
            | OperatorKind::EndsWith
            | OperatorKind::Eq
            | OperatorKind::Ge
            | OperatorKind::Gt
            | OperatorKind::Lt
            | OperatorKind::Streq
            | OperatorKind::Within
    )
}

#[cfg(test)]
fn action_macro_capable(kind: ActionKind) -> bool {
    matches!(
        kind,
        ActionKind::Msg | ActionKind::LogData | ActionKind::SetVar | ActionKind::InitCol
    )
}

struct TemplateBuild<'a, 'source> {
    templates: &'a mut Vec<TemplateRecord<'source>>,
    segments: &'a mut Vec<TemplateSegmentRecord>,
    expansions: &'a mut Vec<ExpansionRecord<'source>>,
    decoded: &'a mut Vec<u8>,
    requirements: &'a mut Vec<RequirementRecord>,
    semantic_charge: &'a mut usize,
    scanned: &'a mut usize,
    inherited: usize,
    limits: SemanticLimits,
}

impl<'source> TemplateBuild<'_, 'source> {
    fn check_operand_scan(&self, operand_bytes: usize, position: SourcePosition) -> Result<(), SemanticError> {
        if operand_bytes > self.limits.operands.max_operand_bytes {
            return Err(SemanticError::OperandByteLimit {
                limit: self.limits.operands.max_operand_bytes,
                position,
            });
        }
        let scanned = self
            .scanned
            .checked_add(operand_bytes)
            .ok_or(SemanticError::ScannedOperandByteLimit {
                limit: self.limits.operands.max_scanned_bytes,
                position,
            })?;
        if scanned > self.limits.operands.max_scanned_bytes {
            return Err(SemanticError::ScannedOperandByteLimit {
                limit: self.limits.operands.max_scanned_bytes,
                position,
            });
        }
        Ok(())
    }

    fn prepare(
        &mut self,
        shape: TemplateShape,
        operand_bytes: usize,
        copied_bytes: usize,
        location: SemanticLocation,
        position: SourcePosition,
    ) -> Result<(), SemanticError> {
        let operand_limits = self.limits.operands;
        if operand_bytes > operand_limits.max_operand_bytes {
            return Err(SemanticError::OperandByteLimit {
                limit: operand_limits.max_operand_bytes,
                position,
            });
        }
        let scanned = self
            .scanned
            .checked_add(operand_bytes)
            .ok_or(SemanticError::ScannedOperandByteLimit {
                limit: operand_limits.max_scanned_bytes,
                position,
            })?;
        if scanned > operand_limits.max_scanned_bytes {
            return Err(SemanticError::ScannedOperandByteLimit {
                limit: operand_limits.max_scanned_bytes,
                position,
            });
        }
        if self
            .templates
            .len()
            .checked_add(1)
            .is_none_or(|count| count > operand_limits.max_templates)
        {
            return Err(SemanticError::TemplateLimit {
                limit: operand_limits.max_templates,
                position,
            });
        }
        if shape.expansions > operand_limits.max_expansions_per_template {
            return Err(SemanticError::TemplateExpansionLimit {
                limit: operand_limits.max_expansions_per_template,
                position,
            });
        }
        if self
            .segments
            .len()
            .checked_add(shape.segments)
            .is_none_or(|count| count > operand_limits.max_segments)
        {
            return Err(SemanticError::TemplateSegmentLimit {
                limit: operand_limits.max_segments,
                position,
            });
        }
        if self
            .expansions
            .len()
            .checked_add(shape.expansions)
            .is_none_or(|count| count > operand_limits.max_expansions)
        {
            return Err(SemanticError::ExpansionLimit {
                limit: operand_limits.max_expansions,
                position,
            });
        }
        if self
            .decoded
            .len()
            .checked_add(copied_bytes)
            .is_none_or(|count| count > operand_limits.max_decoded_bytes)
        {
            return Err(SemanticError::DecodedOperandByteLimit {
                limit: operand_limits.max_decoded_bytes,
                position,
            });
        }
        let dynamic = size_of::<TemplateRecord<'_>>()
            .checked_add(size_of::<TemplateSegmentRecord>().checked_mul(shape.segments).ok_or(
                SemanticError::OwnedByteLimit {
                    limit: self.limits.max_owned_bytes,
                    position,
                },
            )?)
            .and_then(|value| value.checked_add(size_of::<ExpansionRecord<'_>>().checked_mul(shape.expansions)?))
            .and_then(|value| value.checked_add(copied_bytes))
            .and_then(|value| {
                value.checked_add(if shape.expansions > 0 {
                    size_of::<RequirementRecord>()
                } else {
                    0
                })
            })
            .ok_or(SemanticError::OwnedByteLimit {
                limit: self.limits.max_owned_bytes,
                position,
            })?;
        let charge = self
            .semantic_charge
            .checked_add(dynamic)
            .ok_or(SemanticError::OwnedByteLimit {
                limit: self.limits.max_owned_bytes,
                position,
            })?;
        if charge > self.limits.max_owned_bytes {
            return Err(SemanticError::OwnedByteLimit {
                limit: self.limits.max_owned_bytes,
                position,
            });
        }
        if self
            .inherited
            .checked_add(charge)
            .is_none_or(|retained| retained > self.limits.max_retained_bytes)
        {
            return Err(SemanticError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        if shape.expansions > 0 && self.requirements.len() == self.requirements.capacity() {
            return Err(SemanticError::Allocation {
                operation: "preflight compile requirements",
            });
        }
        self.templates.try_reserve(1).map_err(|_| SemanticError::Allocation {
            operation: "operand templates",
        })?;
        self.segments
            .try_reserve(shape.segments)
            .map_err(|_| SemanticError::Allocation {
                operation: "template segments",
            })?;
        self.expansions
            .try_reserve(shape.expansions)
            .map_err(|_| SemanticError::Allocation {
                operation: "macro expansions",
            })?;
        self.decoded
            .try_reserve(copied_bytes)
            .map_err(|_| SemanticError::Allocation {
                operation: "decoded operands",
            })?;
        *self.semantic_charge = charge;
        *self.scanned = scanned;
        if shape.expansions > 0 {
            self.requirements.push(RequirementRecord {
                location,
                requirement: CompileRequirement::RuntimeMacro,
            });
        }
        Ok(())
    }
}

fn mapped_text<'source>(
    line: &'source super::LogicalLine,
    logical: Range<usize>,
) -> Result<SemanticText<'source>, SemanticError> {
    let text = line
        .text()
        .get(logical.clone())
        .ok_or(SemanticError::OperandPositionMapping {
            logical_line: line.span().start.line,
            logical_offset: logical.start,
        })?;
    let start = line
        .position_at(logical.start)
        .ok_or(SemanticError::OperandPositionMapping {
            logical_line: line.span().start.line,
            logical_offset: logical.start,
        })?;
    let end = line
        .position_at_end(logical.end)
        .ok_or(SemanticError::OperandPositionMapping {
            logical_line: line.span().start.line,
            logical_offset: logical.end,
        })?;
    Ok(SemanticText {
        text,
        logical_start: logical.start,
        logical_end: logical.end,
        span: SourceSpan { start, end },
    })
}

fn action_mapped_text<'source>(
    operand: SemanticText<'source>,
    line: &'source super::LogicalLine,
    range: Range<usize>,
) -> Result<SemanticText<'source>, SemanticError> {
    let logical = operand
        .logical_start
        .checked_add(range.start)
        .and_then(|start| operand.logical_start.checked_add(range.end).map(|end| start..end))
        .ok_or(SemanticError::OperandPositionMapping {
            logical_line: line.span().start.line,
            logical_offset: range.start,
        })?;
    mapped_text(line, logical)
}

fn operator_mapped_text<'source>(
    operator: super::OperatorView<'_, 'source>,
    line: &'source super::LogicalLine,
    range: Range<usize>,
) -> Result<SemanticText<'source>, SemanticError> {
    let logical =
        operator
            .map_argument_decoded_nonempty_range(range.clone())
            .ok_or(SemanticError::OperandPositionMapping {
                logical_line: operator.logical_line_index() + 1,
                logical_offset: range.start,
            })?;
    mapped_text(line, logical)
}

fn macro_error_position_action(
    error: MacroParseError,
    operand: SemanticText<'_>,
    line: &super::LogicalLine,
) -> SourcePosition {
    if error.offset < operand.text().len() {
        action_mapped_text(operand, line, error.offset..error.offset + 1)
            .map_or(operand.span().start, |text| text.span().start)
    } else {
        operand.span().end
    }
}

fn macro_error_position_operator(
    error: MacroParseError,
    operator: super::OperatorView<'_, '_>,
    line: &super::LogicalLine,
) -> SourcePosition {
    if error.offset < operator.argument().decoded_len() {
        operator_mapped_text(operator, line, error.offset..error.offset + 1)
            .map_or(operator.argument_span().start, |text| text.span().start)
    } else {
        operator.argument_span().end
    }
}

fn validate_template_mapping<I, F>(bytes: I, decoded_len: usize, mut map: F) -> Result<(), SemanticError>
where
    I: IntoIterator<Item = u8>,
    F: FnMut(Range<usize>) -> Result<SourceSpan, SemanticError>,
{
    let mut failure = None;
    let mut literal = 0usize;
    walk_template(bytes, |macro_| {
        if failure.is_some() {
            return;
        }
        for range in [
            (macro_.start > literal).then_some(literal..macro_.start),
            Some(macro_.start..macro_.end),
            Some(macro_.body.clone()),
            Some(macro_.source.clone()),
            macro_.key.clone(),
        ]
        .into_iter()
        .flatten()
        {
            if let Err(error) = map(range) {
                failure = Some(error);
                break;
            }
        }
        literal = macro_.end;
    })
    .expect("template was validated by the allocation-free count pass");
    if failure.is_none() && literal < decoded_len {
        if let Err(error) = map(literal..decoded_len) {
            failure = Some(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

fn push_template_records<'source, I, F>(
    bytes: I,
    decoded_len: usize,
    storage: OperandStorage<'source>,
    templates: &mut Vec<TemplateRecord<'source>>,
    segments: &mut Vec<TemplateSegmentRecord>,
    expansions: &mut Vec<ExpansionRecord<'source>>,
    mut map: F,
) -> usize
where
    I: IntoIterator<Item = u8>,
    F: FnMut(Range<usize>) -> SemanticText<'source>,
{
    let template = templates.len();
    let segment_start = segments.len();
    let mut literal = 0usize;
    let shape = walk_template(bytes, |macro_| {
        if macro_.start > literal {
            let text = map(literal..macro_.start);
            segments.push(TemplateSegmentRecord {
                kind: TemplateSegmentRecordKind::Literal,
                start: literal,
                end: macro_.start,
                span: text.span(),
            });
        }
        let expansion = expansions.len();
        expansions.push(ExpansionRecord {
            source: macro_.kind,
            body: map(macro_.body.clone()),
            source_spelling: map(macro_.source.clone()),
            key: macro_.key.clone().map(&mut map),
        });
        let text = map(macro_.start..macro_.end);
        segments.push(TemplateSegmentRecord {
            kind: TemplateSegmentRecordKind::Expansion(expansion),
            start: macro_.start,
            end: macro_.end,
            span: text.span(),
        });
        literal = macro_.end;
    })
    .expect("template was validated before bounded arena mutation");
    if literal < decoded_len {
        let text = map(literal..decoded_len);
        segments.push(TemplateSegmentRecord {
            kind: TemplateSegmentRecordKind::Literal,
            start: literal,
            end: decoded_len,
            span: text.span(),
        });
    }
    templates.push(TemplateRecord {
        storage,
        segment_start,
        segment_count: shape.segments,
        expansion_count: shape.expansions,
    });
    template
}

fn attach_operator_template<'source>(
    build: &mut TemplateBuild<'_, 'source>,
    operator: super::OperatorView<'_, 'source>,
    line: &'source super::LogicalLine,
    location: SemanticLocation,
) -> Result<usize, SemanticError> {
    let argument = operator.argument();
    let decoded_len = argument.decoded_len();
    build.check_operand_scan(decoded_len, operator.argument_span().start)?;
    let shape = walk_template(argument.bytes(), |_| {}).map_err(|error| SemanticError::MacroSyntax {
        kind: error.kind,
        position: macro_error_position_operator(error, operator, line),
    })?;
    validate_template_mapping(argument.bytes(), decoded_len, |range| {
        operator_mapped_text(operator, line, range).map(|text| text.span())
    })?;
    let copied = if operator.argument_has_removable_escapes() {
        decoded_len
    } else {
        0
    };
    build.prepare(shape, decoded_len, copied, location, operator.argument_span().start)?;
    let storage = if copied == 0 {
        let range = operator.argument_logical_range();
        OperandStorage::Borrowed(SemanticText {
            text: argument.raw(),
            logical_start: range.start,
            logical_end: range.end,
            span: operator.argument_span(),
        })
    } else {
        let start = build.decoded.len();
        build.decoded.extend(argument.bytes());
        OperandStorage::Decoded {
            start,
            len: decoded_len,
            span: operator.argument_span(),
        }
    };
    Ok(push_template_records(
        argument.bytes(),
        decoded_len,
        storage,
        build.templates,
        build.segments,
        build.expansions,
        |range| operator_mapped_text(operator, line, range).expect("operator subranges were validated before mutation"),
    ))
}

fn strict_i64<I>(bytes: I) -> Result<i64, (IntegerOperandErrorKind, usize)>
where
    I: IntoIterator<Item = u8>,
{
    let mut bytes = bytes.into_iter().enumerate().peekable();
    let mut negative = false;
    if let Some((_, sign @ (b'+' | b'-'))) = bytes.peek().copied() {
        negative = sign == b'-';
        bytes.next();
    }
    if bytes.peek().is_none() {
        return Err((IntegerOperandErrorKind::Invalid, 0));
    }
    let limit = if negative {
        (i64::MAX as u64) + 1
    } else {
        i64::MAX as u64
    };
    let mut magnitude = 0u64;
    for (offset, byte) in bytes {
        if !byte.is_ascii_digit() {
            return Err((IntegerOperandErrorKind::Invalid, offset));
        }
        magnitude = magnitude
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
            .filter(|value| *value <= limit)
            .ok_or((IntegerOperandErrorKind::Overflow, offset))?;
    }
    if negative && magnitude == limit {
        Ok(i64::MIN)
    } else if negative {
        Ok(-(magnitude as i64))
    } else {
        Ok(magnitude as i64)
    }
}

fn operand_position(operator: super::OperatorView<'_, '_>, line: &super::LogicalLine, offset: usize) -> SourcePosition {
    if offset < operator.argument().decoded_len() {
        operator_mapped_text(operator, line, offset..offset + 1)
            .map_or(operator.argument_span().start, |text| text.span().start)
    } else {
        operator.argument_span().end
    }
}

fn split_ascii_words<I>(bytes: I, mut item: impl FnMut(Range<usize>)) -> usize
where
    I: IntoIterator<Item = u8>,
{
    let mut start = None;
    let mut offset = 0usize;
    let mut count = 0usize;
    for byte in bytes {
        if matches!(byte, b' ' | b'\t') {
            if let Some(item_start) = start.take() {
                item(item_start..offset);
                count += 1;
            }
        } else if start.is_none() {
            start = Some(offset);
        }
        offset += 1;
    }
    if let Some(item_start) = start {
        item(item_start..offset);
        count += 1;
    }
    count
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetWalkError {
    ByteLimit { offset: usize },
    Unsafe { kind: AssetErrorKind, offset: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AssetWalkShape {
    count: usize,
    payload_bytes: usize,
    inspected_bytes: usize,
}

fn walk_asset_words<I>(
    bytes: I,
    max_asset_bytes: usize,
    mut item: impl FnMut(Range<usize>),
) -> Result<AssetWalkShape, AssetWalkError>
where
    I: IntoIterator<Item = u8>,
{
    let mut start = None;
    let mut count = 0usize;
    let mut payload_bytes = 0usize;
    let mut inspected_bytes = 0usize;
    let mut byte_limit_offset = None;
    let mut component_start = 0usize;
    let mut component_len = 0usize;
    let mut component_dots = true;
    let mut scheme = true;

    let finish = |end: usize,
                  start: &mut Option<usize>,
                  count: &mut usize,
                  payload_bytes: &mut usize,
                  component_start: usize,
                  component_len: usize,
                  component_dots: bool,
                  item: &mut dyn FnMut(Range<usize>)|
     -> Result<(), AssetWalkError> {
        let Some(word_start) = start.take() else {
            return Ok(());
        };
        if component_len == 0 {
            return Err(AssetWalkError::Unsafe {
                kind: AssetErrorKind::EmptyComponent,
                offset: word_start + component_start,
            });
        }
        if component_dots && component_len <= 2 {
            return Err(AssetWalkError::Unsafe {
                kind: AssetErrorKind::DotComponent,
                offset: word_start + component_start,
            });
        }
        item(word_start..end);
        *count += 1;
        *payload_bytes += end - word_start;
        Ok(())
    };

    let mut offset = 0usize;
    for byte in bytes {
        inspected_bytes += 1;
        if matches!(byte, b' ' | b'\t') {
            finish(
                offset,
                &mut start,
                &mut count,
                &mut payload_bytes,
                component_start,
                component_len,
                component_dots,
                &mut item,
            )?;
            offset += 1;
            continue;
        }
        let word_start = *start.get_or_insert_with(|| {
            component_start = 0;
            component_len = 0;
            component_dots = true;
            scheme = true;
            offset
        });
        let local = offset - word_start;
        if local >= max_asset_bytes && byte_limit_offset.is_none() {
            byte_limit_offset = Some(offset);
        }
        let error = if local == 0 && byte == b'/' {
            Some(AssetErrorKind::Absolute)
        } else if byte == b'\\' {
            Some(AssetErrorKind::Backslash)
        } else if byte.is_ascii_control() || byte == 0x7f {
            Some(AssetErrorKind::Control)
        } else if byte == b':' && local > 0 && scheme {
            Some(AssetErrorKind::Scheme)
        } else {
            None
        };
        if let Some(kind) = error {
            return Err(AssetWalkError::Unsafe { kind, offset });
        }
        scheme &= if local == 0 {
            byte.is_ascii_alphabetic()
        } else {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-')
        };
        if byte == b'/' {
            if component_len == 0 {
                return Err(AssetWalkError::Unsafe {
                    kind: AssetErrorKind::EmptyComponent,
                    offset: word_start + component_start,
                });
            }
            if component_dots && component_len <= 2 {
                return Err(AssetWalkError::Unsafe {
                    kind: AssetErrorKind::DotComponent,
                    offset: word_start + component_start,
                });
            }
            component_start = local + 1;
            component_len = 0;
            component_dots = true;
        } else {
            component_len += 1;
            component_dots &= byte == b'.';
        }
        offset += 1;
    }
    finish(
        offset,
        &mut start,
        &mut count,
        &mut payload_bytes,
        component_start,
        component_len,
        component_dots,
        &mut item,
    )?;
    if let Some(offset) = byte_limit_offset {
        return Err(AssetWalkError::ByteLimit { offset });
    }
    Ok(AssetWalkShape {
        count,
        payload_bytes,
        inspected_bytes,
    })
}

fn project_dynamic(
    semantic_charge: usize,
    inherited: usize,
    dynamic: usize,
    limits: SemanticLimits,
    position: SourcePosition,
) -> Result<usize, SemanticError> {
    let charge = semantic_charge
        .checked_add(dynamic)
        .ok_or(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position,
        })?;
    if charge > limits.max_owned_bytes {
        return Err(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position,
        });
    }
    if inherited
        .checked_add(charge)
        .is_none_or(|retained| retained > limits.max_retained_bytes)
    {
        return Err(SemanticError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        });
    }
    Ok(charge)
}

struct OperatorOperandBuild<'a, 'source> {
    templates: TemplateBuild<'a, 'source>,
    operands: &'a mut Vec<OperatorOperandRecord>,
    static_operands: &'a mut Vec<StaticOperandRecord<'source>>,
    list_items: &'a mut Vec<ListItemRecord>,
    networks: &'a mut Vec<NetworkRecord>,
    phrase_count: &'a mut usize,
    asset_count: &'a mut usize,
    phrase_bytes: &'a mut usize,
    unavailable: &'a mut Vec<CapabilityRecord>,
}

impl<'a, 'source> OperatorOperandBuild<'a, 'source> {
    fn prepare_metadata_only(
        &mut self,
        requirement_records: usize,
        capability_records: usize,
        position: SourcePosition,
    ) -> Result<(), SemanticError> {
        let dynamic = size_of::<RequirementRecord>()
            .checked_mul(requirement_records)
            .and_then(|value| value.checked_add(size_of::<CapabilityRecord>().checked_mul(capability_records)?))
            .ok_or(SemanticError::OwnedByteLimit {
                limit: self.templates.limits.max_owned_bytes,
                position,
            })?;
        let charge = project_dynamic(
            *self.templates.semantic_charge,
            self.templates.inherited,
            dynamic,
            self.templates.limits,
            position,
        )?;
        self.templates
            .requirements
            .try_reserve(requirement_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "operator requirements",
            })?;
        self.unavailable
            .try_reserve(capability_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "operator capabilities",
            })?;
        *self.templates.semantic_charge = charge;
        Ok(())
    }

    fn store_static(
        &mut self,
        operator: super::OperatorView<'_, 'source>,
        extra_records: usize,
        network_records: usize,
        requirement_records: usize,
        capability_records: usize,
        position: SourcePosition,
    ) -> Result<usize, SemanticError> {
        let decoded_len = operator.argument().decoded_len();
        let copied = usize::from(operator.argument_has_removable_escapes()) * decoded_len;
        if self
            .templates
            .decoded
            .len()
            .checked_add(copied)
            .is_none_or(|count| count > self.templates.limits.operands.max_decoded_bytes)
        {
            return Err(SemanticError::DecodedOperandByteLimit {
                limit: self.templates.limits.operands.max_decoded_bytes,
                position,
            });
        }
        let dynamic = size_of::<StaticOperandRecord<'_>>()
            .checked_add(size_of::<ListItemRecord>().checked_mul(extra_records).ok_or(
                SemanticError::OwnedByteLimit {
                    limit: self.templates.limits.max_owned_bytes,
                    position,
                },
            )?)
            .and_then(|value| value.checked_add(size_of::<NetworkRecord>().checked_mul(network_records)?))
            .and_then(|value| value.checked_add(copied))
            .and_then(|value| value.checked_add(size_of::<RequirementRecord>().checked_mul(requirement_records)?))
            .and_then(|value| value.checked_add(size_of::<CapabilityRecord>().checked_mul(capability_records)?))
            .ok_or(SemanticError::OwnedByteLimit {
                limit: self.templates.limits.max_owned_bytes,
                position,
            })?;
        let charge = project_dynamic(
            *self.templates.semantic_charge,
            self.templates.inherited,
            dynamic,
            self.templates.limits,
            position,
        )?;
        self.static_operands
            .try_reserve(1)
            .map_err(|_| SemanticError::Allocation {
                operation: "static operands",
            })?;
        self.list_items
            .try_reserve(extra_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "operator list items",
            })?;
        self.networks
            .try_reserve(network_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "IP networks",
            })?;
        self.templates
            .decoded
            .try_reserve(copied)
            .map_err(|_| SemanticError::Allocation {
                operation: "decoded operands",
            })?;
        self.templates
            .requirements
            .try_reserve(requirement_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "operator requirements",
            })?;
        self.unavailable
            .try_reserve(capability_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "operator capabilities",
            })?;
        let storage = if copied == 0 {
            let logical = operator.argument_logical_range();
            OperandStorage::Borrowed(SemanticText {
                text: operator.argument().raw(),
                logical_start: logical.start,
                logical_end: logical.end,
                span: operator.argument_span(),
            })
        } else {
            let start = self.templates.decoded.len();
            self.templates.decoded.extend(operator.argument().bytes());
            OperandStorage::Decoded {
                start,
                len: decoded_len,
                span: operator.argument_span(),
            }
        };
        let id = self.static_operands.len();
        self.static_operands.push(StaticOperandRecord { storage });
        *self.templates.semantic_charge = charge;
        Ok(id)
    }
}

fn mask_ip(address: IpAddr, prefix: u8) -> IpAddr {
    match address {
        IpAddr::V4(value) => {
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            IpAddr::V4(Ipv4Addr::from(u32::from(value) & mask))
        }
        IpAddr::V6(value) => {
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) };
            IpAddr::V6(Ipv6Addr::from(u128::from(value) & mask))
        }
    }
}

fn trim_ascii(bytes: &[u8], mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end && matches!(bytes[range.start], b' ' | b'\t') {
        range.start += 1;
    }
    while range.start < range.end && matches!(bytes[range.end - 1], b' ' | b'\t') {
        range.end -= 1;
    }
    range
}

fn walk_comma_ranges(bytes: &[u8], mut item: impl FnMut(Range<usize>)) -> Result<usize, usize> {
    let mut count = 0usize;
    let mut start = 0usize;
    for end in bytes
        .iter()
        .enumerate()
        .filter_map(|(index, byte)| (*byte == b',').then_some(index))
        .chain(std::iter::once(bytes.len()))
    {
        let range = trim_ascii(bytes, start..end);
        if range.is_empty() {
            return Err(start);
        }
        item(range);
        count += 1;
        start = end.saturating_add(1);
    }
    Ok(count)
}

fn decimal_u8(bytes: &[u8], range: Range<usize>) -> Result<u8, usize> {
    if range.is_empty() {
        return Err(range.start);
    }
    let mut value = 0u16;
    for (index, byte) in bytes[range.clone()].iter().copied().enumerate() {
        if !byte.is_ascii_digit() {
            return Err(range.start + index);
        }
        value = value
            .checked_mul(10)
            .and_then(|current| current.checked_add(u16::from(byte - b'0')))
            .filter(|current| *current <= 255)
            .ok_or(range.start + index)?;
    }
    Ok(value as u8)
}

fn parse_byte_member(bytes: &[u8], range: Range<usize>) -> Result<(u8, u8), usize> {
    let dash = bytes[range.clone()].iter().position(|byte| *byte == b'-');
    let (start, end) = if let Some(relative) = dash {
        let split = range.start + relative;
        if bytes[split + 1..range.end].contains(&b'-') {
            return Err(split + 1);
        }
        let start_range = trim_ascii(bytes, range.start..split);
        let end_range = trim_ascii(bytes, split + 1..range.end);
        (decimal_u8(bytes, start_range)?, decimal_u8(bytes, end_range)?)
    } else {
        let value = decimal_u8(bytes, range.clone())?;
        (value, value)
    };
    if start > end {
        return Err(range.start);
    }
    Ok((start, end))
}

fn classify_operator_operand<'source>(
    build: &mut OperatorOperandBuild<'_, 'source>,
    operator: super::OperatorView<'_, 'source>,
    line: &'source super::LogicalLine,
    kind: OperatorKind,
    location: SemanticLocation,
    document_index: usize,
) -> Result<usize, SemanticError> {
    let decoded_len = operator.argument().decoded_len();
    build
        .templates
        .check_operand_scan(decoded_len, operator.argument_span().start)?;
    let required = !matches!(
        kind,
        OperatorKind::DetectSqli
            | OperatorKind::DetectXss
            | OperatorKind::UnconditionalMatch
            | OperatorKind::ValidateUrlEncoding
            | OperatorKind::ValidateUtf8Encoding
            | OperatorKind::Rx
    );
    if required && decoded_len == 0 {
        return Err(SemanticError::OperatorOperandPresence {
            expected: OperatorOperandContract::NonEmpty,
            position: operator.argument_span().start,
        });
    }
    let no_argument = matches!(
        kind,
        OperatorKind::DetectSqli
            | OperatorKind::DetectXss
            | OperatorKind::UnconditionalMatch
            | OperatorKind::ValidateUrlEncoding
            | OperatorKind::ValidateUtf8Encoding
    );
    if no_argument && decoded_len != 0 {
        return Err(SemanticError::OperatorOperandPresence {
            expected: OperatorOperandContract::Empty,
            position: operand_position(operator, line, 0),
        });
    }

    let template_kind = operator_macro_capable(kind);
    if template_kind {
        let shape = walk_template(operator.argument().bytes(), |_| {}).map_err(|error| SemanticError::MacroSyntax {
            kind: error.kind,
            position: macro_error_position_operator(error, operator, line),
        })?;
        let constant = if matches!(
            kind,
            OperatorKind::Eq | OperatorKind::Ge | OperatorKind::Gt | OperatorKind::Lt
        ) && shape.expansions == 0
        {
            Some(strict_i64(operator.argument().bytes()).map_err(|(error_kind, offset)| {
                SemanticError::IntegerOperand {
                    kind: error_kind,
                    position: operand_position(operator, line, offset),
                }
            })?)
        } else {
            None
        };
        let template = attach_operator_template(&mut build.templates, operator, line, location)?;
        let operand = match kind {
            OperatorKind::Eq | OperatorKind::Ge | OperatorKind::Gt | OperatorKind::Lt => {
                OperatorOperandRecord::IntegerTemplate { template, constant }
            }
            _ => OperatorOperandRecord::StringTemplate(template),
        };
        let id = build.operands.len();
        build.operands.push(operand);
        return Ok(id);
    }

    let operand = match kind {
        OperatorKind::DetectSqli
        | OperatorKind::DetectXss
        | OperatorKind::UnconditionalMatch
        | OperatorKind::ValidateUrlEncoding
        | OperatorKind::ValidateUtf8Encoding => {
            build.prepare_metadata_only(0, 0, operator.argument_span().start)?;
            OperatorOperandRecord::NoArgument
        }
        OperatorKind::Rx => {
            let storage = build.store_static(operator, 0, 0, 1, 0, operator.argument_span().start)?;
            OperatorOperandRecord::Regex(storage)
        }
        OperatorKind::Pm | OperatorKind::PmFromFile => {
            let bytes = operator.argument().bytes();
            let mut payload_bytes = 0usize;
            let mut mapping_error = None;
            let count = if kind == OperatorKind::PmFromFile {
                let shape = walk_asset_words(
                    bytes.clone(),
                    build.templates.limits.operands.max_asset_bytes,
                    |range| {
                        if mapping_error.is_none() {
                            mapping_error = operator_mapped_text(operator, line, range).err();
                        }
                    },
                )
                .map_err(|error| match error {
                    AssetWalkError::ByteLimit { offset } => SemanticError::AssetByteLimit {
                        limit: build.templates.limits.operands.max_asset_bytes,
                        position: operand_position(operator, line, offset),
                    },
                    AssetWalkError::Unsafe { kind, offset } => SemanticError::UnsafeAsset {
                        kind,
                        position: operand_position(operator, line, offset),
                    },
                })?;
                payload_bytes = shape.payload_bytes;
                shape.count
            } else {
                split_ascii_words(bytes.clone(), |range| {
                    payload_bytes += range.len();
                    if mapping_error.is_none() {
                        mapping_error = operator_mapped_text(operator, line, range).err();
                    }
                })
            };
            if count == 0 {
                return Err(SemanticError::OperatorOperandPresence {
                    expected: OperatorOperandContract::NonEmpty,
                    position: operator.argument_span().start,
                });
            }
            if let Some(error) = mapping_error {
                return Err(error);
            }
            if kind == OperatorKind::Pm {
                if let Some(offset) = bytes.clone().position(|byte| byte == b'|') {
                    return Err(SemanticError::PhraseSyntax {
                        position: operand_position(operator, line, offset),
                    });
                }
                if count > build.templates.limits.operands.max_phrases_per_operator {
                    return Err(SemanticError::OperatorPhraseLimit {
                        limit: build.templates.limits.operands.max_phrases_per_operator,
                        position: operator.argument_span().start,
                    });
                }
                let phrase_count = build
                    .phrase_count
                    .checked_add(count)
                    .ok_or(SemanticError::PhraseLimit {
                        limit: build.templates.limits.operands.max_phrases,
                        position: operator.argument_span().start,
                    })?;
                if phrase_count > build.templates.limits.operands.max_phrases {
                    return Err(SemanticError::PhraseLimit {
                        limit: build.templates.limits.operands.max_phrases,
                        position: operator.argument_span().start,
                    });
                }
                let phrase_bytes =
                    build
                        .phrase_bytes
                        .checked_add(payload_bytes)
                        .ok_or(SemanticError::PhraseByteLimit {
                            limit: build.templates.limits.operands.max_phrase_bytes,
                            position: operator.argument_span().start,
                        })?;
                if phrase_bytes > build.templates.limits.operands.max_phrase_bytes {
                    return Err(SemanticError::PhraseByteLimit {
                        limit: build.templates.limits.operands.max_phrase_bytes,
                        position: operator.argument_span().start,
                    });
                }
                let storage = build.store_static(operator, count, 0, 1, 0, operator.argument_span().start)?;
                let start = build.list_items.len();
                split_ascii_words(bytes, |range| {
                    let span = operator_mapped_text(operator, line, range.clone())
                        .expect("phrase ranges were source-mapped before candidate commit")
                        .span();
                    build.list_items.push(ListItemRecord {
                        start: range.start,
                        end: range.end,
                        span,
                    });
                });
                *build.phrase_count = phrase_count;
                *build.phrase_bytes = phrase_bytes;
                OperatorOperandRecord::PhraseList { storage, start, count }
            } else {
                if count > build.templates.limits.operands.max_assets_per_operator {
                    return Err(SemanticError::OperatorAssetLimit {
                        limit: build.templates.limits.operands.max_assets_per_operator,
                        position: operator.argument_span().start,
                    });
                }
                let asset_count = build.asset_count.checked_add(count).ok_or(SemanticError::AssetLimit {
                    limit: build.templates.limits.operands.max_assets,
                    position: operator.argument_span().start,
                })?;
                if asset_count > build.templates.limits.operands.max_assets {
                    return Err(SemanticError::AssetLimit {
                        limit: build.templates.limits.operands.max_assets,
                        position: operator.argument_span().start,
                    });
                }
                let storage = build.store_static(operator, count, 0, 2, 0, operator.argument_span().start)?;
                let start = build.list_items.len();
                split_ascii_words(bytes, |range| {
                    let span = operator_mapped_text(operator, line, range.clone())
                        .expect("asset ranges were source-mapped before candidate commit")
                        .span();
                    build.list_items.push(ListItemRecord {
                        start: range.start,
                        end: range.end,
                        span,
                    });
                });
                *build.asset_count = asset_count;
                OperatorOperandRecord::ExternalAssets {
                    storage,
                    start,
                    count,
                    document: document_index,
                }
            }
        }
        OperatorKind::IpMatch => {
            if operator.argument_has_removable_escapes() {
                let offset = operator.argument().bytes().position(|byte| byte == b'\"').unwrap_or(0);
                return Err(SemanticError::IpNetworkSyntax {
                    position: operand_position(operator, line, offset),
                });
            }
            let bytes = operator
                .argument()
                .as_borrowed()
                .expect("escape-free operator arguments remain borrowed")
                .as_bytes();
            let mut grammar_error = None;
            let mut mapping_error = None;
            let count = walk_comma_ranges(bytes, |range| {
                if mapping_error.is_none() {
                    mapping_error = operator_mapped_text(operator, line, range.clone()).err();
                }
                if grammar_error.is_none() {
                    grammar_error = parse_network(bytes, range).err();
                }
            })
            .map_err(|offset| SemanticError::IpNetworkSyntax {
                position: operand_position(operator, line, offset),
            })?;
            if let Some(error) = mapping_error {
                return Err(error);
            }
            if let Some(offset) = grammar_error {
                return Err(SemanticError::IpNetworkSyntax {
                    position: operand_position(operator, line, offset),
                });
            }
            if count > build.templates.limits.operands.max_networks_per_operator {
                return Err(SemanticError::OperatorNetworkLimit {
                    limit: build.templates.limits.operands.max_networks_per_operator,
                    position: operator.argument_span().start,
                });
            }
            if build
                .networks
                .len()
                .checked_add(count)
                .is_none_or(|count| count > build.templates.limits.operands.max_networks)
            {
                return Err(SemanticError::NetworkLimit {
                    limit: build.templates.limits.operands.max_networks,
                    position: operator.argument_span().start,
                });
            }
            let storage = build.store_static(operator, 0, count, 0, 0, operator.argument_span().start)?;
            let start = build.networks.len();
            walk_comma_ranges(bytes, |range| {
                let (address, prefix) = parse_network(bytes, range.clone())
                    .expect("IP network grammar was validated before candidate commit");
                build.networks.push(NetworkRecord {
                    address,
                    prefix,
                    span: operator_mapped_text(operator, line, range)
                        .expect("IP ranges were source-mapped before candidate commit")
                        .span(),
                });
            })
            .expect("IP member shape was validated before candidate commit");
            OperatorOperandRecord::IpNetworks {
                storage,
                start,
                count: build.networks.len() - start,
            }
        }
        OperatorKind::ValidateByteRange => {
            if operator.argument_has_removable_escapes() {
                let offset = operator.argument().bytes().position(|byte| byte == b'\"').unwrap_or(0);
                return Err(SemanticError::ByteSetSyntax {
                    position: operand_position(operator, line, offset),
                });
            }
            let bytes = operator
                .argument()
                .as_borrowed()
                .expect("escape-free operator arguments remain borrowed")
                .as_bytes();
            let mut words = [0u64; 4];
            let mut grammar_error = None;
            let member_count = walk_comma_ranges(bytes, |range| {
                if grammar_error.is_none() {
                    match parse_byte_member(bytes, range.clone()) {
                        Ok((start, end)) => {
                            for value in start..=end {
                                words[usize::from(value) / 64] |= 1u64 << (usize::from(value) % 64);
                            }
                        }
                        Err(offset) => grammar_error = Some(offset),
                    }
                }
            })
            .map_err(|offset| SemanticError::ByteSetSyntax {
                position: operand_position(operator, line, offset),
            })?;
            if let Some(offset) = grammar_error {
                return Err(SemanticError::ByteSetSyntax {
                    position: operand_position(operator, line, offset),
                });
            }
            if member_count > build.templates.limits.operands.max_byte_range_members {
                return Err(SemanticError::ByteRangeMemberLimit {
                    limit: build.templates.limits.operands.max_byte_range_members,
                    position: operator.argument_span().start,
                });
            }
            let storage = build.store_static(operator, 0, 0, 0, 0, operator.argument_span().start)?;
            OperatorOperandRecord::ByteSet {
                storage,
                words,
                member_count,
            }
        }
        _ => unreachable!("template-capable operators returned before static classification"),
    };
    *build.templates.scanned += decoded_len;
    let id = build.operands.len();
    build.operands.push(operand);
    match kind {
        OperatorKind::Rx => build.templates.requirements.push(RequirementRecord {
            location,
            requirement: CompileRequirement::RegexCompilation,
        }),
        OperatorKind::Pm => build.templates.requirements.push(RequirementRecord {
            location,
            requirement: CompileRequirement::PhraseMatcherCompilation,
        }),
        OperatorKind::PmFromFile => {
            build.templates.requirements.push(RequirementRecord {
                location,
                requirement: CompileRequirement::ExternalPhraseAsset,
            });
            build.templates.requirements.push(RequirementRecord {
                location,
                requirement: CompileRequirement::PhraseMatcherCompilation,
            });
        }
        _ => {}
    }
    Ok(id)
}

fn parse_network(bytes: &[u8], range: Range<usize>) -> Result<(IpAddr, u8), usize> {
    let member = &bytes[range.clone()];
    let slash = member.iter().position(|byte| *byte == b'/');
    if slash.is_some_and(|first| member[first + 1..].contains(&b'/')) {
        return Err(range.start + slash.unwrap_or(0) + 1);
    }
    let address_end = slash.map_or(range.end, |offset| range.start + offset);
    let address_text = std::str::from_utf8(&bytes[range.start..address_end]).map_err(|_| range.start)?;
    let address = address_text.parse::<IpAddr>().map_err(|_| range.start)?;
    let maximum = if address.is_ipv4() { 32 } else { 128 };
    let prefix = if let Some(offset) = slash {
        let prefix_start = range.start + offset + 1;
        let prefix_bytes = &bytes[prefix_start..range.end];
        if prefix_bytes.is_empty() {
            return Err(prefix_start);
        }
        let mut value = 0u16;
        for (index, byte) in prefix_bytes.iter().copied().enumerate() {
            if !byte.is_ascii_digit() {
                return Err(prefix_start + index);
            }
            value = value
                .checked_mul(10)
                .and_then(|current| current.checked_add(u16::from(byte - b'0')))
                .filter(|current| *current <= maximum)
                .ok_or(prefix_start + index)?;
        }
        value as u8
    } else {
        maximum as u8
    };
    Ok((mask_ip(address, prefix), prefix))
}

#[derive(Debug, Clone)]
struct SlicePlan {
    range: Range<usize>,
    expansions: usize,
}

#[derive(Debug, Clone)]
struct SetVarPlan {
    operation: SetVarOperation,
    key: SlicePlan,
    value: Option<SlicePlan>,
    constant: Option<i64>,
}

#[derive(Debug, Clone)]
struct InitColPlan {
    collection: InitColCollection,
    key: SlicePlan,
}

#[derive(Debug, Clone)]
enum CtlPlan<'source> {
    AuditEngine(AuditEngineMode),
    ForceRequestBodyVariable(OnOff),
    RequestBodyProcessor(RequestBodyProcessor),
    RuleRemoveByTag(SemanticText<'source>),
    RuleRemoveTargetByTag {
        tag: SemanticText<'source>,
        target_range: Range<usize>,
    },
}

enum ActionPlan<'source> {
    NoArgument,
    Local(LocalActionValue),
    Transformation(TransformationKind),
    Template,
    Literal(SemanticText<'source>),
    SetVar(SetVarPlan),
    InitCol(InitColPlan),
    Ctl(CtlPlan<'source>),
}

struct ActionOperandBuild<'a, 'source> {
    templates: &'a mut Vec<TemplateRecord<'source>>,
    segments: &'a mut Vec<TemplateSegmentRecord>,
    expansions: &'a mut Vec<ExpansionRecord<'source>>,
    operands: &'a mut Vec<ActionOperandRecord<'source>>,
    setvars: &'a mut Vec<SetVarRecord>,
    initcols: &'a mut Vec<InitColRecord>,
    ctls: &'a mut Vec<CtlRecord<'source>>,
    ctl_targets: &'a mut Vec<CtlTargetRecord<'source>>,
    structured_count: &'a mut usize,
    requirements: &'a mut Vec<RequirementRecord>,
    unavailable: &'a mut Vec<CapabilityRecord>,
    semantic_charge: &'a mut usize,
    scanned: &'a mut usize,
    inherited: usize,
    limits: SemanticLimits,
}

fn action_position(operand: SemanticText<'_>, line: &super::LogicalLine, offset: usize) -> SourcePosition {
    if offset < operand.text().len() {
        action_mapped_text(operand, line, offset..offset + 1).map_or(operand.span().start, |text| text.span().start)
    } else {
        operand.span().end
    }
}

fn plan_slice<I>(bytes: I, range: Range<usize>) -> Result<SlicePlan, ()>
where
    I: IntoIterator<Item = u8>,
{
    let mut expansions = 0usize;
    let mut crossing = false;
    walk_template(bytes, |macro_| {
        let overlaps = macro_.start < range.end && macro_.end > range.start;
        if overlaps {
            if macro_.start < range.start || macro_.end > range.end {
                crossing = true;
            } else {
                expansions += 1;
            }
        }
    })
    .expect("action template was validated before slice planning");
    if crossing {
        Err(())
    } else {
        Ok(SlicePlan { range, expansions })
    }
}

fn parse_setvar_plan(
    operand: SemanticText<'_>,
    line: &super::LogicalLine,
) -> Result<(SetVarPlan, TemplateShape), SemanticError> {
    let bytes = operand.text().as_bytes();
    let (operation, key_range, value_range) = if bytes.first() == Some(&b'!') {
        if bytes.iter().position(|byte| *byte == b'=').is_some() {
            return Err(SemanticError::SetVarSyntax {
                kind: SetVarErrorKind::Operation,
                position: operand.span().start,
            });
        }
        if bytes.len() < 5 || !bytes[1..3].eq_ignore_ascii_case(b"TX") || bytes[3] != b'.' {
            return Err(SemanticError::SetVarSyntax {
                kind: SetVarErrorKind::Collection,
                position: action_position(operand, line, 1.min(bytes.len())),
            });
        }
        (SetVarOperation::Delete, 4..bytes.len(), None)
    } else {
        if bytes.len() < 4 || !bytes[..2].eq_ignore_ascii_case(b"TX") || bytes[2] != b'.' {
            return Err(SemanticError::SetVarSyntax {
                kind: SetVarErrorKind::Collection,
                position: operand.span().start,
            });
        }
        let equals = bytes
            .iter()
            .position(|byte| *byte == b'=')
            .ok_or(SemanticError::SetVarSyntax {
                kind: SetVarErrorKind::Envelope,
                position: operand.span().end,
            })?;
        let value_start = equals + 1;
        let (operation, value_start) = match bytes.get(value_start) {
            Some(b'+') => (SetVarOperation::Add, value_start + 1),
            Some(b'-') => (SetVarOperation::Subtract, value_start + 1),
            Some(_) => (SetVarOperation::Assign, value_start),
            None => {
                return Err(SemanticError::SetVarSyntax {
                    kind: SetVarErrorKind::Value,
                    position: operand.span().end,
                })
            }
        };
        (operation, 3..equals, Some(value_start..bytes.len()))
    };
    if key_range.is_empty() {
        return Err(SemanticError::SetVarSyntax {
            kind: SetVarErrorKind::Key,
            position: action_position(operand, line, key_range.start),
        });
    }
    if value_range.as_ref().is_some_and(Range::is_empty) {
        return Err(SemanticError::SetVarSyntax {
            kind: SetVarErrorKind::Value,
            position: operand.span().end,
        });
    }
    let shape = walk_template(operand.text().bytes(), |_| {}).map_err(|error| SemanticError::MacroSyntax {
        kind: error.kind,
        position: macro_error_position_action(error, operand, line),
    })?;
    for (offset, byte) in bytes[key_range.clone()].iter().copied().enumerate() {
        if matches!(byte, b' ' | b'\t' | b'=') || byte.is_ascii_control() || byte == 0x7f {
            return Err(SemanticError::SetVarSyntax {
                kind: SetVarErrorKind::Key,
                position: action_position(operand, line, key_range.start + offset),
            });
        }
    }
    let key =
        plan_slice(operand.text().bytes(), key_range.clone()).map_err(|()| SemanticError::TemplateSliceInvariant {
            position: action_position(operand, line, key_range.start),
        })?;
    let value = value_range
        .clone()
        .map(|range| {
            plan_slice(operand.text().bytes(), range.clone()).map_err(|()| SemanticError::TemplateSliceInvariant {
                position: action_position(operand, line, range.start),
            })
        })
        .transpose()?;
    let constant = if matches!(operation, SetVarOperation::Add | SetVarOperation::Subtract)
        && value.as_ref().is_some_and(|slice| slice.expansions == 0)
    {
        let range = value_range.expect("arithmetic operation has a value range");
        Some(
            strict_i64(bytes[range.clone()].iter().copied()).map_err(|(kind, offset)| {
                SemanticError::IntegerOperand {
                    kind,
                    position: action_position(
                        operand,
                        line,
                        range.start
                            + if kind == IntegerOperandErrorKind::Overflow {
                                0
                            } else {
                                offset
                            },
                    ),
                }
            })?,
        )
    } else {
        None
    };
    Ok((
        SetVarPlan {
            operation,
            key,
            value,
            constant,
        },
        shape,
    ))
}

fn parse_initcol_plan(
    operand: SemanticText<'_>,
    line: &super::LogicalLine,
) -> Result<(InitColPlan, TemplateShape), SemanticError> {
    let bytes = operand.text().as_bytes();
    let equals = bytes
        .iter()
        .position(|byte| *byte == b'=')
        .ok_or(SemanticError::InitColSyntax {
            kind: InitColErrorKind::Envelope,
            position: operand.span().end,
        })?;
    if equals == 0 {
        return Err(SemanticError::InitColSyntax {
            kind: InitColErrorKind::Collection,
            position: operand.span().start,
        });
    }
    let collection = if bytes[..equals].eq_ignore_ascii_case(b"global") {
        InitColCollection::Global
    } else if bytes[..equals].eq_ignore_ascii_case(b"ip") {
        InitColCollection::Ip
    } else {
        return Err(SemanticError::InitColSyntax {
            kind: InitColErrorKind::Collection,
            position: operand.span().start,
        });
    };
    let key_range = equals + 1..bytes.len();
    if key_range.is_empty() {
        return Err(SemanticError::InitColSyntax {
            kind: InitColErrorKind::Key,
            position: operand.span().end,
        });
    }
    let shape = walk_template(operand.text().bytes(), |_| {}).map_err(|error| SemanticError::MacroSyntax {
        kind: error.kind,
        position: macro_error_position_action(error, operand, line),
    })?;
    let key =
        plan_slice(operand.text().bytes(), key_range.clone()).map_err(|()| SemanticError::TemplateSliceInvariant {
            position: action_position(operand, line, key_range.start),
        })?;
    Ok((InitColPlan { collection, key }, shape))
}

fn parse_on_off(bytes: &[u8]) -> Option<OnOff> {
    if bytes.eq_ignore_ascii_case(b"On") {
        Some(OnOff::On)
    } else if bytes.eq_ignore_ascii_case(b"Off") {
        Some(OnOff::Off)
    } else {
        None
    }
}

fn parse_ctl_plan<'source>(
    operand: SemanticText<'source>,
    line: &'source super::LogicalLine,
    line_index: usize,
) -> Result<CtlPlan<'source>, SemanticError> {
    let bytes = operand.text().as_bytes();
    let equals = bytes
        .iter()
        .position(|byte| *byte == b'=')
        .ok_or(SemanticError::CtlSyntax {
            kind: CtlErrorKind::Envelope,
            position: operand.span().end,
        })?;
    if equals == 0 || equals + 1 == bytes.len() {
        return Err(SemanticError::CtlSyntax {
            kind: CtlErrorKind::Envelope,
            position: action_position(operand, line, equals),
        });
    }
    let rhs = equals + 1..bytes.len();
    if matches!(bytes[equals - 1], b' ' | b'\t') || matches!(bytes[rhs.start], b' ' | b'\t') {
        return Err(SemanticError::CtlSyntax {
            kind: CtlErrorKind::Delimiter,
            position: action_position(operand, line, equals),
        });
    }
    if matches!(bytes[rhs.end - 1], b' ' | b'\t') {
        return Err(SemanticError::CtlSyntax {
            kind: CtlErrorKind::Value,
            position: action_position(operand, line, rhs.end - 1),
        });
    }
    let value = &bytes[rhs.clone()];
    match &bytes[..equals] {
        b"auditEngine" => {
            let mode = if value.eq_ignore_ascii_case(b"On") {
                AuditEngineMode::On
            } else if value.eq_ignore_ascii_case(b"Off") {
                AuditEngineMode::Off
            } else if value.eq_ignore_ascii_case(b"RelevantOnly") {
                AuditEngineMode::RelevantOnly
            } else {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Value,
                    position: action_position(operand, line, rhs.start),
                });
            };
            Ok(CtlPlan::AuditEngine(mode))
        }
        b"forceRequestBodyVariable" => {
            parse_on_off(value)
                .map(CtlPlan::ForceRequestBodyVariable)
                .ok_or(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Value,
                    position: action_position(operand, line, rhs.start),
                })
        }
        b"requestBodyProcessor" => {
            let processor = if value.eq_ignore_ascii_case(b"URLENCODED") {
                RequestBodyProcessor::UrlEncoded
            } else if value.eq_ignore_ascii_case(b"MULTIPART") {
                RequestBodyProcessor::Multipart
            } else if value.eq_ignore_ascii_case(b"JSON") {
                RequestBodyProcessor::Json
            } else if value.eq_ignore_ascii_case(b"XML") {
                RequestBodyProcessor::Xml
            } else {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Value,
                    position: action_position(operand, line, rhs.start),
                });
            };
            Ok(CtlPlan::RequestBodyProcessor(processor))
        }
        b"ruleRemoveByTag" => {
            if let Some(offset) = value.iter().position(|byte| *byte == b';') {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Delimiter,
                    position: action_position(operand, line, rhs.start + offset),
                });
            }
            Ok(CtlPlan::RuleRemoveByTag(action_mapped_text(operand, line, rhs)?))
        }
        b"ruleRemoveTargetByTag" => {
            let separator = value
                .iter()
                .position(|byte| *byte == b';')
                .ok_or(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Delimiter,
                    position: operand.span().end,
                })?;
            if value[separator + 1..].contains(&b';') {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Delimiter,
                    position: action_position(operand, line, rhs.start + separator + 1),
                });
            }
            let tag_range = rhs.start..rhs.start + separator;
            let target_range = tag_range.end + 1..rhs.end;
            if tag_range.is_empty() || target_range.is_empty() {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Delimiter,
                    position: action_position(operand, line, tag_range.end),
                });
            }
            if matches!(bytes[tag_range.end - 1], b' ' | b'\t') || matches!(bytes[target_range.start], b' ' | b'\t') {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Delimiter,
                    position: action_position(operand, line, tag_range.end),
                });
            }
            let tag = action_mapped_text(operand, line, tag_range)?;
            let target_text = action_mapped_text(operand, line, target_range.clone())?;
            let target = super::target::parse_single_target(
                target_text.text(),
                target_text.logical_range().start,
                line,
                line_index,
            )
            .map_err(SemanticError::CtlTarget)?;
            if CollectionKind::lookup_ascii_insensitive(target.collection().text().as_bytes()).is_none() {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Collection,
                    position: target.collection().span().start,
                });
            }
            Ok(CtlPlan::RuleRemoveTargetByTag { tag, target_range })
        }
        _ => Err(SemanticError::CtlSyntax {
            kind: CtlErrorKind::Command,
            position: operand.span().start,
        }),
    }
}

fn commit_template_slice<'source>(
    plan: SlicePlan,
    template: usize,
    operand: SemanticText<'source>,
    line: &'source super::LogicalLine,
    templates: &[TemplateRecord<'source>],
    segments: &[TemplateSegmentRecord],
) -> TemplateSliceRecord {
    let record = templates[template];
    let all = &segments[record.segment_start..record.segment_start + record.segment_count];
    let first = all
        .iter()
        .position(|segment| segment.end > plan.range.start)
        .expect("nonempty template slice must overlap a validated segment");
    let last = all
        .iter()
        .rposition(|segment| segment.start < plan.range.end)
        .expect("nonempty template slice must overlap a validated segment");
    let span = action_mapped_text(operand, line, plan.range.clone())
        .expect("action slice mapping was validated before candidate commit")
        .span();
    TemplateSliceRecord {
        template,
        start: plan.range.start,
        end: plan.range.end,
        segment_start: record.segment_start + first,
        segment_count: last - first + 1,
        expansion_count: plan.expansions,
        span,
    }
}

fn classify_action_operand<'source>(
    build: &mut ActionOperandBuild<'_, 'source>,
    envelope: ActionEnvelopeKind,
    operand: Option<SemanticText<'source>>,
    name: SemanticText<'source>,
    line: &'source super::LogicalLine,
    line_index: usize,
    location: SemanticLocation,
) -> Result<usize, SemanticError> {
    let operand_bytes = operand.map_or(0, |value| value.text().len());
    if operand_bytes > build.limits.operands.max_operand_bytes {
        return Err(SemanticError::OperandByteLimit {
            limit: build.limits.operands.max_operand_bytes,
            position: operand.map_or(name.span().end, |value| value.span().start),
        });
    }
    let scanned = build
        .scanned
        .checked_add(operand_bytes)
        .ok_or(SemanticError::ScannedOperandByteLimit {
            limit: build.limits.operands.max_scanned_bytes,
            position: operand.map_or(name.span().end, |value| value.span().start),
        })?;
    if scanned > build.limits.operands.max_scanned_bytes {
        return Err(SemanticError::ScannedOperandByteLimit {
            limit: build.limits.operands.max_scanned_bytes,
            position: operand.map_or(name.span().end, |value| value.span().start),
        });
    }

    let mut template_shape = None;
    let plan = match envelope {
        ActionEnvelopeKind::Transformation => {
            let value = required_operand(operand, name)?;
            let transformation = TransformationKind::lookup_ascii_insensitive(value.text().as_bytes()).ok_or(
                SemanticError::UnknownTransformation {
                    position: value.span().start,
                },
            )?;
            ActionPlan::Transformation(transformation)
        }
        ActionEnvelopeKind::Action(action) => match action {
            ActionKind::AuditLog
            | ActionKind::Block
            | ActionKind::Capture
            | ActionKind::Chain
            | ActionKind::Deny
            | ActionKind::Log
            | ActionKind::MultiMatch
            | ActionKind::NoAuditLog
            | ActionKind::NoLog
            | ActionKind::Pass => {
                forbidden_operand(operand, name)?;
                ActionPlan::NoArgument
            }
            ActionKind::Id => ActionPlan::Local(parse_id(required_operand(operand, name)?)?),
            ActionKind::Phase => ActionPlan::Local(parse_phase(required_operand(operand, name)?)?),
            ActionKind::Severity => ActionPlan::Local(parse_severity(required_operand(operand, name)?)?),
            ActionKind::Status => ActionPlan::Local(parse_status(required_operand(operand, name)?)?),
            ActionKind::Skip => ActionPlan::Local(parse_skip(required_operand(operand, name)?)?),
            ActionKind::Msg | ActionKind::LogData => {
                let value = required_operand(operand, name)?;
                let shape =
                    walk_template(value.text().bytes(), |_| {}).map_err(|error| SemanticError::MacroSyntax {
                        kind: error.kind,
                        position: macro_error_position_action(error, value, line),
                    })?;
                validate_template_mapping(value.text().bytes(), value.text().len(), |range| {
                    action_mapped_text(value, line, range).map(|text| text.span())
                })?;
                template_shape = Some(shape);
                ActionPlan::Template
            }
            ActionKind::SetVar => {
                let value = required_operand(operand, name)?;
                let (setvar, shape) = parse_setvar_plan(value, line)?;
                validate_template_mapping(value.text().bytes(), value.text().len(), |range| {
                    action_mapped_text(value, line, range).map(|text| text.span())
                })?;
                if setvar.key.range.len() > build.limits.operands.max_setvar_key_bytes {
                    return Err(SemanticError::SetVarKeyByteLimit {
                        limit: build.limits.operands.max_setvar_key_bytes,
                        position: action_position(value, line, setvar.key.range.start),
                    });
                }
                template_shape = Some(shape);
                ActionPlan::SetVar(setvar)
            }
            ActionKind::InitCol => {
                let value = required_operand(operand, name)?;
                let (initcol, shape) = parse_initcol_plan(value, line)?;
                validate_template_mapping(value.text().bytes(), value.text().len(), |range| {
                    action_mapped_text(value, line, range).map(|text| text.span())
                })?;
                template_shape = Some(shape);
                ActionPlan::InitCol(initcol)
            }
            ActionKind::Ctl => {
                let value = required_operand(operand, name)?;
                let ctl = parse_ctl_plan(value, line, line_index)?;
                let equals = value
                    .text()
                    .as_bytes()
                    .iter()
                    .position(|byte| *byte == b'=')
                    .expect("validated ctl envelope contains an equals delimiter");
                let ctl_bytes = value.text().len() - equals - 1;
                if ctl_bytes > build.limits.operands.max_ctl_value_bytes {
                    return Err(SemanticError::CtlValueByteLimit {
                        limit: build.limits.operands.max_ctl_value_bytes,
                        position: action_position(value, line, equals + 1),
                    });
                }
                ActionPlan::Ctl(ctl)
            }
            ActionKind::Tag | ActionKind::Ver | ActionKind::SkipAfter => {
                ActionPlan::Literal(required_operand(operand, name)?)
            }
        },
    };

    let position = operand.map_or(name.span().end, |value| value.span().start);
    let shape = template_shape.unwrap_or(TemplateShape {
        segments: 0,
        expansions: 0,
    });
    if template_shape.is_some() {
        if build
            .templates
            .len()
            .checked_add(1)
            .is_none_or(|count| count > build.limits.operands.max_templates)
        {
            return Err(SemanticError::TemplateLimit {
                limit: build.limits.operands.max_templates,
                position,
            });
        }
        if shape.expansions > build.limits.operands.max_expansions_per_template {
            return Err(SemanticError::TemplateExpansionLimit {
                limit: build.limits.operands.max_expansions_per_template,
                position,
            });
        }
        if build
            .segments
            .len()
            .checked_add(shape.segments)
            .is_none_or(|count| count > build.limits.operands.max_segments)
        {
            return Err(SemanticError::TemplateSegmentLimit {
                limit: build.limits.operands.max_segments,
                position,
            });
        }
        if build
            .expansions
            .len()
            .checked_add(shape.expansions)
            .is_none_or(|count| count > build.limits.operands.max_expansions)
        {
            return Err(SemanticError::ExpansionLimit {
                limit: build.limits.operands.max_expansions,
                position,
            });
        }
    }
    let structured = usize::from(matches!(
        plan,
        ActionPlan::SetVar(_) | ActionPlan::InitCol(_) | ActionPlan::Ctl(_)
    ));
    let structured_count =
        build
            .structured_count
            .checked_add(structured)
            .ok_or(SemanticError::StructuredActionLimit {
                limit: build.limits.operands.max_structured_actions,
                position,
            })?;
    if structured_count > build.limits.operands.max_structured_actions {
        return Err(SemanticError::StructuredActionLimit {
            limit: build.limits.operands.max_structured_actions,
            position,
        });
    }
    let ctl_target_count = usize::from(matches!(plan, ActionPlan::Ctl(CtlPlan::RuleRemoveTargetByTag { .. })));
    if build
        .ctl_targets
        .len()
        .checked_add(ctl_target_count)
        .is_none_or(|count| count > build.limits.operands.max_ctl_targets)
    {
        return Err(SemanticError::CtlTargetLimit {
            limit: build.limits.operands.max_ctl_targets,
            position,
        });
    }

    let runtime_macro = template_shape.is_some_and(|value| value.expansions > 0);
    let setvar_requirement = matches!(plan, ActionPlan::SetVar(_));
    let block_requirement = matches!(envelope, ActionEnvelopeKind::Action(ActionKind::Block));
    let requirement_count =
        usize::from(runtime_macro) + usize::from(setvar_requirement) + usize::from(block_requirement);
    let capability = match plan {
        ActionPlan::InitCol(_) => Some(UnavailableCapability::InitCol),
        ActionPlan::Ctl(CtlPlan::AuditEngine(_)) => Some(UnavailableCapability::Ctl),
        _ => None,
    };
    let dynamic = if template_shape.is_some() {
        size_of::<TemplateRecord<'_>>()
            .checked_add(size_of::<TemplateSegmentRecord>().checked_mul(shape.segments).ok_or(
                SemanticError::OwnedByteLimit {
                    limit: build.limits.max_owned_bytes,
                    position,
                },
            )?)
            .and_then(|value| value.checked_add(size_of::<ExpansionRecord<'_>>().checked_mul(shape.expansions)?))
    } else {
        Some(0)
    }
    .and_then(|value| {
        value.checked_add(match plan {
            ActionPlan::SetVar(_) => size_of::<SetVarRecord>(),
            ActionPlan::InitCol(_) => size_of::<InitColRecord>(),
            ActionPlan::Ctl(CtlPlan::RuleRemoveTargetByTag { .. }) => {
                size_of::<CtlRecord<'_>>() + size_of::<CtlTargetRecord<'_>>()
            }
            ActionPlan::Ctl(_) => size_of::<CtlRecord<'_>>(),
            _ => 0,
        })
    })
    .and_then(|value| value.checked_add(size_of::<RequirementRecord>().checked_mul(requirement_count)?))
    .and_then(|value| value.checked_add(size_of::<CapabilityRecord>().checked_mul(usize::from(capability.is_some()))?))
    .ok_or(SemanticError::OwnedByteLimit {
        limit: build.limits.max_owned_bytes,
        position,
    })?;
    let charge = project_dynamic(*build.semantic_charge, build.inherited, dynamic, build.limits, position)?;
    if build
        .requirements
        .len()
        .checked_add(requirement_count)
        .is_none_or(|count| count > build.requirements.capacity())
    {
        return Err(SemanticError::Allocation {
            operation: "preflight compile requirements",
        });
    }
    if build
        .unavailable
        .len()
        .checked_add(usize::from(capability.is_some()))
        .is_none_or(|count| count > build.unavailable.capacity())
    {
        return Err(SemanticError::Allocation {
            operation: "preflight unavailable capabilities",
        });
    }
    if template_shape.is_some() {
        build.templates.try_reserve(1).map_err(|_| SemanticError::Allocation {
            operation: "operand templates",
        })?;
        build
            .segments
            .try_reserve(shape.segments)
            .map_err(|_| SemanticError::Allocation {
                operation: "template segments",
            })?;
        build
            .expansions
            .try_reserve(shape.expansions)
            .map_err(|_| SemanticError::Allocation {
                operation: "macro expansions",
            })?;
    }
    match plan {
        ActionPlan::SetVar(_) => build.setvars.try_reserve(1).map_err(|_| SemanticError::Allocation {
            operation: "setvar records",
        })?,
        ActionPlan::InitCol(_) => build.initcols.try_reserve(1).map_err(|_| SemanticError::Allocation {
            operation: "initcol records",
        })?,
        ActionPlan::Ctl(CtlPlan::RuleRemoveTargetByTag { .. }) => {
            build.ctls.try_reserve(1).map_err(|_| SemanticError::Allocation {
                operation: "ctl records",
            })?;
            build
                .ctl_targets
                .try_reserve(1)
                .map_err(|_| SemanticError::Allocation {
                    operation: "ctl target records",
                })?;
        }
        ActionPlan::Ctl(_) => build.ctls.try_reserve(1).map_err(|_| SemanticError::Allocation {
            operation: "ctl records",
        })?,
        _ => {}
    }

    *build.semantic_charge = charge;
    *build.scanned = scanned;
    *build.structured_count = structured_count;
    let template = if template_shape.is_some() {
        let value = operand.expect("template action has a required operand");
        Some(push_template_records(
            value.text().bytes(),
            value.text().len(),
            OperandStorage::Borrowed(value),
            build.templates,
            build.segments,
            build.expansions,
            |range| {
                action_mapped_text(value, line, range).expect("action template mapping was validated before commit")
            },
        ))
    } else {
        None
    };
    let record = match plan {
        ActionPlan::NoArgument => ActionOperandRecord::NoArgument,
        ActionPlan::Local(value) => ActionOperandRecord::Local(value),
        ActionPlan::Transformation(value) => ActionOperandRecord::Transformation(value),
        ActionPlan::Template => ActionOperandRecord::Template(template.expect("template action prepared a template")),
        ActionPlan::Literal(value) => ActionOperandRecord::Literal(value),
        ActionPlan::SetVar(plan) => {
            let template = template.expect("setvar prepared a template");
            let value = operand.expect("setvar has an operand");
            let key = commit_template_slice(plan.key, template, value, line, build.templates, build.segments);
            let operand_value = plan
                .value
                .map(|slice| commit_template_slice(slice, template, value, line, build.templates, build.segments));
            let index = build.setvars.len();
            build.setvars.push(SetVarRecord {
                collection: CollectionKind::Tx,
                operation: plan.operation,
                key,
                value: operand_value,
                constant: plan.constant,
            });
            ActionOperandRecord::SetVar(index)
        }
        ActionPlan::InitCol(plan) => {
            let template = template.expect("initcol prepared a template");
            let value = operand.expect("initcol has an operand");
            let key = commit_template_slice(plan.key, template, value, line, build.templates, build.segments);
            let index = build.initcols.len();
            build.initcols.push(InitColRecord {
                collection: plan.collection,
                key,
            });
            ActionOperandRecord::InitCol(index)
        }
        ActionPlan::Ctl(plan) => {
            let ctl = match plan {
                CtlPlan::AuditEngine(value) => CtlRecord::AuditEngine(value),
                CtlPlan::ForceRequestBodyVariable(value) => CtlRecord::ForceRequestBodyVariable(value),
                CtlPlan::RequestBodyProcessor(value) => CtlRecord::RequestBodyProcessor(value),
                CtlPlan::RuleRemoveByTag(value) => CtlRecord::RuleRemoveByTag(value),
                CtlPlan::RuleRemoveTargetByTag { tag, target_range } => {
                    let value = operand.expect("ctl target action has an operand");
                    let target_text = action_mapped_text(value, line, target_range)
                        .expect("ctl target range was mapped before candidate commit");
                    let target = super::target::parse_single_target(
                        target_text.text(),
                        target_text.logical_range().start,
                        line,
                        line_index,
                    )
                    .expect("ctl target grammar was validated before candidate commit");
                    let collection_spelling = SemanticText::from(target.collection());
                    let collection = CollectionKind::lookup_ascii_insensitive(collection_spelling.text().as_bytes())
                        .expect("ctl target collection was validated before commit");
                    let target_index = build.ctl_targets.len();
                    build.ctl_targets.push(CtlTargetRecord {
                        collection,
                        collection_spelling,
                        selector_kind: target.selector_kind(),
                        selector: target.selector().map(SemanticText::from),
                        regex_body: target.regex_body().map(SemanticText::from),
                        span: target.span(),
                    });
                    CtlRecord::RuleRemoveTargetByTag {
                        tag,
                        target: target_index,
                    }
                }
            };
            let index = build.ctls.len();
            build.ctls.push(ctl);
            ActionOperandRecord::Ctl(index)
        }
    };
    let operand_index = build.operands.len();
    build.operands.push(record);
    if runtime_macro {
        build.requirements.push(RequirementRecord {
            location,
            requirement: CompileRequirement::RuntimeMacro,
        });
    }
    if setvar_requirement {
        build.requirements.push(RequirementRecord {
            location,
            requirement: CompileRequirement::TransactionSetVar,
        });
    }
    if block_requirement {
        build.requirements.push(RequirementRecord {
            location,
            requirement: CompileRequirement::BlockDefaultDisruptive,
        });
    }
    if let Some(capability) = capability {
        build.unavailable.push(CapabilityRecord { location, capability });
    }
    Ok(operand_index)
}

fn scan_semantics(draft: &mut SemanticDraft<'_>, limits: SemanticLimits) -> Result<(), SemanticError> {
    let SemanticDraft {
        documents,
        directives,
        targets,
        operators,
        actions,
        action_operands,
        setvars,
        initcols,
        ctls,
        ctl_targets,
        templates,
        template_segments,
        expansions,
        decoded_operands,
        operator_operands,
        static_operands,
        list_items,
        networks,
        phrase_count,
        asset_count,
        structured_action_count,
        phrase_bytes,
        scanned_operand_bytes,
        operand_limits: _,
        requirements,
        unavailable,
        folds: _,
        effective_transformations: _,
        final_rules: _,
        chains: _,
        chain_members: _,
        markers: _,
        resolved_skip_afters: _,
        semantic_charged_bytes,
        inherited_charged_bytes,
    } = draft;
    for (document_index, document) in documents.iter().enumerate() {
        for directive_index in 0..document.len() {
            let view = document
                .get(directive_index)
                .expect("semantic scan index is bounded by immutable syntax document length");
            let target_start = targets.len();
            let operator_start = operators.len();
            let action_start = actions.len();
            match view.targets() {
                DirectiveTargetView::None | DirectiveTargetView::Remove { .. } => {}
                DirectiveTargetView::Rule { atoms } | DirectiveTargetView::Update { atoms, .. } => {
                    for atom in atoms {
                        let collection_spelling = SemanticText::from(atom.collection());
                        let collection = CollectionKind::lookup_ascii_insensitive(
                            collection_spelling.text().as_bytes(),
                        )
                        .ok_or(SemanticError::UnknownCollection {
                            position: collection_spelling.span().start,
                        })?;
                        let location =
                            location(document_index, directive_index, SemanticNodeKind::Target, targets.len());
                        targets.push(SemanticTarget {
                            location,
                            collection,
                            collection_spelling,
                            selector_kind: atom.selector_kind(),
                            modifier: atom.modifier(),
                            selector: atom.selector().map(SemanticText::from),
                            regex_body: atom.regex_body().map(SemanticText::from),
                            span: atom.span(),
                        });
                        if collection == CollectionKind::ResponseBody && atom.modifier() != TargetModifier::Exclude {
                            push_capability(
                                unavailable,
                                semantic_charged_bytes,
                                *inherited_charged_bytes,
                                location,
                                UnavailableCapability::FullResponseBody,
                                limits,
                                collection_spelling.span().start,
                            )?;
                        }
                    }
                }
            }
            if let Some(operator) = view.operator() {
                let (kind, spelling) = match operator.name() {
                    OperatorNameView::ImplicitRx => (OperatorKind::Rx, None),
                    OperatorNameView::Explicit(name) => {
                        let kind = lookup_operator(name).ok_or(SemanticError::UnknownOperator {
                            position: operator.name_span().expect("explicit name has span").start,
                        })?;
                        let range = operator.name_logical_range().expect("explicit name has range");
                        (
                            kind,
                            Some(SemanticText {
                                text: name.raw(),
                                logical_start: range.start,
                                logical_end: range.end,
                                span: operator.name_span().expect("explicit name has span"),
                            }),
                        )
                    }
                };
                let location = location(
                    document_index,
                    directive_index,
                    SemanticNodeKind::Operator,
                    operators.len(),
                );
                let line = &document.source().lines()[operator.logical_line_index()];
                let operand = classify_operator_operand(
                    &mut OperatorOperandBuild {
                        templates: TemplateBuild {
                            templates,
                            segments: template_segments,
                            expansions,
                            decoded: decoded_operands,
                            requirements,
                            semantic_charge: semantic_charged_bytes,
                            scanned: scanned_operand_bytes,
                            inherited: *inherited_charged_bytes,
                            limits,
                        },
                        operands: operator_operands,
                        static_operands,
                        list_items,
                        networks,
                        phrase_count,
                        asset_count,
                        phrase_bytes,
                        unavailable,
                    },
                    operator,
                    line,
                    kind,
                    location,
                    document_index,
                )?;
                operators.push(SemanticOperatorRecord {
                    location,
                    kind,
                    spelling,
                    negated: operator.negated(),
                    operand,
                });
            }
            if let Some(action_list) = view.actions() {
                for entry in action_list.entries() {
                    let name = SemanticText::from(entry.name());
                    let location = location(document_index, directive_index, SemanticNodeKind::Action, actions.len());
                    let operand = entry.value().map(|value| SemanticText::from(value.body()));
                    let kind = if entry.kind() == ActionEntryKind::Transformation {
                        ActionEnvelopeKind::Transformation
                    } else {
                        let action = ActionKind::lookup_ascii_insensitive(name.text().as_bytes()).ok_or(
                            SemanticError::UnknownAction {
                                position: name.span().start,
                            },
                        )?;
                        ActionEnvelopeKind::Action(action)
                    };
                    let line_index = action_list.logical_line_index();
                    let line = &document.source().lines()[line_index];
                    let operand = classify_action_operand(
                        &mut ActionOperandBuild {
                            templates,
                            segments: template_segments,
                            expansions,
                            operands: action_operands,
                            setvars,
                            initcols,
                            ctls,
                            ctl_targets,
                            structured_count: structured_action_count,
                            requirements,
                            unavailable,
                            semantic_charge: semantic_charged_bytes,
                            scanned: scanned_operand_bytes,
                            inherited: *inherited_charged_bytes,
                            limits,
                        },
                        kind,
                        operand,
                        name,
                        line,
                        line_index,
                        location,
                    )?;
                    actions.push(SemanticActionRecord {
                        location,
                        kind,
                        name,
                        operand,
                    });
                }
            }
            directives.push(DirectiveSemanticRecord {
                document_index,
                directive_index,
                target_start,
                target_count: targets.len() - target_start,
                operator: (operators.len() != operator_start).then_some(operator_start),
                action_start,
                action_count: actions.len() - action_start,
                fold: None,
            });
        }
    }
    Ok(())
}

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

fn fold_local_rules(draft: &mut SemanticDraft<'_>, limits: SemanticLimits) -> Result<(), SemanticError> {
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
                None,
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
                Some((&mut draft.effective_transformations, effective_start)),
            )?
            else {
                previous = None;
                mapping_index += 1;
                continue;
            };
            inspection.record.effective_start = effective_start;
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
    syntax: super::SyntaxDirectiveView<'_, '_>,
    mapping: &DirectiveSemanticRecord,
    document_index: usize,
    directive_index: usize,
    previous: Option<FoldAdjacency>,
    mut effective_sink: Option<(&mut Vec<usize>, usize)>,
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
                    if let Some((sink, start)) = effective_sink.as_mut() {
                        sink.truncate(*start);
                    }
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
                    if let Some((sink, _)) = effective_sink.as_mut() {
                        sink.push(index);
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
                        _ => unreachable!(),
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
struct ChainProjection {
    chain_count: usize,
    chain_members: usize,
    limit_position: SourcePosition,
    chain_count_excess: Option<SourcePosition>,
    chain_member_excess: Option<SourcePosition>,
    chain_length_excess: Option<SourcePosition>,
    #[cfg(test)]
    inspected_folds: usize,
}

fn project_chains(
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

fn finalize_graph(draft: &mut SemanticDraft<'_>, limits: SemanticLimits) -> Result<(), SemanticError> {
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
                        FinalPhaseState::Inherited { .. } => unreachable!(),
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

fn location(document: usize, directive: usize, node_kind: SemanticNodeKind, node_index: usize) -> SemanticLocation {
    SemanticLocation {
        document_index: document,
        directive_index: directive,
        node_kind,
        node_index,
    }
}

fn lookup_operator(value: super::DecodedTextView<'_, '_>) -> Option<OperatorKind> {
    [
        OperatorKind::BeginsWith,
        OperatorKind::Contains,
        OperatorKind::DetectSqli,
        OperatorKind::DetectXss,
        OperatorKind::EndsWith,
        OperatorKind::Eq,
        OperatorKind::Ge,
        OperatorKind::Gt,
        OperatorKind::IpMatch,
        OperatorKind::Lt,
        OperatorKind::Pm,
        OperatorKind::PmFromFile,
        OperatorKind::Rx,
        OperatorKind::Streq,
        OperatorKind::UnconditionalMatch,
        OperatorKind::ValidateByteRange,
        OperatorKind::ValidateUrlEncoding,
        OperatorKind::ValidateUtf8Encoding,
        OperatorKind::Within,
    ]
    .into_iter()
    .find(|kind| {
        let canonical = OperatorKind::lookup_ascii_insensitive(kind.as_str().as_bytes())
            .expect("operator registry contains its canonical spelling");
        value.bytes().eq(canonical.as_str().bytes())
    })
}

fn required_operand<'source>(
    operand: Option<SemanticText<'source>>,
    name: SemanticText<'_>,
) -> Result<SemanticText<'source>, SemanticError> {
    match operand {
        Some(value) if !value.text().is_empty() => Ok(value),
        _ => Err(SemanticError::ValueContract {
            expected: ValueContract::Required,
            actual: ValueContract::Forbidden,
            position: name.span().end,
        }),
    }
}

fn forbidden_operand(operand: Option<SemanticText<'_>>, name: SemanticText<'_>) -> Result<(), SemanticError> {
    if operand.is_some() {
        Err(SemanticError::ValueContract {
            expected: ValueContract::Forbidden,
            actual: ValueContract::Required,
            position: name.span().end,
        })
    } else {
        Ok(())
    }
}

fn scalar_bytes<'source>(value: SemanticText<'source>, kind: ScalarKind) -> Result<&'source [u8], SemanticError> {
    if value.text().contains("%{") {
        return Err(SemanticError::MacroScalar {
            kind,
            position: value.span().start,
        });
    }
    if value.text().is_empty() {
        return Err(SemanticError::InvalidScalar {
            kind,
            position: value.span().start,
        });
    }
    Ok(value.text().as_bytes())
}

fn parse_unsigned(value: SemanticText<'_>, kind: ScalarKind) -> Result<u64, SemanticError> {
    let bytes = scalar_bytes(value, kind)?;
    if !bytes.iter().all(u8::is_ascii_digit) {
        return Err(SemanticError::InvalidScalar {
            kind,
            position: value.span().start,
        });
    }
    let mut parsed = 0_u64;
    for byte in bytes {
        parsed = parsed
            .checked_mul(10)
            .and_then(|number| number.checked_add(u64::from(byte - b'0')))
            .ok_or(SemanticError::ScalarOverflow {
                kind,
                position: value.span().start,
            })?;
    }
    Ok(parsed)
}

fn parse_id(value: SemanticText<'_>) -> Result<LocalActionValue, SemanticError> {
    let parsed = parse_unsigned(value, ScalarKind::Id)?;
    if parsed == 0 {
        return Err(SemanticError::InvalidScalar {
            kind: ScalarKind::Id,
            position: value.span().start,
        });
    }
    Ok(LocalActionValue::Id(parsed))
}

fn parse_phase(value: SemanticText<'_>) -> Result<LocalActionValue, SemanticError> {
    let phase = match value.text().as_bytes() {
        b"request" => Phase::RequestBody,
        b"response" => Phase::ResponseBody,
        b"logging" => Phase::Logging,
        _ => match parse_unsigned(value, ScalarKind::Phase)? {
            1 => Phase::RequestHeaders,
            2 => Phase::RequestBody,
            3 => Phase::ResponseHeaders,
            4 => Phase::ResponseBody,
            5 => Phase::Logging,
            _ => {
                return Err(SemanticError::InvalidScalar {
                    kind: ScalarKind::Phase,
                    position: value.span().start,
                })
            }
        },
    };
    Ok(LocalActionValue::Phase(phase))
}

fn parse_severity(value: SemanticText<'_>) -> Result<LocalActionValue, SemanticError> {
    let bytes = scalar_bytes(value, ScalarKind::Severity)?;
    let severity = if bytes.len() == 1 && matches!(bytes[0], b'0'..=b'7') {
        severity_from_number(bytes[0] - b'0')
    } else {
        [
            Severity::Emergency,
            Severity::Alert,
            Severity::Critical,
            Severity::Error,
            Severity::Warning,
            Severity::Notice,
            Severity::Info,
            Severity::Debug,
        ]
        .into_iter()
        .find(|severity| bytes.eq_ignore_ascii_case(severity_name(*severity).as_bytes()))
        .ok_or(SemanticError::InvalidScalar {
            kind: ScalarKind::Severity,
            position: value.span().start,
        })?
    };
    Ok(LocalActionValue::Severity(severity))
}

fn severity_from_number(value: u8) -> Severity {
    match value {
        0 => Severity::Emergency,
        1 => Severity::Alert,
        2 => Severity::Critical,
        3 => Severity::Error,
        4 => Severity::Warning,
        5 => Severity::Notice,
        6 => Severity::Info,
        _ => Severity::Debug,
    }
}

fn severity_name(value: Severity) -> &'static str {
    match value {
        Severity::Emergency => "emergency",
        Severity::Alert => "alert",
        Severity::Critical => "critical",
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Notice => "notice",
        Severity::Info => "info",
        Severity::Debug => "debug",
    }
}

fn parse_status(value: SemanticText<'_>) -> Result<LocalActionValue, SemanticError> {
    let parsed = parse_unsigned(value, ScalarKind::Status)?;
    if !(200..=599).contains(&parsed) {
        return Err(SemanticError::InvalidScalar {
            kind: ScalarKind::Status,
            position: value.span().start,
        });
    }
    Ok(LocalActionValue::Status(parsed as u16))
}

fn parse_skip(value: SemanticText<'_>) -> Result<LocalActionValue, SemanticError> {
    let parsed = parse_unsigned(value, ScalarKind::Skip)?;
    if parsed == 0 {
        return Err(SemanticError::InvalidScalar {
            kind: ScalarKind::Skip,
            position: value.span().start,
        });
    }
    let parsed = u32::try_from(parsed).map_err(|_| SemanticError::ScalarOverflow {
        kind: ScalarKind::Skip,
        position: value.span().start,
    })?;
    Ok(LocalActionValue::Skip(parsed))
}

fn push_capability(
    records: &mut Vec<CapabilityRecord>,
    semantic_charged_bytes: &mut usize,
    inherited_charged_bytes: usize,
    location: SemanticLocation,
    capability: UnavailableCapability,
    limits: SemanticLimits,
    position: SourcePosition,
) -> Result<(), SemanticError> {
    charge_dynamic::<CapabilityRecord>(semantic_charged_bytes, inherited_charged_bytes, limits, position)?;
    records.push(CapabilityRecord { location, capability });
    Ok(())
}

fn charge_dynamic<T>(
    semantic_charged_bytes: &mut usize,
    inherited_charged_bytes: usize,
    limits: SemanticLimits,
    position: SourcePosition,
) -> Result<(), SemanticError> {
    let semantic = (*semantic_charged_bytes)
        .checked_add(size_of::<T>())
        .ok_or(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position,
        })?;
    if semantic > limits.max_owned_bytes {
        return Err(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position,
        });
    }
    let retained = inherited_charged_bytes
        .checked_add(semantic)
        .ok_or(SemanticError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        })?;
    if retained > limits.max_retained_bytes {
        return Err(SemanticError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        });
    }
    *semantic_charged_bytes = semantic;
    Ok(())
}

const NO_INDEX: usize = usize::MAX;

#[derive(Debug, Clone, Copy, Default)]
struct ExceptionCounters {
    raw: usize,
    distinct: usize,
    selector_work: usize,
    comparisons: usize,
    comparison_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExceptionKind {
    Update,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnavailableRulePolicy {
    Reject,
    ExcludeRuleUnits,
}

pub fn validate_semantic_draft<'source>(
    draft: SemanticDraft<'source>,
    context: SemanticValidationContext,
    limits: ValidationLimits,
) -> Result<ValidatedSemanticDocument<'source>, SemanticValidationError> {
    validate_semantic_draft_inner(draft, context, limits, UnavailableRulePolicy::Reject)
}

pub(crate) fn validate_executable_profile<'source>(
    draft: SemanticDraft<'source>,
    context: SemanticValidationContext,
    limits: ValidationLimits,
) -> Result<ValidatedSemanticDocument<'source>, SemanticValidationError> {
    validate_semantic_draft_inner(draft, context, limits, UnavailableRulePolicy::ExcludeRuleUnits)
}

fn validate_semantic_draft_inner<'source>(
    draft: SemanticDraft<'source>,
    context: SemanticValidationContext,
    limits: ValidationLimits,
    unavailable_policy: UnavailableRulePolicy,
) -> Result<ValidatedSemanticDocument<'source>, SemanticValidationError> {
    let rule_units = draft
        .final_rules
        .iter()
        .filter(|rule| !matches!(rule.role, FinalRuleRole::ChainChild { .. }))
        .count();
    if rule_units > limits.max_rule_units {
        return Err(SemanticValidationError::RuleUnitLimit {
            limit: limits.max_rule_units,
        });
    }
    let id_selectors = draft
        .documents
        .iter()
        .try_fold(0_usize, |count, document| {
            count.checked_add(document.id_selector_count())
        })
        .ok_or(SemanticValidationError::IdSelectorLimit {
            limit: limits.max_id_selectors,
        })?;
    if id_selectors > limits.max_id_selectors {
        return Err(SemanticValidationError::IdSelectorLimit {
            limit: limits.max_id_selectors,
        });
    }
    let (exception_count, update_count, removal_count, overlay_bound) =
        validation_exception_shape(&draft, rule_units, limits)?;
    let nodes = draft.final_rules.len();
    let targets = draft.targets.len();
    let distinct_bound = rule_units.min(limits.max_distinct_matches_per_directive);
    let working = validation_working_bytes(
        rule_units,
        nodes,
        targets,
        distinct_bound,
        overlay_bound,
        unavailable_policy == UnavailableRulePolicy::ExcludeRuleUnits,
        limits,
    )?;
    if working > limits.max_working_bytes {
        return Err(SemanticValidationError::WorkingByteLimit {
            limit: limits.max_working_bytes,
        });
    }

    let mut id_index = Vec::new();
    let mut active = Vec::new();
    let mut states = Vec::new();
    let mut overlay_heads = Vec::new();
    let mut overlay_tails = Vec::new();
    let mut final_counts = Vec::new();
    let mut overlays = Vec::new();
    let mut seen = Vec::new();
    let mut affected = Vec::new();
    let mut target_seen = Vec::new();
    let mut regex_targets = Vec::new();
    let mut fold_units = Vec::new();
    let mut compatibility_excluded = Vec::new();
    reserve_validation(&mut id_index, rule_units, "rule ID index")?;
    reserve_validation(&mut active, rule_units, "active rule state")?;
    reserve_validation(&mut states, nodes, "resolved rule states")?;
    reserve_validation(&mut overlay_heads, nodes, "target overlay heads")?;
    reserve_validation(&mut overlay_tails, nodes, "target overlay tails")?;
    reserve_validation(&mut final_counts, nodes, "final target counts")?;
    reserve_validation(&mut overlays, overlay_bound, "target overlays")?;
    reserve_validation(&mut seen, rule_units, "selector generations")?;
    reserve_validation(&mut affected, distinct_bound, "affected rule candidates")?;
    reserve_validation(&mut target_seen, targets, "target requirement generations")?;
    reserve_validation(&mut regex_targets, targets, "target regex candidates")?;
    if unavailable_policy == UnavailableRulePolicy::ExcludeRuleUnits {
        reserve_validation(&mut fold_units, draft.folds.len(), "capability fold attribution")?;
        reserve_validation(&mut compatibility_excluded, rule_units, "compatibility-excluded units")?;
        fold_units.resize(draft.folds.len(), NO_INDEX);
        compatibility_excluded.resize(rule_units, 0_u8);
    }
    active.resize(rule_units, 1_u8);
    overlay_heads.resize(nodes, NO_INDEX);
    overlay_tails.resize(nodes, NO_INDEX);
    seen.resize(rule_units, 0_u32);
    target_seen.resize(targets, 0_u32);

    for (node, rule) in draft.final_rules.iter().copied().enumerate() {
        if !matches!(rule.role, FinalRuleRole::ChainChild { .. }) {
            id_index.push(RuleIdIndexRecord {
                id: draft.folds[rule.fold].id.expect("finalized rule unit has an ID"),
                unit: node,
            });
        }
    }
    id_index.sort_unstable_by_key(|entry| entry.id);
    reject_duplicate_ids(&draft, &id_index)?;
    resolve_rule_states(&draft, context, &mut states)?;

    let mut unit = 0_usize;
    let mut current_unit = NO_INDEX;
    let mut total_final_targets = 0_usize;
    for (node, state) in states.iter_mut().enumerate() {
        if !matches!(draft.final_rules[node].role, FinalRuleRole::ChainChild { .. }) {
            state.unit = Some(unit);
            current_unit = unit;
            unit += 1;
        }
        if !fold_units.is_empty() {
            fold_units[draft.final_rules[node].fold] = current_unit;
        }
        let mapping = semantic_mapping_for_rule(&draft, draft.final_rules[node]);
        let count = mapping.target_count;
        let location = draft.folds[draft.final_rules[node].fold].location;
        if count > limits.max_final_targets_per_rule {
            return Err(SemanticValidationError::FinalTargetLimit {
                limit: limits.max_final_targets_per_rule,
                location,
            });
        }
        total_final_targets =
            total_final_targets
                .checked_add(count)
                .ok_or(SemanticValidationError::FinalTargetReferenceLimit {
                    limit: limits.max_final_target_references,
                    location,
                })?;
        if total_final_targets > limits.max_final_target_references {
            return Err(SemanticValidationError::FinalTargetReferenceLimit {
                limit: limits.max_final_target_references,
                location,
            });
        }
        for target in &draft.targets[mapping.target_start..mapping.target_start + mapping.target_count] {
            validate_target(target, state.phase)?;
        }
        final_counts.push(count);
    }

    let mut counters = ExceptionCounters::default();
    let mut affected_total = 0_usize;
    let mut directive_ordinal = 0_u32;
    for mapping in &draft.directives {
        let syntax = draft.documents[mapping.document_index]
            .get(mapping.directive_index)
            .expect("validated semantic directive mapping");
        let (kind, ids, update_targets) = match syntax.targets() {
            DirectiveTargetView::Update { ids, .. } => (
                ExceptionKind::Update,
                ids,
                Some(mapping.target_start..mapping.target_start + mapping.target_count),
            ),
            DirectiveTargetView::Remove { ids } => (ExceptionKind::Remove, ids, None),
            DirectiveTargetView::None | DirectiveTargetView::Rule { .. } => continue,
        };
        directive_ordinal += 1;
        let location = DirectiveLocation {
            document_index: mapping.document_index,
            directive_index: mapping.directive_index,
        };
        affected.clear();
        let local = resolve_exception_union(
            &draft,
            &id_index,
            &states,
            ids,
            location,
            directive_ordinal,
            &mut seen,
            &mut affected,
            counters,
            limits,
        )?;
        match kind {
            ExceptionKind::Update => {
                if affected
                    .iter()
                    .any(|root| active[states[*root].unit.expect("affected root has unit")] == 0)
                {
                    return Err(SemanticValidationError::InactiveUpdate { location });
                }
            }
            ExceptionKind::Remove => {
                affected.retain(|root| active[states[*root].unit.expect("affected root has unit")] != 0);
                if affected.is_empty() {
                    return Err(SemanticValidationError::EmptyRemoval { location });
                }
            }
        }
        if let Some(target_range) = update_targets {
            let comparison_delta = validate_update_candidate(
                &draft,
                &states,
                &affected,
                target_range.clone(),
                location,
                &final_counts,
                &overlay_heads,
                &overlays,
                counters,
                limits,
                total_final_targets,
            )?;
            let added = target_range
                .len()
                .checked_mul(affected.len())
                .expect("validated update projection remains representable");
            counters.raw = counters
                .raw
                .checked_add(local.raw)
                .expect("validated raw count remains representable");
            counters.distinct = counters
                .distinct
                .checked_add(local.distinct)
                .expect("validated distinct count remains representable");
            counters.selector_work = counters
                .selector_work
                .checked_add(local.selector_work)
                .expect("validated selector work remains representable");
            counters.comparisons = counters
                .comparisons
                .checked_add(comparison_delta.comparisons)
                .expect("validated comparison count remains representable");
            counters.comparison_bytes = counters
                .comparison_bytes
                .checked_add(comparison_delta.comparison_bytes)
                .expect("validated comparison bytes remain representable");
            for &matched in &affected {
                let root = matched;
                for target in target_range.clone() {
                    let index = overlays.len();
                    overlays.push(TargetOverlayEntry { target, next: NO_INDEX });
                    if overlay_heads[root] == NO_INDEX {
                        overlay_heads[root] = index;
                    } else {
                        overlays[overlay_tails[root]].next = index;
                    }
                    overlay_tails[root] = index;
                }
                final_counts[root] = final_counts[root]
                    .checked_add(target_range.len())
                    .expect("validated per-rule target count remains representable");
            }
            total_final_targets = total_final_targets
                .checked_add(added)
                .expect("validated target reference count remains representable");
        } else {
            counters.raw = counters
                .raw
                .checked_add(local.raw)
                .expect("validated raw count remains representable");
            counters.distinct = counters
                .distinct
                .checked_add(local.distinct)
                .expect("validated distinct count remains representable");
            counters.selector_work = counters
                .selector_work
                .checked_add(local.selector_work)
                .expect("validated selector work remains representable");
            for &matched in &affected {
                active[states[matched].unit.expect("affected root has unit")] = 0;
            }
        }
        affected_total = affected_total
            .checked_add(affected.len())
            .ok_or(SemanticValidationError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
            })?;
    }

    if unavailable_policy == UnavailableRulePolicy::Reject {
        if let Some(record) = draft.unavailable.first() {
            return Err(SemanticValidationError::UnavailableCapability {
                capability: record.capability,
                location: record.location,
            });
        }
    } else {
        for record in &draft.unavailable {
            let unit = capability_unit(&draft, &fold_units, record.location).ok_or(
                SemanticValidationError::UnavailableCapability {
                    capability: record.capability,
                    location: record.location,
                },
            )?;
            compatibility_excluded[unit] = 1;
            active[unit] = 0;
        }
    }

    mark_reachable_regex_targets(
        &draft,
        &states,
        &active,
        &overlay_heads,
        &overlays,
        &mut target_seen,
        &mut regex_targets,
    );
    let draft_requirement_count = draft
        .requirements
        .iter()
        .filter(|record| {
            !is_default_requirement(record.requirement)
                && !requirement_is_excluded(&draft, &fold_units, &compatibility_excluded, record.location)
        })
        .count();
    let requirement_count =
        draft_requirement_count
            .checked_add(regex_targets.len())
            .ok_or(SemanticValidationError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
            })?;
    let validation_charge = validation_owned_bytes(
        nodes,
        rule_units,
        total_final_targets,
        update_count,
        removal_count,
        affected_total,
        requirement_count,
        limits,
    )?;
    if validation_charge > limits.max_owned_bytes {
        return Err(SemanticValidationError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
        });
    }
    if draft
        .retained_charged_bytes()
        .checked_add(validation_charge)
        .is_none_or(|value| value > limits.max_retained_bytes)
    {
        return Err(SemanticValidationError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        });
    }

    let mut validated = ValidatedSemanticDocument {
        draft,
        nodes: Vec::new(),
        units: Vec::new(),
        id_index: Vec::new(),
        final_targets: Vec::new(),
        updates: Vec::new(),
        removals: Vec::new(),
        affected_units: Vec::new(),
        requirements: Vec::new(),
        validation_charged_bytes: validation_charge,
    };
    reserve_validation(&mut validated.nodes, nodes, "validated rule nodes")?;
    reserve_validation(&mut validated.units, rule_units, "validated rule units")?;
    reserve_validation(&mut validated.id_index, rule_units, "validated rule ID index")?;
    reserve_validation(
        &mut validated.final_targets,
        total_final_targets,
        "final target references",
    )?;
    reserve_validation(&mut validated.updates, update_count, "applied target updates")?;
    reserve_validation(&mut validated.removals, removal_count, "applied rule removals")?;
    reserve_validation(&mut validated.affected_units, affected_total, "applied exception units")?;
    reserve_validation(&mut validated.requirements, requirement_count, "validated requirements")?;
    commit_validated_nodes(&mut validated, &states, &overlay_heads, &overlays);
    for (node, rule) in validated.draft.final_rules.iter().copied().enumerate() {
        if matches!(rule.role, FinalRuleRole::ChainChild { .. }) {
            continue;
        }
        let fold = validated.draft.folds[rule.fold];
        let unit = states[node].unit.expect("root state retains unit index");
        debug_assert_eq!(unit, validated.units.len());
        validated.units.push(ValidatedRuleUnitRecord {
            id: fold.id.expect("validated unit ID"),
            root: node,
            active: active[unit] != 0,
            compatibility_excluded: compatibility_excluded.get(unit).is_some_and(|value| *value != 0),
        });
    }
    commit_applied_exceptions(
        &mut validated,
        &states,
        &id_index,
        limits,
        &mut active,
        &mut seen,
        &mut affected,
    )?;
    for mut entry in id_index {
        entry.unit = states[entry.unit].unit.expect("ID root maps to a unit");
        validated.id_index.push(entry);
    }
    for (index, record) in validated.draft.requirements.iter().enumerate() {
        if !is_default_requirement(record.requirement)
            && !requirement_is_excluded(&validated.draft, &fold_units, &compatibility_excluded, record.location)
        {
            validated.requirements.push(ValidatedRequirementReference::Draft(index));
        }
    }
    for target in regex_targets {
        validated
            .requirements
            .push(ValidatedRequirementReference::TargetSelectorRegex { target });
    }
    debug_assert_eq!(exception_count, update_count + removal_count);
    Ok(validated)
}

fn validation_exception_shape(
    draft: &SemanticDraft<'_>,
    units: usize,
    limits: ValidationLimits,
) -> Result<(usize, usize, usize, usize), SemanticValidationError> {
    let mut exceptions = 0_usize;
    let mut updates = 0_usize;
    let mut removals = 0_usize;
    let mut sum = 0_usize;
    let mut maximum = 0_usize;
    let distinct = units.min(limits.max_distinct_matches_per_directive);
    for document in &draft.documents {
        for directive in document.iter() {
            match directive.targets() {
                DirectiveTargetView::Update { atoms, .. } => {
                    exceptions = exceptions
                        .checked_add(1)
                        .ok_or(SemanticValidationError::ExceptionLimit {
                            limit: limits.max_applied_exceptions,
                        })?;
                    updates += 1;
                    maximum = maximum.max(atoms.len());
                    sum = sum
                        .checked_add(atoms.len().checked_mul(distinct).ok_or(
                            SemanticValidationError::WorkingByteLimit {
                                limit: limits.max_working_bytes,
                            },
                        )?)
                        .ok_or(SemanticValidationError::WorkingByteLimit {
                            limit: limits.max_working_bytes,
                        })?;
                }
                DirectiveTargetView::Remove { .. } => {
                    exceptions = exceptions
                        .checked_add(1)
                        .ok_or(SemanticValidationError::ExceptionLimit {
                            limit: limits.max_applied_exceptions,
                        })?;
                    removals += 1;
                }
                DirectiveTargetView::None | DirectiveTargetView::Rule { .. } => {}
            }
        }
    }
    let limit = limits.max_applied_exceptions.min(u32::MAX as usize);
    if exceptions > limit {
        return Err(SemanticValidationError::ExceptionLimit { limit });
    }
    let cumulative =
        maximum
            .checked_mul(limits.max_distinct_matches)
            .ok_or(SemanticValidationError::WorkingByteLimit {
                limit: limits.max_working_bytes,
            })?;
    Ok((exceptions, updates, removals, sum.min(cumulative)))
}

fn capability_unit(draft: &SemanticDraft<'_>, fold_units: &[usize], location: SemanticLocation) -> Option<usize> {
    let mapping = draft
        .directives
        .binary_search_by_key(&(location.document_index, location.directive_index), |record| {
            (record.document_index, record.directive_index)
        })
        .ok()
        .and_then(|index| draft.directives.get(index))?;
    let fold = mapping.fold?;
    let unit = *fold_units.get(fold)?;
    (unit != NO_INDEX).then_some(unit)
}

fn requirement_is_excluded(
    draft: &SemanticDraft<'_>,
    fold_units: &[usize],
    excluded: &[u8],
    location: SemanticLocation,
) -> bool {
    capability_unit(draft, fold_units, location)
        .and_then(|unit| excluded.get(unit))
        .is_some_and(|value| *value != 0)
}

fn validation_working_bytes(
    units: usize,
    rules: usize,
    targets: usize,
    distinct_bound: usize,
    overlay_bound: usize,
    attribute_capabilities: bool,
    limits: ValidationLimits,
) -> Result<usize, SemanticValidationError> {
    let mut total = 0_usize;
    for (count, size) in [
        (units, size_of::<RuleIdIndexRecord>()),
        (units, size_of::<u8>()),
        (rules, size_of::<ResolvedRuleStateRecord>()),
        (rules, 2 * size_of::<usize>()),
        (rules, size_of::<usize>()),
        (overlay_bound, size_of::<TargetOverlayEntry>()),
        (units, size_of::<u32>()),
        (distinct_bound, size_of::<usize>()),
        (targets, size_of::<u32>()),
        (targets, size_of::<usize>()),
        (usize::from(attribute_capabilities) * rules, size_of::<usize>()),
        (usize::from(attribute_capabilities) * units, size_of::<u8>()),
    ] {
        total = total
            .checked_add(
                count
                    .checked_mul(size)
                    .ok_or(SemanticValidationError::WorkingByteLimit {
                        limit: limits.max_working_bytes,
                    })?,
            )
            .ok_or(SemanticValidationError::WorkingByteLimit {
                limit: limits.max_working_bytes,
            })?;
    }
    Ok(total)
}

fn reserve_validation<T>(
    values: &mut Vec<T>,
    count: usize,
    operation: &'static str,
) -> Result<(), SemanticValidationError> {
    #[cfg(test)]
    if VALIDATION_ALLOCATION_FAILURE.with(|failure| failure.get() == Some(operation)) {
        return Err(SemanticValidationError::Allocation { operation });
    }
    values
        .try_reserve_exact(count)
        .map_err(|_| SemanticValidationError::Allocation { operation })
}

#[cfg(test)]
std::thread_local! {
    static VALIDATION_ALLOCATION_FAILURE: std::cell::Cell<Option<&'static str>> = const {
        std::cell::Cell::new(None)
    };
}

fn reject_duplicate_ids(draft: &SemanticDraft<'_>, index: &[RuleIdIndexRecord]) -> Result<(), SemanticValidationError> {
    let mut duplicate = None;
    let mut cursor = 0_usize;
    while cursor < index.len() {
        let mut end = cursor + 1;
        while end < index.len() && index[end].id == index[cursor].id {
            end += 1;
        }
        if end - cursor > 1 {
            let mut first = None;
            let mut second = None;
            for entry in &index[cursor..end] {
                let location = draft.folds[draft.final_rules[entry.unit].fold].location;
                if first.is_none_or(|value| location < value) {
                    second = first;
                    first = Some(location);
                } else if second.is_none_or(|value| location < value) {
                    second = Some(location);
                }
            }
            let candidate = (first.expect("duplicate group"), second.expect("duplicate group"));
            if duplicate.is_none_or(|(_, current)| candidate.1 < current) {
                duplicate = Some(candidate);
            }
        }
        cursor = end;
    }
    if let Some((first, second)) = duplicate {
        Err(SemanticValidationError::DuplicateRuleId { first, second })
    } else {
        Ok(())
    }
}

fn resolve_rule_states(
    draft: &SemanticDraft<'_>,
    context: SemanticValidationContext,
    states: &mut Vec<ResolvedRuleStateRecord>,
) -> Result<(), SemanticValidationError> {
    let mut inherited: Option<ResolvedRuleStateRecord> = None;
    for rule in draft.final_rules.iter().copied() {
        let fold = draft.folds[rule.fold];
        let state = if matches!(rule.role, FinalRuleRole::ChainChild { .. }) {
            let mut value = inherited.expect("validated chain child follows its root");
            value.unit = None;
            value
        } else {
            let phase = match rule.phase {
                FinalPhaseState::Explicit { phase, .. } | FinalPhaseState::Inherited { phase, .. } => phase,
                FinalPhaseState::RequiresDefault { owner } => context
                    .implicit_rule_phase()
                    .ok_or(SemanticValidationError::MissingImplicitPhase { location: owner })?,
            };
            let defaults = context
                .phase_defaults(phase)
                .ok_or(SemanticValidationError::MissingPhaseDefaults {
                    phase,
                    location: fold.location,
                })?;
            let (disruptive, status) = match fold.disruptive.expect("root fold has disruptive behavior") {
                DisruptiveKind::Pass => (ResolvedDisruptive::Pass, None),
                DisruptiveKind::Deny => {
                    let default_status = match defaults.disruptive() {
                        DefaultDisruptive::Pass => None,
                        DefaultDisruptive::Deny { status } => Some(status),
                    };
                    (
                        ResolvedDisruptive::Deny,
                        Some(fold.status.or(default_status).unwrap_or(403)),
                    )
                }
                DisruptiveKind::Block => match defaults.disruptive() {
                    DefaultDisruptive::Pass => {
                        if let Some(action) = fold.status_action {
                            return Err(SemanticValidationError::BlockStatusPass {
                                location: draft.actions[action].location,
                            });
                        }
                        (ResolvedDisruptive::Pass, None)
                    }
                    DefaultDisruptive::Deny { status } => {
                        (ResolvedDisruptive::Deny, Some(fold.status.unwrap_or(status)))
                    }
                },
            };
            ResolvedRuleStateRecord {
                phase,
                disruptive,
                status,
                log: fold.log.unwrap_or(defaults.log()),
                audit_log: fold.audit_log.unwrap_or(defaults.audit_log()),
                unit: None,
            }
        };
        if !matches!(rule.role, FinalRuleRole::ChainChild { .. }) {
            inherited = Some(state);
        }
        states.push(state);
    }
    Ok(())
}

fn semantic_mapping_for_rule<'a>(draft: &'a SemanticDraft<'_>, rule: FinalRuleRecord) -> &'a DirectiveSemanticRecord {
    let location = draft.folds[rule.fold].location;
    let index = draft
        .directives
        .binary_search_by_key(&(location.document_index, location.directive_index), |record| {
            (record.document_index, record.directive_index)
        })
        .expect("final rule location has semantic directive mapping");
    &draft.directives[index]
}

fn validate_target(target: &SemanticTarget<'_>, phase: Phase) -> Result<(), SemanticValidationError> {
    let minimum = minimum_collection_phase(target.collection);
    if phase != Phase::Logging && (phase as u8) < (minimum as u8) {
        return Err(SemanticValidationError::CollectionPhase {
            collection: target.collection,
            phase,
            location: target.location,
        });
    }
    if target.selector_kind != SelectorKind::None && !collection_accepts_selector(target.collection) {
        return Err(SemanticValidationError::SelectorNotAllowed {
            collection: target.collection,
            selector: target.selector_kind,
            location: target.location,
        });
    }
    if target.selector_kind == SelectorKind::Path && target.collection != CollectionKind::Xml {
        return Err(SemanticValidationError::PathSelectorCollection {
            location: target.location,
        });
    }
    Ok(())
}

fn minimum_collection_phase(collection: CollectionKind) -> Phase {
    match collection {
        CollectionKind::Args
        | CollectionKind::ArgsNames
        | CollectionKind::Files
        | CollectionKind::FilesCombinedSize
        | CollectionKind::FilesNames
        | CollectionKind::MultipartPartHeaders
        | CollectionKind::RequestBody
        | CollectionKind::RequestBodyLength
        | CollectionKind::Xml => Phase::RequestBody,
        CollectionKind::ResponseHeaders | CollectionKind::ResponseStatus => Phase::ResponseHeaders,
        CollectionKind::ResponseBody => Phase::ResponseBody,
        _ => Phase::RequestHeaders,
    }
}

fn collection_accepts_selector(collection: CollectionKind) -> bool {
    matches!(
        collection,
        CollectionKind::Args
            | CollectionKind::ArgsGet
            | CollectionKind::ArgsGetNames
            | CollectionKind::ArgsNames
            | CollectionKind::Files
            | CollectionKind::FilesNames
            | CollectionKind::MatchedVars
            | CollectionKind::MultipartPartHeaders
            | CollectionKind::RequestCookies
            | CollectionKind::RequestCookiesNames
            | CollectionKind::RequestHeaders
            | CollectionKind::RequestHeadersNames
            | CollectionKind::ResponseHeaders
            | CollectionKind::Tx
            | CollectionKind::Xml
    )
}

fn is_default_requirement(requirement: CompileRequirement) -> bool {
    matches!(
        requirement,
        CompileRequirement::BlockDefaultDisruptive | CompileRequirement::DefaultPhase
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_exception_union(
    draft: &SemanticDraft<'_>,
    index: &[RuleIdIndexRecord],
    states: &[ResolvedRuleStateRecord],
    selectors: &[IdSelector<'_>],
    directive: DirectiveLocation,
    generation: u32,
    seen: &mut [u32],
    affected: &mut Vec<usize>,
    cumulative: ExceptionCounters,
    limits: ValidationLimits,
) -> Result<ExceptionCounters, SemanticValidationError> {
    let mut local = ExceptionCounters::default();
    let lookup_work = ceil_log2(index.len().saturating_add(1));
    for selector in selectors {
        let position = selector.span().start;
        let (start, end, base_work) = match selector.kind() {
            IdSelectorKind::Single(id) => {
                let start = index.partition_point(|entry| entry.id < id);
                let end = index.partition_point(|entry| entry.id <= id);
                (start, end, lookup_work)
            }
            IdSelectorKind::Range { start, end } => {
                if start > end {
                    return Err(SemanticValidationError::ReversedIdRange { position });
                }
                (
                    index.partition_point(|entry| entry.id < start),
                    index.partition_point(|entry| entry.id <= end),
                    lookup_work.saturating_mul(2),
                )
            }
        };
        if start == end {
            return Err(SemanticValidationError::UnresolvedIdSelector { position });
        }
        for entry in &index[start..end] {
            let location = draft.folds[draft.final_rules[entry.unit].fold].location;
            if location >= directive {
                return Err(SemanticValidationError::ForwardIdSelector { position });
            }
        }
        local.selector_work =
            local
                .selector_work
                .checked_add(base_work)
                .ok_or(SemanticValidationError::SelectorWorkLimit {
                    limit: limits.max_selector_work,
                    position,
                })?;
        if cumulative
            .selector_work
            .checked_add(local.selector_work)
            .is_none_or(|value| value > limits.max_selector_work)
        {
            return Err(SemanticValidationError::SelectorWorkLimit {
                limit: limits.max_selector_work,
                position,
            });
        }
        for entry in &index[start..end] {
            local.raw = local
                .raw
                .checked_add(1)
                .ok_or(SemanticValidationError::RawMatchDirectiveLimit {
                    limit: limits.max_raw_matches_per_directive,
                    position,
                })?;
            if local.raw > limits.max_raw_matches_per_directive {
                return Err(SemanticValidationError::RawMatchDirectiveLimit {
                    limit: limits.max_raw_matches_per_directive,
                    position,
                });
            }
            if cumulative
                .raw
                .checked_add(local.raw)
                .is_none_or(|value| value > limits.max_raw_matches)
            {
                return Err(SemanticValidationError::RawMatchLimit {
                    limit: limits.max_raw_matches,
                    position,
                });
            }
            local.selector_work =
                local
                    .selector_work
                    .checked_add(1)
                    .ok_or(SemanticValidationError::SelectorWorkLimit {
                        limit: limits.max_selector_work,
                        position,
                    })?;
            let unit = states[entry.unit].unit.expect("indexed root has unit");
            if seen[unit] != generation {
                local.distinct =
                    local
                        .distinct
                        .checked_add(1)
                        .ok_or(SemanticValidationError::DistinctMatchDirectiveLimit {
                            limit: limits.max_distinct_matches_per_directive,
                            position,
                        })?;
                if local.distinct > limits.max_distinct_matches_per_directive {
                    return Err(SemanticValidationError::DistinctMatchDirectiveLimit {
                        limit: limits.max_distinct_matches_per_directive,
                        position,
                    });
                }
                if cumulative
                    .distinct
                    .checked_add(local.distinct)
                    .is_none_or(|value| value > limits.max_distinct_matches)
                {
                    return Err(SemanticValidationError::DistinctMatchLimit {
                        limit: limits.max_distinct_matches,
                        position,
                    });
                }
                local.selector_work =
                    local
                        .selector_work
                        .checked_add(1)
                        .ok_or(SemanticValidationError::SelectorWorkLimit {
                            limit: limits.max_selector_work,
                            position,
                        })?;
                if cumulative
                    .selector_work
                    .checked_add(local.selector_work)
                    .is_none_or(|value| value > limits.max_selector_work)
                {
                    return Err(SemanticValidationError::SelectorWorkLimit {
                        limit: limits.max_selector_work,
                        position,
                    });
                }
                seen[unit] = generation;
                affected.push(entry.unit);
            }
            if cumulative
                .selector_work
                .checked_add(local.selector_work)
                .is_none_or(|value| value > limits.max_selector_work)
            {
                return Err(SemanticValidationError::SelectorWorkLimit {
                    limit: limits.max_selector_work,
                    position,
                });
            }
        }
    }
    Ok(local)
}

const fn ceil_log2(value: usize) -> usize {
    if value <= 1 {
        0
    } else {
        usize::BITS as usize - (value - 1).leading_zeros() as usize
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_update_candidate(
    draft: &SemanticDraft<'_>,
    states: &[ResolvedRuleStateRecord],
    affected: &[usize],
    targets: Range<usize>,
    directive: DirectiveLocation,
    final_counts: &[usize],
    overlay_heads: &[usize],
    overlays: &[TargetOverlayEntry],
    cumulative: ExceptionCounters,
    limits: ValidationLimits,
    total_final_targets: usize,
) -> Result<ExceptionCounters, SemanticValidationError> {
    let added =
        targets
            .len()
            .checked_mul(affected.len())
            .ok_or(SemanticValidationError::FinalTargetReferenceLimit {
                limit: limits.max_final_target_references,
                location: directive,
            })?;
    if total_final_targets
        .checked_add(added)
        .is_none_or(|value| value > limits.max_final_target_references)
    {
        return Err(SemanticValidationError::FinalTargetReferenceLimit {
            limit: limits.max_final_target_references,
            location: directive,
        });
    }
    for &root in affected {
        if matches!(draft.final_rules[root].role, FinalRuleRole::Action) {
            return Err(SemanticValidationError::SecActionTargetUpdate { location: directive });
        }
        if final_counts[root]
            .checked_add(targets.len())
            .is_none_or(|value| value > limits.max_final_targets_per_rule)
        {
            return Err(SemanticValidationError::FinalTargetLimit {
                limit: limits.max_final_targets_per_rule,
                location: directive,
            });
        }
    }
    let mut delta = ExceptionCounters::default();
    for target_index in targets.clone() {
        let target = &draft.targets[target_index];
        for &root in affected {
            validate_target(target, states[root].phase)?;
            if target.modifier == TargetModifier::Exclude
                && !has_exclusion_base(
                    draft,
                    root,
                    target,
                    targets.start..target_index,
                    overlay_heads,
                    overlays,
                    cumulative,
                    &mut delta,
                    limits,
                )?
            {
                return Err(SemanticValidationError::MissingExclusionBase {
                    location: target.location,
                });
            }
        }
    }
    Ok(delta)
}

#[allow(clippy::too_many_arguments)]
fn has_exclusion_base(
    draft: &SemanticDraft<'_>,
    root: usize,
    candidate: &SemanticTarget<'_>,
    candidate_prefix: Range<usize>,
    overlay_heads: &[usize],
    overlays: &[TargetOverlayEntry],
    cumulative: ExceptionCounters,
    delta: &mut ExceptionCounters,
    limits: ValidationLimits,
) -> Result<bool, SemanticValidationError> {
    let mapping = semantic_mapping_for_rule(draft, draft.final_rules[root]);
    for base in &draft.targets[mapping.target_start..mapping.target_start + mapping.target_count] {
        if target_base_matches(base, candidate, cumulative, delta, limits)? {
            return Ok(true);
        }
    }
    let mut overlay = overlay_heads[root];
    while overlay != NO_INDEX {
        let entry = overlays[overlay];
        if target_base_matches(&draft.targets[entry.target], candidate, cumulative, delta, limits)? {
            return Ok(true);
        }
        overlay = entry.next;
    }
    for index in candidate_prefix {
        if target_base_matches(&draft.targets[index], candidate, cumulative, delta, limits)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn target_base_matches(
    base: &SemanticTarget<'_>,
    candidate: &SemanticTarget<'_>,
    cumulative: ExceptionCounters,
    delta: &mut ExceptionCounters,
    limits: ValidationLimits,
) -> Result<bool, SemanticValidationError> {
    if base.modifier != TargetModifier::None || base.collection != candidate.collection {
        return Ok(false);
    }
    delta.comparisons = delta
        .comparisons
        .checked_add(1)
        .ok_or(SemanticValidationError::TargetComparisonLimit {
            limit: limits.max_target_comparisons,
            location: candidate.location,
        })?;
    if cumulative
        .comparisons
        .checked_add(delta.comparisons)
        .is_none_or(|value| value > limits.max_target_comparisons)
    {
        return Err(SemanticValidationError::TargetComparisonLimit {
            limit: limits.max_target_comparisons,
            location: candidate.location,
        });
    }
    if base.selector_kind == SelectorKind::None {
        return Ok(true);
    }
    if base.selector_kind != candidate.selector_kind {
        return Ok(false);
    }
    let (left, right) = match base.selector_kind {
        SelectorKind::Regex(_) => (
            base.regex_body.expect("regex base body").text(),
            candidate.regex_body.expect("regex candidate body").text(),
        ),
        SelectorKind::Literal | SelectorKind::Path => (
            base.selector.expect("selected base").text(),
            candidate.selector.expect("selected candidate").text(),
        ),
        SelectorKind::None => unreachable!(),
    };
    let bytes = left
        .len()
        .checked_add(right.len())
        .ok_or(SemanticValidationError::TargetComparisonByteLimit {
            limit: limits.max_target_comparison_bytes,
            location: candidate.location,
        })?;
    delta.comparison_bytes =
        delta
            .comparison_bytes
            .checked_add(bytes)
            .ok_or(SemanticValidationError::TargetComparisonByteLimit {
                limit: limits.max_target_comparison_bytes,
                location: candidate.location,
            })?;
    if cumulative
        .comparison_bytes
        .checked_add(delta.comparison_bytes)
        .is_none_or(|value| value > limits.max_target_comparison_bytes)
    {
        return Err(SemanticValidationError::TargetComparisonByteLimit {
            limit: limits.max_target_comparison_bytes,
            location: candidate.location,
        });
    }
    Ok(left == right)
}

#[allow(clippy::too_many_arguments)]
fn mark_reachable_regex_targets(
    draft: &SemanticDraft<'_>,
    states: &[ResolvedRuleStateRecord],
    active: &[u8],
    overlay_heads: &[usize],
    overlays: &[TargetOverlayEntry],
    seen: &mut [u32],
    candidates: &mut Vec<usize>,
) {
    for (root, rule) in draft.final_rules.iter().copied().enumerate() {
        if matches!(rule.role, FinalRuleRole::ChainChild { .. }) {
            continue;
        }
        let unit = states[root].unit.expect("root has unit");
        if active[unit] == 0 {
            continue;
        }
        let mut mark_node = |node: usize| {
            let mapping = semantic_mapping_for_rule(draft, draft.final_rules[node]);
            let target_range = mapping.target_start..mapping.target_start + mapping.target_count;
            for (offset, marked) in seen[target_range.clone()].iter_mut().enumerate() {
                let target = target_range.start + offset;
                if *marked == 0 && matches!(draft.targets[target].selector_kind, SelectorKind::Regex(_)) {
                    *marked = 1;
                    candidates.push(target);
                }
            }
            let mut overlay = overlay_heads[node];
            while overlay != NO_INDEX {
                let entry = overlays[overlay];
                if seen[entry.target] == 0
                    && matches!(draft.targets[entry.target].selector_kind, SelectorKind::Regex(_))
                {
                    seen[entry.target] = 1;
                    candidates.push(entry.target);
                }
                overlay = entry.next;
            }
        };
        if let Some(chain) = rule.chain {
            let chain = draft.chains[chain];
            for &node in &draft.chain_members[chain.member_start..chain.member_start + chain.member_count] {
                mark_node(node);
            }
        } else {
            mark_node(root);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validation_owned_bytes(
    nodes: usize,
    units: usize,
    final_targets: usize,
    updates: usize,
    removals: usize,
    affected: usize,
    requirements: usize,
    limits: ValidationLimits,
) -> Result<usize, SemanticValidationError> {
    let mut total = 0_usize;
    for (count, size) in [
        (nodes, size_of::<ValidatedRuleNodeRecord>()),
        (units, size_of::<ValidatedRuleUnitRecord>()),
        (units, size_of::<RuleIdIndexRecord>()),
        (final_targets, size_of::<usize>()),
        (updates, size_of::<AppliedExceptionRecord>()),
        (removals, size_of::<AppliedExceptionRecord>()),
        (affected, size_of::<usize>()),
        (requirements, size_of::<ValidatedRequirementReference>()),
    ] {
        total = total
            .checked_add(count.checked_mul(size).ok_or(SemanticValidationError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
            })?)
            .ok_or(SemanticValidationError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
            })?;
    }
    Ok(total)
}

fn commit_validated_nodes(
    validated: &mut ValidatedSemanticDocument<'_>,
    states: &[ResolvedRuleStateRecord],
    overlay_heads: &[usize],
    overlays: &[TargetOverlayEntry],
) {
    for node in 0..validated.draft.final_rules.len() {
        let mapping = semantic_mapping_for_rule(&validated.draft, validated.draft.final_rules[node]);
        let target_start = validated.final_targets.len();
        validated
            .final_targets
            .extend(mapping.target_start..mapping.target_start + mapping.target_count);
        let mut overlay = overlay_heads[node];
        while overlay != NO_INDEX {
            let entry = overlays[overlay];
            validated.final_targets.push(entry.target);
            overlay = entry.next;
        }
        validated.nodes.push(ValidatedRuleNodeRecord {
            rule: node,
            state: states[node],
            target_start,
            target_count: validated.final_targets.len() - target_start,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn commit_applied_exceptions(
    validated: &mut ValidatedSemanticDocument<'_>,
    states: &[ResolvedRuleStateRecord],
    id_index: &[RuleIdIndexRecord],
    limits: ValidationLimits,
    active: &mut [u8],
    seen: &mut [u32],
    affected: &mut Vec<usize>,
) -> Result<(), SemanticValidationError> {
    active.fill(1);
    seen.fill(0);
    let mut generation = 0_u32;
    let mut counters = ExceptionCounters::default();
    for (directive_index, mapping) in validated.draft.directives.iter().enumerate() {
        let syntax = validated.draft.documents[mapping.document_index]
            .get(mapping.directive_index)
            .expect("validated exception replay mapping");
        let (kind, ids) = match syntax.targets() {
            DirectiveTargetView::Update { ids, .. } => (ExceptionKind::Update, ids),
            DirectiveTargetView::Remove { ids } => (ExceptionKind::Remove, ids),
            DirectiveTargetView::None | DirectiveTargetView::Rule { .. } => continue,
        };
        generation += 1;
        let location = DirectiveLocation {
            document_index: mapping.document_index,
            directive_index: mapping.directive_index,
        };
        affected.clear();
        let local = resolve_exception_union(
            &validated.draft,
            id_index,
            states,
            ids,
            location,
            generation,
            seen,
            affected,
            counters,
            limits,
        )?;
        match kind {
            ExceptionKind::Update => {
                debug_assert!(affected
                    .iter()
                    .all(|root| active[states[*root].unit.expect("replay root unit")] != 0));
            }
            ExceptionKind::Remove => {
                affected.retain(|root| active[states[*root].unit.expect("replay root unit")] != 0);
            }
        }
        let affected_start = validated.affected_units.len();
        for &root in affected.iter() {
            let unit = states[root].unit.expect("replay root unit");
            validated.affected_units.push(unit);
            if kind == ExceptionKind::Remove {
                active[unit] = 0;
            }
        }
        let record = AppliedExceptionRecord {
            directive: directive_index,
            affected_start,
            affected_count: affected.len(),
        };
        match kind {
            ExceptionKind::Update => validated.updates.push(record),
            ExceptionKind::Remove => validated.removals.push(record),
        }
        counters.raw += local.raw;
        counters.distinct += local.distinct;
        counters.selector_work += local.selector_work;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::seclang::{parse_syntax, scan_source, SourceLimits, SyntaxLimits};

    fn with_draft<T>(text: &str, inspect: impl FnOnce(&SemanticDraft<'_>) -> T) -> Result<T, SemanticError> {
        with_draft_limits(text, SemanticLimits::default(), inspect)
    }

    fn with_draft_limits<T>(
        text: &str,
        limits: SemanticLimits,
        inspect: impl FnOnce(&SemanticDraft<'_>) -> T,
    ) -> Result<T, SemanticError> {
        let source = scan_source("test.conf", text.as_bytes(), SourceLimits::default()).expect("source");
        let syntax = parse_syntax(&source, SyntaxLimits::default()).expect("syntax");
        analyze_semantic_draft_inner(vec![syntax], limits, false, false).map(|draft| inspect(&draft))
    }

    fn crs_validation_context() -> SemanticValidationContext {
        let defaults = [
            Phase::RequestHeaders,
            Phase::RequestBody,
            Phase::ResponseHeaders,
            Phase::ResponseBody,
            Phase::Logging,
        ]
        .map(|phase| PhaseDefaults::pass(phase, true, true));
        SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).expect("valid test defaults")
    }

    fn with_validated<T>(
        text: &str,
        context: SemanticValidationContext,
        limits: ValidationLimits,
        inspect: impl FnOnce(&ValidatedSemanticDocument<'_>) -> T,
    ) -> Result<T, SemanticValidationError> {
        let source = scan_source("test.conf", text.as_bytes(), SourceLimits::default()).expect("source");
        let syntax = parse_syntax(&source, SyntaxLimits::default()).expect("syntax");
        let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default()).expect("semantic draft");
        validate_semantic_draft(draft, context, limits).map(|validated| inspect(&validated))
    }

    fn with_full_draft<T>(text: &str, inspect: impl FnOnce(&SemanticDraft<'_>) -> T) -> Result<T, SemanticError> {
        let source = scan_source("test.conf", text.as_bytes(), SourceLimits::default()).expect("source");
        let syntax = parse_syntax(&source, SyntaxLimits::default()).expect("syntax");
        analyze_one_semantic_draft(syntax, SemanticLimits::default()).map(|draft| inspect(&draft))
    }

    fn one_value(text: &str) -> Result<LocalActionValue, SemanticError> {
        with_draft(text, |draft| draft.actions().next().expect("action").local_value())
    }

    fn valid_operator_argument(kind: OperatorKind) -> &'static str {
        match kind {
            OperatorKind::DetectSqli
            | OperatorKind::DetectXss
            | OperatorKind::UnconditionalMatch
            | OperatorKind::ValidateUrlEncoding
            | OperatorKind::ValidateUtf8Encoding => "",
            OperatorKind::Eq | OperatorKind::Ge | OperatorKind::Gt | OperatorKind::Lt => "1",
            OperatorKind::IpMatch => "192.0.2.1",
            OperatorKind::ValidateByteRange => "0-255",
            _ => "x",
        }
    }

    #[test]
    fn registries_have_frozen_cardinality_and_case_policy() {
        assert_eq!(
            (
                CollectionKind::ALL.len(),
                OperatorKind::ALL.len(),
                ActionKind::ALL.len(),
                TransformationKind::ALL.len()
            ),
            (33, 19, 23, 20)
        );
        for &kind in CollectionKind::ALL {
            assert_eq!(
                CollectionKind::lookup_ascii_insensitive(kind.as_str().to_ascii_lowercase().as_bytes()),
                Some(kind)
            );
        }
        for &kind in ActionKind::ALL {
            assert_eq!(
                ActionKind::lookup_ascii_insensitive(kind.as_str().to_ascii_uppercase().as_bytes()),
                Some(kind)
            );
        }
        for &kind in TransformationKind::ALL {
            assert_eq!(
                TransformationKind::lookup_ascii_insensitive(kind.as_str().to_ascii_uppercase().as_bytes()),
                Some(kind)
            );
        }
        let mut rules = String::new();
        for (index, kind) in OperatorKind::ALL.iter().enumerate() {
            rules.push_str(&format!(
                "SecRule ARGS \"@{} {}\" \"id:{}\"\n",
                kind.as_str(),
                valid_operator_argument(*kind),
                index + 1
            ));
        }
        assert_eq!(
            with_draft(&rules, |draft| draft
                .operators()
                .map(|record| record.kind())
                .collect::<Vec<_>>()),
            Ok(OperatorKind::ALL.to_vec())
        );
        assert_eq!(
            with_draft("SecRule ARGS \"@rx x\" \"id:1\"\n", |draft| draft
                .operators()
                .next()
                .unwrap()
                .kind()),
            Ok(OperatorKind::Rx)
        );
        assert!(matches!(
            with_draft("SecRule ARGS \"@RX x\" \"id:1\"\n", |_| ()),
            Err(SemanticError::UnknownOperator { .. })
        ));
    }

    #[test]
    fn classifies_typed_records_and_separates_requirements_from_capabilities() {
        let summary = with_draft(
            "SecRule RESPONSE_BODY \"@rx %{TX.pattern}\" \"id:001,phase:request,severity:WARNING,status:403,skip:1,t:lowercase,block,ctl:auditEngine=Off\"\n",
            |draft| {
                (
                    draft.target_count(), draft.operator_count(), draft.action_count(),
                    draft.actions().find_map(|action| action.transformation()),
                    draft.requirements().map(|record| record.requirement()).collect::<Vec<_>>(),
                    draft.unavailable_capabilities().map(|record| record.capability()).collect::<Vec<_>>(),
                )
            },
        ).expect("semantic draft");
        assert_eq!(summary.0..=summary.2, 1..=8);
        assert_eq!(summary.3, Some(TransformationKind::Lowercase));
        assert_eq!(
            summary.4,
            [
                CompileRequirement::RegexCompilation,
                CompileRequirement::BlockDefaultDisruptive
            ]
        );
        assert_eq!(
            summary.5,
            [UnavailableCapability::FullResponseBody, UnavailableCapability::Ctl]
        );
    }

    #[test]
    fn scalar_boundaries_and_profiles_are_exact() {
        assert_eq!(one_value("SecAction \"id:0001\"\n"), Ok(LocalActionValue::Id(1)));
        assert_eq!(
            one_value("SecAction \"phase:logging\"\n"),
            Ok(LocalActionValue::Phase(Phase::Logging))
        );
        assert_eq!(
            one_value("SecAction \"severity:CrItIcAl\"\n"),
            Ok(LocalActionValue::Severity(Severity::Critical))
        );
        assert_eq!(
            one_value("SecAction \"status:0599\"\n"),
            Ok(LocalActionValue::Status(599))
        );
        assert_eq!(
            one_value(&format!("SecAction \"skip:{}\"\n", u32::MAX)),
            Ok(LocalActionValue::Skip(u32::MAX))
        );
        for input in [
            "SecAction \"id:0\"\n",
            "SecAction \"phase:Request\"\n",
            "SecAction \"severity:08\"\n",
            "SecAction \"status:100\"\n",
            "SecAction \"status:101\"\n",
            "SecAction \"status:199\"\n",
            "SecAction \"status:600\"\n",
            "SecAction \"skip:0\"\n",
        ] {
            assert!(one_value(input).is_err(), "{input}");
        }
        assert!(matches!(
            one_value("SecAction \"id:%{TX.id}\"\n"),
            Err(SemanticError::MacroScalar {
                kind: ScalarKind::Id,
                ..
            })
        ));
        assert!(matches!(
            one_value("SecAction \"skip:4294967296\"\n"),
            Err(SemanticError::ScalarOverflow {
                kind: ScalarKind::Skip,
                ..
            })
        ));
    }

    fn with_fold<T>(text: &str, inspect: impl FnOnce(&SemanticDraft<'_>) -> T) -> Result<T, SemanticError> {
        let source = scan_source("fold.conf", text.as_bytes(), SourceLimits::default()).expect("source");
        let syntax = parse_syntax(&source, SyntaxLimits::default()).expect("syntax");
        analyze_semantic_draft_inner(vec![syntax], SemanticLimits::default(), true, false).map(|draft| inspect(&draft))
    }

    #[test]
    fn local_fold_exposes_roles_presence_logging_and_normalized_transformations() {
        let summary = with_fold(
            "SecRule ARGS \"@rx x\" \"id:1,block,phase:1,log,nolog,auditlog,t:lowercase,t:none,t:length,chain\"\n\
             SecRule ARGS \"@rx y\"\n\
             SecAction \"id:2,pass,t:none\"\n",
            |draft| {
                let folds = draft.folded_rules().collect::<Vec<_>>();
                assert_eq!(draft.directive(0).unwrap().local_rule_fold().unwrap().id(), Some(1));
                (
                    folds.iter().map(|fold| fold.envelope()).collect::<Vec<_>>(),
                    folds.iter().map(|fold| fold.role()).collect::<Vec<_>>(),
                    folds.iter().map(|fold| fold.action_presence()).collect::<Vec<_>>(),
                    folds[0].log_override(),
                    folds[0].audit_log_override(),
                    folds[0]
                        .effective_transformations()
                        .map(|action| action.transformation().unwrap())
                        .collect::<Vec<_>>(),
                    folds[0].reset_action().unwrap().transformation(),
                    draft
                        .requirements()
                        .filter(|record| record.requirement() == CompileRequirement::DefaultPhase)
                        .count(),
                )
            },
        )
        .unwrap();
        assert_eq!(
            summary.0,
            [
                LocalRuleEnvelope::Rule,
                LocalRuleEnvelope::Rule,
                LocalRuleEnvelope::Action
            ]
        );
        assert_eq!(summary.1[0], Some(ProvisionalRuleRole::Root));
        assert_eq!(
            summary.1[1],
            Some(ProvisionalRuleRole::Child {
                predecessor: DirectiveLocation {
                    document_index: 0,
                    directive_index: 0
                },
                root: DirectiveLocation {
                    document_index: 0,
                    directive_index: 0
                },
            })
        );
        assert_eq!(summary.1[2], None);
        assert_eq!(
            summary.2,
            [
                FoldActionPresence::PresentNonempty,
                FoldActionPresence::Absent,
                FoldActionPresence::PresentNonempty
            ]
        );
        assert_eq!((summary.3, summary.4), (Some(false), Some(true)));
        assert_eq!(summary.5, [TransformationKind::Length]);
        assert_eq!(summary.6, Some(TransformationKind::None));
        assert_eq!(summary.7, 1);
    }

    #[test]
    fn fold_adjacency_crosses_documents_and_non_rules_interrupt_it() {
        let first = scan_source(
            "first",
            b"SecRule ARGS \"@rx x\" \"id:1,block,phase:1,chain\"\n",
            SourceLimits::default(),
        )
        .unwrap();
        let second = scan_source("second", b"SecRule ARGS \"@rx y\"\n", SourceLimits::default()).unwrap();
        let draft = analyze_semantic_draft(
            vec![
                parse_syntax(&first, SyntaxLimits::default()).unwrap(),
                parse_syntax(&second, SyntaxLimits::default()).unwrap(),
            ],
            SemanticLimits::default(),
        )
        .unwrap();
        assert!(matches!(
            draft.folded_rules().nth(1).unwrap().role(),
            Some(ProvisionalRuleRole::Child {
                predecessor: DirectiveLocation { document_index: 0, .. },
                ..
            })
        ));

        assert!(matches!(
            with_fold(
                "SecRule ARGS \"@rx x\" \"id:1,block,phase:1,chain\"\nSecMarker END\nSecRule ARGS \"@rx y\"\n",
                |_| ()
            ),
            Err(SemanticError::FoldMissing {
                kind: FoldRequiredKind::Id,
                ..
            })
        ));
    }

    #[test]
    fn fold_cardinality_secaction_and_status_pass_errors_are_typed_and_redacted() {
        let cases = [
            ("SecAction \"id:1,id:2,pass\"\n", "FoldDuplicate"),
            ("SecAction \"id:1,pass,capture\"\n", "SecActionForbidden"),
            ("SecAction \"id:1,pass,t:lowercase\"\n", "SecActionForbidden"),
            ("SecAction \"id:1,pass,status:403\"\n", "StatusPass"),
            ("SecAction \"pass\"\n", "FoldMissing"),
            ("SecAction \"id:1\"\n", "FoldMissing"),
        ];
        for (source, family) in cases {
            let error = with_fold(source, |_| ()).unwrap_err();
            let rendered = format!("{error:?} {error}");
            assert!(rendered.contains(family), "{rendered}");
            assert!(!rendered.contains("403"));
        }
    }

    #[test]
    fn fold_cross_category_error_precedence_is_frozen() {
        let cases = [
            ("SecAction \"capture,id:1,id:2\"\n", FoldErrorFamily::Duplicate),
            ("SecAction \"capture,pass,status:403\"\n", FoldErrorFamily::Forbidden),
            ("SecAction \"pass,status:403\"\n", FoldErrorFamily::MissingId),
            ("SecAction \"id:1,status:403\"\n", FoldErrorFamily::MissingDisruptive),
            ("SecAction \"id:1,pass,status:403\"\n", FoldErrorFamily::StatusPass),
        ];
        for (source, expected) in cases {
            let error = with_fold(source, |_| ()).unwrap_err();
            let actual = match error {
                SemanticError::FoldDuplicate { .. } => FoldErrorFamily::Duplicate,
                SemanticError::SecActionForbidden { .. } => FoldErrorFamily::Forbidden,
                SemanticError::FoldMissing {
                    kind: FoldRequiredKind::Id,
                    ..
                } => FoldErrorFamily::MissingId,
                SemanticError::FoldMissing {
                    kind: FoldRequiredKind::Disruptive,
                    ..
                } => FoldErrorFamily::MissingDisruptive,
                SemanticError::StatusPass { .. } => FoldErrorFamily::StatusPass,
                other => panic!("unexpected fold error: {other:?}"),
            };
            assert_eq!(actual, expected, "{source}");
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FoldErrorFamily {
        Duplicate,
        Forbidden,
        MissingId,
        MissingDisruptive,
        StatusPass,
    }

    #[test]
    fn later_content_error_precedes_early_effective_aggregate_excess() {
        let source = scan_source(
            "precedence",
            b"SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,t:lowercase\"\nSecAction \"id:2,id:3,pass\"\n",
            SourceLimits::default(),
        )
        .unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(matches!(
            analyze_one_semantic_draft(
                syntax,
                SemanticLimits {
                    fold: FoldLimits {
                        max_effective_transformations: 0,
                        ..FoldLimits::default()
                    },
                    ..SemanticLimits::default()
                }
            ),
            Err(SemanticError::FoldDuplicate {
                kind: FoldSingletonKind::Id,
                ..
            })
        ));
    }

    #[test]
    fn dangling_chain_invalid_child_and_present_empty_are_deferred() {
        assert!(with_fold("SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\n", |_| ()).is_ok());
        let presence = with_fold(
            "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\n\
             SecRule ARGS \"@rx y\" \"id:2,phase:2,deny,skip:1,msg:x,chain\"\n\
             SecRule ARGS \"@rx z\" \"\"\n",
            |draft| {
                draft
                    .folded_rules()
                    .map(|fold| fold.action_presence())
                    .collect::<Vec<_>>()
            },
        )
        .unwrap();
        assert_eq!(
            presence,
            [
                FoldActionPresence::PresentNonempty,
                FoldActionPresence::PresentNonempty,
                FoldActionPresence::PresentEmpty,
            ]
        );
    }

    fn with_graph<T>(text: &str, inspect: impl FnOnce(&SemanticDraft<'_>) -> T) -> Result<T, SemanticError> {
        let source = scan_source("graph.conf", text.as_bytes(), SourceLimits::default()).expect("source");
        let syntax = parse_syntax(&source, SyntaxLimits::default()).expect("syntax");
        analyze_one_semantic_draft(syntax, SemanticLimits::default()).map(|draft| inspect(&draft))
    }

    #[test]
    fn final_graph_exposes_owner_bound_roles_phases_chains_markers_and_resolution() {
        let summary = with_graph(
            "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain,skipAfter:END\"\n\
             SecRule ARGS \"@rx y\" \"t:lowercase\"\n\
             SecRule ARGS \"@rx z\" \"id:2,pass\"\n\
             SecAction \"id:3,pass,phase:2\"\n\
             SecMarker END\n",
            |draft| {
                let rules = draft.final_rules().collect::<Vec<_>>();
                let chain = draft.chains().next().unwrap();
                let resolution = draft.resolved_skip_afters().next().unwrap();
                (
                    rules.iter().map(|rule| rule.role()).collect::<Vec<_>>(),
                    rules.iter().map(|rule| rule.phase()).collect::<Vec<_>>(),
                    chain
                        .members()
                        .map(|member| (member.ordinal(), member.rule().location()))
                        .collect::<Vec<_>>(),
                    draft
                        .markers()
                        .map(|marker| (marker.name().to_owned(), marker.location()))
                        .collect::<Vec<_>>(),
                    (
                        resolution.referrer(),
                        resolution.source_action().kind(),
                        resolution.target().name().to_owned(),
                    ),
                    draft.directive(0).unwrap().final_rule().unwrap().location(),
                    draft.folded_rules().next().unwrap().final_rule().unwrap().location(),
                )
            },
        )
        .unwrap();
        assert!(matches!(summary.0[0], FinalRuleRole::ChainRoot { .. }));
        assert!(matches!(summary.0[1], FinalRuleRole::ChainChild { ordinal: 1, .. }));
        assert_eq!(summary.0[2], FinalRuleRole::StandaloneRule);
        assert_eq!(summary.0[3], FinalRuleRole::Action);
        assert!(matches!(
            summary.1[0],
            FinalPhaseState::Explicit {
                phase: Phase::RequestHeaders,
                ..
            }
        ));
        assert!(matches!(
            summary.1[1],
            FinalPhaseState::Inherited {
                phase: Phase::RequestHeaders,
                ..
            }
        ));
        assert!(matches!(summary.1[2], FinalPhaseState::RequiresDefault { .. }));
        assert_eq!(summary.2.len(), 2);
        assert_eq!(summary.3[0].0, "END");
        assert_eq!(summary.4 .1, ActionEnvelopeKind::Action(ActionKind::SkipAfter));
        assert_eq!(summary.4 .2, "END");
        assert_eq!(
            summary.5,
            DirectiveLocation {
                document_index: 0,
                directive_index: 0
            }
        );
        assert_eq!(summary.6, summary.5);
    }

    #[test]
    fn semantic_targets_and_exception_selectors_preserve_owner_bound_source_views() {
        let summary = with_draft(
            r#"SecRule ARGS|REQUEST_HEADERS:name|XML:/x|REQUEST_HEADERS:/a\/b/|REQUEST_HEADERS:'/c/' "@rx x"
SecRuleUpdateTargetById 1-2 ARGS
SecRuleRemoveById 3 4-5
"#,
            |draft| {
                let targets = draft.targets().copied().collect::<Vec<_>>();
                let update = draft.directive(1).unwrap();
                let remove = draft.directive(2).unwrap();
                (
                    targets
                        .iter()
                        .map(|target| {
                            (
                                target.collection_spelling().text().to_owned(),
                                target.selector_kind(),
                                target.selector().map(|value| value.text().to_owned()),
                                target.regex_body().map(|value| value.text().to_owned()),
                                target.span(),
                            )
                        })
                        .collect::<Vec<_>>(),
                    update
                        .id_selectors()
                        .iter()
                        .map(|value| value.kind())
                        .collect::<Vec<_>>(),
                    update.targets().len(),
                    remove
                        .id_selectors()
                        .iter()
                        .map(|value| value.kind())
                        .collect::<Vec<_>>(),
                    remove.targets().len(),
                )
            },
        )
        .unwrap();
        assert_eq!(summary.0.len(), 6);
        assert_eq!(summary.0[0].1, SelectorKind::None);
        assert_eq!((summary.0[0].2.as_deref(), summary.0[0].3.as_deref()), (None, None));
        assert_eq!(summary.0[1].1, SelectorKind::Literal);
        assert_eq!(
            (summary.0[1].2.as_deref(), summary.0[1].3.as_deref()),
            (Some("name"), None)
        );
        assert_eq!(summary.0[2].1, SelectorKind::Path);
        assert_eq!(
            (summary.0[2].2.as_deref(), summary.0[2].3.as_deref()),
            (Some("/x"), None)
        );
        assert_eq!(summary.0[3].1, SelectorKind::Regex(crate::seclang::RegexStyle::Bare));
        assert_eq!(
            (summary.0[3].2.as_deref(), summary.0[3].3.as_deref()),
            (Some("/a\\/b/"), Some("a\\/b"))
        );
        assert_eq!(
            summary.0[4].1,
            SelectorKind::Regex(crate::seclang::RegexStyle::SingleQuoted)
        );
        assert_eq!(
            (summary.0[4].2.as_deref(), summary.0[4].3.as_deref()),
            (Some("'/c/'"), Some("c"))
        );
        assert_eq!(summary.1, [crate::seclang::IdSelectorKind::Range { start: 1, end: 2 }]);
        assert_eq!(summary.2, 1);
        assert_eq!(
            summary.3,
            [
                crate::seclang::IdSelectorKind::Single(3),
                crate::seclang::IdSelectorKind::Range { start: 4, end: 5 }
            ]
        );
        assert_eq!(summary.4, 0);
        assert!(summary
            .0
            .iter()
            .all(|value| value.4.start.byte_offset < value.4.end.byte_offset));

        with_draft("SecRule REQUEST_HEADERS:na\\\nme \"@rx x\"\n", |draft| {
            let target = *draft.targets().next().unwrap();
            let selector = target.selector().unwrap();
            assert_eq!(selector.text(), "name");
            assert_eq!((selector.span().start.line, selector.span().end.line), (1, 2));
            assert_eq!((target.span().start.line, target.span().end.line), (1, 2));
            let line = &draft.documents[0].source().lines()[0];
            let line_start = line.text().as_ptr() as usize;
            let line_end = line_start + line.text().len();
            assert!((line_start..line_end).contains(&(selector.text().as_ptr() as usize)));
        })
        .unwrap();
    }

    #[test]
    fn remove_envelope_interrupts_chains_and_target_metadata_charge_is_exact() {
        assert!(matches!(
            with_graph(
                "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRuleRemoveById 1\nSecRule ARGS \"@rx y\" \"id:2,pass,phase:1\"\n",
                |_| ()
            ),
            Err(SemanticError::DanglingChain { .. })
        ));

        let source = scan_source(
            "target-charge",
            b"SecRule ARGS|REQUEST_HEADERS:name \"@rx x\"",
            SourceLimits::default(),
        )
        .unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        let inherited = syntax.retained_charged_bytes();
        let exact = analyze_semantic_draft_inner(vec![syntax], SemanticLimits::default(), false, false)
            .unwrap()
            .semantic_charged_bytes();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(analyze_semantic_draft_inner(
            vec![syntax],
            SemanticLimits {
                max_owned_bytes: exact,
                max_retained_bytes: inherited + exact,
                ..SemanticLimits::default()
            },
            false,
            false,
        )
        .is_ok());
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(matches!(
            analyze_semantic_draft_inner(
                vec![syntax],
                SemanticLimits {
                    max_owned_bytes: exact - 1,
                    max_retained_bytes: usize::MAX,
                    ..SemanticLimits::default()
                },
                false,
                false,
            ),
            Err(SemanticError::PreflightOwnedByteLimit { .. } | SemanticError::OwnedByteLimit { .. })
        ));
        #[cfg(target_pointer_width = "64")]
        assert_eq!(size_of::<SemanticTarget<'_>>(), 328);
    }

    #[test]
    fn graph_rejects_every_child_restriction_and_dangling_interruption() {
        let forbidden = [
            "id:2",
            "phase:2",
            "block",
            "deny",
            "pass",
            "skip:1",
            "skipAfter:END",
            "status:403",
            "severity:error",
            "msg:x",
            "logdata:x",
            "tag:x",
            "ver:x",
        ];
        for action in forbidden {
            let source = format!("SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRule ARGS \"@rx y\" \"{action}\"\nSecMarker END\n");
            assert!(
                matches!(with_graph(&source, |_| ()), Err(SemanticError::ChildAction { .. })),
                "{action}"
            );
        }
        for interruption in ["", "SecMarker X\n", "SecAction \"id:2,pass,phase:1\"\n"] {
            let source = format!("SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\n{interruption}");
            assert!(matches!(
                with_graph(&source, |_| ()),
                Err(SemanticError::DanglingChain { .. })
            ));
        }
        assert!(with_graph(
            "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRule ARGS \"@rx y\" \"t:lowercase,capture,multiMatch,setvar:TX.a=1,log\"\n",
            |_| ()
        ).is_ok());
    }

    #[test]
    fn child_structural_candidates_follow_textual_action_order() {
        let forbidden_first =
            "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRule ARGS \"@rx y\" \"skipAfter:END,chain\"\n";
        let error = with_graph(forbidden_first, |_| ()).unwrap_err();
        assert!(matches!(
            error,
            SemanticError::ChildAction {
                kind: ActionKind::SkipAfter,
                position
            } if position.byte_offset == forbidden_first.find("skipAfter").unwrap()
        ));

        let relationship_first =
            "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRule ARGS \"@rx y\" \"chain,skipAfter:END\"\n";
        let error = with_graph(relationship_first, |_| ()).unwrap_err();
        assert!(matches!(
            error,
            SemanticError::DanglingChain { position }
                if position.byte_offset == relationship_first.rfind("chain").unwrap()
        ));
    }

    #[test]
    fn invalid_separated_chain_roots_have_linear_projection_work() {
        const ROOTS: usize = 1_024;
        let mut text = String::new();
        for id in 1..=ROOTS {
            text.push_str(&format!(
                "SecRule ARGS \"@rx x\" \"id:{id},pass,phase:1,chain\"\nSecMarker M{id}\n"
            ));
        }
        let source = scan_source("linear.conf", text.as_bytes(), SourceLimits::default()).unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        let draft = analyze_semantic_draft_inner(vec![syntax], SemanticLimits::default(), true, false).unwrap();
        let projection = project_chains(&draft.folds, &draft.actions, GraphLimits::default());
        assert_eq!(projection.inspected_folds, draft.folds.len());
        assert_eq!(projection.chain_count, ROOTS);
        assert_eq!(projection.chain_members, ROOTS);
    }

    #[test]
    fn marker_namespace_and_forward_resolution_are_exact_and_redacted() {
        assert!(with_graph("SecMarker Case\nSecMarker case\n", |draft| draft.markers().len()).is_ok());
        assert!(with_graph("SecMarker 123\nSecMarker \"a\tb\"\nSecMarker café\n", |_| ()).is_ok());
        for source in ["SecMarker SAME\nSecMarker \"SAME\"\n", "SecMarker \"bad\u{7f}\"\n"] {
            assert!(with_graph(source, |_| ()).is_err());
        }
        assert!(matches!(
            with_graph("SecMarker END\nSecAction \"id:1,pass,phase:1,skipAfter:END\"\n", |_| ()),
            Err(SemanticError::NonForwardSkipAfter { .. })
        ));
        let error = with_graph("SecAction \"id:1,pass,phase:1,skipAfter:SECRET\"\n", |_| ()).unwrap_err();
        assert!(matches!(error, SemanticError::UnresolvedSkipAfter { .. }));
        assert!(!format!("{error:?} {error}").contains("SECRET"));
    }

    #[test]
    fn generated_chain_lengths_and_cross_document_many_to_one_resolution_match_model() {
        for length in 2..=8_usize {
            let mut source = String::from("SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\n");
            for ordinal in 1..length {
                let actions = if ordinal + 1 == length { "" } else { " \"chain\"" };
                source.push_str(&format!("SecRule ARGS \"@rx x\"{actions}\n"));
            }
            let ordinals = with_graph(&source, |draft| {
                draft
                    .chains()
                    .next()
                    .unwrap()
                    .members()
                    .map(|member| member.ordinal())
                    .collect::<Vec<_>>()
            })
            .unwrap();
            assert_eq!(ordinals, (0..length).collect::<Vec<_>>());
        }
        let first = scan_source(
            "first",
            b"SecAction \"id:1,pass,phase:1,skipAfter:END\"\nSecAction \"id:2,pass,phase:2,skipAfter:END\"\n",
            SourceLimits::default(),
        )
        .unwrap();
        let second = scan_source("second", b"SecMarker END\n", SourceLimits::default()).unwrap();
        let draft = analyze_semantic_draft(
            vec![
                parse_syntax(&first, SyntaxLimits::default()).unwrap(),
                parse_syntax(&second, SyntaxLimits::default()).unwrap(),
            ],
            SemanticLimits::default(),
        )
        .unwrap();
        assert_eq!(draft.resolved_skip_afters().len(), 2);
        assert!(draft
            .resolved_skip_afters()
            .all(|resolution| resolution.target().location().document_index() == 1));
    }

    #[test]
    fn graph_limits_accept_exact_and_reject_first_excess() {
        let text =
            "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain,skipAfter:END\"\nSecRule ARGS \"@rx y\"\nSecMarker END\n";
        let source = scan_source("limits", text.as_bytes(), SourceLimits::default()).unwrap();
        let exact = GraphLimits {
            max_chains: 1,
            max_chain_members: 2,
            max_members_per_chain: 2,
            max_markers: 1,
            max_marker_name_bytes: 3,
            max_marker_name_bytes_total: 3,
            max_skip_after_refs: 1,
            max_registry_work_bytes: MARKER_WORK_ENTRY_CHARGE + 6,
        };
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                graph: exact,
                ..SemanticLimits::default()
            }
        )
        .is_ok());
        for graph in [
            GraphLimits { max_chains: 0, ..exact },
            GraphLimits {
                max_chain_members: 1,
                ..exact
            },
            GraphLimits {
                max_members_per_chain: 1,
                ..exact
            },
            GraphLimits {
                max_markers: 0,
                ..exact
            },
            GraphLimits {
                max_marker_name_bytes: 2,
                ..exact
            },
            GraphLimits {
                max_marker_name_bytes_total: 2,
                ..exact
            },
            GraphLimits {
                max_skip_after_refs: 0,
                ..exact
            },
            GraphLimits {
                max_registry_work_bytes: MARKER_WORK_ENTRY_CHARGE + 5,
                ..exact
            },
        ] {
            let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
            assert!(analyze_one_semantic_draft(
                syntax,
                SemanticLimits {
                    graph,
                    ..SemanticLimits::default()
                }
            )
            .is_err());
        }
    }

    #[test]
    fn graph_error_categories_and_child_skip_work_precedence_are_frozen() {
        assert!(matches!(
            with_graph(
                "SecMarker X\nSecMarker X\nSecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\n",
                |_| ()
            ),
            Err(SemanticError::DanglingChain { .. })
        ));
        assert!(matches!(
            with_graph(
                "SecAction \"id:1,pass,phase:1,skipAfter:MISSING\"\nSecMarker X\nSecMarker X\n",
                |_| ()
            ),
            Err(SemanticError::DuplicateMarker { .. })
        ));
        let source = scan_source(
            "child-work",
            b"SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRule ARGS \"@rx y\" \"skipAfter:END\"\nSecMarker END\n",
            SourceLimits::default(),
        ).unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(matches!(
            analyze_one_semantic_draft(
                syntax,
                SemanticLimits {
                    graph: GraphLimits {
                        max_skip_after_refs: 0,
                        max_registry_work_bytes: MARKER_WORK_ENTRY_CHARGE + 3,
                        ..GraphLimits::default()
                    },
                    ..SemanticLimits::default()
                }
            ),
            Err(SemanticError::ChildAction {
                kind: ActionKind::SkipAfter,
                ..
            })
        ));
    }

    #[test]
    fn graph_owned_and_retained_charges_have_exact_boundaries() {
        let source = scan_source(
            "graph-charge",
            b"SecAction \"id:1,pass,phase:1\"\nSecMarker END\n",
            SourceLimits::default(),
        )
        .unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        let inherited = syntax.retained_charged_bytes();
        let exact = analyze_one_semantic_draft(syntax, SemanticLimits::default())
            .unwrap()
            .semantic_charged_bytes();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                max_owned_bytes: exact,
                max_retained_bytes: inherited + exact,
                ..SemanticLimits::default()
            }
        )
        .is_ok());
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                max_owned_bytes: exact - 1,
                max_retained_bytes: usize::MAX,
                ..SemanticLimits::default()
            }
        )
        .is_err());
    }

    #[test]
    fn fold_limits_accept_exact_and_reject_first_excess() {
        let source = scan_source(
            "limits",
            b"SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,t:none,t:lowercase\"\n",
            SourceLimits::default(),
        )
        .unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                fold: FoldLimits {
                    max_folded_rules: 1,
                    max_effective_transformations: 1
                },
                ..SemanticLimits::default()
            }
        )
        .is_ok());
        for fold in [
            FoldLimits {
                max_folded_rules: 0,
                max_effective_transformations: 1,
            },
            FoldLimits {
                max_folded_rules: 1,
                max_effective_transformations: 0,
            },
        ] {
            let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
            assert!(analyze_one_semantic_draft(
                syntax,
                SemanticLimits {
                    fold,
                    ..SemanticLimits::default()
                }
            )
            .is_err());
        }
    }

    #[test]
    fn transformation_reset_fold_matches_suffix_model() {
        let choices = ["t:none", "t:lowercase", "t:length"];
        for mut model in 0_usize..243 {
            let mut entries = Vec::new();
            let mut expected = Vec::new();
            for _ in 0..5 {
                let choice = choices[model % choices.len()];
                model /= choices.len();
                entries.push(choice);
                match choice {
                    "t:none" => expected.clear(),
                    "t:lowercase" => expected.push(TransformationKind::Lowercase),
                    _ => expected.push(TransformationKind::Length),
                }
            }
            let source = format!("SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,{}\"\n", entries.join(","));
            let actual = with_fold(&source, |draft| {
                draft
                    .folded_rules()
                    .next()
                    .unwrap()
                    .effective_transformations()
                    .map(|action| action.transformation().unwrap())
                    .collect::<Vec<_>>()
            })
            .unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn every_fold_singleton_and_group_rejects_its_second_member() {
        for duplicate in [
            "id:2",
            "phase:2",
            "severity:warning",
            "status:404",
            "msg:b",
            "logdata:b",
            "ver:b",
            "skipAfter:END",
            "chain",
            "capture",
            "multiMatch",
            "deny",
        ] {
            let source = format!(
                "SecRule ARGS \"@rx x\" \"id:1,phase:1,severity:error,status:403,msg:a,logdata:a,ver:a,skip:1,chain,capture,multiMatch,block,{duplicate}\"\n"
            );
            assert!(matches!(
                with_fold(&source, |_| ()),
                Err(SemanticError::FoldDuplicate { .. })
            ));
        }
        assert!(with_fold(
            "SecAction \"id:1,pass,phase:1,tag:a,tag:b,setvar:TX.a=1,setvar:TX.b=2,log,nolog,auditlog,noauditlog,t:none,t:none\"\n",
            |_| ()
        )
        .is_ok());
    }

    #[test]
    fn fold_owned_and_retained_charges_have_exact_boundaries() {
        let text = b"SecAction \"id:1,block\"\n";
        let source = scan_source("charge", text, SourceLimits::default()).unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        let inherited = syntax.retained_charged_bytes();
        let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default()).unwrap();
        assert_eq!(
            draft
                .requirements()
                .map(|record| record.requirement())
                .collect::<Vec<_>>(),
            [
                CompileRequirement::BlockDefaultDisruptive,
                CompileRequirement::DefaultPhase
            ]
        );
        let exact = draft.semantic_charged_bytes();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                max_owned_bytes: exact,
                max_retained_bytes: inherited + exact,
                ..SemanticLimits::default()
            }
        )
        .is_ok());
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                max_owned_bytes: exact - 1,
                max_retained_bytes: usize::MAX,
                ..SemanticLimits::default()
            }
        )
        .is_err());
    }

    #[test]
    fn every_ordinary_action_obeys_its_value_presence_contract() {
        let forbidden = [
            ActionKind::AuditLog,
            ActionKind::Block,
            ActionKind::Capture,
            ActionKind::Chain,
            ActionKind::Deny,
            ActionKind::Log,
            ActionKind::MultiMatch,
            ActionKind::NoAuditLog,
            ActionKind::NoLog,
            ActionKind::Pass,
        ];
        for kind in forbidden {
            assert!(with_draft(&format!("SecAction \"{}\"\n", kind.as_str()), |_| ()).is_ok());
            assert!(matches!(
                with_draft(&format!("SecAction \"{}:x\"\n", kind.as_str()), |_| ()),
                Err(SemanticError::ValueContract {
                    expected: ValueContract::Forbidden,
                    ..
                })
            ));
        }
        let required = [
            (ActionKind::Id, "1"),
            (ActionKind::Phase, "2"),
            (ActionKind::Severity, "3"),
            (ActionKind::Status, "403"),
            (ActionKind::Skip, "1"),
            (ActionKind::Ctl, "auditEngine=Off"),
            (ActionKind::InitCol, "global=key"),
            (ActionKind::LogData, "x"),
            (ActionKind::Msg, "x"),
            (ActionKind::SetVar, "TX.key=1"),
            (ActionKind::SkipAfter, "x"),
            (ActionKind::Tag, "x"),
            (ActionKind::Ver, "x"),
        ];
        for (kind, value) in required {
            assert!(with_draft(&format!("SecAction \"{}:{}\"\n", kind.as_str(), value), |_| ()).is_ok());
            assert!(matches!(
                with_draft(&format!("SecAction \"{}\"\n", kind.as_str()), |_| ()),
                Err(SemanticError::ValueContract {
                    expected: ValueContract::Required,
                    ..
                })
            ));
            assert!(matches!(
                with_draft(&format!("SecAction \"{}:\"\n", kind.as_str()), |_| ()),
                Err(SemanticError::ValueContract {
                    expected: ValueContract::Required,
                    ..
                })
            ));
        }
    }

    #[test]
    fn every_action_operand_is_typed_and_owner_bound() {
        let source = concat!(
            "SecAction \"auditlog\"\n",
            "SecAction \"id:7\"\n",
            "SecAction \"t:lowercase\"\n",
            "SecAction \"msg:prefix%{TX.value}suffix\"\n",
            "SecAction \"tag:%{BROKEN\"\n",
            "SecAction \"setvar:TX.pre%{TX.key}post=+%{TX.delta}\"\n",
            "SecAction \"initcol:ip=%{REMOTE_ADDR}\"\n",
            "SecAction \"ctl:auditEngine=RelevantOnly\"\n",
        );
        with_draft(source, |draft| {
            let mut actions = draft.actions();
            assert!(matches!(
                actions.next().unwrap().operand(),
                ActionOperandView::NoArgument
            ));
            assert!(matches!(
                actions.next().unwrap().operand(),
                ActionOperandView::Local(LocalActionValue::Id(7))
            ));
            assert!(matches!(
                actions.next().unwrap().operand(),
                ActionOperandView::Transformation(TransformationKind::Lowercase)
            ));
            let ActionOperandView::Template(message) = actions.next().unwrap().operand() else {
                panic!("message template")
            };
            assert_eq!(
                (message.text(), message.expansion_count()),
                ("prefix%{TX.value}suffix", 1)
            );
            assert!(matches!(
                actions.next().unwrap().operand(),
                ActionOperandView::Literal(value) if value.text() == "%{BROKEN"
            ));
            let ActionOperandView::SetVar(setvar) = actions.next().unwrap().operand() else {
                panic!("setvar")
            };
            assert_eq!(
                (
                    setvar.collection(),
                    setvar.operation(),
                    setvar.key().text(),
                    setvar.constant()
                ),
                (CollectionKind::Tx, SetVarOperation::Add, "pre%{TX.key}post", None)
            );
            assert_eq!(
                setvar
                    .key()
                    .segments()
                    .map(|segment| segment.text())
                    .collect::<String>(),
                setvar.key().text()
            );
            assert_eq!(setvar.key().segments().len(), 3);
            let value = setvar.value().unwrap();
            assert_eq!((value.text(), value.expansion_count()), ("%{TX.delta}", 1));
            assert_eq!(
                value.segments().map(|segment| segment.text()).collect::<String>(),
                value.text()
            );
            let ActionOperandView::InitCol(initcol) = actions.next().unwrap().operand() else {
                panic!("initcol")
            };
            assert_eq!(
                (initcol.collection(), initcol.key().text()),
                (InitColCollection::Ip, "%{REMOTE_ADDR}")
            );
            assert!(matches!(
                actions.next().unwrap().operand(),
                ActionOperandView::Ctl(CtlView::AuditEngine(AuditEngineMode::RelevantOnly))
            ));
            assert!(actions.next().is_none());
        })
        .unwrap();
        with_draft("SecAction \"ctl:ruleRemoveTargetByTag=tag;ARGS:/^name$/\"\n", |draft| {
            let ActionOperandView::Ctl(CtlView::RuleRemoveTargetByTag { target, .. }) =
                draft.actions().next().unwrap().operand()
            else {
                panic!("regex ctl target")
            };
            assert_eq!(
                target.selector_kind(),
                SelectorKind::Regex(crate::seclang::RegexStyle::Bare)
            );
            assert_eq!(target.regex_body().unwrap().text(), "^name$");
            assert!(draft.requirements().next().is_none());
            assert_eq!(draft.unavailable_capabilities().len(), 0);
        })
        .unwrap();
    }

    #[test]
    fn setvar_operations_arithmetic_and_first_equals_are_exact() {
        let source = concat!(
            "SecAction \"setvar:TX.assign=a=b\"\n",
            "SecAction \"setvar:TX.add=+9223372036854775807\"\n",
            "SecAction \"setvar:TX.subtract=-5\"\n",
            "SecAction \"setvar:!TX.%{TX.key}\"\n",
        );
        let values = with_draft(source, |draft| {
            draft
                .actions()
                .map(|action| {
                    let ActionOperandView::SetVar(setvar) = action.operand() else {
                        panic!("setvar")
                    };
                    (
                        setvar.operation(),
                        setvar.key().text().to_owned(),
                        setvar.value().map(|value| value.text().to_owned()),
                        setvar.constant(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap();
        assert_eq!(
            values,
            [
                (
                    SetVarOperation::Assign,
                    "assign".to_owned(),
                    Some("a=b".to_owned()),
                    None
                ),
                (
                    SetVarOperation::Add,
                    "add".to_owned(),
                    Some("9223372036854775807".to_owned()),
                    Some(i64::MAX),
                ),
                (
                    SetVarOperation::Subtract,
                    "subtract".to_owned(),
                    Some("5".to_owned()),
                    Some(5)
                ),
                (SetVarOperation::Delete, "%{TX.key}".to_owned(), None, None),
            ]
        );
        for source in [
            "SecAction \"setvar:TX.key\"\n",
            "SecAction \"setvar:IP.key=1\"\n",
            "SecAction \"setvar:!TX.key=1\"\n",
            "SecAction \"setvar:TX.=1\"\n",
            "SecAction \"setvar:TX.key=+\"\n",
        ] {
            assert!(
                matches!(with_draft(source, |_| ()), Err(SemanticError::SetVarSyntax { .. })),
                "{source}"
            );
        }
        let overflow_source = "SecAction \"setvar:TX.key=+9223372036854775808\"\n";
        let number_offset = overflow_source.find("922").unwrap();
        assert!(matches!(
            with_draft(overflow_source, |_| ()),
            Err(SemanticError::IntegerOperand {
                kind: IntegerOperandErrorKind::Overflow,
                position: SourcePosition { line: 1, column, byte_offset },
            }) if byte_offset == number_offset && column == number_offset + 1
        ));
    }

    #[test]
    fn initcol_and_ctl_commands_keep_only_unsupported_actions_unavailable() {
        let source = concat!(
            "SecAction \"initcol:GLOBAL=key\"\n",
            "SecAction \"ctl:auditEngine=oN\"\n",
            "SecAction \"ctl:forceRequestBodyVariable=off\"\n",
            "SecAction \"ctl:requestBodyProcessor=json\"\n",
            "SecAction \"ctl:ruleRemoveByTag=criterion\"\n",
            "SecAction \"ctl:ruleRemoveTargetByTag=tag;XML:/root/node\"\n",
        );
        with_draft(source, |draft| {
            assert_eq!(draft.structured_action_count(), 6);
            assert_eq!(draft.ctl_target_count(), 1);
            let operands = draft.actions().map(|action| action.operand()).collect::<Vec<_>>();
            assert!(matches!(operands[0], ActionOperandView::InitCol(_)));
            assert!(matches!(operands[1], ActionOperandView::Ctl(CtlView::AuditEngine(AuditEngineMode::On))));
            assert!(matches!(
                operands[2],
                ActionOperandView::Ctl(CtlView::ForceRequestBodyVariable(OnOff::Off))
            ));
            assert!(matches!(
                operands[3],
                ActionOperandView::Ctl(CtlView::RequestBodyProcessor(RequestBodyProcessor::Json))
            ));
            assert!(matches!(operands[4], ActionOperandView::Ctl(CtlView::RuleRemoveByTag(value)) if value.text() == "criterion"));
            let ActionOperandView::Ctl(CtlView::RuleRemoveTargetByTag { tag, target }) = operands[5] else {
                panic!("target ctl")
            };
            assert_eq!(tag.text(), "tag");
            assert_eq!(target.collection(), CollectionKind::Xml);
            assert_eq!(target.selector_kind(), SelectorKind::Path);
            assert_eq!(target.selector().unwrap().text(), "/root/node");
            assert_eq!(
                draft
                    .unavailable_capabilities()
                    .map(|record| record.capability())
                    .collect::<Vec<_>>(),
                [
                    UnavailableCapability::InitCol,
                    UnavailableCapability::Ctl,
                ]
            );
        })
        .unwrap();
    }

    #[test]
    fn structured_action_grammars_reject_upstream_coercions_and_spacing() {
        for source in [
            "SecAction \"initcol:resource=key\"\n",
            "SecAction \"initcol:global=\"\n",
            "SecAction \"ctl:AuditEngine=On\"\n",
            "SecAction \"ctl:ruleEngine=Off\"\n",
            "SecAction \"ctl:requestBodyProcessor=text\"\n",
            "SecAction \"ctl:'auditEngine =On'\"\n",
            "SecAction \"ctl:'auditEngine= On'\"\n",
            "SecAction \"ctl:ruleRemoveByTag=a;b\"\n",
            "SecAction \"ctl:ruleRemoveTargetByTag=tagARGS\"\n",
            "SecAction \"ctl:ruleRemoveTargetByTag=tag;ARGS;TX\"\n",
            "SecAction \"ctl:'ruleRemoveTargetByTag=tag;ARGS '\"\n",
            "SecAction \"ctl:ruleRemoveTargetByTag=tag;!ARGS\"\n",
            "SecAction \"ctl:ruleRemoveTargetByTag=tag;ARGS|TX\"\n",
            "SecAction \"ctl:ruleRemoveTargetByTag=tag;UNKNOWN\"\n",
        ] {
            assert!(with_draft(source, |_| ()).is_err(), "{source}");
        }
        let error = with_draft("SecAction \"ctl:ruleRemoveTargetByTag=private;UNKNOWN\"\n", |_| ()).unwrap_err();
        let mut source = std::error::Error::source(&error);
        let rendered = format!("{error:?} {error}").to_ascii_lowercase();
        assert!(!rendered.contains("private") && !rendered.contains("unknown"));
        while let Some(cause) = source {
            let rendered = format!("{cause:?} {cause}").to_ascii_lowercase();
            assert!(!rendered.contains("private") && !rendered.contains("unknown"));
            source = cause.source();
        }
    }

    #[test]
    fn structured_action_limits_are_exact_and_slices_keep_continuation_provenance() {
        let source = "SecAction \"setvar:TX.é%{TX.key}=left\\\nright\"\nSecAction \"ctl:ruleRemoveTargetByTag=tag;REQUEST_FILENAME\"\n";
        let exact = OperandLimits {
            max_structured_actions: 2,
            max_ctl_targets: 1,
            max_setvar_key_bytes: 11,
            max_ctl_value_bytes: 20,
            ..OperandLimits::default()
        };
        with_draft_limits(
            source,
            SemanticLimits {
                operands: exact,
                ..SemanticLimits::default()
            },
            |draft| {
                let ActionOperandView::SetVar(setvar) = draft.actions().next().unwrap().operand() else {
                    panic!("setvar")
                };
                assert_eq!(setvar.key().text(), "é%{TX.key}");
                assert_eq!(setvar.value().unwrap().text(), "leftright");
                assert_eq!(setvar.value().unwrap().span().start.line, 1);
                assert_eq!(setvar.value().unwrap().span().end.line, 2);
            },
        )
        .unwrap();
        for operands in [
            OperandLimits {
                max_structured_actions: 1,
                ..exact
            },
            OperandLimits {
                max_ctl_targets: 0,
                ..exact
            },
            OperandLimits {
                max_setvar_key_bytes: 10,
                ..exact
            },
            OperandLimits {
                max_ctl_value_bytes: 19,
                ..exact
            },
        ] {
            assert!(with_draft_limits(
                source,
                SemanticLimits {
                    operands,
                    ..SemanticLimits::default()
                },
                |_| ()
            )
            .is_err());
        }

        let owned_source = scan_source(
            "structured-owned",
            b"SecAction \"setvar:TX.key=%{TX.value}\"\n",
            SourceLimits::default(),
        )
        .unwrap();
        let inherited = parse_syntax(&owned_source, SyntaxLimits::default())
            .unwrap()
            .retained_charged_bytes();
        let exact_owned = with_draft("SecAction \"setvar:TX.key=%{TX.value}\"\n", |draft| {
            draft.semantic_charged_bytes()
        })
        .unwrap();
        let syntax = parse_syntax(&owned_source, SyntaxLimits::default()).unwrap();
        assert!(analyze_semantic_draft_inner(
            vec![syntax],
            SemanticLimits {
                max_owned_bytes: exact_owned,
                max_retained_bytes: inherited + exact_owned,
                ..SemanticLimits::default()
            },
            false,
            false,
        )
        .is_ok());
        let syntax = parse_syntax(&owned_source, SyntaxLimits::default()).unwrap();
        assert!(matches!(
            analyze_semantic_draft_inner(
                vec![syntax],
                SemanticLimits {
                    max_owned_bytes: exact_owned - 1,
                    max_retained_bytes: inherited + exact_owned,
                    ..SemanticLimits::default()
                },
                false,
                false,
            ),
            Err(SemanticError::OwnedByteLimit { .. } | SemanticError::PreflightOwnedByteLimit { .. })
        ));
    }

    #[test]
    fn generated_setvar_template_slices_reconstruct_exactly() {
        for index in 0..128usize {
            let key = format!("key{index}%{{TX.suffix}}tail");
            let value = format!("left%{{TX.value}}right={index}");
            let source = format!("SecAction \"setvar:TX.{key}={value}\"\n");
            with_draft(&source, |draft| {
                let ActionOperandView::SetVar(setvar) = draft.actions().next().unwrap().operand() else {
                    panic!("generated setvar")
                };
                assert_eq!(setvar.operation(), SetVarOperation::Assign);
                assert_eq!(setvar.key().text(), key);
                assert_eq!(setvar.value().unwrap().text(), value);
                for slice in [setvar.key(), setvar.value().unwrap()] {
                    assert_eq!(
                        slice.segments().map(|segment| segment.text()).collect::<String>(),
                        slice.text()
                    );
                    assert_eq!(slice.expansion_count(), 1);
                }
            })
            .unwrap();
        }
    }

    #[test]
    fn templates_parse_exact_sources_keys_adjacency_and_literal_percent_forms() {
        let result = with_draft(
            "SecAction \"msg:%literal} %{tX.Key}%{ARGS.name}%{REQUEST_HEADERS.Content-Type}%{MATCHED_VAR}%{MATCHED_VAR_NAME}%{REMOTE_ADDR}%{REQUEST_LINE}\"\n",
            |draft| {
                let template = draft.actions().next().unwrap().template().unwrap();
                let segments = template.segments().map(|segment| {
                    let expansion = match segment.kind() {
                        TemplateSegmentKind::Literal => None,
                        TemplateSegmentKind::Expansion(value) => Some((value.source(), value.source_spelling().text().to_owned(), value.key().map(|key| key.text().to_owned()))),
                    };
                    (segment.text().to_owned(), expansion)
                }).collect::<Vec<_>>();
                (template.text().to_owned(), template.expansion_count(), segments)
            },
        ).unwrap();
        assert_eq!(result.1, 7);
        assert_eq!(result.2.len(), 8);
        assert_eq!(result.2[0].0, "%literal} ");
        assert_eq!(
            result.2[1].1,
            Some((MacroSource::Tx, "tX".to_owned(), Some("Key".to_owned())))
        );
        assert!(result.0.ends_with("%{REQUEST_LINE}"));

        let adjacent = with_draft("SecAction \"msg:%{TX.a}%{ARGS.b}\"\n", |draft| {
            let template = draft.actions().next().unwrap().template().unwrap();
            (template.segments().len(), template.expansion_count())
        })
        .unwrap();
        assert_eq!(adjacent, (2, 2));
        assert!(matches!(
            with_draft("SecRule ARGS \"@eq\" \"id:1\"\n", |_| ()),
            Err(SemanticError::OperatorOperandPresence {
                expected: OperatorOperandContract::NonEmpty,
                ..
            })
        ));
    }

    #[test]
    fn malformed_macro_errors_are_typed_directional_and_redacted() {
        let cases = [
            ("%{TX.x", MacroErrorKind::Unclosed),
            ("%{}", MacroErrorKind::EmptyBody),
            ("%{TX.%{ARGS.x}}", MacroErrorKind::Nested),
            ("%{TX.x!}", MacroErrorKind::InvalidCharacter),
            ("%{SECRET.x}", MacroErrorKind::UnknownSource),
            ("%{TX}", MacroErrorKind::MissingKey),
            ("%{REMOTE_ADDR.x}", MacroErrorKind::ForbiddenKey),
        ];
        for (value, kind) in cases {
            let source = format!("SecAction \"msg:prefix{value}\"\n");
            let error = with_draft(&source, |_| ()).unwrap_err();
            assert!(matches!(error, SemanticError::MacroSyntax { kind: actual, .. } if actual == kind));
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("SECRET") && !rendered.contains("prefix"));
            let mut cause = std::error::Error::source(&error);
            while let Some(source) = cause {
                assert!(!format!("{source:?} {source}").contains("SECRET"));
                cause = source.source();
            }
        }
    }

    #[test]
    fn macro_capability_matrix_is_exact_and_static_fields_keep_macro_text_literal() {
        let mut source = String::new();
        for (index, &kind) in OperatorKind::ALL.iter().enumerate() {
            let operand = if operator_macro_capable(kind) {
                if matches!(
                    kind,
                    OperatorKind::Eq | OperatorKind::Ge | OperatorKind::Gt | OperatorKind::Lt
                ) {
                    "1"
                } else {
                    "literal"
                }
            } else if matches!(
                kind,
                OperatorKind::DetectSqli
                    | OperatorKind::DetectXss
                    | OperatorKind::UnconditionalMatch
                    | OperatorKind::ValidateUrlEncoding
                    | OperatorKind::ValidateUtf8Encoding
            ) {
                ""
            } else {
                valid_operator_argument(kind)
            };
            source.push_str(&format!(
                "SecRule ARGS \"@{} {operand}\" \"id:{}\"\n",
                kind.as_str(),
                index + 1
            ));
        }
        for &kind in ActionKind::ALL {
            let entry = match kind {
                ActionKind::AuditLog
                | ActionKind::Block
                | ActionKind::Capture
                | ActionKind::Chain
                | ActionKind::Deny
                | ActionKind::Log
                | ActionKind::MultiMatch
                | ActionKind::NoAuditLog
                | ActionKind::NoLog
                | ActionKind::Pass => kind.as_str().to_owned(),
                ActionKind::Id => "id:1".to_owned(),
                ActionKind::Phase => "phase:2".to_owned(),
                ActionKind::Severity => "severity:3".to_owned(),
                ActionKind::Status => "status:403".to_owned(),
                ActionKind::Skip => "skip:1".to_owned(),
                ActionKind::SetVar => "setvar:TX.key=literal".to_owned(),
                ActionKind::InitCol => "initcol:global=literal".to_owned(),
                ActionKind::Msg | ActionKind::LogData => format!("{}:literal", kind.as_str()),
                ActionKind::Ctl => "ctl:auditEngine=Off".to_owned(),
                _ => format!("{}:%{{BROKEN", kind.as_str()),
            };
            source.push_str(&format!("SecAction \"{entry}\"\n"));
        }
        let (operators, actions) = with_draft(&source, |draft| {
            (
                draft
                    .operators()
                    .map(|operator| (operator.kind(), operator.template().is_some()))
                    .collect::<Vec<_>>(),
                draft
                    .actions()
                    .skip(OperatorKind::ALL.len())
                    .map(|action| (action.kind(), action.template().is_some()))
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap();
        assert_eq!(operators.len(), OperatorKind::ALL.len());
        for ((kind, has_template), expected) in operators.into_iter().zip(OperatorKind::ALL) {
            assert_eq!(kind, *expected);
            assert_eq!(has_template, operator_macro_capable(*expected), "{expected:?}");
        }
        assert_eq!(actions.len(), ActionKind::ALL.len());
        for ((kind, has_template), expected) in actions.into_iter().zip(ActionKind::ALL) {
            assert_eq!(kind, ActionEnvelopeKind::Action(*expected));
            assert_eq!(has_template, action_macro_capable(*expected), "{expected:?}");
        }
    }

    #[test]
    fn every_operator_contract_publishes_one_typed_owner_bound_operand() {
        let source = concat!(
            "SecRule ARGS \"@detectSQLi\" \"id:1\"\n",
            "SecRule ARGS \"\" \"id:2\"\n",
            "SecRule ARGS \"@contains needle\" \"id:3\"\n",
            "SecRule ARGS \"@eq +001\" \"id:4\"\n",
            "SecRule ARGS \"@pm alpha\tbeta alpha\" \"id:5\"\n",
            "SecRule ARGS \"@pmFromFile rules/a.data rules/b.data\" \"id:6\"\n",
            "SecRule ARGS \"@ipMatch 192.0.2.129/24,2001:db8::1/64\" \"id:7\"\n",
            "SecRule ARGS \"@validateByteRange 0, 2-4,4\" \"id:8\"\n",
        );
        with_draft(source, |draft| {
            let mut operators = draft.operators();
            assert!(matches!(
                operators.next().unwrap().operand(),
                OperatorOperandView::NoArgument
            ));
            let OperatorOperandView::Regex(regex) = operators.next().unwrap().operand() else {
                panic!("implicit rx operand")
            };
            assert_eq!(regex.text(), "");
            let OperatorOperandView::StringTemplate(template) = operators.next().unwrap().operand() else {
                panic!("string template operand")
            };
            assert_eq!(template.text(), "needle");
            let OperatorOperandView::IntegerTemplate { template, constant } = operators.next().unwrap().operand()
            else {
                panic!("integer template operand")
            };
            assert_eq!((template.text(), constant), ("+001", Some(1)));
            let OperatorOperandView::PhraseList(phrases) = operators.next().unwrap().operand() else {
                panic!("phrase list operand")
            };
            assert_eq!(phrases.text().text(), "alpha\tbeta alpha");
            assert_eq!(
                phrases.phrases().map(|phrase| phrase.text()).collect::<Vec<_>>(),
                ["alpha", "beta", "alpha"]
            );
            let OperatorOperandView::ExternalAssets(assets) = operators.next().unwrap().operand() else {
                panic!("external assets operand")
            };
            assert_eq!(assets.text().text(), "rules/a.data rules/b.data");
            assert_eq!(
                assets
                    .assets()
                    .map(|asset| (asset.document_index(), asset.handle()))
                    .collect::<Vec<_>>(),
                [(0, "rules/a.data"), (0, "rules/b.data")]
            );
            let OperatorOperandView::IpNetworks(networks) = operators.next().unwrap().operand() else {
                panic!("IP network operand")
            };
            assert_eq!(networks.text().text(), "192.0.2.129/24,2001:db8::1/64");
            assert_eq!(
                networks
                    .networks()
                    .map(|network| (network.address(), network.prefix()))
                    .collect::<Vec<_>>(),
                [("192.0.2.0".parse().unwrap(), 24), ("2001:db8::".parse().unwrap(), 64),]
            );
            let OperatorOperandView::ByteSet(bytes) = operators.next().unwrap().operand() else {
                panic!("byte-set operand")
            };
            assert_eq!(bytes.text().text(), "0, 2-4,4");
            assert_eq!(bytes.member_count(), 3);
            assert!(bytes.contains(0) && bytes.contains(2) && bytes.contains(3) && bytes.contains(4));
            assert!(!bytes.contains(1) && !bytes.contains(5));
            assert!(operators.next().is_none());
        })
        .unwrap();
    }

    #[test]
    fn strict_integer_templates_accept_i64_edges_and_reject_coercions() {
        for (value, expected) in [
            ("+000", 0),
            ("9223372036854775807", i64::MAX),
            ("-9223372036854775808", i64::MIN),
        ] {
            let source = format!("SecRule ARGS \"@eq {value}\" \"id:1\"\n");
            let actual = with_draft(&source, |draft| {
                let OperatorOperandView::IntegerTemplate { constant, .. } = draft.operators().next().unwrap().operand()
                else {
                    panic!("integer operand")
                };
                constant
            });
            assert_eq!(actual, Ok(Some(expected)));
        }
        let macro_value = with_draft("SecRule ARGS \"@eq %{TX.value}\" \"id:1\"\n", |draft| {
            let OperatorOperandView::IntegerTemplate { constant, .. } = draft.operators().next().unwrap().operand()
            else {
                panic!("integer operand")
            };
            constant
        });
        assert_eq!(macro_value, Ok(None));
        for (value, kind) in [
            ("+", IntegerOperandErrorKind::Invalid),
            ("1 2", IntegerOperandErrorKind::Invalid),
            ("1x", IntegerOperandErrorKind::Invalid),
            ("9223372036854775808", IntegerOperandErrorKind::Overflow),
            ("-9223372036854775809", IntegerOperandErrorKind::Overflow),
        ] {
            let source = format!("SecRule ARGS \"@eq {value}\" \"id:1\"\n");
            assert!(matches!(
                with_draft(&source, |_| ()),
                Err(SemanticError::IntegerOperand { kind: actual, .. }) if actual == kind
            ));
        }
    }

    #[test]
    fn phrase_asset_ip_and_byte_grammars_fail_closed_without_echoing_content() {
        let cases = [
            ("SecRule ARGS \"@pm safe|private\" \"id:1\"\n", "PhraseSyntax"),
            ("SecRule ARGS \"@pmFromFile /private.data\" \"id:1\"\n", "UnsafeAsset"),
            ("SecRule ARGS \"@pmFromFile ../private.data\" \"id:1\"\n", "UnsafeAsset"),
            ("SecRule ARGS \"@pmFromFile https:private\" \"id:1\"\n", "UnsafeAsset"),
            (
                "SecRule ARGS \"@ipMatch private.example\" \"id:1\"\n",
                "IpNetworkSyntax",
            ),
            ("SecRule ARGS \"@ipMatch 192.0.2.1/33\" \"id:1\"\n", "IpNetworkSyntax"),
            ("SecRule ARGS \"@validateByteRange 9-1\" \"id:1\"\n", "ByteSetSyntax"),
            ("SecRule ARGS \"@validateByteRange 256\" \"id:1\"\n", "ByteSetSyntax"),
        ];
        for (source, family) in cases {
            let error = with_draft(source, |_| ()).unwrap_err();
            assert!(format!("{error:?}").contains(family));
            let rendered = format!("{error:?} {error}").to_ascii_lowercase();
            assert!(!rendered.contains("private") && !rendered.contains("example"));
        }

        let phrase = with_draft("SecRule ARGS \"@pm ab|cd\" \"id:1\"\n", |_| ()).unwrap_err();
        assert!(matches!(
            phrase,
            SemanticError::PhraseSyntax {
                position: SourcePosition {
                    line: 1,
                    column: 21,
                    byte_offset: 20,
                }
            }
        ));
        let integer = with_draft("SecRule ARGS \"@eq 12x\" \"id:1\"\n", |_| ()).unwrap_err();
        assert!(matches!(
            integer,
            SemanticError::IntegerOperand {
                position: SourcePosition {
                    line: 1,
                    column: 21,
                    byte_offset: 20,
                },
                ..
            }
        ));
    }

    #[test]
    fn captured_upstream_operand_divergences_remain_explicit() {
        let cases = [
            // Edgion and ModSecurity accept multiple handles; Coraza treats this as one raw path.
            ("SecRule ARGS \"@pmFromFile a.data b.data\" \"id:1\"\n", true),
            // ModSecurity can fetch HTTPS assets; this parsing profile deliberately forbids schemes and IO.
            ("SecRule ARGS \"@pmFromFile https:data\" \"id:1\"\n", false),
            // Upstream phrase parsers support Snort hex forms; partial interpretation is forbidden here.
            ("SecRule ARGS \"@pm a|20|b\" \"id:1\"\n", false),
            // Coraza integer conversion can coerce invalid text to zero; this profile is strict.
            ("SecRule ARGS \"@eq invalid\" \"id:1\"\n", false),
        ];
        for (source, accepted) in cases {
            assert_eq!(with_draft(source, |_| ()).is_ok(), accepted, "{source}");
        }
    }

    #[test]
    fn operator_list_limits_accept_exact_and_reject_first_excess() {
        let phrases = "SecRule ARGS \"@pm a bb\" \"id:1\"\n";
        let exact = OperandLimits {
            max_phrases_per_operator: 2,
            max_phrases: 2,
            max_phrase_bytes: 3,
            ..OperandLimits::default()
        };
        assert!(with_draft_limits(
            phrases,
            SemanticLimits {
                operands: exact,
                ..SemanticLimits::default()
            },
            |_| ()
        )
        .is_ok());
        for (limits, family) in [
            (
                OperandLimits {
                    max_phrases_per_operator: 1,
                    ..exact
                },
                "OperatorPhraseLimit",
            ),
            (
                OperandLimits {
                    max_phrases: 1,
                    ..exact
                },
                "PhraseLimit",
            ),
            (
                OperandLimits {
                    max_phrase_bytes: 2,
                    ..exact
                },
                "PhraseByteLimit",
            ),
        ] {
            let error = with_draft_limits(
                phrases,
                SemanticLimits {
                    operands: limits,
                    ..SemanticLimits::default()
                },
                |_| (),
            )
            .unwrap_err();
            assert!(format!("{error:?}").contains(family));
        }

        let assets = "SecRule ARGS \"@pmFromFile a b\" \"id:1\"\n";
        for limits in [
            OperandLimits {
                max_assets_per_operator: 1,
                ..OperandLimits::default()
            },
            OperandLimits {
                max_assets: 1,
                ..OperandLimits::default()
            },
            OperandLimits {
                max_asset_bytes: 0,
                ..OperandLimits::default()
            },
        ] {
            assert!(with_draft_limits(
                assets,
                SemanticLimits {
                    operands: limits,
                    ..SemanticLimits::default()
                },
                |_| ()
            )
            .is_err());
        }
        assert!(matches!(
            with_draft_limits(
                "SecRule ARGS \"@ipMatch 192.0.2.1,198.51.100.1\" \"id:1\"\n",
                SemanticLimits {
                    operands: OperandLimits {
                        max_networks_per_operator: 1,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::OperatorNetworkLimit { limit: 1, .. })
        ));
        assert!(matches!(
            with_draft_limits(
                "SecRule ARGS \"@validateByteRange 1,2\" \"id:1\"\n",
                SemanticLimits {
                    operands: OperandLimits {
                        max_byte_range_members: 1,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::ByteRangeMemberLimit { limit: 1, .. })
        ));

        assert!(matches!(
            with_draft_limits(
                "SecRule ARGS \"@ipMatch 192.0.2.1\" \"id:1\"\nSecRule ARGS \"@ipMatch 198.51.100.1\" \"id:2\"\n",
                SemanticLimits {
                    operands: OperandLimits {
                        max_networks_per_operator: 1,
                        max_networks: 1,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::NetworkLimit { limit: 1, .. })
        ));
    }

    #[test]
    fn external_asset_validation_visits_each_decoded_byte_once() {
        let handle = "a".repeat(OperandLimits::default().max_asset_bytes);
        let operand = std::iter::repeat_n(handle.as_str(), OperandLimits::default().max_assets_per_operator)
            .collect::<Vec<_>>()
            .join(" ");
        let shape = walk_asset_words(operand.bytes(), OperandLimits::default().max_asset_bytes, |_| {}).unwrap();
        assert_eq!(shape.count, OperandLimits::default().max_assets_per_operator);
        assert_eq!(
            shape.payload_bytes,
            OperandLimits::default().max_assets_per_operator * OperandLimits::default().max_asset_bytes
        );
        assert_eq!(shape.inspected_bytes, operand.len());

        let source = format!("SecRule ARGS \"@pmFromFile {operand}\" \"id:1\"\n");
        assert_eq!(
            with_draft(&source, |draft| draft.external_asset_count()),
            Ok(OperandLimits::default().max_assets_per_operator)
        );
    }

    #[test]
    fn operator_requirements_and_b2_error_precedence_are_exact() {
        let requirements = with_draft(
            "SecRule ARGS \"@pm a\" \"id:1\"\nSecRule ARGS \"@pmFromFile a.data\" \"id:2\"\n",
            |draft| {
                draft
                    .requirements()
                    .map(|record| record.requirement())
                    .collect::<Vec<_>>()
            },
        )
        .unwrap();
        assert_eq!(
            requirements,
            [
                CompileRequirement::PhraseMatcherCompilation,
                CompileRequirement::ExternalPhraseAsset,
                CompileRequirement::PhraseMatcherCompilation,
            ]
        );
        let constrained = SemanticLimits {
            operands: OperandLimits {
                max_templates: 0,
                max_segments: 0,
                max_expansions: 0,
                ..OperandLimits::default()
            },
            ..SemanticLimits::default()
        };
        assert!(matches!(
            with_draft_limits("SecRule ARGS \"@eq\" \"id:1\"\n", constrained, |_| ()),
            Err(SemanticError::OperatorOperandPresence { .. })
        ));
        assert!(matches!(
            with_draft_limits("SecRule ARGS \"@eq bad\" \"id:1\"\n", constrained, |_| ()),
            Err(SemanticError::IntegerOperand { .. })
        ));

        let count_first = SemanticLimits {
            operands: OperandLimits {
                max_assets_per_operator: 0,
                max_networks_per_operator: 0,
                ..OperandLimits::default()
            },
            ..SemanticLimits::default()
        };
        assert!(matches!(
            with_draft_limits(
                "SecRule ARGS \"@pmFromFile ../private.data safe.data\" \"id:1\"\n",
                count_first,
                |_| ()
            ),
            Err(SemanticError::UnsafeAsset { .. })
        ));
        assert!(matches!(
            with_draft_limits(
                "SecRule ARGS \"@pmFromFile too-long ../private.data\" \"id:1\"\n",
                SemanticLimits {
                    operands: OperandLimits {
                        max_asset_bytes: 1,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::UnsafeAsset { .. })
        ));
        assert!(matches!(
            with_draft_limits(
                "SecRule ARGS \"@ipMatch invalid,192.0.2.1\" \"id:1\"\n",
                count_first,
                |_| ()
            ),
            Err(SemanticError::IpNetworkSyntax { .. })
        ));
    }

    #[test]
    fn decoded_copy_limit_is_shared_by_b1_templates_and_b2_static_operands() {
        for source in [
            concat!(
                "SecRule ARGS \"@rx a\\\"b\" \"id:1\"\n",
                "SecRule ARGS \"@contains c\\\"d\" \"id:2\"\n",
            ),
            concat!(
                "SecRule ARGS \"@contains c\\\"d\" \"id:1\"\n",
                "SecRule ARGS \"@rx a\\\"b\" \"id:2\"\n",
            ),
        ] {
            assert_eq!(
                with_draft_limits(
                    source,
                    SemanticLimits {
                        operands: OperandLimits {
                            max_decoded_bytes: 6,
                            ..OperandLimits::default()
                        },
                        ..SemanticLimits::default()
                    },
                    |draft| draft.decoded_operand_bytes()
                ),
                Ok(6)
            );
            assert!(matches!(
                with_draft_limits(
                    source,
                    SemanticLimits {
                        operands: OperandLimits {
                            max_decoded_bytes: 5,
                            ..OperandLimits::default()
                        },
                        ..SemanticLimits::default()
                    },
                    |_| ()
                ),
                Err(SemanticError::DecodedOperandByteLimit { limit: 5, .. })
            ));
        }
    }

    #[test]
    fn escaped_utf8_continued_phrase_ranges_keep_exact_source_provenance() {
        with_draft("SecRule ARGS \"@pm é\\\nleft\\\"right tail\" \"id:1\"\n", |draft| {
            let OperatorOperandView::PhraseList(phrases) = draft.operators().next().unwrap().operand() else {
                panic!("phrase list")
            };
            let phrases = phrases.phrases().collect::<Vec<_>>();
            assert_eq!(
                phrases.iter().map(|phrase| phrase.text()).collect::<Vec<_>>(),
                ["éleft\"right", "tail"]
            );
            assert_eq!(phrases[0].span().start.line, 1);
            assert_eq!(phrases[0].span().end.line, 2);
            assert_eq!(phrases[1].span().start.line, 2);
        })
        .unwrap();
    }

    #[test]
    fn template_expansion_and_arena_limits_are_exact_and_atomic() {
        let exact_value = "%{TX.x}".repeat(1_024);
        let exact = format!("SecAction \"msg:{exact_value}\"\n");
        assert_eq!(with_draft(&exact, |draft| draft.expansion_count()), Ok(1_024));
        let excess = format!("SecAction \"msg:{}\"\n", "%{TX.x}".repeat(1_025));
        assert!(matches!(
            with_draft(&excess, |_| ()),
            Err(SemanticError::TemplateExpansionLimit { limit: 1_024, .. })
        ));

        let literal = "SecAction \"msg:abc\"\n";
        for limits in [
            OperandLimits {
                max_templates: 0,
                ..OperandLimits::default()
            },
            OperandLimits {
                max_segments: 0,
                ..OperandLimits::default()
            },
            OperandLimits {
                max_operand_bytes: 2,
                ..OperandLimits::default()
            },
            OperandLimits {
                max_scanned_bytes: 2,
                ..OperandLimits::default()
            },
        ] {
            assert!(with_draft_limits(
                literal,
                SemanticLimits {
                    operands: limits,
                    ..SemanticLimits::default()
                },
                |_| ()
            )
            .is_err());
        }
        assert_eq!(
            with_draft(literal, |draft| (
                draft.template_count(),
                draft.template_segment_count()
            )),
            Ok((1, 1))
        );

        let segmented = "SecAction \"msg:a%{TX.x}b\"\n";
        assert!(with_draft_limits(
            segmented,
            SemanticLimits {
                operands: OperandLimits {
                    max_segments: 3,
                    max_expansions: 1,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        )
        .is_ok());
        assert!(matches!(
            with_draft_limits(
                segmented,
                SemanticLimits {
                    operands: OperandLimits {
                        max_segments: 2,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::TemplateSegmentLimit { limit: 2, .. })
        ));

        let (owned, retained) = with_draft(segmented, |draft| {
            (draft.semantic_charged_bytes(), draft.retained_charged_bytes())
        })
        .unwrap();
        assert!(with_draft_limits(
            segmented,
            SemanticLimits {
                max_owned_bytes: owned,
                max_retained_bytes: retained,
                ..SemanticLimits::default()
            },
            |_| ()
        )
        .is_ok());
        assert!(matches!(
            with_draft_limits(
                segmented,
                SemanticLimits {
                    max_owned_bytes: owned - 1,
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::OwnedByteLimit { .. })
        ));
        assert!(matches!(
            with_draft_limits(
                segmented,
                SemanticLimits {
                    max_retained_bytes: retained - 1,
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::RetainedByteLimit { .. })
        ));

        let escaped = "SecRule ARGS \"@eq a\\\"b%{TX.key}\" \"id:1\"\n";
        assert!(with_draft_limits(
            escaped,
            SemanticLimits {
                operands: OperandLimits {
                    max_decoded_bytes: 12,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        )
        .is_ok());
        assert!(matches!(
            with_draft_limits(
                escaped,
                SemanticLimits {
                    operands: OperandLimits {
                        max_decoded_bytes: 11,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::DecodedOperandByteLimit { limit: 11, .. })
        ));

        let cumulative = "SecAction \"msg:abc\"\nSecAction \"msg:def\"\n";
        assert!(with_draft_limits(
            cumulative,
            SemanticLimits {
                operands: OperandLimits {
                    max_scanned_bytes: 6,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        )
        .is_ok());
        assert!(matches!(
            with_draft_limits(
                cumulative,
                SemanticLimits {
                    operands: OperandLimits {
                        max_scanned_bytes: 5,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::ScannedOperandByteLimit { limit: 5, .. })
        ));

        let cumulative_macros = "SecAction \"msg:%{TX.a}\"\nSecAction \"msg:%{TX.b}\"\n";
        assert!(with_draft_limits(
            cumulative_macros,
            SemanticLimits {
                operands: OperandLimits {
                    max_templates: 2,
                    max_expansions: 2,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        )
        .is_ok());
        assert!(matches!(
            with_draft_limits(
                cumulative_macros,
                SemanticLimits {
                    operands: OperandLimits {
                        max_templates: 1,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::TemplateLimit { limit: 1, .. })
        ));
        assert!(matches!(
            with_draft_limits(
                cumulative_macros,
                SemanticLimits {
                    operands: OperandLimits {
                        max_expansions: 1,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::ExpansionLimit { limit: 1, .. })
        ));
    }

    #[test]
    fn decoded_operator_and_action_subranges_preserve_source_provenance() {
        let operator = with_draft("SecRule ARGS \"@eq a\\\"b%{TX.key}\" \"id:1\"\n", |draft| {
            let template = draft.operators().next().unwrap().template().unwrap();
            let segments = template
                .segments()
                .map(|segment| (segment.text().to_owned(), segment.span()))
                .collect::<Vec<_>>();
            (template.text().to_owned(), draft.decoded_operand_bytes(), segments)
        })
        .unwrap();
        assert_eq!((operator.0.as_str(), operator.1), ("a\"b%{TX.key}", 12));
        assert_eq!(operator.2[0].0, "a\"b");
        assert!(operator.2[0].1.end.column > operator.2[0].1.start.column + operator.2[0].0.len());

        let action = with_draft("SecAction \"msg:é\\\n%{TX.key}\"\n", |draft| {
            draft
                .actions()
                .next()
                .unwrap()
                .template()
                .unwrap()
                .segments()
                .map(|segment| (segment.text().to_owned(), segment.span()))
                .collect::<Vec<_>>()
        })
        .unwrap();
        assert_eq!(action.len(), 2);
        assert_eq!(action[0].0, "é");
        assert_eq!(action[1].1.start.line, 2);

        let continued_operator = with_draft("SecRule ARGS \"@eq é\\\n\\\"x%{TX.key}\" \"id:1\"\n", |draft| {
            draft
                .operators()
                .next()
                .unwrap()
                .template()
                .unwrap()
                .segments()
                .map(|segment| (segment.text().to_owned(), segment.span()))
                .collect::<Vec<_>>()
        })
        .unwrap();
        assert_eq!(continued_operator[0].0, "é\"x");
        assert_eq!(continued_operator[0].1.start.line, 1);
        assert_eq!(continued_operator[0].1.end.line, 2);
        assert_eq!(continued_operator[1].1.start.line, 2);
    }

    #[test]
    fn long_literal_percent_input_remains_one_bounded_segment() {
        let value = "%x}".repeat(100_000);
        let source = format!("SecAction \"msg:{value}\"\n");
        let result = with_draft(&source, |draft| {
            let template = draft.actions().next().unwrap().template().unwrap();
            (
                template.segments().len(),
                template.expansion_count(),
                draft.scanned_operand_bytes(),
            )
        })
        .unwrap();
        assert_eq!(result, (1, 0, value.len()));
    }

    #[test]
    fn bounded_generated_templates_reconstruct_exactly_without_empty_segments() {
        for mask in 0u16..256 {
            let mut value = String::new();
            for bit in 0..8 {
                if mask & (1 << bit) == 0 {
                    value.push_str("lit%");
                } else {
                    value.push_str("%{TX.k}");
                }
            }
            let source = format!("SecAction \"msg:{value}\"\n");
            let rebuilt = with_draft(&source, |draft| {
                let template = draft.actions().next().unwrap().template().unwrap();
                let segments = template
                    .segments()
                    .map(|segment| segment.text().to_owned())
                    .collect::<Vec<_>>();
                (template.text().to_owned(), segments)
            })
            .unwrap();
            assert_eq!(rebuilt.0, value);
            assert!(rebuilt.1.iter().all(|segment| !segment.is_empty()));
            assert_eq!(rebuilt.1.concat(), value);
        }
    }

    #[test]
    fn empty_and_multiple_documents_preserve_caller_order() {
        let empty = analyze_semantic_draft(Vec::new(), SemanticLimits::default()).expect("empty");
        assert_eq!((empty.document_count(), empty.directive_count()), (0, 0));
        let first = scan_source("first", b"SecAction \"id:1\"\n", SourceLimits::default()).unwrap();
        let second = scan_source("second", b"SecAction \"id:2\"\n", SourceLimits::default()).unwrap();
        let docs = vec![
            parse_syntax(&first, SyntaxLimits::default()).unwrap(),
            parse_syntax(&second, SyntaxLimits::default()).unwrap(),
        ];
        let draft = analyze_semantic_draft_inner(docs, SemanticLimits::default(), false, false).unwrap();
        assert_eq!(
            draft.actions().map(|action| action.local_value()).collect::<Vec<_>>(),
            [LocalActionValue::Id(1), LocalActionValue::Id(2)]
        );
        assert_eq!(
            draft.directives().map(|view| view.document_index()).collect::<Vec<_>>(),
            [0, 1]
        );
    }

    #[test]
    fn preflight_limits_precede_content_and_error_is_redacted() {
        let source = scan_source(
            "redacted",
            b"SecAction \"secret_action:value\"\n",
            SourceLimits::default(),
        )
        .unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        let limits = SemanticLimits {
            max_directives: 0,
            ..SemanticLimits::default()
        };
        assert!(matches!(
            analyze_one_semantic_draft(syntax, limits),
            Err(SemanticError::DirectiveLimit { limit: 0 })
        ));
        let error = with_draft("SecAction \"secret_action:value\"\n", |_| ()).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("secret_action"));
    }

    #[test]
    fn exact_owned_and_retained_limits_are_enforced() {
        let source = scan_source("limits", b"SecAction \"id:1\"\n", SourceLimits::default()).unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        let inherited = syntax.retained_charged_bytes();
        let fixed = size_of::<SyntaxDocument<'_>>()
            + size_of::<DirectiveSemanticRecord>()
            + size_of::<SemanticActionRecord<'_>>()
            + size_of::<ActionOperandRecord<'_>>();
        let mut limits = SemanticLimits {
            max_owned_bytes: fixed,
            max_retained_bytes: inherited + fixed,
            ..SemanticLimits::default()
        };
        let draft = analyze_semantic_draft_inner(vec![syntax], limits, false, false).unwrap();
        assert_eq!(draft.semantic_charged_bytes(), fixed);
        let source = scan_source("limits", b"SecAction \"id:1\"\n", SourceLimits::default()).unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        limits.max_owned_bytes -= 1;
        assert!(matches!(
            analyze_semantic_draft_inner(vec![syntax], limits, false, false),
            Err(SemanticError::PreflightOwnedByteLimit { .. })
        ));
    }

    #[test]
    fn document_rule_and_node_limits_accept_exact_and_reject_first_excess() {
        let empty_sources = [
            scan_source("a", b"", SourceLimits::default()).unwrap(),
            scan_source("b", b"", SourceLimits::default()).unwrap(),
            scan_source("c", b"", SourceLimits::default()).unwrap(),
        ];
        let documents = empty_sources[..2]
            .iter()
            .map(|source| parse_syntax(source, SyntaxLimits::default()).unwrap())
            .collect();
        assert!(analyze_semantic_draft(
            documents,
            SemanticLimits {
                max_documents: 2,
                ..SemanticLimits::default()
            }
        )
        .is_ok());
        let documents = empty_sources
            .iter()
            .map(|source| parse_syntax(source, SyntaxLimits::default()).unwrap())
            .collect();
        assert!(matches!(
            analyze_semantic_draft(
                documents,
                SemanticLimits {
                    max_documents: 2,
                    ..SemanticLimits::default()
                }
            ),
            Err(SemanticError::DocumentLimit { limit: 2 })
        ));

        let rules = scan_source(
            "rules",
            b"SecRule ARGS \"@eq 1\" \"id:1\"\nSecRule ARGS \"@eq 1\" \"id:2\"\n",
            SourceLimits::default(),
        )
        .unwrap();
        let syntax = parse_syntax(&rules, SyntaxLimits::default()).unwrap();
        assert!(analyze_semantic_draft_inner(
            vec![syntax],
            SemanticLimits {
                max_rules: 2,
                ..SemanticLimits::default()
            },
            false,
            false,
        )
        .is_ok());
        let syntax = parse_syntax(&rules, SyntaxLimits::default()).unwrap();
        assert!(matches!(
            analyze_one_semantic_draft(
                syntax,
                SemanticLimits {
                    max_rules: 1,
                    ..SemanticLimits::default()
                }
            ),
            Err(SemanticError::RuleLimit { limit: 1 })
        ));

        let nodes = scan_source("nodes", b"SecRule ARGS \"@eq 1\" \"id:1\"\n", SourceLimits::default()).unwrap();
        let syntax = parse_syntax(&nodes, SyntaxLimits::default()).unwrap();
        assert!(analyze_semantic_draft_inner(
            vec![syntax],
            SemanticLimits {
                max_nodes: 3,
                ..SemanticLimits::default()
            },
            false,
            false,
        )
        .is_ok());
        let syntax = parse_syntax(&nodes, SyntaxLimits::default()).unwrap();
        assert!(matches!(
            analyze_one_semantic_draft(
                syntax,
                SemanticLimits {
                    max_nodes: 2,
                    ..SemanticLimits::default()
                }
            ),
            Err(SemanticError::NodeLimit { limit: 2 })
        ));
    }

    #[test]
    fn document_slots_are_charged_and_caller_capacity_is_normalized() {
        let source = scan_source("empty", b"", SourceLimits::default()).unwrap();
        let slot = size_of::<SyntaxDocument<'_>>();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(matches!(
            analyze_one_semantic_draft(
                syntax,
                SemanticLimits {
                    max_owned_bytes: slot - 1,
                    ..SemanticLimits::default()
                }
            ),
            Err(SemanticError::PreflightOwnedByteLimit { .. })
        ));
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        let draft = analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                max_owned_bytes: slot,
                ..SemanticLimits::default()
            },
        )
        .unwrap();
        assert_eq!(draft.semantic_charged_bytes(), slot);

        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        let mut documents = Vec::with_capacity(4_096);
        documents.push(syntax);
        let caller_backing = documents.as_ptr();
        let draft = analyze_semantic_draft(
            documents,
            SemanticLimits {
                max_owned_bytes: slot,
                ..SemanticLimits::default()
            },
        )
        .unwrap();
        assert_eq!(draft.semantic_charged_bytes(), slot);
        assert_ne!(draft.documents.as_ptr(), caller_backing);
    }

    #[test]
    fn aggregate_multidocument_and_dynamic_charges_have_exact_boundaries() {
        let first = scan_source("first", b"SecAction \"id:1\"\n", SourceLimits::default()).unwrap();
        let second = scan_source("second", b"SecAction \"id:2\"\n", SourceLimits::default()).unwrap();
        let inherited = parse_syntax(&first, SyntaxLimits::default())
            .unwrap()
            .retained_charged_bytes()
            + parse_syntax(&second, SyntaxLimits::default())
                .unwrap()
                .retained_charged_bytes();
        let fixed = 2
            * (size_of::<SyntaxDocument<'_>>()
                + size_of::<DirectiveSemanticRecord>()
                + size_of::<SemanticActionRecord<'_>>()
                + size_of::<ActionOperandRecord<'_>>());
        let documents = vec![
            parse_syntax(&first, SyntaxLimits::default()).unwrap(),
            parse_syntax(&second, SyntaxLimits::default()).unwrap(),
        ];
        assert!(analyze_semantic_draft_inner(
            documents,
            SemanticLimits {
                max_owned_bytes: fixed,
                max_retained_bytes: inherited + fixed,
                ..SemanticLimits::default()
            },
            false,
            false,
        )
        .is_ok());
        let documents = vec![
            parse_syntax(&first, SyntaxLimits::default()).unwrap(),
            parse_syntax(&second, SyntaxLimits::default()).unwrap(),
        ];
        assert!(matches!(
            analyze_semantic_draft_inner(
                documents,
                SemanticLimits {
                    max_owned_bytes: fixed,
                    max_retained_bytes: inherited + fixed - 1,
                    ..SemanticLimits::default()
                },
                false,
                false,
            ),
            Err(SemanticError::RetainedByteLimit { .. })
        ));

        for (text, dynamic_size, capability) in [
            (
                "SecRule ARGS \"@rx x\" \"id:1\"\n",
                size_of::<RequirementRecord>() + size_of::<StaticOperandRecord<'_>>(),
                false,
            ),
            (
                "SecRule RESPONSE_BODY \"@validateUrlEncoding\" \"id:1\"\n",
                size_of::<CapabilityRecord>(),
                true,
            ),
        ] {
            let source = scan_source("dynamic", text.as_bytes(), SourceLimits::default()).unwrap();
            let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
            let inherited = syntax.retained_charged_bytes();
            let fixed = size_of::<SyntaxDocument<'_>>()
                + size_of::<DirectiveSemanticRecord>()
                + size_of::<SemanticTarget<'_>>()
                + size_of::<SemanticOperatorRecord<'_>>()
                + size_of::<OperatorOperandRecord>()
                + size_of::<SemanticActionRecord<'_>>()
                + size_of::<ActionOperandRecord<'_>>();
            let draft = analyze_semantic_draft_inner(
                vec![syntax],
                SemanticLimits {
                    max_owned_bytes: fixed + dynamic_size,
                    max_retained_bytes: inherited + fixed + dynamic_size,
                    ..SemanticLimits::default()
                },
                false,
                false,
            )
            .unwrap();
            assert_eq!(
                (draft.requirements().len(), draft.unavailable_capabilities().len()),
                if capability { (0, 1) } else { (1, 0) }
            );
            let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
            assert!(matches!(
                analyze_semantic_draft_inner(
                    vec![syntax],
                    SemanticLimits {
                        max_owned_bytes: fixed + dynamic_size - 1,
                        max_retained_bytes: usize::MAX,
                        ..SemanticLimits::default()
                    },
                    false,
                    false,
                ),
                Err(SemanticError::OwnedByteLimit { .. })
            ));
            let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
            assert!(matches!(
                analyze_semantic_draft_inner(
                    vec![syntax],
                    SemanticLimits {
                        max_owned_bytes: usize::MAX,
                        max_retained_bytes: inherited + fixed + dynamic_size - 1,
                        ..SemanticLimits::default()
                    },
                    false,
                    false,
                ),
                Err(SemanticError::RetainedByteLimit { .. })
            ));
        }
    }

    #[test]
    fn response_body_capability_follows_positive_or_count_atoms_only() {
        let result = with_draft(
            "SecRule RESPONSE_BODY|!RESPONSE_BODY|&RESPONSE_BODY \"@eq 1\" \"id:1\"\n",
            |draft| {
                (
                    draft.targets().map(|target| target.modifier()).collect::<Vec<_>>(),
                    draft
                        .unavailable_capabilities()
                        .filter(|record| record.capability() == UnavailableCapability::FullResponseBody)
                        .count(),
                )
            },
        )
        .unwrap();
        assert_eq!(
            result.0,
            [TargetModifier::None, TargetModifier::Exclude, TargetModifier::Count]
        );
        assert_eq!(result.1, 2);
        let exclusion = with_draft("SecRule !RESPONSE_BODY \"@eq 1\" \"id:1\"\n", |draft| {
            draft.unavailable_capabilities().len()
        })
        .unwrap();
        assert_eq!(exclusion, 0);
    }

    #[test]
    fn semantic_error_families_are_atomic_and_redacted_through_source_chain() {
        let cases = [
            "SecRule SECRET_COLLECTION \"@eq 1\" \"id:1\"\n",
            "SecRule ARGS \"@secret_operator 1\" \"id:1\"\n",
            "SecAction \"secret_action:value\"\n",
            "SecAction \"t:secret_transform\"\n",
            "SecAction \"id:secret_scalar\"\n",
            "SecAction \"id:%{SECRET.scalar}\"\n",
            "SecAction \"block:secret_value\"\n",
        ];
        for text in cases {
            let error = with_draft(text, |_| ()).unwrap_err();
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.to_ascii_lowercase().contains("secret"));
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                let rendered = format!("{cause:?} {cause}");
                assert!(!rendered.to_ascii_lowercase().contains("secret"));
                source = cause.source();
            }
        }
        assert_eq!(with_draft("SecAction \"id:7\"\n", |draft| draft.action_count()), Ok(1));
    }

    #[test]
    fn validation_context_and_default_resolution_are_typed_and_exact() {
        assert!(matches!(
            SemanticValidationContext::try_new(
                None,
                &[
                    PhaseDefaults::pass(Phase::RequestBody, true, true),
                    PhaseDefaults::pass(Phase::RequestBody, false, false),
                ],
            ),
            Err(DefaultsContextError::DuplicatePhase {
                phase: Phase::RequestBody
            })
        ));
        for status in [99, 100, 101, 199, 600] {
            assert!(matches!(
                PhaseDefaults::try_deny(Phase::RequestBody, status, true, true),
                Err(DefaultsContextError::InvalidStatus {
                    phase: Phase::RequestBody
                })
            ));
        }
        for status in [200, 599] {
            assert!(PhaseDefaults::try_deny(Phase::RequestBody, status, true, true).is_ok());
        }

        let deny = PhaseDefaults::try_deny(Phase::RequestBody, 403, true, false).unwrap();
        let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &[deny]).unwrap();
        let state = with_validated(
            "SecRule ARGS \"@eq 1\" \"id:1,block,status:451,nolog,auditlog\"\n",
            context,
            ValidationLimits::default(),
            |validated| {
                let state = validated.rule_by_id(1).unwrap().root().resolved_state();
                (
                    state.phase(),
                    state.disruptive(),
                    state.status(),
                    state.log(),
                    state.audit_log(),
                )
            },
        )
        .unwrap();
        assert_eq!(
            state,
            (Phase::RequestBody, ResolvedDisruptive::Deny, Some(451), false, true)
        );

        let missing = SemanticValidationContext::try_new(None, &[deny]).unwrap();
        assert!(matches!(
            with_validated(
                "SecAction \"id:1,pass\"\n",
                missing,
                ValidationLimits::default(),
                |_| (),
            ),
            Err(SemanticValidationError::MissingImplicitPhase { .. })
        ));
        let pass = SemanticValidationContext::try_new(
            Some(Phase::RequestBody),
            &[PhaseDefaults::pass(Phase::RequestBody, true, true)],
        )
        .unwrap();
        assert!(matches!(
            with_validated(
                "SecAction \"id:1,block,status:451\"\n",
                pass,
                ValidationLimits::default(),
                |_| (),
            ),
            Err(SemanticValidationError::BlockStatusPass { .. })
        ));
    }

    #[test]
    fn sealed_validation_views_preserve_units_order_updates_and_removals() {
        let text = concat!(
            "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
            "SecRule REQUEST_HEADERS \"@eq 1\" \"id:2,phase:2,pass\"\n",
            "SecRuleUpdateTargetById 1-2 TX:added\n",
            "SecRuleRemoveById 2\n",
        );
        let evidence = with_validated(
            text,
            crs_validation_context(),
            ValidationLimits::default(),
            |validated| {
                let units = validated
                    .rule_units()
                    .map(|unit| (unit.id(), unit.is_active(), unit.members().len()))
                    .collect::<Vec<_>>();
                let active = validated.active_rule_units().map(|unit| unit.id()).collect::<Vec<_>>();
                let update = validated.applied_target_updates().next().unwrap();
                let removal = validated.applied_rule_removals().next().unwrap();
                (
                    units,
                    active,
                    validated.rule_by_id(2).is_none(),
                    update.affected_units().map(|unit| unit.id()).collect::<Vec<_>>(),
                    removal.affected_units().map(|unit| unit.id()).collect::<Vec<_>>(),
                    validated.rule_by_id(1).unwrap().root().final_targets().len(),
                )
            },
        )
        .unwrap();
        assert_eq!(evidence.0, [(1, true, 1), (2, false, 1)]);
        assert_eq!(evidence.1, [1]);
        assert!(evidence.2);
        assert_eq!(evidence.3, [1, 2]);
        assert_eq!(evidence.4, [2]);
        assert_eq!(evidence.5, 2);
    }

    #[test]
    fn exception_selectors_enforce_order_activity_and_whole_chain_removal() {
        let cases = [
            (
                concat!(
                    "SecRuleUpdateTargetById 1 TX:x\n",
                    "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
                ),
                "forward",
            ),
            (
                concat!(
                    "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
                    "SecRuleRemoveById 1\n",
                    "SecRuleUpdateTargetById 1 TX:x\n",
                ),
                "inactive",
            ),
            (
                concat!(
                    "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
                    "SecRuleRemoveById 1\n",
                    "SecRuleRemoveById 1\n",
                ),
                "empty",
            ),
        ];
        for (text, expected) in cases {
            let error =
                with_validated(text, crs_validation_context(), ValidationLimits::default(), |_| ()).unwrap_err();
            assert!(
                matches!(
                    (&error, expected),
                    (SemanticValidationError::ForwardIdSelector { .. }, "forward")
                        | (SemanticValidationError::InactiveUpdate { .. }, "inactive")
                        | (SemanticValidationError::EmptyRemoval { .. }, "empty")
                ),
                "unexpected {error:?}"
            );
        }

        let chain = concat!(
            "SecRule ARGS \"@eq 1\" \"id:3,phase:2,pass,chain\"\n",
            "SecRule REQUEST_HEADERS \"@eq 1\" \"\"\n",
            "SecRuleRemoveById 3\n",
        );
        let result = with_validated(
            chain,
            crs_validation_context(),
            ValidationLimits::default(),
            |validated| {
                let unit = validated.rule_units().next().unwrap();
                (
                    unit.members().len(),
                    unit.is_active(),
                    validated.active_rule_units().len(),
                )
            },
        )
        .unwrap();
        assert_eq!(result, (2, false, 0));
    }

    #[test]
    fn selector_union_counts_work_and_first_discovery_order_are_exact() {
        let text = concat!(
            "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
            "SecRule ARGS \"@eq 1\" \"id:3,phase:2,pass\"\n",
            "SecRuleRemoveById 1-3 3\n",
        );
        let exact = ValidationLimits {
            max_raw_matches_per_directive: 3,
            max_distinct_matches_per_directive: 2,
            max_raw_matches: 3,
            max_distinct_matches: 2,
            max_selector_work: 11,
            ..ValidationLimits::default()
        };
        let affected = with_validated(text, crs_validation_context(), exact, |validated| {
            validated
                .applied_rule_removals()
                .next()
                .unwrap()
                .affected_units()
                .map(|unit| unit.id())
                .collect::<Vec<_>>()
        })
        .unwrap();
        assert_eq!(affected, [1, 3]);
        for (limits, expected) in [
            (
                ValidationLimits {
                    max_raw_matches_per_directive: 2,
                    ..exact
                },
                "raw",
            ),
            (
                ValidationLimits {
                    max_distinct_matches_per_directive: 1,
                    ..exact
                },
                "distinct",
            ),
            (
                ValidationLimits {
                    max_selector_work: 10,
                    ..exact
                },
                "work",
            ),
        ] {
            let error = with_validated(text, crs_validation_context(), limits, |_| ()).unwrap_err();
            assert!(
                matches!(
                    (&error, expected),
                    (SemanticValidationError::RawMatchDirectiveLimit { .. }, "raw")
                        | (SemanticValidationError::DistinctMatchDirectiveLimit { .. }, "distinct")
                        | (SemanticValidationError::SelectorWorkLimit { .. }, "work")
                ),
                "unexpected {error:?}"
            );
        }
    }

    #[test]
    fn target_exclusions_require_an_applicable_positive_base() {
        for (initial, exclusion) in [("ARGS", "!ARGS:any"), ("ARGS:name", "!ARGS:name")] {
            let text =
                format!("SecRule {initial} \"@eq 1\" \"id:1,phase:2,pass\"\nSecRuleUpdateTargetById 1 {exclusion}\n");
            assert!(with_validated(&text, crs_validation_context(), ValidationLimits::default(), |_| (),).is_ok());
        }
        for (initial, exclusion) in [("ARGS:name", "!ARGS:other"), ("&ARGS", "!ARGS")] {
            let text =
                format!("SecRule {initial} \"@eq 1\" \"id:1,phase:2,pass\"\nSecRuleUpdateTargetById 1 {exclusion}\n");
            assert!(matches!(
                with_validated(&text, crs_validation_context(), ValidationLimits::default(), |_| (),),
                Err(SemanticValidationError::MissingExclusionBase { .. })
            ));
        }
    }

    #[test]
    fn duplicate_ids_are_global_even_when_the_first_unit_is_removed() {
        let error = with_validated(
            concat!(
                "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
                "SecRuleRemoveById 1\n",
                "SecAction \"id:1,phase:2,pass\"\n",
            ),
            crs_validation_context(),
            ValidationLimits::default(),
            |_| (),
        )
        .unwrap_err();
        assert!(matches!(error, SemanticValidationError::DuplicateRuleId { .. }));
    }

    #[test]
    fn collection_phase_and_selector_tables_cover_every_registered_collection() {
        let request_body = [
            CollectionKind::Args,
            CollectionKind::ArgsNames,
            CollectionKind::Files,
            CollectionKind::FilesCombinedSize,
            CollectionKind::FilesNames,
            CollectionKind::MultipartPartHeaders,
            CollectionKind::RequestBody,
            CollectionKind::RequestBodyLength,
            CollectionKind::Xml,
        ];
        let response_headers = [CollectionKind::ResponseHeaders, CollectionKind::ResponseStatus];
        for &collection in CollectionKind::ALL {
            let expected = if request_body.contains(&collection) {
                Phase::RequestBody
            } else if response_headers.contains(&collection) {
                Phase::ResponseHeaders
            } else if collection == CollectionKind::ResponseBody {
                Phase::ResponseBody
            } else {
                Phase::RequestHeaders
            };
            assert_eq!(minimum_collection_phase(collection), expected, "{collection:?}");
        }
        let selectable = [
            CollectionKind::Args,
            CollectionKind::ArgsGet,
            CollectionKind::ArgsGetNames,
            CollectionKind::ArgsNames,
            CollectionKind::Files,
            CollectionKind::FilesNames,
            CollectionKind::MatchedVars,
            CollectionKind::MultipartPartHeaders,
            CollectionKind::RequestCookies,
            CollectionKind::RequestCookiesNames,
            CollectionKind::RequestHeaders,
            CollectionKind::RequestHeadersNames,
            CollectionKind::ResponseHeaders,
            CollectionKind::Tx,
            CollectionKind::Xml,
        ];
        for &collection in CollectionKind::ALL {
            assert_eq!(
                collection_accepts_selector(collection),
                selectable.contains(&collection)
            );
        }
    }

    #[test]
    fn target_regex_requirements_follow_final_target_identity_only() {
        let text = concat!(
            "SecRule ARGS:/first/ \"@rx value\" \"id:1,phase:2,pass\"\n",
            "SecRule ARGS \"@eq 1\" \"id:2,phase:2,pass\"\n",
            "SecRuleUpdateTargetById 1-2 ARGS:/added/\n",
            "SecRuleRemoveById 1\n",
        );
        let requirements = with_validated(
            text,
            crs_validation_context(),
            ValidationLimits::default(),
            |validated| {
                validated
                    .requirements()
                    .map(|requirement| (requirement.kind(), requirement.target().is_some()))
                    .collect::<Vec<_>>()
            },
        )
        .unwrap();
        assert_eq!(
            requirements,
            [
                (CompileRequirement::RegexCompilation, false),
                (CompileRequirement::TargetSelectorRegexCompilation, true),
            ]
        );
    }

    #[test]
    fn removed_units_cannot_hide_unavailable_capabilities() {
        let error = with_validated(
            concat!(
                "SecRule RESPONSE_BODY \"@validateUrlEncoding\" \"id:1,phase:4,pass\"\n",
                "SecRuleRemoveById 1\n",
            ),
            crs_validation_context(),
            ValidationLimits::default(),
            |_| (),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            SemanticValidationError::UnavailableCapability {
                capability: UnavailableCapability::FullResponseBody,
                ..
            }
        ));
    }

    #[test]
    fn request_body_processor_is_header_available_but_body_bytes_are_not() {
        assert!(with_validated(
            "SecRule REQBODY_PROCESSOR \"@streq JSON\" \"id:1,phase:1,pass\"\n",
            crs_validation_context(),
            ValidationLimits::default(),
            |_| (),
        )
        .is_ok());
        assert!(matches!(
            with_validated(
                "SecRule REQUEST_BODY \"@streq value\" \"id:1,phase:1,pass\"\n",
                crs_validation_context(),
                ValidationLimits::default(),
                |_| (),
            ),
            Err(SemanticValidationError::CollectionPhase {
                collection: CollectionKind::RequestBody,
                phase: Phase::RequestHeaders,
                ..
            })
        ));
    }

    #[test]
    fn validation_limits_have_exact_success_and_first_excess_boundaries() {
        let text = "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n";
        let charge = with_validated(
            text,
            crs_validation_context(),
            ValidationLimits::default(),
            |validated| (validated.validation_charged_bytes(), validated.retained_charged_bytes()),
        )
        .unwrap();
        assert!(with_validated(
            text,
            crs_validation_context(),
            ValidationLimits {
                max_owned_bytes: charge.0,
                max_retained_bytes: charge.1,
                ..ValidationLimits::default()
            },
            |_| (),
        )
        .is_ok());
        assert!(matches!(
            with_validated(
                text,
                crs_validation_context(),
                ValidationLimits {
                    max_owned_bytes: charge.0 - 1,
                    ..ValidationLimits::default()
                },
                |_| (),
            ),
            Err(SemanticValidationError::OwnedByteLimit { .. })
        ));
        assert!(matches!(
            with_validated(
                text,
                crs_validation_context(),
                ValidationLimits {
                    max_rule_units: 0,
                    ..ValidationLimits::default()
                },
                |_| (),
            ),
            Err(SemanticValidationError::RuleUnitLimit { limit: 0 })
        ));
    }

    #[test]
    fn validation_work_exception_target_and_comparison_limits_are_exact() {
        let simple = "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n";
        let working = validation_working_bytes(1, 1, 1, 1, 0, false, ValidationLimits::default()).unwrap();
        assert!(with_validated(
            simple,
            crs_validation_context(),
            ValidationLimits {
                max_working_bytes: working,
                ..ValidationLimits::default()
            },
            |_| (),
        )
        .is_ok());
        assert!(matches!(
            with_validated(
                simple,
                crs_validation_context(),
                ValidationLimits {
                    max_working_bytes: working - 1,
                    ..ValidationLimits::default()
                },
                |_| (),
            ),
            Err(SemanticValidationError::WorkingByteLimit { .. })
        ));
        for (limits, expected) in [
            (
                ValidationLimits {
                    max_final_targets_per_rule: 0,
                    ..ValidationLimits::default()
                },
                "per-rule",
            ),
            (
                ValidationLimits {
                    max_final_target_references: 0,
                    ..ValidationLimits::default()
                },
                "aggregate",
            ),
        ] {
            let error = with_validated(simple, crs_validation_context(), limits, |_| ()).unwrap_err();
            assert!(
                matches!(
                    (&error, expected),
                    (SemanticValidationError::FinalTargetLimit { .. }, "per-rule")
                        | (SemanticValidationError::FinalTargetReferenceLimit { .. }, "aggregate")
                ),
                "unexpected {error:?}"
            );
        }

        let exception = concat!(
            "SecRule ARGS:name \"@eq 1\" \"id:1,phase:2,pass\"\n",
            "SecRuleUpdateTargetById 1 !ARGS:name\n",
        );
        assert!(with_validated(
            exception,
            crs_validation_context(),
            ValidationLimits {
                max_id_selectors: 1,
                max_applied_exceptions: 1,
                max_target_comparisons: 1,
                max_target_comparison_bytes: 8,
                ..ValidationLimits::default()
            },
            |_| (),
        )
        .is_ok());
        for (limits, expected) in [
            (
                ValidationLimits {
                    max_id_selectors: 0,
                    ..ValidationLimits::default()
                },
                "selectors",
            ),
            (
                ValidationLimits {
                    max_applied_exceptions: 0,
                    ..ValidationLimits::default()
                },
                "exceptions",
            ),
            (
                ValidationLimits {
                    max_target_comparisons: 0,
                    ..ValidationLimits::default()
                },
                "comparisons",
            ),
            (
                ValidationLimits {
                    max_target_comparison_bytes: 7,
                    ..ValidationLimits::default()
                },
                "comparison-bytes",
            ),
        ] {
            let error = with_validated(exception, crs_validation_context(), limits, |_| ()).unwrap_err();
            assert!(
                matches!(
                    (&error, expected),
                    (SemanticValidationError::IdSelectorLimit { .. }, "selectors")
                        | (SemanticValidationError::ExceptionLimit { .. }, "exceptions")
                        | (SemanticValidationError::TargetComparisonLimit { .. }, "comparisons")
                        | (
                            SemanticValidationError::TargetComparisonByteLimit { .. },
                            "comparison-bytes"
                        )
                ),
                "unexpected {error:?}"
            );
        }
    }

    #[test]
    fn overlay_preflight_bound_covers_zero_per_directive_cumulative_and_composed_shapes() {
        let rules = concat!(
            "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
            "SecRule ARGS \"@eq 1\" \"id:2,phase:2,pass\"\n",
            "SecRule ARGS \"@eq 1\" \"id:3,phase:2,pass\"\n",
        );
        let cases = [
            (rules.to_owned(), 3, 100, 0),
            (
                format!("{rules}SecRuleUpdateTargetById 1-2 TX:a|REQUEST_HEADERS:b\n"),
                2,
                100,
                4,
            ),
            (
                format!(
                    "{rules}SecRuleUpdateTargetById 1-3 TX:a|REQUEST_HEADERS:b\nSecRuleUpdateTargetById 1 1-3 TX:c|REQUEST_HEADERS:d|ARGS_GET:e\n"
                ),
                3,
                2,
                6,
            ),
            (
                format!(
                    "{rules}SecRuleUpdateTargetById 1-2 1 TX:a|REQUEST_HEADERS:b\nSecRuleRemoveById 2\nSecRuleUpdateTargetById 1 3 TX:c|REQUEST_HEADERS:d|ARGS_GET:e\n"
                ),
                2,
                100,
                10,
            ),
        ];
        for (text, per_directive, cumulative, expected_bound) in cases {
            let limits = ValidationLimits {
                max_distinct_matches_per_directive: per_directive,
                max_distinct_matches: cumulative,
                ..ValidationLimits::default()
            };
            let (nodes, targets, evidence) = with_full_draft(&text, |draft| {
                let units = draft
                    .final_rules
                    .iter()
                    .filter(|rule| !matches!(rule.role, FinalRuleRole::ChainChild { .. }))
                    .count();
                (
                    draft.final_rules.len(),
                    draft.targets.len(),
                    validation_exception_shape(draft, units, limits).unwrap(),
                )
            })
            .unwrap();
            assert_eq!(evidence.3, expected_bound, "{text}");
            let working = validation_working_bytes(
                3,
                nodes,
                targets,
                3_usize.min(per_directive),
                expected_bound,
                false,
                limits,
            )
            .unwrap();
            if cumulative >= 3 || expected_bound == 0 {
                assert!(with_validated(
                    &text,
                    crs_validation_context(),
                    ValidationLimits {
                        max_working_bytes: working,
                        ..limits
                    },
                    |_| (),
                )
                .is_ok());
            }
            assert!(matches!(
                with_validated(
                    &text,
                    crs_validation_context(),
                    ValidationLimits {
                        max_working_bytes: working - 1,
                        ..limits
                    },
                    |_| (),
                ),
                Err(SemanticValidationError::WorkingByteLimit { .. })
            ));
        }
    }

    #[test]
    fn cumulative_raw_and_distinct_limits_anchor_the_first_excess_selector() {
        let text = concat!(
            "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
            "SecRule ARGS \"@eq 1\" \"id:2,phase:2,pass\"\n",
            "SecRuleUpdateTargetById 1-2 2 TX:first\n",
            "SecRuleUpdateTargetById 1 1-2 TX:secret_second\n",
        );
        let exact = ValidationLimits {
            max_raw_matches_per_directive: 3,
            max_distinct_matches_per_directive: 2,
            max_raw_matches: 6,
            max_distinct_matches: 4,
            max_selector_work: 22,
            ..ValidationLimits::default()
        };
        let overlays = with_validated(text, crs_validation_context(), exact, |validated| {
            (
                validated
                    .applied_target_updates()
                    .map(|update| update.affected_units().map(|unit| unit.id()).collect::<Vec<_>>())
                    .collect::<Vec<_>>(),
                validated
                    .rule_units()
                    .map(|unit| unit.root().final_targets().len())
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap();
        assert_eq!(overlays, (vec![vec![1, 2], vec![1, 2]], vec![3, 3]));

        let selector_offset = text.find("1-2 TX:secret_second").unwrap();
        for (limits, expected) in [
            (
                ValidationLimits {
                    max_raw_matches: 5,
                    ..exact
                },
                "raw",
            ),
            (
                ValidationLimits {
                    max_distinct_matches: 3,
                    ..exact
                },
                "distinct",
            ),
        ] {
            let error = with_validated(text, crs_validation_context(), limits, |_| ()).unwrap_err();
            let position = match (&error, expected) {
                (SemanticValidationError::RawMatchLimit { position, .. }, "raw")
                | (SemanticValidationError::DistinctMatchLimit { position, .. }, "distinct") => *position,
                _ => panic!("unexpected {error:?}"),
            };
            assert_eq!((position.line, position.byte_offset), (4, selector_offset));
            let rendered = format!("{error:?} {error}").to_ascii_lowercase();
            assert!(!rendered.contains("secret"));
        }
    }

    #[test]
    fn generated_selector_unions_and_final_overlays_match_the_reference_model() {
        let mut seed = 0x05e1_ec70_1d15_ca11_u64;
        let next = |state: &mut u64| {
            *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            *state
        };
        for case in 0..64_u64 {
            let unit_count = 1 + (next(&mut seed) % 5) as usize;
            let mut source = String::new();
            let mut active = vec![true; unit_count];
            let mut overlays = vec![vec![(CollectionKind::Args, String::new())]; unit_count];
            let mut expected_updates = Vec::new();
            let mut expected_removals = Vec::new();
            for id in 1..=unit_count {
                source.push_str(&format!("SecRule ARGS \"@eq 1\" \"id:{id},phase:2,pass\"\n"));
            }
            let update_count = 1 + (next(&mut seed) % 3) as usize;
            for update in 0..update_count {
                let left = 1 + (next(&mut seed) % unit_count as u64) as usize;
                let right = 1 + (next(&mut seed) % unit_count as u64) as usize;
                let (start, end) = (left.min(right), left.max(right));
                let shape = next(&mut seed) % 4;
                let (selectors, affected) = match shape {
                    0 => (left.to_string(), vec![left]),
                    1 => ((format!("{start}-{end}")), (start..=end).collect::<Vec<_>>()),
                    2 => (format!("{start}-{end} {end}"), (start..=end).collect::<Vec<_>>()),
                    _ => {
                        let mut affected = vec![end];
                        affected.extend((start..end).filter(|id| *id != end));
                        (format!("{end} {start}-{end}"), affected)
                    }
                };
                let first = format!("u{case}_{update}");
                let second = format!("h{case}_{update}");
                let two_atoms = next(&mut seed) & 1 != 0;
                let targets = if two_atoms {
                    format!("TX:{first}|REQUEST_HEADERS:{second}")
                } else {
                    format!("TX:{first}")
                };
                source.push_str(&format!("SecRuleUpdateTargetById {selectors} {targets}\n"));
                for &id in &affected {
                    overlays[id - 1].push((CollectionKind::Tx, first.clone()));
                    if two_atoms {
                        overlays[id - 1].push((CollectionKind::RequestHeaders, second.clone()));
                    }
                }
                expected_updates.push(affected);
            }
            if next(&mut seed) & 1 != 0 {
                let start = 1 + (next(&mut seed) % unit_count as u64) as usize;
                let end = 1 + (next(&mut seed) % unit_count as u64) as usize;
                let (start, end) = (start.min(end), start.max(end));
                let affected = (start..=end).collect::<Vec<_>>();
                source.push_str(&format!("SecRuleRemoveById {start}-{end} {end}\n"));
                for &id in &affected {
                    active[id - 1] = false;
                }
                expected_removals.push(affected);
            }
            assert!(source.len() <= 4 * 1024);
            let actual = with_validated(
                &source,
                crs_validation_context(),
                ValidationLimits::default(),
                |validated| {
                    let updates = validated
                        .applied_target_updates()
                        .map(|value| {
                            value
                                .affected_units()
                                .map(|unit| unit.id() as usize)
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    let removals = validated
                        .applied_rule_removals()
                        .map(|value| {
                            value
                                .affected_units()
                                .map(|unit| unit.id() as usize)
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    let units = validated
                        .rule_units()
                        .map(|unit| {
                            (
                                unit.is_active(),
                                unit.root()
                                    .final_targets()
                                    .map(|target| {
                                        (
                                            target.collection(),
                                            target
                                                .selector()
                                                .map(|selector| selector.text().to_owned())
                                                .unwrap_or_default(),
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect::<Vec<_>>();
                    (updates, removals, units)
                },
            )
            .unwrap_or_else(|error| panic!("case {case}: {error:?}\n{source}"));
            assert_eq!(actual.0, expected_updates, "case {case}");
            assert_eq!(actual.1, expected_removals, "case {case}");
            assert_eq!(
                actual.2,
                active.into_iter().zip(overlays).collect::<Vec<_>>(),
                "case {case}"
            );
        }
    }

    #[test]
    fn validation_temporary_and_retained_allocation_failures_are_typed_and_atomic() {
        let text = concat!(
            "SecRule ARGS:/first/ \"@rx value\" \"id:1,phase:2,pass\"\n",
            "SecRule ARGS \"@eq 1\" \"id:2,phase:2,pass\"\n",
            "SecRuleUpdateTargetById 1-2 ARGS:/added/\n",
            "SecRuleRemoveById 2\n",
        );
        let operations = [
            "rule ID index",
            "active rule state",
            "resolved rule states",
            "target overlay heads",
            "target overlay tails",
            "final target counts",
            "target overlays",
            "selector generations",
            "affected rule candidates",
            "target requirement generations",
            "target regex candidates",
            "validated rule nodes",
            "validated rule units",
            "validated rule ID index",
            "final target references",
            "applied target updates",
            "applied rule removals",
            "applied exception units",
            "validated requirements",
        ];
        for operation in operations {
            VALIDATION_ALLOCATION_FAILURE.with(|failure| failure.set(Some(operation)));
            let result = with_validated(text, crs_validation_context(), ValidationLimits::default(), |_| ());
            VALIDATION_ALLOCATION_FAILURE.with(|failure| failure.set(None));
            assert_eq!(result, Err(SemanticValidationError::Allocation { operation }));
            let error = result.unwrap_err();
            let rendered = format!("{error:?} {error}").to_ascii_lowercase();
            assert!(!rendered.contains("first"));
        }
        let evidence = with_validated(
            text,
            crs_validation_context(),
            ValidationLimits::default(),
            |validated| {
                (
                    validated.rule_units().map(|unit| unit.is_active()).collect::<Vec<_>>(),
                    validated.applied_target_updates().len(),
                    validated.applied_rule_removals().len(),
                    validated.requirements().len(),
                )
            },
        )
        .unwrap();
        assert_eq!(evidence, (vec![true, false], 1, 1, 3));
    }

    #[test]
    fn validation_errors_are_content_free_through_the_complete_source_chain() {
        let error = with_validated(
            concat!(
                "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
                "SecRuleUpdateTargetById 1 !TX:secret_selector\n",
            ),
            crs_validation_context(),
            ValidationLimits::default(),
            |_| (),
        )
        .unwrap_err();
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        while let Some(cause) = current {
            let rendered = format!("{cause:?} {cause}").to_ascii_lowercase();
            assert!(!rendered.contains("secret"));
            current = cause.source();
        }
    }

    #[test]
    fn pinned_crs_validation_pre_capability_evidence_and_final_gate() {
        let Some(root) = std::env::var_os("EDGION_WAF_CRS_RULES_DIR") else {
            return;
        };
        let root = PathBuf::from(root);
        let mut paths = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "conf"))
            .collect::<Vec<_>>();
        paths.sort();
        let sources = paths
            .iter()
            .map(|path| {
                let source_id = path.strip_prefix(&root).unwrap().to_string_lossy();
                scan_source(&source_id, &fs::read(path).unwrap(), SourceLimits::default()).unwrap()
            })
            .collect::<Vec<_>>();
        let make_draft = || {
            let documents = sources
                .iter()
                .map(|source| parse_syntax(source, SyntaxLimits::default()).unwrap())
                .collect::<Vec<_>>();
            analyze_semantic_draft(documents, SemanticLimits::default()).unwrap()
        };

        let final_error =
            validate_semantic_draft(make_draft(), crs_validation_context(), ValidationLimits::default()).unwrap_err();
        assert!(
            matches!(
                final_error,
                SemanticValidationError::UnavailableCapability {
                    capability: UnavailableCapability::Ctl,
                    ..
                }
            ),
            "{final_error:?}"
        );

        let mut draft = make_draft();
        assert_eq!(draft.unavailable.len(), 79);
        draft.unavailable.clear();
        let validated = validate_semantic_draft(draft, crs_validation_context(), ValidationLimits::default()).unwrap();
        let unit_ids = validated.rule_units().map(|unit| unit.id()).collect::<Vec<_>>();
        assert_eq!(
            (
                unit_ids.len(),
                unit_ids.iter().copied().min(),
                unit_ids.iter().copied().max(),
                validated.active_rule_units().len(),
                validated.applied_target_updates().len(),
                validated.applied_rule_removals().len(),
                validated
                    .applied_target_updates()
                    .map(|update| update.affected_units().len())
                    .sum::<usize>(),
                validated
                    .rule_nodes()
                    .map(|node| node.final_targets().len())
                    .sum::<usize>(),
            ),
            (627, Some(901_001), Some(980_170), 627, 55, 0, 55, 1_790)
        );
        let mut requirement_counts = [0_usize; 8];
        for requirement in validated.requirements() {
            requirement_counts[match requirement.kind() {
                CompileRequirement::RegexCompilation => 0,
                CompileRequirement::TargetSelectorRegexCompilation => 1,
                CompileRequirement::PhraseMatcherCompilation => 2,
                CompileRequirement::ExternalPhraseAsset => 3,
                CompileRequirement::RuntimeMacro => 4,
                CompileRequirement::BlockDefaultDisruptive => 5,
                CompileRequirement::TransactionSetVar => 6,
                CompileRequirement::DefaultPhase => 7,
            }] += 1;
        }
        assert_eq!(requirement_counts, [314, 37, 33, 21, 996, 0, 678, 0]);
        let mut update_collections = BTreeMap::new();
        let mut update_selectors = [0_usize; 3];
        let mut max_atom_bytes = 0_usize;
        let mut total_atom_bytes = 0_usize;
        for update in validated.applied_target_updates() {
            for target in update.source_directive().targets() {
                *update_collections.entry(target.collection()).or_insert(0_usize) += 1;
                update_selectors[match target.selector_kind() {
                    SelectorKind::Literal => 0,
                    SelectorKind::Regex(_) => 1,
                    SelectorKind::Path => 2,
                    SelectorKind::None => unreachable!("pinned updates have selectors"),
                }] += 1;
                let bytes = target.span().end.byte_offset - target.span().start.byte_offset;
                max_atom_bytes = max_atom_bytes.max(bytes);
                total_atom_bytes += bytes;
            }
        }
        assert_eq!(
            update_collections,
            BTreeMap::from([(CollectionKind::ArgsNames, 1), (CollectionKind::RequestCookies, 54)])
        );
        assert_eq!(
            (update_selectors, max_atom_bytes, total_atom_bytes),
            ([27, 28, 0], 33, 1_416)
        );
        assert_eq!(
            (validated.validation_charged_bytes(), validated.retained_charged_bytes()),
            (108_024, 9_724_171)
        );
        assert_eq!(
            (
                size_of::<ResolvedRuleStateRecord>(),
                size_of::<ValidatedRuleNodeRecord>(),
                size_of::<ValidatedRuleUnitRecord>(),
                size_of::<RuleIdIndexRecord>(),
                size_of::<TargetOverlayEntry>(),
                size_of::<AppliedExceptionRecord>(),
                size_of::<ValidatedRequirementReference>(),
            ),
            (24, 48, 24, 16, 16, 24, 16)
        );
        eprintln!(
            "validated_charge={} retained={} record_sizes={}/{}/{}/{}/{}/{}/{} requirements={:?}",
            validated.validation_charged_bytes(),
            validated.retained_charged_bytes(),
            size_of::<ResolvedRuleStateRecord>(),
            size_of::<ValidatedRuleNodeRecord>(),
            size_of::<ValidatedRuleUnitRecord>(),
            size_of::<RuleIdIndexRecord>(),
            size_of::<TargetOverlayEntry>(),
            size_of::<AppliedExceptionRecord>(),
            size_of::<ValidatedRequirementReference>(),
            requirement_counts,
        );
    }

    #[test]
    fn pinned_crs_semantic_classification_gate() {
        let Some(root) = std::env::var_os("EDGION_WAF_CRS_RULES_DIR") else {
            return;
        };
        let root = PathBuf::from(root);
        let mut paths = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "conf"))
            .collect::<Vec<PathBuf>>();
        paths.sort();
        let sources = paths
            .iter()
            .map(|path| {
                let source_id = path
                    .strip_prefix(&root)
                    .expect("enumerated CRS path remains under configured root")
                    .to_string_lossy();
                scan_source(&source_id, &fs::read(path).unwrap(), SourceLimits::default()).unwrap()
            })
            .collect::<Vec<_>>();
        let documents = sources
            .iter()
            .map(|source| parse_syntax(source, SyntaxLimits::default()).unwrap())
            .collect::<Vec<_>>();
        let draft = analyze_semantic_draft(documents, SemanticLimits::default()).unwrap();
        let mut collections = BTreeMap::new();
        let mut operators = BTreeMap::new();
        let mut actions = BTreeMap::new();
        let mut transforms = BTreeMap::new();
        let mut scalars = BTreeMap::new();
        let mut ids = Vec::new();
        let mut phases = [0usize; 5];
        let mut severities = [0usize; 8];
        let mut statuses = BTreeMap::new();
        let mut skips = BTreeMap::new();
        let mut macro_sources = BTreeMap::new();
        let mut macro_fields = BTreeMap::new();
        let mut macro_templates = 0usize;
        let mut max_expansions = 0usize;
        let mut max_segments = 0usize;
        let mut normalized_macro_bodies = std::collections::BTreeSet::new();
        let mut max_body_bytes = 0usize;
        let mut operand_classes = BTreeMap::new();
        let mut phrase_total = 0usize;
        let mut max_phrases_per_operator = 0usize;
        let mut asset_total = 0usize;
        let mut unique_assets = std::collections::BTreeSet::new();
        let mut max_asset_bytes = 0usize;
        let mut network_total = 0usize;
        let mut max_networks_per_operator = 0usize;
        let mut byte_member_total = 0usize;
        let mut max_byte_members = 0usize;
        let mut max_regex_bytes = 0usize;
        let mut integer_constants = BTreeMap::<&str, Vec<i64>>::new();
        let mut action_operand_classes = BTreeMap::new();
        let mut setvar_operations = [0usize; 4];
        let mut max_setvar_whole = 0usize;
        let mut max_setvar_key = 0usize;
        let mut max_setvar_value = 0usize;
        let mut macro_setvar_keys = 0usize;
        let mut macro_setvar_values = 0usize;
        let mut macro_setvar_union = 0usize;
        let mut initcols = Vec::new();
        let mut ctl_kinds = BTreeMap::new();
        let mut ctl_target_collections = BTreeMap::new();
        for target in draft.targets() {
            *collections.entry(target.collection().as_str()).or_insert(0usize) += 1;
        }
        for operator in draft.operators() {
            *operators.entry(operator.kind().as_str()).or_insert(0usize) += 1;
            let class = match operator.operand() {
                OperatorOperandView::NoArgument => "no_argument",
                OperatorOperandView::Regex(text) => {
                    max_regex_bytes = max_regex_bytes.max(text.text().len());
                    "regex"
                }
                OperatorOperandView::StringTemplate(_) => "string_template",
                OperatorOperandView::IntegerTemplate { constant, .. } => {
                    if let Some(value) = constant {
                        integer_constants
                            .entry(operator.kind().as_str())
                            .or_default()
                            .push(value);
                    }
                    "integer_template"
                }
                OperatorOperandView::PhraseList(phrases) => {
                    phrase_total += phrases.phrases().len();
                    max_phrases_per_operator = max_phrases_per_operator.max(phrases.phrases().len());
                    "phrase_list"
                }
                OperatorOperandView::ExternalAssets(assets) => {
                    asset_total += assets.assets().len();
                    for asset in assets.assets() {
                        max_asset_bytes = max_asset_bytes.max(asset.handle().len());
                        unique_assets.insert(asset.handle().to_owned());
                    }
                    "external_assets"
                }
                OperatorOperandView::IpNetworks(networks) => {
                    network_total += networks.networks().len();
                    max_networks_per_operator = max_networks_per_operator.max(networks.networks().len());
                    "ip_networks"
                }
                OperatorOperandView::ByteSet(bytes) => {
                    byte_member_total += bytes.member_count();
                    max_byte_members = max_byte_members.max(bytes.member_count());
                    "byte_set"
                }
            };
            *operand_classes.entry(class).or_insert(0usize) += 1;
        }
        for action in draft.actions() {
            match action.kind() {
                ActionEnvelopeKind::Action(kind) => *actions.entry(kind.as_str()).or_insert(0usize) += 1,
                ActionEnvelopeKind::Transformation => {
                    *transforms
                        .entry(action.transformation().unwrap().as_str())
                        .or_insert(0usize) += 1
                }
            }
            let key = match action.local_value() {
                LocalActionValue::Id(value) => {
                    ids.push(value);
                    Some("id")
                }
                LocalActionValue::Phase(value) => {
                    phases[value as usize - 1] += 1;
                    Some("phase")
                }
                LocalActionValue::Severity(value) => {
                    severities[value as usize] += 1;
                    Some("severity")
                }
                LocalActionValue::Status(value) => {
                    *statuses.entry(value).or_insert(0usize) += 1;
                    Some("status")
                }
                LocalActionValue::Skip(value) => {
                    *skips.entry(value).or_insert(0usize) += 1;
                    Some("skip")
                }
                _ => None,
            };
            if let Some(key) = key {
                *scalars.entry(key).or_insert(0usize) += 1;
            }
            let class = match action.operand() {
                ActionOperandView::NoArgument => "no_argument",
                ActionOperandView::Local(_) => "scalar",
                ActionOperandView::Transformation(_) => "transformation",
                ActionOperandView::Template(_) => "template",
                ActionOperandView::Literal(_) => "literal",
                ActionOperandView::SetVar(setvar) => {
                    setvar_operations[match setvar.operation() {
                        SetVarOperation::Assign => 0,
                        SetVarOperation::Add => 1,
                        SetVarOperation::Subtract => 2,
                        SetVarOperation::Delete => 3,
                    }] += 1;
                    let whole = action.template().expect("setvar template");
                    let key = setvar.key();
                    max_setvar_whole = max_setvar_whole.max(whole.text().len());
                    max_setvar_key = max_setvar_key.max(key.text().len());
                    if key.expansion_count() > 0 {
                        macro_setvar_keys += 1;
                    }
                    let value_has_macro = setvar.value().is_some_and(|value| {
                        max_setvar_value = max_setvar_value.max(value.text().len());
                        value.expansion_count() > 0
                    });
                    macro_setvar_values += usize::from(value_has_macro);
                    macro_setvar_union += usize::from(key.expansion_count() > 0 || value_has_macro);
                    "setvar"
                }
                ActionOperandView::InitCol(initcol) => {
                    initcols.push((initcol.collection(), initcol.key().text().to_owned()));
                    "initcol"
                }
                ActionOperandView::Ctl(ctl) => {
                    let key = match ctl {
                        CtlView::AuditEngine(_) => "auditEngine",
                        CtlView::ForceRequestBodyVariable(_) => "forceRequestBodyVariable",
                        CtlView::RequestBodyProcessor(_) => "requestBodyProcessor",
                        CtlView::RuleRemoveByTag(_) => "ruleRemoveByTag",
                        CtlView::RuleRemoveTargetByTag { target, .. } => {
                            *ctl_target_collections
                                .entry(target.collection().as_str())
                                .or_insert(0usize) += 1;
                            "ruleRemoveTargetByTag"
                        }
                    };
                    *ctl_kinds.entry(key).or_insert(0usize) += 1;
                    "ctl"
                }
            };
            *action_operand_classes.entry(class).or_insert(0usize) += 1;
        }
        for expansion in &draft.expansions {
            *macro_sources.entry(format!("{:?}", expansion.source)).or_insert(0usize) += 1;
            normalized_macro_bodies.insert(expansion.body.text().to_ascii_lowercase());
            max_body_bytes = max_body_bytes.max(expansion.body.text().len());
        }
        for operator in draft.operators() {
            if let Some(template) = operator.template() {
                max_expansions = max_expansions.max(template.expansion_count());
                max_segments = max_segments.max(template.segments().len());
                if template.expansion_count() > 0 {
                    macro_templates += 1;
                    *macro_fields
                        .entry(format!("operator:{}", operator.kind().as_str()))
                        .or_insert(0usize) += 1;
                }
            }
        }
        for action in draft.actions() {
            if let Some(template) = action.template() {
                max_expansions = max_expansions.max(template.expansion_count());
                max_segments = max_segments.max(template.segments().len());
                if template.expansion_count() > 0 {
                    macro_templates += 1;
                    let ActionEnvelopeKind::Action(kind) = action.kind() else {
                        unreachable!()
                    };
                    *macro_fields
                        .entry(format!("action:{}", kind.as_str()))
                        .or_insert(0usize) += 1;
                }
            }
        }
        assert_eq!(
            (collections.len(), operators.len(), actions.len(), transforms.len()),
            (33, 19, 23, 20)
        );
        assert_eq!(
            (draft.target_count(), draft.operator_count(), draft.action_count()),
            (1_790, 693, 8_975)
        );
        assert_eq!(transforms.values().sum::<usize>(), 832);
        assert_eq!(
            scalars,
            BTreeMap::from([
                ("id", 627),
                ("phase", 627),
                ("severity", 342),
                ("skip", 1),
                ("status", 2)
            ])
        );
        ids.sort_unstable();
        assert_eq!(
            (ids.len(), ids.first(), ids.last()),
            (627, Some(&901_001), Some(&980_170))
        );
        ids.dedup();
        assert_eq!(ids.len(), 627);
        assert_eq!(phases, [173, 289, 43, 109, 13]);
        assert_eq!(severities, [0, 0, 301, 16, 20, 5, 0, 0]);
        assert_eq!(statuses, BTreeMap::from([(500, 2)]));
        assert_eq!(skips, BTreeMap::from([(1, 1)]));
        macro_rules! assert_map {
            ($actual:expr, [$($name:literal => $count:literal),+ $(,)?]) => {
                assert_eq!($actual, BTreeMap::from([$(($name, $count)),+]));
            };
        }
        assert_map!(collections, [
            "ARGS"=>189, "ARGS_COMBINED_SIZE"=>1, "ARGS_GET"=>2, "ARGS_GET_NAMES"=>1,
            "ARGS_NAMES"=>186, "FILES"=>9, "FILES_COMBINED_SIZE"=>1, "FILES_NAMES"=>4,
            "MATCHED_VAR"=>3, "MATCHED_VARS"=>12, "MULTIPART_PART_HEADERS"=>5, "QUERY_STRING"=>1,
            "REMOTE_ADDR"=>2, "REQBODY_PROCESSOR"=>5, "REQUEST_BASENAME"=>5, "REQUEST_BODY"=>17,
            "REQUEST_BODY_LENGTH"=>1, "REQUEST_COOKIES"=>219, "REQUEST_COOKIES_NAMES"=>163,
            "REQUEST_FILENAME"=>61, "REQUEST_HEADERS"=>164, "REQUEST_HEADERS_NAMES"=>3,
            "REQUEST_LINE"=>7, "REQUEST_METHOD"=>8, "REQUEST_PROTOCOL"=>2, "REQUEST_URI"=>3,
            "REQUEST_URI_RAW"=>7, "RESPONSE_BODY"=>59, "RESPONSE_HEADERS"=>7, "RESPONSE_STATUS"=>2,
            "TX"=>292, "UNIQUE_ID"=>1, "XML"=>348
        ]);
        assert_map!(operators, [
            "beginsWith"=>1, "contains"=>4, "detectSQLi"=>2, "detectXSS"=>2, "endsWith"=>6,
            "eq"=>55, "ge"=>42, "gt"=>15, "ipMatch"=>2, "lt"=>190, "pm"=>12,
            "pmFromFile"=>21, "rx"=>314, "streq"=>9, "unconditionalMatch"=>1,
            "validateByteRange"=>6, "validateUrlEncoding"=>1, "validateUtf8Encoding"=>1, "within"=>9
        ]);
        assert_map!(operand_classes, [
            "no_argument"=>7, "regex"=>314, "string_template"=>29,
            "integer_template"=>302, "phrase_list"=>12, "external_assets"=>21,
            "ip_networks"=>2, "byte_set"=>6
        ]);
        assert_eq!(
            (
                phrase_total,
                draft.phrase_bytes(),
                max_phrases_per_operator,
                asset_total,
                unique_assets.len(),
                max_asset_bytes,
                network_total,
                max_networks_per_operator,
                byte_member_total,
                max_byte_members,
                max_regex_bytes,
            ),
            (56, 395, 11, 21, 19, 32, 4, 2, 29, 8, 12_106)
        );
        assert_map!(actions, [
            "auditlog"=>1, "block"=>341, "capture"=>275, "chain"=>73, "ctl"=>18, "deny"=>6,
            "id"=>627, "initcol"=>2, "log"=>8, "logdata"=>327, "msg"=>351, "multiMatch"=>8,
            "noauditlog"=>5, "nolog"=>278, "pass"=>280, "phase"=>627, "setvar"=>678,
            "severity"=>342, "skip"=>1, "skipAfter"=>206, "status"=>2, "tag"=>3060, "ver"=>627
        ]);
        assert_map!(action_operand_classes, [
            "no_argument"=>1275, "scalar"=>1599, "transformation"=>832,
            "template"=>678, "literal"=>3893, "setvar"=>678, "initcol"=>2, "ctl"=>18
        ]);
        assert_eq!(setvar_operations, [73, 605, 0, 0]);
        assert_eq!(
            (
                max_setvar_whole,
                max_setvar_key,
                max_setvar_value,
                macro_setvar_keys,
                macro_setvar_values,
                macro_setvar_union,
            ),
            (672, 71, 647, 5, 628, 629)
        );
        assert_eq!(
            initcols,
            [
                (InitColCollection::Global, "global".to_owned()),
                (InitColCollection::Ip, "%{remote_addr}_%{MATCHED_VAR}".to_owned()),
            ]
        );
        assert_map!(ctl_kinds, [
            "auditEngine"=>2, "forceRequestBodyVariable"=>2, "requestBodyProcessor"=>1,
            "ruleRemoveByTag"=>3, "ruleRemoveTargetByTag"=>10
        ]);
        assert_map!(ctl_target_collections, ["REQUEST_FILENAME"=>1, "XML"=>9]);
        assert_map!(transforms, [
            "base64Decode"=>3, "cmdLine"=>13, "compressWhitespace"=>5, "cssDecode"=>23,
            "escapeSeqDecode"=>7, "hexEncode"=>2, "htmlEntityDecode"=>35, "jsDecode"=>37,
            "length"=>3, "lowercase"=>46, "none"=>427, "normalizePath"=>8,
            "normalizePathWin"=>4, "removeCommentsChar"=>1, "removeNulls"=>27,
            "removeWhitespace"=>11, "replaceComments"=>9, "sha1"=>2, "urlDecodeUni"=>137,
            "utf8toUnicode"=>32
        ]);
        let mut fold_roles = [0_usize; 3];
        let mut disruptive = [0_usize; 3];
        let mut reset_lists = 0_usize;
        let mut effective = BTreeMap::<String, usize>::new();
        let mut max_effective = 0_usize;
        let mut max_actions = [0_usize; 3];
        let mut child_presence = [0_usize; 3];
        for fold in draft.folded_rules() {
            let class = match (fold.envelope(), fold.role()) {
                (LocalRuleEnvelope::Rule, Some(ProvisionalRuleRole::Root)) => 0,
                (LocalRuleEnvelope::Rule, Some(ProvisionalRuleRole::Child { .. })) => 1,
                (LocalRuleEnvelope::Action, None) => 2,
                _ => panic!("invalid provisional fold role"),
            };
            fold_roles[class] += 1;
            max_actions[class] = max_actions[class].max(fold.actions().len());
            if class == 1 {
                child_presence[match fold.action_presence() {
                    FoldActionPresence::Absent => 0,
                    FoldActionPresence::PresentEmpty => 1,
                    FoldActionPresence::PresentNonempty => 2,
                }] += 1;
            }
            if fold.reset() {
                reset_lists += 1;
            }
            max_effective = max_effective.max(fold.effective_transformations().len());
            for action in fold.effective_transformations() {
                *effective
                    .entry(action.transformation().unwrap().as_str().to_owned())
                    .or_default() += 1;
            }
            if let Some(value) = fold.disruptive() {
                disruptive[match value {
                    DisruptiveKind::Block => 0,
                    DisruptiveKind::Deny => 1,
                    DisruptiveKind::Pass => 2,
                }] += 1;
            }
        }
        assert_eq!(fold_roles, [620, 73, 7]);
        assert_eq!(disruptive, [341, 6, 280]);
        assert_eq!(max_actions, [29, 5, 28]);
        assert_eq!(child_presence, [2, 0, 71]);
        assert_eq!(reset_lists, 427);
        assert_eq!(effective.values().sum::<usize>(), 405);
        assert_eq!(effective.get("none"), None);
        assert_eq!(effective.len(), 19);
        assert_eq!(max_effective, 7);
        for (kind, count) in &effective {
            assert_eq!(transforms.get(kind.as_str()), Some(count));
        }
        let mut final_roles = [0_usize; 4];
        let mut inherited_phases = [0_usize; 5];
        for rule in draft.final_rules() {
            final_roles[match rule.role() {
                FinalRuleRole::StandaloneRule => 0,
                FinalRuleRole::ChainRoot { .. } => 1,
                FinalRuleRole::ChainChild { .. } => 2,
                FinalRuleRole::Action => 3,
            }] += 1;
            if let FinalPhaseState::Inherited { phase, .. } = rule.phase() {
                inherited_phases[phase as usize - 1] += 1;
            }
        }
        assert_eq!(final_roles, [565, 55, 73, 7]);
        assert_eq!(inherited_phases, [42, 29, 1, 1, 0]);
        let mut chain_lengths = BTreeMap::new();
        let mut starter_phases = [0_usize; 5];
        let mut terminal_presence = [0_usize; 3];
        for chain in draft.chains() {
            *chain_lengths.entry(chain.members().len()).or_default() += 1;
            if let FinalPhaseState::Explicit { phase, .. } = chain.phase() {
                starter_phases[phase as usize - 1] += 1;
            }
            terminal_presence[match chain.members().last().unwrap().rule().fold().action_presence() {
                FoldActionPresence::Absent => 0,
                FoldActionPresence::PresentEmpty => 1,
                FoldActionPresence::PresentNonempty => 2,
            }] += 1;
        }
        assert_eq!(chain_lengths, BTreeMap::from([(2, 41), (3, 11), (4, 2), (5, 1)]));
        assert_eq!(starter_phases, [28, 25, 1, 1, 0]);
        assert_eq!(terminal_presence, [2, 0, 53]);
        let marker_names = draft.markers().map(|marker| marker.name()).collect::<BTreeSet<_>>();
        assert_eq!(
            (
                marker_names.len(),
                marker_names.iter().map(|name| name.len()).max(),
                marker_names.iter().map(|name| name.len()).sum::<usize>()
            ),
            (30, Some(51), 955)
        );
        let resolutions = draft.resolved_skip_afters().collect::<Vec<_>>();
        let referenced = resolutions
            .iter()
            .map(|resolution| resolution.target().name())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            (
                resolutions.len(),
                referenced.len(),
                resolutions.iter().map(|value| value.target().name().len()).max(),
                resolutions
                    .iter()
                    .map(|value| value.target().name().len())
                    .sum::<usize>(),
                marker_names.difference(&referenced).count()
            ),
            (206, 28, Some(51), 7_041, 2)
        );
        assert_eq!(
            (draft.requirements().len(), draft.unavailable_capabilities().len()),
            (2_383, 79)
        );
        let mut requirement_counts = [0usize; 8];
        for record in draft.requirements() {
            requirement_counts[match record.requirement() {
                CompileRequirement::RegexCompilation => 0,
                CompileRequirement::TargetSelectorRegexCompilation => 1,
                CompileRequirement::PhraseMatcherCompilation => 2,
                CompileRequirement::ExternalPhraseAsset => 3,
                CompileRequirement::RuntimeMacro => 4,
                CompileRequirement::BlockDefaultDisruptive => 5,
                CompileRequirement::TransactionSetVar => 6,
                CompileRequirement::DefaultPhase => 7,
            }] += 1;
        }
        let mut unavailable_counts = [0usize; 3];
        for record in draft.unavailable_capabilities() {
            unavailable_counts[match record.capability() {
                UnavailableCapability::FullResponseBody => 0,
                UnavailableCapability::Ctl => 1,
                UnavailableCapability::InitCol => 2,
            }] += 1;
        }
        assert_eq!(requirement_counts, [314, 0, 33, 21, 996, 341, 678, 0]);
        assert_eq!(unavailable_counts, [59, 18, 2]);
        assert_eq!(
            (
                draft.template_count(),
                draft.template_segment_count(),
                draft.expansion_count()
            ),
            (1_689, 3_665, 1_515)
        );
        assert_eq!(
            (
                macro_templates,
                normalized_macro_bodies.len(),
                max_expansions,
                max_segments,
                max_body_bytes
            ),
            (996, 66, 23, 47, 39)
        );
        assert_eq!(
            macro_sources,
            BTreeMap::from([
                ("Args".to_owned(), 4),
                ("MatchedVar".to_owned(), 265),
                ("MatchedVarName".to_owned(), 293),
                ("RemoteAddr".to_owned(), 1),
                ("RequestHeaders".to_owned(), 3),
                ("RequestLine".to_owned(), 1),
                ("Tx".to_owned(), 948),
            ])
        );
        assert_eq!(
            macro_fields,
            BTreeMap::from([
                ("action:initcol".to_owned(), 1),
                ("action:logdata".to_owned(), 327),
                ("action:msg".to_owned(), 9),
                ("action:setvar".to_owned(), 629),
                ("operator:endsWith".to_owned(), 2),
                ("operator:ge".to_owned(), 8),
                ("operator:gt".to_owned(), 6),
                ("operator:lt".to_owned(), 3),
                ("operator:streq".to_owned(), 3),
                ("operator:within".to_owned(), 8),
            ])
        );
        assert_eq!(
            (draft.scanned_operand_bytes(), draft.decoded_operand_bytes()),
            (345_591, 123_498)
        );
        assert_eq!(
            (
                size_of::<TemplateRecord<'_>>(),
                size_of::<TemplateSegmentRecord>(),
                size_of::<ExpansionRecord<'_>>()
            ),
            (104, 80, 248)
        );
        assert_eq!(
            (
                size_of::<OperatorOperandRecord>(),
                size_of::<StaticOperandRecord<'_>>(),
                size_of::<ListItemRecord>(),
                size_of::<NetworkRecord>()
            ),
            (56, 80, 64, 72)
        );
        assert_eq!(
            (
                size_of::<SyntaxDocument<'_>>(),
                size_of::<DirectiveSemanticRecord>(),
                size_of::<SemanticTarget<'_>>(),
                size_of::<SemanticOperatorRecord<'_>>(),
                size_of::<SemanticActionRecord<'_>>(),
                size_of::<RequirementRecord>(),
                size_of::<CapabilityRecord>()
            ),
            (336, 80, 328, 128, 128, 40, 40)
        );
        assert_eq!(
            (
                size_of::<ActionOperandRecord<'_>>(),
                size_of::<TemplateSliceRecord>(),
                size_of::<SetVarRecord>(),
                size_of::<InitColRecord>(),
                size_of::<CtlRecord<'_>>(),
                size_of::<CtlTargetRecord<'_>>(),
            ),
            (88, 96, 224, 104, 96, 296)
        );
        assert_eq!((size_of::<LocalRuleFoldRecord>(), size_of::<usize>()), (264, 8));
        assert_eq!(
            (
                size_of::<FinalRuleRecord>(),
                size_of::<ChainRecord>(),
                size_of::<MarkerRecord<'_>>(),
                size_of::<ResolvedSkipAfterRecord>()
            ),
            (112, 56, 80, 24)
        );
        assert_eq!(
            (draft.semantic_charged_bytes(), draft.retained_charged_bytes()),
            (4_260_170, 9_616_147)
        );
        for values in integer_constants.values_mut() {
            values.sort_unstable();
        }
        assert_eq!(
            integer_constants
                .iter()
                .map(|(kind, values)| (*kind, (values.len(), values[0], values[values.len() - 1])))
                .collect::<BTreeMap<_, _>>(),
            BTreeMap::from([
                ("eq", (55, 0, 100)),
                ("ge", (34, 1, 5)),
                ("gt", (9, 0, 100)),
                ("lt", (187, 1, 4)),
            ])
        );
        eprintln!(
            "operand_classes={operand_classes:?} integer_ranges={integer_constants:?} phrases={phrase_total}/{} max_phrases={max_phrases_per_operator} assets={asset_total}/{} max_asset={max_asset_bytes} networks={network_total}/{max_networks_per_operator} byte_members={byte_member_total}/{max_byte_members} regex_max={max_regex_bytes} requirements={} unavailable={} templates={}/{}/{} macro_templates={macro_templates} distinct_bodies={} maxima={max_expansions}/{max_segments}/{max_body_bytes} scanned={} decoded={} operand_sizes={}/{}/{}/{} charge={} retained={}",
            draft.phrase_bytes(),
            unique_assets.len(),
            draft.requirements().len(),
            draft.unavailable_capabilities().len(),
            draft.template_count(),
            draft.template_segment_count(),
            draft.expansion_count(),
            normalized_macro_bodies.len(),
            draft.scanned_operand_bytes(),
            draft.decoded_operand_bytes(),
            size_of::<OperatorOperandRecord>(),
            size_of::<StaticOperandRecord<'_>>(),
            size_of::<ListItemRecord>(),
            size_of::<NetworkRecord>(),
            draft.semantic_charged_bytes(),
            draft.retained_charged_bytes()
        );
    }
}
