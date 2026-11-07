use std::mem::size_of;
use std::net::IpAddr;

use thiserror::Error;

use crate::matcher::{
    CompiledCapturePhraseProgram, CompiledCaptureRegexProgram, CompiledMatcher, CompiledRegexProgram,
    MatcherBuildProjection, MatcherCompileError, MatcherLimits, MatcherPatternBytes, MatcherPatternCounts, PatternSpec,
    RegexProgramError, RegexProgramLimits,
};

use super::{
    ActionEnvelopeKind, ActionOperandView, CollectionKind, CompileRequirement, CtlView, DirectiveLocation,
    ExternalAssetView, LocalActionValue, MacroExpansionView, MacroSource, OnOff, OperandTemplateView, Phase,
    RegexStyle, RequestBodyProcessor, ResolvedDisruptive, SelectorKind, SemanticActionView, SemanticLocation,
    SemanticNodeKind, SetVarOperation, TargetModifier, TemplateSegmentKind, TemplateSliceView, TransformationKind,
    ValidatedSemanticDocument,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InspectionDomain {
    Json,
    Xml,
}
#[path = "xml_inspection_plan.rs"]
mod xml_inspection_plan;
pub use xml_inspection_plan::XmlInspectionUnsupported;
#[cfg(test)]
pub(crate) use xml_inspection_plan::XmlInspectionUnsupportedReason;
#[path = "capture_hit_plan.rs"]
mod capture_hit_plan;
#[path = "expensive_checks.rs"]
mod expensive_checks;
#[path = "inspection_activation.rs"]
mod inspection_activation;
#[path = "json_inspection_plan.rs"]
mod json_inspection_plan;
pub use json_inspection_plan::JsonInspectionUnsupported;

#[path = "transaction.rs"]
mod transaction;

pub use transaction::{
    EvaluationMode, JsonInspectionLimits, JsonInspectionSession, JsonInspectionUsage, JsonResultLimit,
    JsonResultLimits, PhaseOutcome, RequestBodyPrefixCompletion, RequestHead, RequestHeader, ResponseHead,
    ResponseHeader, ResponseWindow, RuleCheckPolicy, RuleEngineMode, RuntimeError, RuntimeLimits, Transaction,
    XmlInspectionLimits, XmlInspectionSession, XmlInspectionUsage, DEFAULT_REQUEST_BODY_PREFIX_BYTES,
    MAX_REQUEST_BODY_PREFIX_BYTES,
};

#[cfg(test)]
pub(crate) use transaction::DEFAULT_MAX_RUNTIME_OPERATOR_WORK_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompilerLimits {
    pub matcher: MatcherLimits,
    pub regex: RegexProgramLimits,
    pub max_units: usize,
    pub max_nodes: usize,
    pub max_action_plans: usize,
    pub max_schedule_entries: usize,
    pub max_schedule_entries_per_phase: usize,
    pub max_matcher_groups: usize,
    pub max_matcher_patterns: usize,
    pub max_matcher_source_bytes: usize,
    pub max_templates: usize,
    pub max_template_segments: usize,
    pub max_owned_template_metadata_bytes: usize,
    pub max_template_static_bytes: usize,
    pub max_template_runtime_bytes: usize,
    pub max_rule_runtime_expansion_bytes: usize,
    pub max_asset_requests: usize,
    pub max_asset_bytes: usize,
    pub max_total_asset_bytes: usize,
    pub max_phrases_per_asset: usize,
    pub max_asset_phrases: usize,
    pub max_asset_phrase_bytes: usize,
    pub max_regex_programs: usize,
    pub max_compiler_owned_non_matcher_bytes: usize,
    pub max_compiled_matcher_bytes: usize,
    pub max_generation_bytes: usize,
    pub max_compilation_peak_bytes: usize,
    pub max_group_table_probes: usize,
    pub max_group_key_record_comparisons: usize,
    pub max_group_key_comparison_bytes: usize,
    pub max_group_key_hash_bytes: usize,
}

impl Default for CompilerLimits {
    fn default() -> Self {
        Self {
            matcher: MatcherLimits::default(),
            regex: RegexProgramLimits::default(),
            max_units: 100_000,
            max_nodes: 100_000,
            max_action_plans: 500_000,
            max_schedule_entries: 500_000,
            max_schedule_entries_per_phase: 200_000,
            max_matcher_groups: 100_000,
            max_matcher_patterns: 1_000_000,
            max_matcher_source_bytes: 64 * 1024 * 1024,
            max_templates: 200_000,
            max_template_segments: 500_000,
            max_owned_template_metadata_bytes: 64 * 1024 * 1024,
            max_template_static_bytes: 64 * 1024,
            max_template_runtime_bytes: 256 * 1024,
            max_rule_runtime_expansion_bytes: 8 * 1024 * 1024,
            max_asset_requests: 200_000,
            max_asset_bytes: 4 * 1024 * 1024,
            max_total_asset_bytes: 64 * 1024 * 1024,
            max_phrases_per_asset: 100_000,
            max_asset_phrases: 1_000_000,
            max_asset_phrase_bytes: 64 * 1024 * 1024,
            max_regex_programs: 500_000,
            max_compiler_owned_non_matcher_bytes: 128 * 1024 * 1024,
            max_compiled_matcher_bytes: 512 * 1024 * 1024,
            max_generation_bytes: 768 * 1024 * 1024,
            max_compilation_peak_bytes: 1024 * 1024 * 1024,
            max_group_table_probes: 4_000_000,
            max_group_key_record_comparisons: 4_000_000,
            max_group_key_comparison_bytes: 256 * 1024 * 1024,
            max_group_key_hash_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhraseAssetFailure {
    NotFound,
    Denied,
    Unavailable,
    InvalidData,
    Other,
}

impl std::fmt::Display for PhraseAssetFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "phrase asset provider failed with {self:?}")
    }
}

impl std::error::Error for PhraseAssetFailure {}

#[derive(Clone, Copy)]
pub struct PhraseAssetRequest<'view> {
    request_id: usize,
    location: SemanticLocation,
    asset: ExternalAssetView<'view>,
}

impl std::fmt::Debug for PhraseAssetRequest<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PhraseAssetRequest")
            .field("request_id", &self.request_id)
            .field("document_index", &self.document_index())
            .field("location", &self.location)
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'view> PhraseAssetRequest<'view> {
    pub const fn request_id(self) -> usize {
        self.request_id
    }
    pub const fn document_index(self) -> usize {
        self.asset.document_index()
    }
    pub const fn location(self) -> SemanticLocation {
        self.location
    }
    pub const fn handle(self) -> &'view str {
        self.asset.handle()
    }
    pub const fn span(self) -> super::SourceSpan {
        self.asset.span()
    }
}

pub trait PhraseAssetSource {
    fn load(
        &mut self,
        request: PhraseAssetRequest<'_>,
        sink: &mut PhraseAssetSink<'_>,
    ) -> Result<(), PhraseAssetFailure>;
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PhraseAssetSinkError {
    #[error("phrase asset byte arithmetic overflowed")]
    Arithmetic,
    #[error("phrase asset has {actual} bytes, exceeding the per-asset limit of {limit}")]
    AssetByteLimit { actual: usize, limit: usize },
    #[error("phrase assets have {actual} cumulative bytes, exceeding the limit of {limit}")]
    TotalAssetByteLimit { actual: usize, limit: usize },
    #[error("phrase asset loading requires {actual} working bytes, exceeding the limit of {limit}")]
    WorkingByteLimit { actual: usize, limit: usize },
    #[error("cannot allocate bounded phrase asset bytes")]
    Allocation,
}

pub struct PhraseAssetSink<'sink> {
    bytes: &'sink mut Vec<u8>,
    sticky: Option<PhraseAssetSinkError>,
    prior_asset_bytes: usize,
    working_base_bytes: usize,
    limits: CompilerLimits,
    peak_bytes: &'sink mut usize,
}

impl PhraseAssetSink<'_> {
    pub fn extend_from_slice(&mut self, chunk: &[u8]) -> Result<(), PhraseAssetSinkError> {
        if let Some(error) = self.sticky {
            return Err(error);
        }
        let result = self.extend_once(chunk);
        if let Err(error) = result {
            self.sticky = Some(error);
        }
        result
    }

    fn extend_once(&mut self, chunk: &[u8]) -> Result<(), PhraseAssetSinkError> {
        let actual = self
            .bytes
            .len()
            .checked_add(chunk.len())
            .ok_or(PhraseAssetSinkError::Arithmetic)?;
        let cumulative = self
            .prior_asset_bytes
            .checked_add(actual)
            .ok_or(PhraseAssetSinkError::Arithmetic)?;
        let working = self
            .working_base_bytes
            .checked_add(actual)
            .ok_or(PhraseAssetSinkError::Arithmetic)?;
        *self.peak_bytes = (*self.peak_bytes).max(working);
        if actual > self.limits.max_asset_bytes {
            return Err(PhraseAssetSinkError::AssetByteLimit {
                actual,
                limit: self.limits.max_asset_bytes,
            });
        }
        if cumulative > self.limits.max_total_asset_bytes {
            return Err(PhraseAssetSinkError::TotalAssetByteLimit {
                actual: cumulative,
                limit: self.limits.max_total_asset_bytes,
            });
        }
        if working > self.limits.max_compilation_peak_bytes {
            return Err(PhraseAssetSinkError::WorkingByteLimit {
                actual: working,
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        self.bytes
            .try_reserve_exact(chunk.len())
            .map_err(|_| PhraseAssetSinkError::Allocation)?;
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompilerError {
    #[error("validated compiler input violates private invariant {kind:?}")]
    PrivateInvariant { kind: CompilerInvariant },
    #[error("compiler preflight arithmetic overflowed")]
    PreflightArithmetic,
    #[error("compiler input has {actual} rule units, exceeding the limit of {limit}")]
    UnitLimit { actual: usize, limit: usize },
    #[error("compiler input has {actual} rule nodes, exceeding the limit of {limit}")]
    NodeLimit { actual: usize, limit: usize },
    #[error("compiler input has {actual} actions, exceeding the limit of {limit}")]
    ActionLimit { actual: usize, limit: usize },
    #[error("request-body ctl action is only valid in request headers at {location:?}")]
    CtlPhase { location: SemanticLocation },
    #[error("unsupported XML ctl target selector at {location:?}")]
    CtlXmlSelector { location: SemanticLocation },
    #[error("compiler input has {actual} asset requests, exceeding the limit of {limit}")]
    AssetRequestLimit { actual: usize, limit: usize },
    #[error("phrase asset source is missing for request {request_id} at {location:?}")]
    MissingAssetSource {
        request_id: usize,
        location: SemanticLocation,
    },
    #[error("phrase asset request {request_id} failed with {failure:?} at {location:?}")]
    AssetSource {
        request_id: usize,
        failure: PhraseAssetFailure,
        location: SemanticLocation,
    },
    #[error("phrase asset request {request_id} failed its bounded sink at {location:?}: {error}")]
    AssetSink {
        request_id: usize,
        error: PhraseAssetSinkError,
        location: SemanticLocation,
    },
    #[error("phrase asset request {request_id} contains invalid UTF-8 at {location:?}")]
    AssetUtf8 {
        request_id: usize,
        location: SemanticLocation,
    },
    #[error("phrase asset request {request_id} contains a bare carriage return on line {line} at {location:?}")]
    AssetBareCarriageReturn {
        request_id: usize,
        line: usize,
        location: SemanticLocation,
    },
    #[error("phrase asset request {request_id} contains no phrases at {location:?}")]
    EmptyAsset {
        request_id: usize,
        location: SemanticLocation,
    },
    #[error("phrase asset request {request_id} has {actual} phrases, exceeding the limit of {limit} at {location:?}")]
    AssetPhraseLimit {
        request_id: usize,
        actual: usize,
        limit: usize,
        location: SemanticLocation,
    },
    #[error("asset phrases have {actual} cumulative entries, exceeding the limit of {limit} at {location:?}")]
    TotalAssetPhraseLimit {
        actual: usize,
        limit: usize,
        location: SemanticLocation,
    },
    #[error("asset phrases have {actual} cumulative bytes, exceeding the limit of {limit} at {location:?}")]
    AssetPhraseByteLimit {
        actual: usize,
        limit: usize,
        location: SemanticLocation,
    },
    #[error("compiled template count exceeds the limit of {limit} at {location:?}")]
    TemplateLimit { limit: usize, location: SemanticLocation },
    #[error("compiled template segment count exceeds the limit of {limit} at {location:?}")]
    TemplateSegmentLimit { limit: usize, location: SemanticLocation },
    #[error("template has {actual} static bytes, exceeding the limit of {limit} at {location:?}")]
    TemplateStaticByteLimit {
        actual: usize,
        limit: usize,
        location: SemanticLocation,
    },
    #[error("template static bytes exceed runtime limit {limit} at {location:?}")]
    TemplateRuntimeByteLimit { limit: usize, location: SemanticLocation },
    #[error("member runtime expansion caps total {actual}, exceeding the limit of {limit} at {location:?}")]
    MemberRuntimeByteLimit {
        actual: usize,
        limit: usize,
        location: DirectiveLocation,
    },
    #[error("compiler-owned template and metadata bytes exceed the limit of {limit}")]
    OwnedByteLimit { limit: usize },
    #[error("compile peak exceeds the limit of {limit}")]
    PeakByteLimit { limit: usize },
    #[error("regex program failed at {location:?}: {error}")]
    RegexProgram {
        location: SemanticLocation,
        #[source]
        error: RegexProgramError,
    },
    #[error("compiler input has {actual} regex programs, exceeding the limit of {limit}")]
    RegexProgramLimit { actual: usize, limit: usize },
    #[error("compiler input has {actual} matcher groups, exceeding the limit of {limit}")]
    MatcherGroupLimit { actual: usize, limit: usize },
    #[error("compiler input has {actual} matcher patterns, exceeding the limit of {limit}")]
    MatcherPatternLimit { actual: usize, limit: usize },
    #[error("compiler matcher sources have {actual} bytes, exceeding the limit of {limit}")]
    MatcherSourceByteLimit { actual: usize, limit: usize },
    #[error("compiled schedules have {actual} entries, exceeding the limit of {limit}")]
    ScheduleEntryLimit { actual: usize, limit: usize },
    #[error("compiled phase {phase} schedule has {actual} entries, exceeding the limit of {limit}")]
    PhaseScheduleEntryLimit { phase: usize, actual: usize, limit: usize },
    #[error("matcher canonical tables exceed the probe-work limit of {limit}")]
    GroupTableProbeLimit { limit: usize },
    #[error("matcher canonical keys exceed the record-comparison limit of {limit}")]
    GroupKeyRecordComparisonLimit { limit: usize },
    #[error("matcher canonical keys exceed the byte-comparison limit of {limit}")]
    GroupKeyComparisonByteLimit { limit: usize },
    #[error("matcher canonical keys exceed the hash-work limit of {limit}")]
    GroupKeyHashByteLimit { limit: usize },
    #[error("phrase matcher failed at {location:?} with bounded category {kind:?}")]
    PhraseMatcher {
        location: SemanticLocation,
        kind: PhraseMatcherErrorKind,
    },
    #[error("compiled matcher bytes exceed the limit of {limit}")]
    CompiledMatcherByteLimit { limit: usize },
    #[error("compiled generation bytes exceed the limit of {limit}")]
    GenerationByteLimit { limit: usize },
    #[error("cannot allocate bounded compiler arena {operation}")]
    Allocation { operation: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompilerInvariant {
    DefaultRequirement,
    MissingRequirement,
    DuplicateRequirement,
    UnexpectedActionOperand,
    InvalidByteRange,
    MissingTargetRegex,
    ScheduleOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhraseMatcherErrorKind {
    Arithmetic,
    Allocation,
    PatternLimit,
    PatternByteLimit,
    EmptyPhrase,
    StateLimit,
    TransitionLimit,
    OutputLimit,
    CompiledByteLimit,
    Build,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: usize,
    len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompiledSegmentKind {
    Literal(ByteRange),
    Macro(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledTemplateSegment {
    kind: CompiledSegmentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledMacroRef {
    source: MacroSource,
    key: ByteRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledTemplate {
    segment_start: usize,
    segment_count: usize,
    static_bytes: usize,
    runtime_byte_limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledSetVar {
    collection: CollectionKind,
    operation: SetVarOperation,
    key: usize,
    value: Option<usize>,
    constant: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompiledCtlSelector {
    None,
    Literal(ByteRange),
    Path(ByteRange),
    RegexSource(ByteRange),
    Resolved(CompiledTargetSelector),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompiledCtl {
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
struct CompiledTagRuleRef {
    tag: ByteRange,
    unit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompiledActionOperand {
    None,
    Local(LocalActionValue),
    Transformation(TransformationKind),
    Template(usize),
    Literal(ByteRange),
    SetVar(usize),
    Ctl(CompiledCtl),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledActionPlan {
    kind: ActionEnvelopeKind,
    operand: CompiledActionOperand,
    location: SemanticLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledMemberPlan {
    active: bool,
    expensive_aggregate_actions: bool,
    operator_template: Option<usize>,
    target_plan: Option<usize>,
    transformation_plan: Option<usize>,
    phase: Phase,
    disruptive: ResolvedDisruptive,
    status: Option<u16>,
    log: bool,
    audit_log: bool,
    action_start: usize,
    action_count: usize,
    transformation_start: usize,
    transformation_count: usize,
    runtime_expansion_byte_limit: usize,
    skip_after_marker: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledPhrase {
    bytes: ByteRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedAsset {
    request_id: usize,
    phrase_start: usize,
    phrase_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct RequirementState {
    kind: CompileRequirement,
    location: SemanticLocation,
    discharged: bool,
}

#[derive(Debug)]
struct AssetTemplateState<'source> {
    compilation_input_bytes: usize,
    validated: ValidatedSemanticDocument<'source>,
    bytes: Vec<u8>,
    asset_phrase_bytes: Vec<u8>,
    templates: Vec<CompiledTemplate>,
    segments: Vec<CompiledTemplateSegment>,
    macros: Vec<CompiledMacroRef>,
    actions: Vec<CompiledActionPlan>,
    setvars: Vec<CompiledSetVar>,
    members: Vec<CompiledMemberPlan>,
    transformations: Vec<TransformationKind>,
    assets: Vec<ResolvedAsset>,
    phrases: Vec<CompiledPhrase>,
    requirements: Vec<RequirementState>,
    compiler_owned_bytes: usize,
    compiler_working_bytes: usize,
    peak_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringRelation {
    BeginsWith,
    Contains,
    EndsWith,
    Equals,
    Within,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntegerRelation {
    Equal,
    GreaterOrEqual,
    Greater,
    Less,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompiledIntegerOperand {
    Static(i64),
    Template(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledIpNetwork {
    address: IpAddr,
    prefix: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegexPlanRef {
    Boolean(usize),
    Capture(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompiledOperatorOperand {
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
struct CompiledOperatorPlan {
    kind: super::OperatorKind,
    negated: bool,
    operand: CompiledOperatorOperand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledTargetRegexPlan {
    identity: SemanticLocation,
    program: usize,
}

#[derive(Debug, Clone, Copy)]
struct RegexCandidate<'source> {
    source: &'source str,
    location: SemanticLocation,
    capture: bool,
    active: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct RegexCandidateCounts {
    selectors: usize,
    active_boolean: usize,
    active_capture: usize,
    removed_boolean: usize,
    removed_capture: usize,
}

impl RegexCandidateCounts {
    fn total(self) -> Option<usize> {
        self.selectors
            .checked_add(self.active_boolean)?
            .checked_add(self.active_capture)?
            .checked_add(self.removed_boolean)?
            .checked_add(self.removed_capture)
    }
    fn active(self) -> Option<usize> {
        self.selectors.checked_add(self.active_capture)
    }
}

#[derive(Debug)]
struct MatcherState<'source> {
    asset_templates: AssetTemplateState<'source>,
    operator_plans: Vec<Option<CompiledOperatorPlan>>,
    ip_networks: Vec<CompiledIpNetwork>,
    target_regexes: Vec<CompiledTargetRegexPlan>,
    boolean_regexes: Vec<CompiledRegexProgram>,
    capture_regexes: Vec<CompiledCaptureRegexProgram>,
    canonical_work: CanonicalWork,
    compiler_owned_bytes: usize,
    matcher_charged_bytes: usize,
    peak_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    None,
    #[cfg_attr(not(test), expect(dead_code, reason = "capture regex programs remain dedicated"))]
    RegexGroups,
    #[cfg_attr(not(test), expect(dead_code, reason = "capture phrase programs remain dedicated"))]
    PhraseSpans,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteView {
    RawBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompiledTargetSelector {
    None,
    Literal(ByteRange),
    Path(ByteRange),
    Regex { style: RegexStyle, program: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledTargetRecord {
    collection: CollectionKind,
    modifier: TargetModifier,
    selector: CompiledTargetSelector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledTargetPlan {
    record_start: usize,
    record_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledTransformationPlan {
    transformation_start: usize,
    transformation_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledMatcherGroupKey {
    phase: Phase,
    target_plan: usize,
    transformation_plan: usize,
    multi_match: bool,
    capture_mode: CaptureMode,
    byte_view: ByteView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledPatternMapping {
    unit: usize,
    member: usize,
    local_ordinal: usize,
}

#[derive(Debug)]
struct CompiledMatcherGroup {
    key: CompiledMatcherGroupKey,
    matcher: CompiledMatcher,
    mapping_start: usize,
    mapping_count: usize,
}

#[derive(Debug)]
struct GroupedMatcherState<'source> {
    pattern_preflight: GroupPatternPreflight,
    canonical_work: CanonicalWork,
    matchers: MatcherState<'source>,
    target_bytes: Vec<u8>,
    target_records: Vec<CompiledTargetRecord>,
    target_plans: Vec<CompiledTargetPlan>,
    canonical_transformations: Vec<TransformationKind>,
    transformation_plans: Vec<CompiledTransformationPlan>,
    groups: Vec<CompiledMatcherGroup>,
    pattern_mappings: Vec<CompiledPatternMapping>,
    capture_phrases: Vec<CompiledCapturePhraseProgram>,
    compiler_owned_bytes: usize,
    matcher_charged_bytes: usize,
    peak_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompiledScheduleEntry {
    RuleUnit(usize),
    MarkerSentinel(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledRuleUnit {
    expensive_check: bool,
    id: u64,
    phase: Phase,
    disruptive: ResolvedDisruptive,
    status: Option<u16>,
    log: bool,
    audit_log: bool,
    member_start: usize,
    member_count: usize,
    schedule_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompiledMarker {
    schedule_indices: [usize; 5],
}

/// Fully owned immutable SecLang generation ready for transaction execution.
#[derive(Debug)]
#[expect(dead_code, reason = "transaction execution consumes the immutable compiled arenas")]
pub struct CompiledGeneration {
    capture_hits: capture_hit_plan::CaptureHitPlan,
    has_expensive_checks: bool,
    inspection_activation: inspection_activation::CompiledActivation,
    json_inspection: json_inspection_plan::CompiledJsonPlan,
    xml_inspection: json_inspection_plan::CompiledJsonPlan,
    prefix_validation: Result<(), &'static str>,
    requires_complete_request_body: bool,
    bytes: Vec<u8>,
    templates: Vec<CompiledTemplate>,
    segments: Vec<CompiledTemplateSegment>,
    macros: Vec<CompiledMacroRef>,
    actions: Vec<CompiledActionPlan>,
    tag_rule_refs: Vec<CompiledTagRuleRef>,
    setvars: Vec<CompiledSetVar>,
    members: Vec<CompiledMemberPlan>,
    operator_plans: Vec<Option<CompiledOperatorPlan>>,
    ip_networks: Vec<CompiledIpNetwork>,
    target_regexes: Vec<CompiledTargetRegexPlan>,
    boolean_regexes: Vec<CompiledRegexProgram>,
    capture_regexes: Vec<CompiledCaptureRegexProgram>,
    target_bytes: Vec<u8>,
    target_records: Vec<CompiledTargetRecord>,
    target_plans: Vec<CompiledTargetPlan>,
    transformations: Vec<TransformationKind>,
    transformation_plans: Vec<CompiledTransformationPlan>,
    groups: Vec<CompiledMatcherGroup>,
    pattern_mappings: Vec<CompiledPatternMapping>,
    capture_phrases: Vec<CompiledCapturePhraseProgram>,
    units: Vec<CompiledRuleUnit>,
    markers: Vec<CompiledMarker>,
    schedules: [Vec<CompiledScheduleEntry>; 5],
    compiler_owned_bytes: usize,
    matcher_charged_bytes: usize,
    peak_bytes: usize,
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

    pub fn operator_kind(self) -> Option<super::OperatorKind> {
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

    fn has_complete_request_body_dependency(&self) -> bool {
        self.units.iter().any(|unit| {
            if unit.phase == Phase::RequestHeaders {
                return false;
            }
            self.members[unit.member_start..unit.member_start + unit.member_count]
                .iter()
                .any(|member| self.member_requires_request_body(*member))
        })
    }

    fn member_requires_request_body(&self, member: CompiledMemberPlan) -> bool {
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

    fn check_request_body_prefix(&self) -> Result<(), &'static str> {
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

    fn inspection_plan(&self, domain: InspectionDomain) -> &json_inspection_plan::CompiledJsonPlan {
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

fn member_group_key(plan: CompiledMemberPlan, multi_match: bool) -> Result<CompiledMatcherGroupKey, CompilerError> {
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
            ActionEnvelopeKind::Action(kind) => is_streaming_safe_action(kind) || kind == super::ActionKind::Msg,
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
    if !generation.operator_plans[index].is_some_and(|operator| {
        !operator.negated && matches!(operator.kind, super::OperatorKind::Contains | super::OperatorKind::Pm)
    }) {
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
            // JSON duplicate keys replace earlier subtrees, and object traversal
            // order affects captures and actions. Keep complete publication until
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
            matches!(plan.kind, super::OperatorKind::Pm | super::OperatorKind::PmFromFile)
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
/// additionally accepts `Msg` (see `prefix_actions_supported`): `Msg`/`LogData`
/// templates are expanded at finding time (`transaction.rs` `expand_template`)
/// and a streaming raw member has no retained value for them. Follow-up F-1 in
/// the 2026-11-07 WAF optimization spec tracks whether to unify.
fn is_streaming_safe_action(kind: super::ActionKind) -> bool {
    matches!(
        kind,
        super::ActionKind::AuditLog
            | super::ActionKind::Block
            | super::ActionKind::Deny
            | super::ActionKind::Id
            | super::ActionKind::Log
            | super::ActionKind::NoAuditLog
            | super::ActionKind::NoLog
            | super::ActionKind::Pass
            | super::ActionKind::Phase
            | super::ActionKind::Severity
            | super::ActionKind::Status
            | super::ActionKind::Tag
            | super::ActionKind::Ver
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateTargetSelector {
    None,
    Literal(ByteRange),
    Path(ByteRange),
    Regex { style: RegexStyle, program: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CandidateTargetRecord {
    collection: CollectionKind,
    modifier: TargetModifier,
    selector: CandidateTargetSelector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CandidateTargetPlan {
    record_start: usize,
    record_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CanonicalSlot {
    hash: u64,
    id: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidatePatternKind {
    Phrase,
    Regex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CandidatePattern {
    group: usize,
    kind: CandidatePatternKind,
    bytes: ByteRange,
    mapping: CompiledPatternMapping,
    next: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MemberMatcherBinding {
    member: usize,
    group: usize,
    pattern_start: usize,
    pattern_count: usize,
    kind: super::OperatorKind,
    negated: bool,
}

const fn phase_index(phase: Phase) -> usize {
    match phase {
        Phase::RequestHeaders => 0,
        Phase::RequestBody => 1,
        Phase::ResponseHeaders => 2,
        Phase::ResponseBody => 3,
        Phase::Logging => 4,
    }
}

impl CanonicalSlot {
    const EMPTY: Self = Self {
        hash: 0,
        id: usize::MAX,
    };

    const fn is_empty(self) -> bool {
        self.id == usize::MAX
    }
}

#[derive(Clone, Copy)]
struct RegexProgramSource<'source> {
    source: &'source str,
    ascii_case_insensitive: bool,
}

/// Compilation-local identity for exact source and byte compilation profile.
/// Sources borrow the validated arena; no source copy survives publication.
struct RegexProgramInterner<'source> {
    slots: Vec<CanonicalSlot>,
    sources: Vec<RegexProgramSource<'source>>,
}
impl<'source> RegexProgramInterner<'source> {
    fn new(occurrences: usize, memory: &mut MatcherCompileWork) -> Result<Self, CompilerError> {
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

    fn compile(
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

    fn compile_with<P>(
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

    fn release(self, memory: &mut MatcherCompileWork) {
        let slots = self.slots.capacity();
        let sources = self.sources.capacity();
        drop(self);
        memory.drop_working_records::<CanonicalSlot>(slots);
        memory.drop_working_records::<RegexProgramSource<'_>>(sources);
    }
}

const FNV1A_OFFSET_BASIS: u64 = 14_695_981_039_346_656_037;
const FNV1A_PRIME: u64 = 1_099_511_628_211;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct CanonicalWork {
    probes: usize,
    record_comparisons: usize,
    comparison_bytes: usize,
    hash_bytes: usize,
}

impl CanonicalWork {
    fn charge_probe(&mut self, limits: CompilerLimits) -> Result<(), CompilerError> {
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

    fn hash_bytes(&mut self, hash: &mut u64, bytes: &[u8], limits: CompilerLimits) -> Result<(), CompilerError> {
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

    fn hash_u64(&mut self, hash: &mut u64, value: u64, limits: CompilerLimits) -> Result<(), CompilerError> {
        self.hash_bytes(hash, &value.to_le_bytes(), limits)
    }

    fn hash_enum_name(&mut self, hash: &mut u64, value: &str, limits: CompilerLimits) -> Result<(), CompilerError> {
        self.hash_u64(
            hash,
            u64::try_from(value.len()).map_err(|_| CompilerError::PreflightArithmetic)?,
            limits,
        )?;
        self.hash_bytes(hash, value.as_bytes(), limits)
    }
}

fn canonical_table_slots(candidates: usize) -> Result<usize, CompilerError> {
    if candidates == 0 {
        return Ok(0);
    }
    candidates
        .checked_mul(2)
        .and_then(|count| count.max(2).checked_next_power_of_two())
        .ok_or(CompilerError::PreflightArithmetic)
}

fn byte_range(bytes: &[u8], range: ByteRange) -> Result<&[u8], CompilerError> {
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

fn hash_target_plan(
    plan: CandidateTargetPlan,
    records: &[CandidateTargetRecord],
    bytes: &[u8],
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<u64, CompilerError> {
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

#[allow(clippy::too_many_arguments)]
fn target_plan_equal(
    candidate: CandidateTargetPlan,
    candidate_records: &[CandidateTargetRecord],
    candidate_bytes: &[u8],
    canonical: CompiledTargetPlan,
    canonical_records: &[CompiledTargetRecord],
    canonical_bytes: &[u8],
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<bool, CompilerError> {
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

#[allow(clippy::too_many_arguments)]
fn copy_target_plan(
    candidate: CandidateTargetPlan,
    candidate_records: &[CandidateTargetRecord],
    candidate_bytes: &[u8],
    target_records: &mut Vec<CompiledTargetRecord>,
    target_bytes: &mut Vec<u8>,
    target_plans: &mut Vec<CompiledTargetPlan>,
    work: &mut MatcherCompileWork,
) -> Result<usize, CompilerError> {
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
                    _ => unreachable!(),
                }
            }
            CandidateTargetSelector::Regex { style, program } => CompiledTargetSelector::Regex { style, program },
        };
        work.reserve_canonical(target_records, 1, "canonical target records")?;
        target_records.push(CompiledTargetRecord {
            collection: record.collection,
            modifier: record.modifier,
            selector,
        });
    }
    work.reserve_canonical(target_plans, 1, "canonical target plans")?;
    let id = target_plans.len();
    target_plans.push(CompiledTargetPlan {
        record_start,
        record_count: candidate.record_count,
    });
    Ok(id)
}

fn hash_transformations(
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

#[allow(clippy::too_many_arguments)]
fn insert_target_plan(
    candidate: CandidateTargetPlan,
    hash: u64,
    slots: &mut [CanonicalSlot],
    candidate_records: &[CandidateTargetRecord],
    candidate_bytes: &[u8],
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
            let id = copy_target_plan(
                candidate,
                candidate_records,
                candidate_bytes,
                target_records,
                target_bytes,
                target_plans,
                compile_work,
            )?;
            slots[index] = CanonicalSlot { hash, id };
            return Ok(id);
        }
        if slot.hash == hash
            && target_plan_equal(
                candidate,
                candidate_records,
                candidate_bytes,
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

fn insert_transformation_plan(
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

fn hash_group_key(
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

fn insert_group_key(
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

fn map_phrase_matcher_error(error: MatcherCompileError, location: SemanticLocation) -> CompilerError {
    let kind = match error {
        MatcherCompileError::PeakArithmetic => PhraseMatcherErrorKind::Arithmetic,
        MatcherCompileError::Allocation { .. } => PhraseMatcherErrorKind::Allocation,
        MatcherCompileError::PatternLimit { .. } => PhraseMatcherErrorKind::PatternLimit,
        MatcherCompileError::PatternByteLimit { .. } => PhraseMatcherErrorKind::PatternByteLimit,
        MatcherCompileError::EmptyPhrase { .. } => PhraseMatcherErrorKind::EmptyPhrase,
        MatcherCompileError::PhraseStateLimit { .. } => PhraseMatcherErrorKind::StateLimit,
        MatcherCompileError::PhraseTransitionLimit { .. } => PhraseMatcherErrorKind::TransitionLimit,
        MatcherCompileError::PhraseOutputLimit { .. } => PhraseMatcherErrorKind::OutputLimit,
        MatcherCompileError::PhraseCompiledByteLimit { .. } => PhraseMatcherErrorKind::CompiledByteLimit,
        MatcherCompileError::PhraseBuild { .. }
        | MatcherCompileError::Regex { .. }
        | MatcherCompileError::RegexSourceByteLimit { .. }
        | MatcherCompileError::RegexSourceGroupByteLimit { .. }
        | MatcherCompileError::RegexProgramByteLimit { .. } => PhraseMatcherErrorKind::Build,
    };
    CompilerError::PhraseMatcher { location, kind }
}

fn visit_phrase_operand(
    operand: super::OperatorOperandView<'_, '_>,
    assets: &[ResolvedAsset],
    phrases: &[CompiledPhrase],
    asset_bytes: &[u8],
    asset_cursor: &mut usize,
    mut visit: impl FnMut(&str, usize) -> Result<(), CompilerError>,
) -> Result<usize, CompilerError> {
    let mut ordinal = 0_usize;
    match operand {
        super::OperatorOperandView::PhraseList(list) => {
            for phrase in list.phrases() {
                visit(phrase.text(), ordinal)?;
                ordinal = checked_inc(ordinal)?;
            }
        }
        super::OperatorOperandView::ExternalAssets(external) => {
            for _ in external.assets() {
                let asset = assets.get(*asset_cursor).ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                })?;
                if asset.request_id != *asset_cursor {
                    return Err(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    });
                }
                let end = asset
                    .phrase_start
                    .checked_add(asset.phrase_count)
                    .ok_or(CompilerError::PreflightArithmetic)?;
                for phrase in phrases
                    .get(asset.phrase_start..end)
                    .ok_or(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    })?
                {
                    let bytes = byte_range(asset_bytes, phrase.bytes)?;
                    let text = std::str::from_utf8(bytes).map_err(|_| CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    })?;
                    visit(text, ordinal)?;
                    ordinal = checked_inc(ordinal)?;
                }
                *asset_cursor = checked_inc(*asset_cursor)?;
            }
        }
        _ => {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::UnexpectedActionOperand,
            });
        }
    }
    Ok(ordinal)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct GroupPatternPreflight {
    matcher_members: usize,
    grouped_patterns: usize,
    grouped_source_bytes: usize,
    active_capture_phrase_programs: usize,
    active_patterns: usize,
    active_source_bytes: usize,
}

impl GroupPatternPreflight {
    fn inspect(state: &MatcherState<'_>, limits: CompilerLimits) -> Result<Self, CompilerError> {
        let mut result = Self::default();
        let mut asset_cursor = 0_usize;
        for unit in state.asset_templates.validated.rule_units() {
            if unit.is_compatibility_excluded() {
                continue;
            }
            for member in unit.members() {
                let Some(operator) = member.operator() else {
                    continue;
                };
                let capture = member.rule().fold().capture_action().is_some();
                match operator.kind() {
                    super::OperatorKind::Pm | super::OperatorKind::PmFromFile => {
                        let mut patterns = 0_usize;
                        let mut bytes = 0_usize;
                        visit_phrase_operand(
                            operator.operand(),
                            &state.asset_templates.assets,
                            &state.asset_templates.phrases,
                            &state.asset_templates.asset_phrase_bytes,
                            &mut asset_cursor,
                            |phrase, _| {
                                patterns = checked_inc(patterns)?;
                                bytes = checked_add(bytes, phrase.len())?;
                                Ok(())
                            },
                        )?;
                        if unit.is_active() {
                            result.active_patterns = checked_add(result.active_patterns, patterns)?;
                            result.active_source_bytes = checked_add(result.active_source_bytes, bytes)?;
                            if capture {
                                result.active_capture_phrase_programs =
                                    checked_inc(result.active_capture_phrase_programs)?;
                            } else {
                                result.matcher_members = checked_inc(result.matcher_members)?;
                                result.grouped_patterns = checked_add(result.grouped_patterns, patterns)?;
                                result.grouped_source_bytes = checked_add(result.grouped_source_bytes, bytes)?;
                            }
                        }
                    }
                    super::OperatorKind::Rx if unit.is_active() && !capture => {
                        let super::OperatorOperandView::Regex(source) = operator.operand() else {
                            return Err(CompilerError::PrivateInvariant {
                                kind: CompilerInvariant::UnexpectedActionOperand,
                            });
                        };
                        result.matcher_members = checked_inc(result.matcher_members)?;
                        result.grouped_patterns = checked_inc(result.grouped_patterns)?;
                        result.grouped_source_bytes = checked_add(result.grouped_source_bytes, source.text().len())?;
                        result.active_patterns = checked_inc(result.active_patterns)?;
                        result.active_source_bytes = checked_add(result.active_source_bytes, source.text().len())?;
                    }
                    _ => {}
                }
            }
        }
        if asset_cursor != state.asset_templates.assets.len() {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            });
        }
        if result.active_patterns > limits.max_matcher_patterns {
            return Err(CompilerError::MatcherPatternLimit {
                actual: result.active_patterns,
                limit: limits.max_matcher_patterns,
            });
        }
        if result.active_source_bytes > limits.max_matcher_source_bytes {
            return Err(CompilerError::MatcherSourceByteLimit {
                actual: result.active_source_bytes,
                limit: limits.max_matcher_source_bytes,
            });
        }
        Ok(result)
    }
}

#[allow(clippy::too_many_arguments)]
fn append_candidate_pattern(
    group: usize,
    kind: CandidatePatternKind,
    source: &str,
    mapping: CompiledPatternMapping,
    pattern_bytes: &mut Vec<u8>,
    patterns: &mut Vec<CandidatePattern>,
    heads: &mut [usize],
    tails: &mut [usize],
    counts: &mut [usize],
) -> Result<(), CompilerError> {
    let start = pattern_bytes.len();
    pattern_bytes.extend_from_slice(source.as_bytes());
    let index = patterns.len();
    patterns.push(CandidatePattern {
        group,
        kind,
        bytes: ByteRange {
            start,
            len: source.len(),
        },
        mapping,
        next: usize::MAX,
    });
    let head = heads.get_mut(group).ok_or(CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    })?;
    let tail = tails.get_mut(group).ok_or(CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    })?;
    let count = counts.get_mut(group).ok_or(CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    })?;
    if *tail == usize::MAX {
        *head = index;
    } else {
        patterns
            .get_mut(*tail)
            .ok_or(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            })?
            .next = index;
    }
    *tail = index;
    *count = checked_inc(*count)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn compile_phrase_operand_program(
    operand: super::OperatorOperandView<'_, '_>,
    assets: &[ResolvedAsset],
    phrases: &[CompiledPhrase],
    asset_bytes: &[u8],
    asset_cursor: &mut usize,
    location: SemanticLocation,
    retain: bool,
    work: &mut MatcherCompileWork,
) -> Result<CompiledCapturePhraseProgram, CompilerError> {
    let mut count_cursor = *asset_cursor;
    let mut pattern_count = 0_usize;
    let mut source_bytes = 0_usize;
    visit_phrase_operand(
        operand.clone(),
        assets,
        phrases,
        asset_bytes,
        &mut count_cursor,
        |phrase, _| {
            pattern_count = checked_inc(pattern_count)?;
            source_bytes = checked_add(source_bytes, phrase.len())?;
            Ok(())
        },
    )?;
    work.retain_working_bytes(source_bytes)?;
    work.retain_working_records::<ByteRange>(pattern_count)?;
    work.retain_working_records::<&str>(pattern_count)?;
    let mut owned = Vec::new();
    let mut ranges = Vec::new();
    owned
        .try_reserve_exact(source_bytes)
        .map_err(|_| CompilerError::Allocation {
            operation: "capture phrase source bytes",
        })?;
    reserve_exact(&mut ranges, pattern_count, "capture phrase ranges")?;
    visit_phrase_operand(operand, assets, phrases, asset_bytes, asset_cursor, |phrase, _| {
        let start = owned.len();
        owned.extend_from_slice(phrase.as_bytes());
        ranges.push(ByteRange {
            start,
            len: phrase.len(),
        });
        Ok(())
    })?;
    if *asset_cursor != count_cursor || owned.len() != source_bytes || ranges.len() != pattern_count {
        return Err(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        });
    }
    let mut refs = Vec::new();
    reserve_exact(&mut refs, pattern_count, "capture phrase references")?;
    for range in &ranges {
        refs.push(
            std::str::from_utf8(byte_range(&owned, *range)?).map_err(|_| CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            })?,
        );
    }
    let program = work.compile_capture_phrase(&refs, source_bytes, location, retain)?;
    work.drop_working_records::<&str>(pattern_count);
    work.drop_working_records::<ByteRange>(pattern_count);
    work.drop_working_bytes(source_bytes);
    Ok(program)
}

struct CompilerBuilder<'source, 'assets> {
    external_live_bytes: usize,
    validated: ValidatedSemanticDocument<'source>,
    assets: Option<&'assets mut dyn PhraseAssetSource>,
    limits: CompilerLimits,
}

impl<'source, 'assets> CompilerBuilder<'source, 'assets> {
    fn new(
        validated: ValidatedSemanticDocument<'source>,
        assets: Option<&'assets mut dyn PhraseAssetSource>,
        limits: CompilerLimits,
    ) -> Self {
        Self {
            external_live_bytes: 0,
            validated,
            assets,
            limits,
        }
    }

    fn prepare(self) -> Result<AssetTemplateState<'source>, CompilerError> {
        let compilation_input_bytes = checked_add(self.validated.retained_charged_bytes(), self.external_live_bytes)?;
        let unit_count = self.validated.rule_units().len();
        let node_count = self.validated.rule_nodes().len();
        if unit_count > self.limits.max_units {
            return Err(CompilerError::UnitLimit {
                actual: unit_count,
                limit: self.limits.max_units,
            });
        }
        if node_count > self.limits.max_nodes {
            return Err(CompilerError::NodeLimit {
                actual: node_count,
                limit: self.limits.max_nodes,
            });
        }
        let preflight = KnownPreflight::inspect(&self.validated, self.limits)?;
        preflight.validate(self.limits)?;
        for requirement in self.validated.requirements() {
            if matches!(
                requirement.kind(),
                CompileRequirement::BlockDefaultDisruptive | CompileRequirement::DefaultPhase
            ) {
                return Err(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::DefaultRequirement,
                });
            }
        }
        preflight.validate_peak(
            compilation_input_bytes,
            self.validated.requirements().len(),
            self.limits,
        )?;
        let mut requirements = Vec::new();
        requirements
            .try_reserve_exact(self.validated.requirements().len())
            .map_err(|_| CompilerError::Allocation {
                operation: "requirement discharge",
            })?;
        for requirement in self.validated.requirements() {
            requirements.push(RequirementState {
                kind: requirement.kind(),
                location: requirement.location(),
                discharged: false,
            });
        }
        requirements.sort_unstable_by_key(|requirement| requirement_key(requirement.kind, requirement.location));
        let mut work = CompileWork::new(compilation_input_bytes, self.limits, requirements, preflight)?;
        let mut assets = self.assets;
        for unit in self.validated.rule_units() {
            if unit.is_compatibility_excluded() {
                continue;
            }
            for member in unit.members() {
                if let Some(operator) = member.operator() {
                    if let super::OperatorOperandView::ExternalAssets(external) = operator.operand() {
                        for asset in external.assets() {
                            work.resolve_asset(operator.location(), asset, &mut assets)?;
                        }
                        work.discharge(CompileRequirement::ExternalPhraseAsset, operator.location())?;
                    }
                }
            }
        }
        for unit in self.validated.rule_units() {
            for member in unit.members() {
                work.compile_member(member, unit.is_active(), unit.is_compatibility_excluded())?;
            }
        }
        work.verify_a_requirements()?;
        Ok(AssetTemplateState {
            compilation_input_bytes,
            validated: self.validated,
            bytes: work.bytes,
            asset_phrase_bytes: work.asset_phrase_bytes,
            templates: work.templates,
            segments: work.segments,
            macros: work.macros,
            actions: work.actions,
            setvars: work.setvars,
            members: work.members,
            transformations: work.transformations,
            assets: work.assets,
            phrases: work.phrases,
            requirements: work.requirements,
            compiler_owned_bytes: work.owned_bytes,
            compiler_working_bytes: work.working_bytes,
            peak_bytes: work.peak_bytes,
        })
    }
}

impl<'source> MatcherState<'source> {
    fn compile_with_work(
        mut state: AssetTemplateState<'source>,
        limits: CompilerLimits,
        mut canonical_work: CanonicalWork,
    ) -> Result<Self, CompilerError> {
        let target_regex_count = state
            .requirements
            .iter()
            .filter(|requirement| requirement.kind == CompileRequirement::TargetSelectorRegexCompilation)
            .count();
        let mut regex_counts = RegexCandidateCounts {
            selectors: target_regex_count,
            ..RegexCandidateCounts::default()
        };
        for unit in state.validated.rule_units() {
            if unit.is_compatibility_excluded() {
                continue;
            }
            for member in unit.members() {
                let Some(operator) = member.operator() else {
                    continue;
                };
                if operator.kind() != super::OperatorKind::Rx {
                    continue;
                }
                let capture = member.rule().fold().capture_action().is_some();
                match (unit.is_active(), capture) {
                    (true, false) => regex_counts.active_boolean += 1,
                    (true, true) => regex_counts.active_capture += 1,
                    (false, false) => regex_counts.removed_boolean += 1,
                    (false, true) => regex_counts.removed_capture += 1,
                }
            }
        }
        let active_regex_count = regex_counts.active().ok_or(CompilerError::PreflightArithmetic)?;
        if active_regex_count > limits.max_regex_programs {
            return Err(CompilerError::RegexProgramLimit {
                actual: active_regex_count,
                limit: limits.max_regex_programs,
            });
        }
        let ip_count = state
            .validated
            .rule_units()
            .filter(|unit| unit.is_active())
            .flat_map(|unit| unit.members())
            .filter_map(|member| member.operator())
            .map(|operator| match operator.operand() {
                super::OperatorOperandView::IpNetworks(networks) => networks.networks().len(),
                _ => 0,
            })
            .try_fold(0_usize, |total, count| total.checked_add(count))
            .ok_or(CompilerError::PreflightArithmetic)?;
        let target_identity_slots = state
            .validated
            .active_rule_units()
            .flat_map(|unit| unit.members())
            .flat_map(|member| member.final_targets())
            .filter(|target| target.regex_body().is_some())
            .map(|target| target.location().node_index().saturating_add(1))
            .max()
            .unwrap_or(0);

        let mut work = MatcherCompileWork {
            limits,
            validated_bytes: state.compilation_input_bytes,
            working_bytes: state.compiler_working_bytes,
            compiler_owned_bytes: state.compiler_owned_bytes,
            matcher_charged_bytes: 0,
            peak_bytes: state.peak_bytes,
            requirement_cursors: [0; 8],
        };
        work.retain_compiler_records::<Option<CompiledOperatorPlan>>(state.members.len())?;
        work.retain_compiler_records::<CompiledIpNetwork>(ip_count)?;
        work.retain_compiler_records::<CompiledTargetRegexPlan>(target_regex_count)?;
        work.retain_compiler_records::<CompiledRegexProgram>(target_regex_count)?;
        work.retain_compiler_records::<CompiledCaptureRegexProgram>(regex_counts.active_capture)?;
        work.retain_working_records::<usize>(target_identity_slots)?;
        let candidate_count = regex_counts.total().ok_or(CompilerError::PreflightArithmetic)?;
        work.retain_working_records::<RegexCandidate<'_>>(candidate_count)?;

        let mut operator_plans = Vec::new();
        let mut ip_networks = Vec::new();
        let mut target_regexes = Vec::new();
        let mut boolean_regexes = Vec::new();
        let mut capture_regexes = Vec::new();
        reserve_exact(&mut operator_plans, state.members.len(), "operator plans")?;
        reserve_exact(&mut ip_networks, ip_count, "compiled IP networks")?;
        reserve_exact(&mut target_regexes, target_regex_count, "target regex plans")?;
        reserve_exact(&mut boolean_regexes, target_regex_count, "boolean regex programs")?;
        reserve_exact(
            &mut capture_regexes,
            regex_counts.active_capture,
            "capture regex programs",
        )?;
        let mut regex_candidates = Vec::new();
        reserve_exact(&mut regex_candidates, candidate_count, "regex candidates")?;
        let mut target_ids = Vec::new();
        reserve_exact(&mut target_ids, target_identity_slots, "target regex identity map")?;
        target_ids.resize(target_identity_slots, usize::MAX);

        for unit in state.validated.active_rule_units() {
            for member in unit.members() {
                for target in member.final_targets() {
                    if let Some(body) = target.regex_body() {
                        let identity = target.location().node_index();
                        if target_ids[identity] != usize::MAX {
                            continue;
                        }
                        target_ids[identity] = 0;
                        regex_candidates.push(RegexCandidate {
                            source: body.text(),
                            location: target.location(),
                            capture: false,
                            active: true,
                        });
                    }
                }
            }
        }
        for unit in state.validated.rule_units() {
            if unit.is_compatibility_excluded() {
                continue;
            }
            for member in unit.members() {
                if let Some(operator) = member.operator() {
                    if let super::OperatorOperandView::Regex(source) = operator.operand() {
                        regex_candidates.push(RegexCandidate {
                            source: source.text(),
                            location: operator.location(),
                            capture: member.rule().fold().capture_action().is_some(),
                            active: unit.is_active(),
                        });
                    }
                }
            }
        }
        debug_assert_eq!(regex_candidates.len(), candidate_count);
        target_ids.fill(usize::MAX);

        let mut selector_interner = RegexProgramInterner::new(target_regex_count, &mut work)?;
        for unit in state.validated.active_rule_units() {
            for member in unit.members() {
                for target in member.final_targets() {
                    let Some(body) = target.regex_body() else {
                        continue;
                    };
                    let identity = target.location().node_index();
                    if target_ids[identity] != usize::MAX {
                        continue;
                    }
                    let ascii_case_insensitive = target.collection() == CollectionKind::Tx;
                    let program_id = selector_interner.compile_with(
                        body.text(),
                        ascii_case_insensitive,
                        &mut boolean_regexes,
                        &mut work,
                        &mut canonical_work,
                        |work| {
                            if ascii_case_insensitive {
                                work.compile_ascii_case_insensitive_boolean_regex(body.text(), target.location(), true)
                            } else {
                                work.compile_boolean_regex(body.text(), target.location(), true)
                            }
                        },
                    )?;
                    let plan_id = target_regexes.len();
                    target_regexes.push(CompiledTargetRegexPlan {
                        identity: target.location(),
                        program: program_id,
                    });
                    target_ids[identity] = plan_id;
                    discharge_requirement(
                        &mut state.requirements,
                        &mut work.requirement_cursors,
                        CompileRequirement::TargetSelectorRegexCompilation,
                        target.location(),
                    )?;
                }
            }
        }

        selector_interner.release(&mut work);
        let mut capture_interner = RegexProgramInterner::new(regex_counts.active_capture, &mut work)?;
        let mut member_index = 0_usize;
        for unit in state.validated.rule_units() {
            for member in unit.members() {
                let plan = if unit.is_compatibility_excluded() {
                    None
                } else if let Some(operator) = member.operator() {
                    compile_operator_plan(
                        operator,
                        member,
                        state.members[member_index].operator_template,
                        &mut ip_networks,
                        &mut boolean_regexes,
                        &mut capture_regexes,
                        &mut capture_interner,
                        &mut canonical_work,
                        &mut state.requirements,
                        &mut work,
                        unit.is_active(),
                    )?
                } else {
                    None
                };
                operator_plans.push(plan);
                member_index += 1;
            }
        }
        debug_assert_eq!(member_index, state.members.len());
        verify_b_requirements(&state.requirements)?;
        debug_assert!(regex_candidates.iter().all(|candidate| {
            !candidate.source.is_empty()
                || candidate.location.node_kind() == super::SemanticNodeKind::Operator
                || !candidate.capture
                || candidate.active
        }));
        capture_interner.release(&mut work);
        work.drop_working_records::<RegexCandidate<'_>>(candidate_count);
        work.drop_working_records::<usize>(target_identity_slots);
        Ok(Self {
            asset_templates: state,
            operator_plans,
            ip_networks,
            target_regexes,
            boolean_regexes,
            capture_regexes,
            canonical_work,
            compiler_owned_bytes: work.compiler_owned_bytes,
            matcher_charged_bytes: work.matcher_charged_bytes,
            peak_bytes: work.peak_bytes,
        })
    }
}

impl<'source> GroupedMatcherState<'source> {
    fn compile(mut state: MatcherState<'source>, limits: CompilerLimits) -> Result<Self, CompilerError> {
        let mut active_members = 0_usize;
        let mut target_records_count = 0_usize;
        let mut target_selector_bytes = 0_usize;
        let mut target_identity_slots = 0_usize;
        for unit in state.asset_templates.validated.active_rule_units() {
            for member in unit.members() {
                active_members = checked_inc(active_members)?;
                target_records_count = checked_add(target_records_count, member.final_targets().len())?;
                for target in member.final_targets() {
                    if matches!(target.selector_kind(), SelectorKind::Literal | SelectorKind::Path) {
                        let selector = target.selector().ok_or(CompilerError::PrivateInvariant {
                            kind: CompilerInvariant::InvalidByteRange,
                        })?;
                        target_selector_bytes = checked_add(target_selector_bytes, selector.text().len())?;
                    }
                    if target.regex_body().is_some() {
                        target_identity_slots = target_identity_slots.max(
                            target
                                .location()
                                .node_index()
                                .checked_add(1)
                                .ok_or(CompilerError::PreflightArithmetic)?,
                        );
                    }
                }
            }
        }
        let target_table_slots = canonical_table_slots(active_members)?;
        let transformation_table_slots = canonical_table_slots(active_members)?;
        let pattern_preflight = GroupPatternPreflight::inspect(&state, limits)?;
        let group_table_slots = canonical_table_slots(pattern_preflight.matcher_members)?;
        let mut compile_work = MatcherCompileWork {
            limits,
            validated_bytes: state.asset_templates.compilation_input_bytes,
            working_bytes: state.asset_templates.compiler_working_bytes,
            compiler_owned_bytes: state.compiler_owned_bytes,
            matcher_charged_bytes: state.matcher_charged_bytes,
            peak_bytes: state.peak_bytes,
            requirement_cursors: [0; 8],
        };
        compile_work.retain_working_records::<usize>(active_members)?;
        compile_work.retain_working_records::<usize>(target_identity_slots)?;
        compile_work.retain_working_records::<CandidateTargetPlan>(active_members)?;
        compile_work.retain_working_records::<CandidateTargetRecord>(target_records_count)?;
        compile_work.retain_working_bytes(target_selector_bytes)?;
        compile_work.retain_working_records::<CanonicalSlot>(target_table_slots)?;
        compile_work.retain_working_records::<CanonicalSlot>(transformation_table_slots)?;
        compile_work.retain_working_records::<CanonicalSlot>(group_table_slots)?;
        compile_work.retain_working_records::<CompiledMatcherGroupKey>(pattern_preflight.matcher_members)?;
        compile_work.retain_working_records::<CandidatePattern>(pattern_preflight.grouped_patterns)?;
        compile_work.retain_working_records::<MemberMatcherBinding>(pattern_preflight.matcher_members)?;
        compile_work.retain_working_records::<usize>(
            pattern_preflight
                .matcher_members
                .checked_mul(3)
                .ok_or(CompilerError::PreflightArithmetic)?,
        )?;
        compile_work.retain_working_records::<SemanticLocation>(pattern_preflight.matcher_members)?;
        compile_work.retain_working_records::<bool>(pattern_preflight.matcher_members)?;
        compile_work.retain_working_bytes(pattern_preflight.grouped_source_bytes)?;

        let mut active_member_indices = Vec::new();
        let mut target_regex_by_identity = Vec::new();
        let mut candidate_plans = Vec::new();
        let mut candidate_records = Vec::new();
        let mut candidate_bytes = Vec::new();
        let mut target_slots = Vec::new();
        let mut transformation_slots = Vec::new();
        let mut group_slots = Vec::new();
        let mut group_keys = Vec::new();
        let mut candidate_patterns = Vec::new();
        let mut member_bindings = Vec::new();
        let mut pattern_bytes = Vec::new();
        let mut group_heads = Vec::new();
        let mut group_tails = Vec::new();
        let mut group_counts = Vec::new();
        let mut group_locations = Vec::new();
        let mut group_has_regex = Vec::new();
        reserve_exact(&mut active_member_indices, active_members, "active member identities")?;
        reserve_exact(
            &mut target_regex_by_identity,
            target_identity_slots,
            "target regex identity map",
        )?;
        target_regex_by_identity.resize(target_identity_slots, usize::MAX);
        reserve_exact(&mut candidate_plans, active_members, "candidate target plans")?;
        reserve_exact(&mut candidate_records, target_records_count, "candidate target records")?;
        candidate_bytes
            .try_reserve_exact(target_selector_bytes)
            .map_err(|_| CompilerError::Allocation {
                operation: "candidate target bytes",
            })?;
        reserve_exact(&mut target_slots, target_table_slots, "target canonical table")?;
        target_slots.resize(target_table_slots, CanonicalSlot::EMPTY);
        reserve_exact(
            &mut transformation_slots,
            transformation_table_slots,
            "transformation canonical table",
        )?;
        transformation_slots.resize(transformation_table_slots, CanonicalSlot::EMPTY);
        reserve_exact(&mut group_slots, group_table_slots, "matcher group canonical table")?;
        group_slots.resize(group_table_slots, CanonicalSlot::EMPTY);
        reserve_exact(&mut group_keys, pattern_preflight.matcher_members, "matcher group keys")?;
        reserve_exact(
            &mut candidate_patterns,
            pattern_preflight.grouped_patterns,
            "candidate matcher patterns",
        )?;
        reserve_exact(
            &mut member_bindings,
            pattern_preflight.matcher_members,
            "matcher member bindings",
        )?;
        reserve_exact(
            &mut group_heads,
            pattern_preflight.matcher_members,
            "matcher group heads",
        )?;
        reserve_exact(
            &mut group_tails,
            pattern_preflight.matcher_members,
            "matcher group tails",
        )?;
        reserve_exact(
            &mut group_counts,
            pattern_preflight.matcher_members,
            "matcher group counts",
        )?;
        reserve_exact(
            &mut group_locations,
            pattern_preflight.matcher_members,
            "matcher group locations",
        )?;
        reserve_exact(
            &mut group_has_regex,
            pattern_preflight.matcher_members,
            "matcher group regex flags",
        )?;
        pattern_bytes
            .try_reserve_exact(pattern_preflight.grouped_source_bytes)
            .map_err(|_| CompilerError::Allocation {
                operation: "candidate matcher source bytes",
            })?;
        for target in &state.target_regexes {
            let identity = target.identity.node_index();
            let slot = target_regex_by_identity
                .get_mut(identity)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::MissingTargetRegex,
                })?;
            *slot = target.program;
        }

        let mut member_index = 0_usize;
        for unit in state.asset_templates.validated.rule_units() {
            for member in unit.members() {
                if unit.is_active() {
                    active_member_indices.push(member_index);
                    let record_start = candidate_records.len();
                    for target in member.final_targets() {
                        let selector = match target.selector_kind() {
                            SelectorKind::None => CandidateTargetSelector::None,
                            SelectorKind::Literal | SelectorKind::Path => {
                                let selector = target.selector().ok_or(CompilerError::PrivateInvariant {
                                    kind: CompilerInvariant::InvalidByteRange,
                                })?;
                                let start = candidate_bytes.len();
                                candidate_bytes.extend_from_slice(selector.text().as_bytes());
                                let range = ByteRange {
                                    start,
                                    len: selector.text().len(),
                                };
                                if target.selector_kind() == SelectorKind::Literal {
                                    CandidateTargetSelector::Literal(range)
                                } else {
                                    CandidateTargetSelector::Path(range)
                                }
                            }
                            SelectorKind::Regex(style) => {
                                let identity = target.location().node_index();
                                let program =
                                    *target_regex_by_identity
                                        .get(identity)
                                        .ok_or(CompilerError::PrivateInvariant {
                                            kind: CompilerInvariant::MissingTargetRegex,
                                        })?;
                                if program == usize::MAX {
                                    return Err(CompilerError::PrivateInvariant {
                                        kind: CompilerInvariant::MissingTargetRegex,
                                    });
                                }
                                CandidateTargetSelector::Regex { style, program }
                            }
                        };
                        candidate_records.push(CandidateTargetRecord {
                            collection: target.collection(),
                            modifier: target.modifier(),
                            selector,
                        });
                    }
                    candidate_plans.push(CandidateTargetPlan {
                        record_start,
                        record_count: candidate_records.len() - record_start,
                    });
                }
                member_index = checked_inc(member_index)?;
            }
        }
        if candidate_records.len() != target_records_count
            || candidate_bytes.len() != target_selector_bytes
            || candidate_plans.len() != active_members
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            });
        }

        let mut target_bytes = Vec::new();
        let mut target_records = Vec::new();
        let mut target_plans = Vec::new();
        let mut canonical_transformations = Vec::new();
        let mut transformation_plans = Vec::new();
        compile_work.reserve_canonical(&mut target_bytes, target_selector_bytes, "canonical target bytes")?;
        compile_work.reserve_canonical(&mut target_records, target_records_count, "canonical target records")?;
        compile_work.reserve_canonical(&mut target_plans, active_members, "canonical target plans")?;
        compile_work.reserve_canonical(
            &mut canonical_transformations,
            state.asset_templates.transformations.len(),
            "canonical transformations",
        )?;
        compile_work.reserve_canonical(
            &mut transformation_plans,
            active_members,
            "canonical transformation plans",
        )?;
        let mut canonical_work = std::mem::take(&mut state.canonical_work);
        for (ordinal, member_index) in active_member_indices.iter().copied().enumerate() {
            let target_candidate = candidate_plans[ordinal];
            let target_hash = hash_target_plan(
                target_candidate,
                &candidate_records,
                &candidate_bytes,
                &mut canonical_work,
                limits,
            )?;
            let target_plan = insert_target_plan(
                target_candidate,
                target_hash,
                &mut target_slots,
                &candidate_records,
                &candidate_bytes,
                &mut target_records,
                &mut target_bytes,
                &mut target_plans,
                &mut canonical_work,
                &mut compile_work,
            )?;
            let member = state.asset_templates.members[member_index];
            let transformation_end = member
                .transformation_start
                .checked_add(member.transformation_count)
                .ok_or(CompilerError::PreflightArithmetic)?;
            let transformations = state
                .asset_templates
                .transformations
                .get(member.transformation_start..transformation_end)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                })?;
            let transformation_hash = hash_transformations(transformations, &mut canonical_work, limits)?;
            let transformation_plan = insert_transformation_plan(
                transformations,
                transformation_hash,
                &mut transformation_slots,
                &mut canonical_transformations,
                &mut transformation_plans,
                &mut canonical_work,
                &mut compile_work,
            )?;
            state.asset_templates.members[member_index].target_plan = Some(target_plan);
            state.asset_templates.members[member_index].transformation_plan = Some(transformation_plan);
        }
        let old_transformation_bytes = state
            .asset_templates
            .transformations
            .len()
            .checked_mul(size_of::<TransformationKind>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        compile_work.drop_compiler_bytes(old_transformation_bytes)?;
        state.asset_templates.transformations.clear();

        let mut capture_phrases = Vec::new();
        reserve_exact(
            &mut capture_phrases,
            pattern_preflight.active_capture_phrase_programs,
            "capture phrase programs",
        )?;
        let mut asset_cursor = 0_usize;
        let mut source_member_index = 0_usize;
        for (unit_index, unit) in state.asset_templates.validated.rule_units().enumerate() {
            if unit.is_compatibility_excluded() {
                source_member_index = checked_add(source_member_index, unit.members().len())?;
                continue;
            }
            for member in unit.members() {
                let Some(operator) = member.operator() else {
                    source_member_index = checked_inc(source_member_index)?;
                    continue;
                };
                let capture = member.rule().fold().capture_action().is_some();
                let is_phrase = matches!(
                    operator.kind(),
                    super::OperatorKind::Pm | super::OperatorKind::PmFromFile
                );
                if is_phrase {
                    if unit.is_active() && !capture {
                        let key = member_group_key(
                            state.asset_templates.members[source_member_index],
                            member.rule().fold().multi_match_action().is_some(),
                        )?;
                        let hash = hash_group_key(key, &mut canonical_work, limits)?;
                        let group = insert_group_key(
                            key,
                            hash,
                            &mut group_slots,
                            &mut group_keys,
                            &mut canonical_work,
                            limits,
                        )?;
                        while group_heads.len() < group_keys.len() {
                            group_heads.push(usize::MAX);
                            group_tails.push(usize::MAX);
                            group_counts.push(0);
                            group_locations.push(operator.location());
                            group_has_regex.push(false);
                        }
                        let pattern_start = group_counts[group];
                        let pattern_before = candidate_patterns.len();
                        let count = visit_phrase_operand(
                            operator.operand(),
                            &state.asset_templates.assets,
                            &state.asset_templates.phrases,
                            &state.asset_templates.asset_phrase_bytes,
                            &mut asset_cursor,
                            |phrase, local_ordinal| {
                                append_candidate_pattern(
                                    group,
                                    CandidatePatternKind::Phrase,
                                    phrase,
                                    CompiledPatternMapping {
                                        unit: unit_index,
                                        member: source_member_index,
                                        local_ordinal,
                                    },
                                    &mut pattern_bytes,
                                    &mut candidate_patterns,
                                    &mut group_heads,
                                    &mut group_tails,
                                    &mut group_counts,
                                )
                            },
                        )?;
                        if candidate_patterns.len() - pattern_before != count {
                            return Err(CompilerError::PrivateInvariant {
                                kind: CompilerInvariant::InvalidByteRange,
                            });
                        }
                        member_bindings.push(MemberMatcherBinding {
                            member: source_member_index,
                            group,
                            pattern_start,
                            pattern_count: count,
                            kind: operator.kind(),
                            negated: operator.negated(),
                        });
                    } else {
                        let program = compile_phrase_operand_program(
                            operator.operand(),
                            &state.asset_templates.assets,
                            &state.asset_templates.phrases,
                            &state.asset_templates.asset_phrase_bytes,
                            &mut asset_cursor,
                            operator.location(),
                            unit.is_active(),
                            &mut compile_work,
                        )?;
                        if unit.is_active() {
                            compile_work.retain_compiler_records::<CompiledCapturePhraseProgram>(1)?;
                            let id = capture_phrases.len();
                            capture_phrases.push(program);
                            state.operator_plans[source_member_index] = Some(CompiledOperatorPlan {
                                kind: operator.kind(),
                                negated: operator.negated(),
                                operand: CompiledOperatorOperand::CapturePhrase(id),
                            });
                        }
                    }
                    discharge_requirement(
                        &mut state.asset_templates.requirements,
                        &mut compile_work.requirement_cursors,
                        CompileRequirement::PhraseMatcherCompilation,
                        operator.location(),
                    )?;
                } else if operator.kind() == super::OperatorKind::Rx && unit.is_active() && !capture {
                    let super::OperatorOperandView::Regex(source) = operator.operand() else {
                        return Err(CompilerError::PrivateInvariant {
                            kind: CompilerInvariant::UnexpectedActionOperand,
                        });
                    };
                    let key = member_group_key(
                        state.asset_templates.members[source_member_index],
                        member.rule().fold().multi_match_action().is_some(),
                    )?;
                    let hash = hash_group_key(key, &mut canonical_work, limits)?;
                    let group = insert_group_key(
                        key,
                        hash,
                        &mut group_slots,
                        &mut group_keys,
                        &mut canonical_work,
                        limits,
                    )?;
                    while group_heads.len() < group_keys.len() {
                        group_heads.push(usize::MAX);
                        group_tails.push(usize::MAX);
                        group_counts.push(0);
                        group_locations.push(operator.location());
                        group_has_regex.push(false);
                    }
                    group_has_regex[group] = true;
                    let pattern_start = group_counts[group];
                    append_candidate_pattern(
                        group,
                        CandidatePatternKind::Regex,
                        source.text(),
                        CompiledPatternMapping {
                            unit: unit_index,
                            member: source_member_index,
                            local_ordinal: 0,
                        },
                        &mut pattern_bytes,
                        &mut candidate_patterns,
                        &mut group_heads,
                        &mut group_tails,
                        &mut group_counts,
                    )?;
                    member_bindings.push(MemberMatcherBinding {
                        member: source_member_index,
                        group,
                        pattern_start,
                        pattern_count: 1,
                        kind: operator.kind(),
                        negated: operator.negated(),
                    });
                    discharge_requirement(
                        &mut state.asset_templates.requirements,
                        &mut compile_work.requirement_cursors,
                        CompileRequirement::RegexCompilation,
                        operator.location(),
                    )?;
                }
                source_member_index = checked_inc(source_member_index)?;
            }
        }
        if asset_cursor != state.asset_templates.assets.len()
            || candidate_patterns.len() != pattern_preflight.grouped_patterns
            || pattern_bytes.len() != pattern_preflight.grouped_source_bytes
            || member_bindings.len() != pattern_preflight.matcher_members
            || capture_phrases.len() != pattern_preflight.active_capture_phrase_programs
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            });
        }

        // Selector identity records can share a program. Programs are inserted as
        // a contiguous prefix before any operator-only boolean programs.
        let target_boolean_programs = state
            .target_regexes
            .iter()
            .map(|selector| selector.program)
            .max()
            .map(checked_inc)
            .transpose()?
            .unwrap_or(0);
        let final_regex_programs = target_boolean_programs
            .checked_add(state.capture_regexes.len())
            .and_then(|count| count.checked_add(group_has_regex.iter().filter(|present| **present).count()))
            .ok_or(CompilerError::PreflightArithmetic)?;
        if final_regex_programs > limits.max_regex_programs {
            return Err(CompilerError::RegexProgramLimit {
                actual: final_regex_programs,
                limit: limits.max_regex_programs,
            });
        }

        compile_work.retain_compiler_records::<CompiledMatcherGroup>(group_keys.len())?;
        let mapping_bytes = candidate_patterns
            .len()
            .checked_mul(size_of::<CompiledPatternMapping>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        compile_work.retain_matcher(mapping_bytes)?;
        let mut groups = Vec::new();
        let mut pattern_mappings = Vec::new();
        reserve_exact(&mut groups, group_keys.len(), "compiled matcher groups")?;
        reserve_exact(
            &mut pattern_mappings,
            candidate_patterns.len(),
            "matcher pattern mappings",
        )?;
        for group in 0..group_keys.len() {
            let count = group_counts[group];
            compile_work.retain_working_records::<PatternSpec<'_>>(count)?;
            let mut patterns = Vec::new();
            reserve_exact(&mut patterns, count, "group matcher pattern references")?;
            let mapping_start = pattern_mappings.len();
            let mut counts = MatcherPatternCounts::default();
            let mut bytes = MatcherPatternBytes::default();
            let mut next = group_heads[group];
            while next != usize::MAX {
                let candidate = *candidate_patterns.get(next).ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                })?;
                if candidate.group != group {
                    return Err(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    });
                }
                let source = std::str::from_utf8(byte_range(&pattern_bytes, candidate.bytes)?).map_err(|_| {
                    CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    }
                })?;
                match candidate.kind {
                    CandidatePatternKind::Phrase => {
                        counts.insensitive_phrases = checked_inc(counts.insensitive_phrases)?;
                        bytes.insensitive_phrases = checked_add(bytes.insensitive_phrases, source.len())?;
                        patterns.push(PatternSpec::Phrase {
                            value: source,
                            ascii_case_insensitive: true,
                        });
                    }
                    CandidatePatternKind::Regex => {
                        counts.regexes = checked_inc(counts.regexes)?;
                        bytes.regexes = checked_add(bytes.regexes, source.len())?;
                        patterns.push(PatternSpec::Regex(source));
                    }
                }
                pattern_mappings.push(candidate.mapping);
                next = candidate.next;
            }
            if patterns.len() != count {
                return Err(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                });
            }
            let matcher = compile_work.compile_group_matcher(&patterns, counts, bytes, group_locations[group])?;
            compile_work.drop_working_records::<PatternSpec<'_>>(count);
            groups.push(CompiledMatcherGroup {
                key: group_keys[group],
                matcher,
                mapping_start,
                mapping_count: count,
            });
        }
        for binding in &member_bindings {
            state.operator_plans[binding.member] = Some(CompiledOperatorPlan {
                kind: binding.kind,
                negated: binding.negated,
                operand: CompiledOperatorOperand::Matcher {
                    group: binding.group,
                    pattern_start: binding.pattern_start,
                    pattern_count: binding.pattern_count,
                },
            });
        }
        let dropped_boolean_bytes = state
            .boolean_regexes
            .get(target_boolean_programs..)
            .ok_or(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            })?
            .iter()
            .try_fold(0_usize, |total, program| total.checked_add(program.memory_usage()))
            .ok_or(CompilerError::PreflightArithmetic)?;
        state.boolean_regexes.truncate(target_boolean_programs);
        compile_work.drop_matcher(dropped_boolean_bytes)?;
        if state
            .asset_templates
            .requirements
            .iter()
            .any(|requirement| !requirement.discharged)
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::MissingRequirement,
            });
        }

        compile_work.drop_working_records::<bool>(pattern_preflight.matcher_members);
        compile_work.drop_working_records::<SemanticLocation>(pattern_preflight.matcher_members);
        compile_work.drop_working_records::<usize>(
            pattern_preflight
                .matcher_members
                .checked_mul(3)
                .ok_or(CompilerError::PreflightArithmetic)?,
        );
        compile_work.drop_working_bytes(pattern_preflight.grouped_source_bytes);
        compile_work.drop_working_records::<MemberMatcherBinding>(pattern_preflight.matcher_members);
        compile_work.drop_working_records::<CandidatePattern>(pattern_preflight.grouped_patterns);
        compile_work.drop_working_records::<CompiledMatcherGroupKey>(pattern_preflight.matcher_members);
        compile_work.drop_working_records::<CanonicalSlot>(group_table_slots);
        compile_work.drop_working_records::<CanonicalSlot>(transformation_table_slots);
        compile_work.drop_working_records::<CanonicalSlot>(target_table_slots);
        compile_work.drop_working_bytes(target_selector_bytes);
        compile_work.drop_working_records::<CandidateTargetRecord>(target_records_count);
        compile_work.drop_working_records::<CandidateTargetPlan>(active_members);
        compile_work.drop_working_records::<usize>(target_identity_slots);
        compile_work.drop_working_records::<usize>(active_members);

        Ok(Self {
            pattern_preflight,
            matchers: state,
            canonical_work,
            target_bytes,
            target_records,
            target_plans,
            canonical_transformations,
            transformation_plans,
            groups,
            pattern_mappings,
            capture_phrases,
            compiler_owned_bytes: compile_work.compiler_owned_bytes,
            matcher_charged_bytes: compile_work.matcher_charged_bytes,
            peak_bytes: compile_work.peak_bytes,
        })
    }
}

impl CompiledGeneration {
    fn publish_with_work(
        mut state: GroupedMatcherState<'_>,
        limits: CompilerLimits,
    ) -> Result<(Self, CanonicalWork), CompilerError> {
        let active_units = state.matchers.asset_templates.validated.active_rule_units().len();
        let marker_count = state.matchers.asset_templates.validated.markers().len();
        let mut phase_units = [0_usize; 5];
        for unit in state.matchers.asset_templates.validated.active_rule_units() {
            let phase = unit.root().resolved_state().phase();
            phase_units[phase_index(phase)] = checked_inc(phase_units[phase_index(phase)])?;
        }
        let marker_entries = marker_count.checked_mul(5).ok_or(CompilerError::PreflightArithmetic)?;
        let total_entries = active_units
            .checked_add(marker_entries)
            .ok_or(CompilerError::PreflightArithmetic)?;
        if total_entries > limits.max_schedule_entries {
            return Err(CompilerError::ScheduleEntryLimit {
                actual: total_entries,
                limit: limits.max_schedule_entries,
            });
        }
        let mut phase_entries = [0_usize; 5];
        for index in 0..5 {
            phase_entries[index] = phase_units[index]
                .checked_add(marker_count)
                .ok_or(CompilerError::PreflightArithmetic)?;
            if phase_entries[index] > limits.max_schedule_entries_per_phase {
                return Err(CompilerError::PhaseScheduleEntryLimit {
                    phase: index + 1,
                    actual: phase_entries[index],
                    limit: limits.max_schedule_entries_per_phase,
                });
            }
        }

        let mut work = MatcherCompileWork {
            limits,
            validated_bytes: state.matchers.asset_templates.compilation_input_bytes,
            working_bytes: state.matchers.asset_templates.compiler_working_bytes,
            compiler_owned_bytes: state.compiler_owned_bytes,
            matcher_charged_bytes: state.matcher_charged_bytes,
            peak_bytes: state.peak_bytes,
            requirement_cursors: [0; 8],
        };
        work.retain_working_records::<DirectiveLocation>(
            active_units
                .checked_add(marker_count)
                .ok_or(CompilerError::PreflightArithmetic)?,
        )?;
        work.retain_compiler_records::<CompiledRuleUnit>(active_units)?;
        work.retain_compiler_records::<CompiledMarker>(marker_count)?;
        work.retain_compiler_records::<CompiledScheduleEntry>(total_entries)?;

        let mut units = Vec::new();
        let mut unit_locations = Vec::new();
        let mut markers = Vec::new();
        let mut marker_locations = Vec::new();
        let mut schedules: [Vec<CompiledScheduleEntry>; 5] = std::array::from_fn(|_| Vec::new());
        reserve_exact(&mut units, active_units, "compiled rule units")?;
        reserve_exact(&mut unit_locations, active_units, "compiled rule locations")?;
        reserve_exact(&mut markers, marker_count, "compiled markers")?;
        reserve_exact(&mut marker_locations, marker_count, "compiled marker locations")?;
        for index in 0..5 {
            reserve_exact(&mut schedules[index], phase_entries[index], "compiled phase schedule")?;
        }

        let optional_checks = expensive_checks::classify(
            &state.matchers.asset_templates.validated,
            &state.matchers.capture_regexes,
            &mut state.canonical_work,
            limits,
        )?;
        let mut member_start = 0_usize;
        for unit in state.matchers.asset_templates.validated.rule_units() {
            let member_count = unit.members().len();
            if unit.is_active() {
                let root = unit.root();
                let resolved = root.resolved_state();
                unit_locations.push(root.rule().location());
                units.push(CompiledRuleUnit {
                    expensive_check: optional_checks.skips_unit(unit.id()),
                    id: unit.id(),
                    phase: resolved.phase(),
                    disruptive: resolved.disruptive(),
                    status: resolved.status(),
                    log: resolved.log(),
                    audit_log: resolved.audit_log(),
                    member_start,
                    member_count,
                    schedule_index: usize::MAX,
                });
            }
            member_start = checked_add(member_start, member_count)?;
        }
        for marker in state.matchers.asset_templates.validated.markers() {
            marker_locations.push(marker.location());
            markers.push(CompiledMarker {
                schedule_indices: [usize::MAX; 5],
            });
        }

        let mut unit_index = 0_usize;
        let mut marker_index = 0_usize;
        while unit_index < units.len() || marker_index < markers.len() {
            let take_unit = match (unit_locations.get(unit_index), marker_locations.get(marker_index)) {
                (Some(unit), Some(marker)) => unit < marker,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            if take_unit {
                let phase = phase_index(units[unit_index].phase);
                units[unit_index].schedule_index = schedules[phase].len();
                schedules[phase].push(CompiledScheduleEntry::RuleUnit(unit_index));
                unit_index = checked_inc(unit_index)?;
            } else {
                if unit_locations.get(unit_index) == marker_locations.get(marker_index) {
                    return Err(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::ScheduleOrder,
                    });
                }
                for (phase, schedule) in schedules.iter_mut().enumerate() {
                    markers[marker_index].schedule_indices[phase] = schedule.len();
                    schedule.push(CompiledScheduleEntry::MarkerSentinel(marker_index));
                }
                marker_index = checked_inc(marker_index)?;
            }
        }
        if schedules
            .iter()
            .zip(phase_entries)
            .any(|(schedule, expected)| schedule.len() != expected)
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::ScheduleOrder,
            });
        }
        for unit in &units {
            let member_end = unit
                .member_start
                .checked_add(unit.member_count)
                .ok_or(CompilerError::PreflightArithmetic)?;
            for member in state
                .matchers
                .asset_templates
                .members
                .get_mut(unit.member_start..member_end)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::ScheduleOrder,
                })?
            {
                member.expensive_aggregate_actions = optional_checks.multipart && unit.id == 922150;
                let Some(marker) = member.skip_after_marker else {
                    continue;
                };
                let phase = phase_index(member.phase);
                let sentinel = markers
                    .get(marker)
                    .ok_or(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::ScheduleOrder,
                    })?
                    .schedule_indices[phase];
                if sentinel <= unit.schedule_index {
                    return Err(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::ScheduleOrder,
                    });
                }
                member.skip_after_marker = Some(sentinel);
            }
        }

        work.drop_working_records::<DirectiveLocation>(
            active_units
                .checked_add(marker_count)
                .ok_or(CompilerError::PreflightArithmetic)?,
        );
        let GroupedMatcherState {
            pattern_preflight,
            matchers,
            target_bytes,
            target_records,
            target_plans,
            canonical_transformations,
            transformation_plans,
            groups,
            pattern_mappings,
            capture_phrases,
            ..
        } = state;
        let MatcherState {
            asset_templates,
            operator_plans,
            ip_networks,
            target_regexes,
            mut boolean_regexes,
            capture_regexes,
            ..
        } = matchers;
        let AssetTemplateState {
            compilation_input_bytes: _,
            validated,
            bytes,
            templates,
            segments,
            macros,
            mut actions,
            setvars,
            members,
            ..
        } = asset_templates;
        let mut target_bytes = target_bytes;
        for action in &mut actions {
            let CompiledActionOperand::Ctl(CompiledCtl::RuleRemoveTargetByTag {
                collection, selector, ..
            }) = &mut action.operand
            else {
                continue;
            };
            let resolved = match *selector {
                CompiledCtlSelector::None => CompiledTargetSelector::None,
                CompiledCtlSelector::Literal(source) | CompiledCtlSelector::Path(source) => {
                    let value = byte_range(&bytes, source)?;
                    if matches!(*selector, CompiledCtlSelector::Path(_)) && !matches!(value, b"/*" | b"//@*") {
                        return Err(CompilerError::CtlXmlSelector {
                            location: action.location,
                        });
                    }
                    let range = ByteRange {
                        start: target_bytes.len(),
                        len: value.len(),
                    };
                    work.reserve_canonical(&mut target_bytes, value.len(), "compiled ctl target bytes")?;
                    target_bytes.extend_from_slice(value);
                    if matches!(*selector, CompiledCtlSelector::Literal(_)) {
                        CompiledTargetSelector::Literal(range)
                    } else {
                        CompiledTargetSelector::Path(range)
                    }
                }
                CompiledCtlSelector::RegexSource(source) => {
                    let count = boolean_regexes
                        .len()
                        .checked_add(capture_regexes.len())
                        .and_then(|count| count.checked_add(1))
                        .ok_or(CompilerError::PreflightArithmetic)?;
                    if count > limits.max_regex_programs {
                        return Err(CompilerError::RegexProgramLimit {
                            actual: count,
                            limit: limits.max_regex_programs,
                        });
                    }
                    let pattern = std::str::from_utf8(byte_range(&bytes, source)?).map_err(|_| {
                        CompilerError::PrivateInvariant {
                            kind: CompilerInvariant::InvalidByteRange,
                        }
                    })?;
                    let regex = if *collection == CollectionKind::Tx {
                        work.compile_ascii_case_insensitive_boolean_regex(pattern, action.location, true)?
                    } else {
                        work.compile_boolean_regex(pattern, action.location, true)?
                    };
                    work.retain_compiler_records::<CompiledRegexProgram>(1)?;
                    boolean_regexes.try_reserve(1).map_err(|_| CompilerError::Allocation {
                        operation: "compiled ctl target regex",
                    })?;
                    let program = boolean_regexes.len();
                    boolean_regexes.push(regex);
                    CompiledTargetSelector::Regex {
                        style: RegexStyle::Bare,
                        program,
                    }
                }
                CompiledCtlSelector::Resolved(_) => {
                    return Err(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::UnexpectedActionOperand,
                    });
                }
            };
            *selector = CompiledCtlSelector::Resolved(resolved);
        }
        let tag_count = units.iter().try_fold(0_usize, |mut total, unit| {
            let end = unit
                .member_start
                .checked_add(unit.member_count)
                .ok_or(CompilerError::PreflightArithmetic)?;
            let members = members
                .get(unit.member_start..end)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::ScheduleOrder,
                })?;
            for member in members {
                let end = member
                    .action_start
                    .checked_add(member.action_count)
                    .ok_or(CompilerError::PreflightArithmetic)?;
                let actions = actions
                    .get(member.action_start..end)
                    .ok_or(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::ScheduleOrder,
                    })?;
                total = total
                    .checked_add(
                        actions
                            .iter()
                            .filter(|action| {
                                matches!(
                                    (action.kind, action.operand),
                                    (
                                        ActionEnvelopeKind::Action(super::ActionKind::Tag),
                                        CompiledActionOperand::Literal(_)
                                    )
                                )
                            })
                            .count(),
                    )
                    .ok_or(CompilerError::PreflightArithmetic)?;
            }
            Ok::<_, CompilerError>(total)
        })?;
        work.retain_compiler_records::<CompiledTagRuleRef>(tag_count)?;
        let mut tag_rule_refs = Vec::new();
        reserve_exact(&mut tag_rule_refs, tag_count, "compiled ctl tag index")?;
        for (unit_id, unit) in units.iter().enumerate() {
            for member in &members[unit.member_start..unit.member_start + unit.member_count] {
                for action in &actions[member.action_start..member.action_start + member.action_count] {
                    if let (ActionEnvelopeKind::Action(super::ActionKind::Tag), CompiledActionOperand::Literal(tag)) =
                        (action.kind, action.operand)
                    {
                        tag_rule_refs.push(CompiledTagRuleRef { tag, unit: unit_id });
                    }
                }
            }
        }
        tag_rule_refs.sort_unstable_by(|left, right| {
            byte_range(&bytes, left.tag)
                .expect("compiled tag range is valid")
                .cmp(byte_range(&bytes, right.tag).expect("compiled tag range is valid"))
                .then(left.unit.cmp(&right.unit))
        });
        let mut generation = Self {
            capture_hits: capture_hit_plan::CaptureHitPlan::default(),
            inspection_activation: inspection_activation::CompiledActivation::default(),
            json_inspection: json_inspection_plan::CompiledJsonPlan::default(),
            xml_inspection: json_inspection_plan::CompiledJsonPlan::default(),
            has_expensive_checks: optional_checks.hpp || optional_checks.multipart,
            prefix_validation: Ok(()),
            requires_complete_request_body: false,
            bytes,
            templates,
            segments,
            macros,
            actions,
            tag_rule_refs,
            setvars,
            members,
            operator_plans,
            ip_networks,
            target_regexes,
            boolean_regexes,
            capture_regexes,
            target_bytes,
            target_records,
            target_plans,
            transformations: canonical_transformations,
            transformation_plans,
            groups,
            pattern_mappings,
            capture_phrases,
            units,
            markers,
            schedules,
            compiler_owned_bytes: work.compiler_owned_bytes,
            matcher_charged_bytes: work.matcher_charged_bytes,
            peak_bytes: work.peak_bytes,
        };
        generation.inspection_activation =
            inspection_activation::CompiledActivation::compile(&generation, &mut work, &mut state.canonical_work)?;
        generation.json_inspection =
            json_inspection_plan::CompiledJsonPlan::compile(&generation, &mut work, &mut state.canonical_work)?;
        generation.xml_inspection = json_inspection_plan::CompiledJsonPlan::compile_domain(
            &generation,
            &mut work,
            &mut state.canonical_work,
            InspectionDomain::Xml,
        )?;
        capture_hit_plan::compile_optional(
            &mut generation,
            &validated,
            pattern_preflight,
            &mut work,
            &mut state.canonical_work,
        )?;
        for program in &mut generation.boolean_regexes {
            install_optional_regex_hint(
                program,
                limits.regex.max_program_bytes,
                &mut work,
                &mut state.canonical_work,
            )?;
        }
        for matcher in generation
            .groups
            .iter_mut()
            .map(|group| &mut group.matcher)
            .chain(generation.capture_hits.matchers.iter_mut())
        {
            if let Some(program) = matcher.regex_program_mut() {
                install_optional_regex_hint(
                    program,
                    limits.matcher.max_regex_program_bytes,
                    &mut work,
                    &mut state.canonical_work,
                )?;
            }
        }
        generation.compiler_owned_bytes = work.compiler_owned_bytes;
        generation.matcher_charged_bytes = work.matcher_charged_bytes;
        generation.peak_bytes = work.peak_bytes;
        generation.prefix_validation = generation.check_request_body_prefix();
        generation.requires_complete_request_body = generation.has_complete_request_body_dependency();
        Ok((generation, state.canonical_work))
    }
}

/// Compile sealed semantic input into one atomically published owned generation.
pub fn compile_validated(
    validated: ValidatedSemanticDocument<'_>,
    assets: Option<&mut dyn PhraseAssetSource>,
    limits: CompilerLimits,
) -> Result<CompiledGeneration, CompilerError> {
    compile_validated_with_budget(validated, assets, limits, 0, &mut CanonicalWork::default())
}

/// Share canonical work across sequential compile passes and charge externally live input.
pub(crate) fn compile_validated_with_budget(
    validated: ValidatedSemanticDocument<'_>,
    assets: Option<&mut dyn PhraseAssetSource>,
    limits: CompilerLimits,
    external_live_bytes: usize,
    work: &mut CanonicalWork,
) -> Result<CompiledGeneration, CompilerError> {
    let mut builder = CompilerBuilder::new(validated, assets, limits);
    builder.external_live_bytes = external_live_bytes;
    let prepared = builder.prepare()?;
    let matched = MatcherState::compile_with_work(prepared, limits, *work)?;
    let grouped = GroupedMatcherState::compile(matched, limits)?;
    let (generation, used) = CompiledGeneration::publish_with_work(grouped, limits)?;
    *work = used;
    Ok(generation)
}

#[derive(Clone, Copy)]
struct MatcherCompileWork {
    limits: CompilerLimits,
    validated_bytes: usize,
    working_bytes: usize,
    compiler_owned_bytes: usize,
    matcher_charged_bytes: usize,
    peak_bytes: usize,
    requirement_cursors: [usize; 8],
}

fn install_optional_regex_hint(
    program: &mut CompiledRegexProgram,
    program_cap: usize,
    work: &mut MatcherCompileWork,
    canonical: &mut CanonicalWork,
) -> Result<(), CompilerError> {
    let base = work.matcher_peak_base()?;
    let generation_bytes = work
        .compiler_owned_bytes
        .checked_add(work.matcher_charged_bytes)
        .ok_or(CompilerError::PreflightArithmetic)?;
    let owned = work
        .limits
        .max_compiled_matcher_bytes
        .saturating_sub(work.matcher_charged_bytes)
        .min(work.limits.max_generation_bytes.saturating_sub(generation_bytes))
        .min(program_cap.saturating_sub(program.memory_usage().max(program.conservative_program_memory_bound())));
    let mut budget = crate::matcher::RegexGuardBudget {
        owned,
        peak: work.limits.max_compilation_peak_bytes.saturating_sub(base),
        work: work
            .limits
            .max_group_key_record_comparisons
            .saturating_sub(canonical.record_comparisons),
    };
    let (simple, installed) = program.lower_simple(budget).map_err(|error| match error {
        RegexProgramError::Arithmetic => CompilerError::PreflightArithmetic,
        _ => CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        },
    })?;
    canonical.charge_records(simple.work, work.limits)?;
    if simple.peak != 0 {
        work.record_matcher_peak(simple.peak)?;
    }
    if installed {
        return Ok(());
    }
    budget.work = budget
        .work
        .checked_sub(simple.work)
        .ok_or(CompilerError::PreflightArithmetic)?;
    let outcome = program.add_literal_guard(budget).map_err(|error| match error {
        RegexProgramError::Arithmetic => CompilerError::PreflightArithmetic,
        _ => CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        },
    })?;
    canonical.charge_records(outcome.work, work.limits)?;
    if outcome.peak != 0 {
        work.record_matcher_peak(outcome.peak)?;
    }
    work.retain_matcher(outcome.owned)?;
    Ok(())
}

impl MatcherCompileWork {
    // Canonical identity counts are independent of retained backing capacity.
    // Admit growth and old/new overlap before allocation, then reconcile excess.
    fn reserve_canonical<T>(
        &mut self,
        values: &mut Vec<T>,
        additional: usize,
        operation: &'static str,
    ) -> Result<(), CompilerError> {
        let required = checked_add(values.len(), additional)?;
        let old = values.capacity();
        if required <= old {
            return Ok(());
        }
        let overlap = old
            .checked_mul(size_of::<T>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.retain_compiler_records::<T>(required - old)?;
        self.record_matcher_peak(overlap)?;
        reserve_exact(values, additional, operation)?;
        let excess = values
            .capacity()
            .checked_sub(required)
            .ok_or(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            })?;
        self.retain_compiler_records::<T>(excess)?;
        self.record_matcher_peak(overlap)
    }

    fn matcher_peak_base(&self) -> Result<usize, CompilerError> {
        self.validated_bytes
            .checked_add(self.compiler_owned_bytes)
            .and_then(|bytes| bytes.checked_add(self.working_bytes))
            .and_then(|bytes| bytes.checked_add(self.matcher_charged_bytes))
            .ok_or(CompilerError::PreflightArithmetic)
    }

    fn record_matcher_peak(&mut self, additional: usize) -> Result<(), CompilerError> {
        let snapshot = self
            .matcher_peak_base()?
            .checked_add(additional)
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.peak_bytes = self.peak_bytes.max(snapshot);
        if snapshot > self.limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        Ok(())
    }

    fn precheck_matcher_projection(&mut self, projection: MatcherBuildProjection) -> Result<(), CompilerError> {
        let peak = projection
            .conservative_peak_bytes()
            .map_err(|_| CompilerError::PreflightArithmetic)?;
        self.record_matcher_peak(peak)
    }

    fn compile_capture_phrase(
        &mut self,
        patterns: &[&str],
        source_bytes: usize,
        location: SemanticLocation,
        retain: bool,
    ) -> Result<CompiledCapturePhraseProgram, CompilerError> {
        let projection = MatcherBuildProjection::for_counts(
            MatcherPatternCounts {
                insensitive_phrases: patterns.len(),
                ..MatcherPatternCounts::default()
            },
            MatcherPatternBytes {
                insensitive_phrases: source_bytes,
                ..MatcherPatternBytes::default()
            },
            self.limits.matcher,
        )
        .map_err(|error| map_phrase_matcher_error(error, location))?;
        self.precheck_matcher_projection(projection)?;
        let program = CompiledCapturePhraseProgram::compile(patterns, self.limits.matcher)
            .map_err(|error| map_phrase_matcher_error(error, location))?;
        self.record_matcher_peak(program.estimated_peak_build_memory_usage())?;
        if retain {
            self.retain_matcher(program.memory_usage())?;
        }
        Ok(program)
    }

    fn compile_group_matcher(
        &mut self,
        patterns: &[PatternSpec<'_>],
        counts: MatcherPatternCounts,
        bytes: MatcherPatternBytes,
        location: SemanticLocation,
    ) -> Result<CompiledMatcher, CompilerError> {
        let projection = MatcherBuildProjection::for_counts(counts, bytes, self.limits.matcher)
            .map_err(|error| map_phrase_matcher_error(error, location))?;
        self.precheck_matcher_projection(projection)?;
        let matcher = CompiledMatcher::compile_pcre_bytes(patterns, self.limits.matcher)
            .map_err(|error| map_phrase_matcher_error(error, location))?;
        self.record_matcher_peak(matcher.estimated_peak_build_memory_usage())?;
        self.retain_matcher(matcher.memory_usage())?;
        Ok(matcher)
    }

    fn compile_boolean_regex(
        &mut self,
        source: &str,
        location: SemanticLocation,
        active: bool,
    ) -> Result<CompiledRegexProgram, CompilerError> {
        let (base, budget) = self.regex_peak_budget()?;
        let program = CompiledRegexProgram::compile_one_pcre_bytes_with_peak_budget(source, self.limits.regex, budget)
            .map_err(|error| self.map_regex_error(location, error))?;
        self.record_regex_peak(base, program.estimated_peak_build_memory_usage())?;
        if active {
            self.retain_matcher(program.memory_usage())?;
        }
        Ok(program)
    }

    fn compile_ascii_case_insensitive_boolean_regex(
        &mut self,
        source: &str,
        location: SemanticLocation,
        active: bool,
    ) -> Result<CompiledRegexProgram, CompilerError> {
        let (base, budget) = self.regex_peak_budget()?;
        let program = CompiledRegexProgram::compile_one_ascii_case_insensitive_with_peak_budget(
            source,
            self.limits.regex,
            budget,
        )
        .map_err(|error| self.map_regex_error(location, error))?;
        self.record_regex_peak(base, program.estimated_peak_build_memory_usage())?;
        if active {
            self.retain_matcher(program.memory_usage())?;
        }
        Ok(program)
    }

    fn compile_capture_regex(
        &mut self,
        source: &str,
        location: SemanticLocation,
        active: bool,
    ) -> Result<CompiledCaptureRegexProgram, CompilerError> {
        let (base, budget) = self.regex_peak_budget()?;
        let program =
            CompiledCaptureRegexProgram::compile_one_pcre_bytes_with_peak_budget(source, self.limits.regex, budget)
                .map_err(|error| self.map_regex_error(location, error))?;
        self.record_regex_peak(base, program.estimated_peak_build_memory_usage())?;
        if active {
            self.retain_matcher(program.memory_usage())?;
        }
        Ok(program)
    }

    fn regex_peak_budget(&self) -> Result<(usize, usize), CompilerError> {
        let base = self
            .validated_bytes
            .checked_add(self.compiler_owned_bytes)
            .and_then(|bytes| bytes.checked_add(self.working_bytes))
            .and_then(|bytes| bytes.checked_add(self.matcher_charged_bytes))
            .and_then(|bytes| bytes.checked_add(size_of::<&str>()))
            .ok_or(CompilerError::PreflightArithmetic)?;
        let budget = self
            .limits
            .max_compilation_peak_bytes
            .checked_sub(base)
            .ok_or(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            })?;
        Ok((base, budget))
    }

    fn record_regex_peak(&mut self, base: usize, regex_peak: usize) -> Result<(), CompilerError> {
        let snapshot = base.checked_add(regex_peak).ok_or(CompilerError::PreflightArithmetic)?;
        self.peak_bytes = self.peak_bytes.max(snapshot);
        if snapshot > self.limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        Ok(())
    }

    fn map_regex_error(&self, location: SemanticLocation, error: RegexProgramError) -> CompilerError {
        if error == RegexProgramError::SyntaxPeakLimit {
            CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            }
        } else {
            CompilerError::RegexProgram { location, error }
        }
    }

    fn retain_matcher(&mut self, bytes: usize) -> Result<(), CompilerError> {
        let retained = self
            .matcher_charged_bytes
            .checked_add(bytes)
            .ok_or(CompilerError::PreflightArithmetic)?;
        if retained > self.limits.max_compiled_matcher_bytes {
            return Err(CompilerError::CompiledMatcherByteLimit {
                limit: self.limits.max_compiled_matcher_bytes,
            });
        }
        let generation = self
            .compiler_owned_bytes
            .checked_add(retained)
            .ok_or(CompilerError::PreflightArithmetic)?;
        if generation > self.limits.max_generation_bytes {
            return Err(CompilerError::GenerationByteLimit {
                limit: self.limits.max_generation_bytes,
            });
        }
        self.matcher_charged_bytes = retained;
        Ok(())
    }

    fn drop_matcher(&mut self, bytes: usize) -> Result<(), CompilerError> {
        self.matcher_charged_bytes =
            self.matcher_charged_bytes
                .checked_sub(bytes)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                })?;
        Ok(())
    }

    fn retain_compiler_records<T>(&mut self, count: usize) -> Result<(), CompilerError> {
        let bytes = count
            .checked_mul(size_of::<T>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.retain_compiler_bytes(bytes)
    }

    fn retain_compiler_bytes(&mut self, bytes: usize) -> Result<(), CompilerError> {
        let owned = self
            .compiler_owned_bytes
            .checked_add(bytes)
            .ok_or(CompilerError::PreflightArithmetic)?;
        if owned > self.limits.max_compiler_owned_non_matcher_bytes {
            return Err(CompilerError::OwnedByteLimit {
                limit: self.limits.max_compiler_owned_non_matcher_bytes,
            });
        }
        let generation = owned
            .checked_add(self.matcher_charged_bytes)
            .ok_or(CompilerError::PreflightArithmetic)?;
        if generation > self.limits.max_generation_bytes {
            return Err(CompilerError::GenerationByteLimit {
                limit: self.limits.max_generation_bytes,
            });
        }
        let snapshot = self
            .validated_bytes
            .checked_add(owned)
            .and_then(|bytes| bytes.checked_add(self.working_bytes))
            .and_then(|bytes| bytes.checked_add(self.matcher_charged_bytes))
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.peak_bytes = self.peak_bytes.max(snapshot);
        if snapshot > self.limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        self.compiler_owned_bytes = owned;
        Ok(())
    }

    fn drop_compiler_bytes(&mut self, bytes: usize) -> Result<(), CompilerError> {
        self.compiler_owned_bytes =
            self.compiler_owned_bytes
                .checked_sub(bytes)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                })?;
        Ok(())
    }

    fn retain_working_records<T>(&mut self, count: usize) -> Result<(), CompilerError> {
        let bytes = count
            .checked_mul(size_of::<T>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.retain_working_bytes(bytes)
    }

    fn retain_working_bytes(&mut self, bytes: usize) -> Result<(), CompilerError> {
        self.working_bytes = self
            .working_bytes
            .checked_add(bytes)
            .ok_or(CompilerError::PreflightArithmetic)?;
        let snapshot = self
            .validated_bytes
            .checked_add(self.compiler_owned_bytes)
            .and_then(|value| value.checked_add(self.working_bytes))
            .and_then(|value| value.checked_add(self.matcher_charged_bytes))
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.peak_bytes = self.peak_bytes.max(snapshot);
        if snapshot > self.limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        Ok(())
    }

    fn drop_working_records<T>(&mut self, count: usize) {
        let bytes = count
            .checked_mul(size_of::<T>())
            .expect("bounded working record charge remains representable");
        self.working_bytes = self
            .working_bytes
            .checked_sub(bytes)
            .expect("working records were retained before drop");
    }

    fn drop_working_bytes(&mut self, bytes: usize) {
        self.working_bytes = self
            .working_bytes
            .checked_sub(bytes)
            .expect("working bytes were retained before drop");
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_operator_plan<'view, 'source>(
    operator: super::SemanticOperatorView<'view, 'source>,
    member: super::ValidatedRuleNodeView<'view, 'source>,
    operator_template: Option<usize>,
    ip_networks: &mut Vec<CompiledIpNetwork>,
    boolean_regexes: &mut Vec<CompiledRegexProgram>,
    capture_regexes: &mut Vec<CompiledCaptureRegexProgram>,
    capture_interner: &mut RegexProgramInterner<'view>,
    canonical_work: &mut CanonicalWork,
    requirements: &mut [RequirementState],
    work: &mut MatcherCompileWork,
    active: bool,
) -> Result<Option<CompiledOperatorPlan>, CompilerError> {
    use super::OperatorKind;

    let kind = operator.kind();
    if matches!(kind, OperatorKind::Pm | OperatorKind::PmFromFile) {
        return Ok(None);
    }
    if matches!(kind, OperatorKind::DetectSqli | OperatorKind::DetectXss) && !active {
        return Ok(None);
    }
    let operand = match (kind, operator.operand()) {
        (
            OperatorKind::BeginsWith
            | OperatorKind::Contains
            | OperatorKind::EndsWith
            | OperatorKind::Streq
            | OperatorKind::Within,
            super::OperatorOperandView::StringTemplate(_),
        ) => {
            let relation = match kind {
                OperatorKind::BeginsWith => StringRelation::BeginsWith,
                OperatorKind::Contains => StringRelation::Contains,
                OperatorKind::EndsWith => StringRelation::EndsWith,
                OperatorKind::Streq => StringRelation::Equals,
                OperatorKind::Within => StringRelation::Within,
                _ => unreachable!(),
            };
            CompiledOperatorOperand::String {
                relation,
                template: operator_template.ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::UnexpectedActionOperand,
                })?,
            }
        }
        (
            OperatorKind::Eq | OperatorKind::Ge | OperatorKind::Gt | OperatorKind::Lt,
            super::OperatorOperandView::IntegerTemplate { constant, .. },
        ) => {
            let relation = match kind {
                OperatorKind::Eq => IntegerRelation::Equal,
                OperatorKind::Ge => IntegerRelation::GreaterOrEqual,
                OperatorKind::Gt => IntegerRelation::Greater,
                OperatorKind::Lt => IntegerRelation::Less,
                _ => unreachable!(),
            };
            let operand = match constant {
                Some(value) => CompiledIntegerOperand::Static(value),
                None => CompiledIntegerOperand::Template(operator_template.ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::UnexpectedActionOperand,
                })?),
            };
            CompiledOperatorOperand::Integer { relation, operand }
        }
        (OperatorKind::IpMatch, super::OperatorOperandView::IpNetworks(networks)) => {
            let start = ip_networks.len();
            if active {
                for network in networks.networks() {
                    ip_networks.push(CompiledIpNetwork {
                        address: network.address(),
                        prefix: network.prefix(),
                    });
                }
            }
            CompiledOperatorOperand::IpNetworks {
                start,
                count: if active { ip_networks.len() - start } else { 0 },
            }
        }
        (OperatorKind::ValidateByteRange, super::OperatorOperandView::ByteSet(bytes)) => {
            CompiledOperatorOperand::ByteSet { words: bytes.words() }
        }
        (OperatorKind::ValidateUrlEncoding, super::OperatorOperandView::NoArgument) => {
            CompiledOperatorOperand::ValidateUrlEncoding
        }
        (OperatorKind::ValidateUtf8Encoding, super::OperatorOperandView::NoArgument) => {
            CompiledOperatorOperand::ValidateUtf8Encoding
        }
        (OperatorKind::UnconditionalMatch, super::OperatorOperandView::NoArgument) => {
            CompiledOperatorOperand::Unconditional
        }
        (OperatorKind::DetectSqli, super::OperatorOperandView::NoArgument) => CompiledOperatorOperand::DetectSqli {
            capture: member.rule().fold().capture_action().is_some(),
        },
        (OperatorKind::DetectXss, super::OperatorOperandView::NoArgument) => CompiledOperatorOperand::DetectXss,
        (OperatorKind::Rx, super::OperatorOperandView::Regex(regex)) => {
            let location = operator.location();
            let capture = member.rule().fold().capture_action().is_some();
            if active && !capture {
                return Ok(None);
            }
            let reference = if capture {
                if active {
                    let id = capture_interner.compile(regex.text(), location, capture_regexes, work, canonical_work)?;
                    RegexPlanRef::Capture(id)
                } else {
                    // Removed, nonexcluded rules still validate independently.
                    work.compile_capture_regex(regex.text(), location, false)?;
                    RegexPlanRef::Capture(usize::MAX)
                }
            } else {
                let program = work.compile_boolean_regex(regex.text(), location, active)?;
                if active {
                    let id = boolean_regexes.len();
                    boolean_regexes.push(program);
                    RegexPlanRef::Boolean(id)
                } else {
                    RegexPlanRef::Boolean(usize::MAX)
                }
            };
            discharge_requirement(
                requirements,
                &mut work.requirement_cursors,
                CompileRequirement::RegexCompilation,
                location,
            )?;
            CompiledOperatorOperand::Regex(reference)
        }
        _ => {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::UnexpectedActionOperand,
            });
        }
    };
    if !active {
        return Ok(None);
    }
    Ok(Some(CompiledOperatorPlan {
        kind,
        negated: operator.negated(),
        operand,
    }))
}

fn discharge_requirement(
    requirements: &mut [RequirementState],
    cursors: &mut [usize; 8],
    kind: CompileRequirement,
    location: SemanticLocation,
) -> Result<(), CompilerError> {
    let category = requirement_category(kind);
    let key = requirement_key(kind, location);
    let Ok(index) = requirements.binary_search_by_key(&key, |requirement| {
        requirement_key(requirement.kind, requirement.location)
    }) else {
        return Err(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::MissingRequirement,
        });
    };
    let requirement = &mut requirements[index];
    if requirement.discharged {
        return Err(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::DuplicateRequirement,
        });
    }
    requirement.discharged = true;
    cursors[category] = cursors[category].saturating_add(1);
    Ok(())
}

fn verify_b_requirements(requirements: &[RequirementState]) -> Result<(), CompilerError> {
    for requirement in requirements {
        if !matches!(
            requirement.kind,
            CompileRequirement::PhraseMatcherCompilation | CompileRequirement::RegexCompilation
        ) && !requirement.discharged
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::MissingRequirement,
            });
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct KnownPreflight {
    members: usize,
    actions: usize,
    transformations: usize,
    templates: usize,
    segments: usize,
    macros: usize,
    setvars: usize,
    metadata_bytes: usize,
    asset_requests: usize,
    first_template_location: Option<SemanticLocation>,
    max_member_runtime_templates: usize,
    first_member_runtime_error: Option<(usize, DirectiveLocation)>,
}

impl KnownPreflight {
    fn inspect(validated: &ValidatedSemanticDocument<'_>, limits: CompilerLimits) -> Result<Self, CompilerError> {
        let mut result = Self::default();
        for unit in validated.rule_units() {
            for member in unit.members() {
                result.members = checked_inc(result.members)?;
                if unit.is_compatibility_excluded() {
                    continue;
                }
                let mut runtime_templates = 0_usize;
                if let Some(operator) = member.operator() {
                    if let super::OperatorOperandView::ExternalAssets(external) = operator.operand() {
                        result.asset_requests = checked_add(result.asset_requests, external.assets().len())?;
                    }
                    if let Some(template) = operator.template() {
                        runtime_templates = checked_inc(runtime_templates)?;
                        result.add_template(
                            template.segments().map(|segment| (segment.text(), segment.kind())),
                            operator.location(),
                        )?;
                    }
                }
                for action in member.actions() {
                    result.actions = checked_inc(result.actions)?;
                    match action.operand() {
                        ActionOperandView::Template(template) => {
                            runtime_templates = checked_inc(runtime_templates)?;
                            result.add_template(
                                template.segments().map(|segment| (segment.text(), segment.kind())),
                                action.location(),
                            )?;
                        }
                        ActionOperandView::Literal(value) => {
                            result.metadata_bytes = checked_add(result.metadata_bytes, value.text().len())?;
                        }
                        ActionOperandView::SetVar(setvar) => {
                            runtime_templates = checked_inc(runtime_templates)?;
                            result.setvars = checked_inc(result.setvars)?;
                            result.add_template(
                                setvar.key().segments().map(|segment| (segment.text(), segment.kind())),
                                action.location(),
                            )?;
                            if let Some(value) = setvar.value() {
                                result.add_template(
                                    value.segments().map(|segment| (segment.text(), segment.kind())),
                                    action.location(),
                                )?;
                            }
                        }
                        ActionOperandView::Ctl(ctl) => {
                            let bytes = match ctl {
                                CtlView::AuditEngine(_)
                                | CtlView::ForceRequestBodyVariable(_)
                                | CtlView::RequestBodyProcessor(_) => 0,
                                CtlView::RuleRemoveByTag(tag) => tag.text().len(),
                                CtlView::RuleRemoveTargetByTag { tag, target } => tag
                                    .text()
                                    .len()
                                    .checked_add(
                                        target
                                            .regex_body()
                                            .or_else(|| target.selector())
                                            .map_or(0, |value| value.text().len()),
                                    )
                                    .ok_or(CompilerError::PreflightArithmetic)?,
                            };
                            result.metadata_bytes = checked_add(result.metadata_bytes, bytes)?;
                        }
                        ActionOperandView::NoArgument
                        | ActionOperandView::Local(_)
                        | ActionOperandView::Transformation(_)
                        | ActionOperandView::InitCol(_) => {}
                    }
                }
                result.transformations = checked_add(result.transformations, member.effective_transformations().len())?;
                let runtime_projection = runtime_templates.checked_mul(limits.max_template_runtime_bytes);
                let runtime_bytes = runtime_projection.unwrap_or(usize::MAX);
                if (runtime_projection.is_none() || runtime_bytes > limits.max_rule_runtime_expansion_bytes)
                    && result.first_member_runtime_error.is_none()
                {
                    result.first_member_runtime_error = Some((runtime_bytes, member.rule().location()));
                }
                result.max_member_runtime_templates = result.max_member_runtime_templates.max(runtime_templates);
            }
        }
        Ok(result)
    }

    fn add_template<'text, 'source>(
        &mut self,
        segments: impl Iterator<Item = (&'text str, TemplateSegmentKind<'text, 'source>)>,
        location: SemanticLocation,
    ) -> Result<(), CompilerError>
    where
        'source: 'text,
    {
        self.templates = checked_inc(self.templates)?;
        self.first_template_location.get_or_insert(location);
        for (text, kind) in segments {
            self.segments = checked_inc(self.segments)?;
            match kind {
                TemplateSegmentKind::Literal => {
                    self.metadata_bytes = checked_add(self.metadata_bytes, text.len())?;
                }
                TemplateSegmentKind::Expansion(expansion) => {
                    self.macros = checked_inc(self.macros)?;
                    self.metadata_bytes =
                        checked_add(self.metadata_bytes, expansion.key().map_or(0, |key| key.text().len()))?;
                }
            }
        }
        Ok(())
    }

    fn validate(&self, limits: CompilerLimits) -> Result<(), CompilerError> {
        if self.actions > limits.max_action_plans {
            return Err(CompilerError::ActionLimit {
                actual: self.actions,
                limit: limits.max_action_plans,
            });
        }
        if self.asset_requests > limits.max_asset_requests {
            return Err(CompilerError::AssetRequestLimit {
                actual: self.asset_requests,
                limit: limits.max_asset_requests,
            });
        }
        if self.templates > limits.max_templates {
            return Err(CompilerError::TemplateLimit {
                limit: limits.max_templates,
                location: self
                    .first_template_location
                    .expect("nonzero compiled template count has a source location"),
            });
        }
        if self.segments > limits.max_template_segments {
            return Err(CompilerError::TemplateSegmentLimit {
                limit: limits.max_template_segments,
                location: self
                    .first_template_location
                    .expect("nonzero compiled segment count has a source location"),
            });
        }
        if let Some((actual, location)) = self.first_member_runtime_error {
            return Err(CompilerError::MemberRuntimeByteLimit {
                actual,
                limit: limits.max_rule_runtime_expansion_bytes,
                location,
            });
        }
        let known_owned = self.known_owned_bytes()?;
        if known_owned > limits.max_owned_template_metadata_bytes
            || known_owned > limits.max_compiler_owned_non_matcher_bytes
        {
            return Err(CompilerError::OwnedByteLimit {
                limit: limits
                    .max_owned_template_metadata_bytes
                    .min(limits.max_compiler_owned_non_matcher_bytes),
            });
        }
        Ok(())
    }

    fn known_owned_bytes(&self) -> Result<usize, CompilerError> {
        let records = self
            .templates
            .checked_mul(size_of::<CompiledTemplate>())
            .and_then(|value| value.checked_add(self.segments.checked_mul(size_of::<CompiledTemplateSegment>())?))
            .and_then(|value| value.checked_add(self.macros.checked_mul(size_of::<CompiledMacroRef>())?))
            .and_then(|value| value.checked_add(self.actions.checked_mul(size_of::<CompiledActionPlan>())?))
            .and_then(|value| value.checked_add(self.setvars.checked_mul(size_of::<CompiledSetVar>())?))
            .and_then(|value| value.checked_add(self.members.checked_mul(size_of::<CompiledMemberPlan>())?))
            .and_then(|value| value.checked_add(self.transformations.checked_mul(size_of::<TransformationKind>())?))
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.metadata_bytes
            .checked_add(records)
            .ok_or(CompilerError::PreflightArithmetic)
    }

    fn validate_peak(
        &self,
        validated_bytes: usize,
        requirement_count: usize,
        limits: CompilerLimits,
    ) -> Result<(), CompilerError> {
        let projected = self.projected_peak_bytes(validated_bytes, requirement_count)?;
        if projected > limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: limits.max_compilation_peak_bytes,
            });
        }
        Ok(())
    }

    fn projected_peak_bytes(&self, validated_bytes: usize, requirement_count: usize) -> Result<usize, CompilerError> {
        requirement_count
            .checked_mul(size_of::<RequirementState>())
            .and_then(|bytes| bytes.checked_add(validated_bytes))
            .and_then(|bytes| bytes.checked_add(self.known_owned_bytes().ok()?))
            .and_then(|bytes| {
                self.max_member_runtime_templates
                    .checked_mul(size_of::<(usize, usize)>())
                    .and_then(|runtime| bytes.checked_add(runtime))
            })
            .ok_or(CompilerError::PreflightArithmetic)
    }
}

struct CompileWork {
    limits: CompilerLimits,
    validated_bytes: usize,
    bytes: Vec<u8>,
    asset_phrase_bytes: Vec<u8>,
    templates: Vec<CompiledTemplate>,
    segments: Vec<CompiledTemplateSegment>,
    macros: Vec<CompiledMacroRef>,
    actions: Vec<CompiledActionPlan>,
    setvars: Vec<CompiledSetVar>,
    members: Vec<CompiledMemberPlan>,
    transformations: Vec<TransformationKind>,
    assets: Vec<ResolvedAsset>,
    phrases: Vec<CompiledPhrase>,
    requirements: Vec<RequirementState>,
    runtime_templates: Vec<(usize, usize)>,
    requirement_cursors: [usize; 8],
    total_raw_asset_bytes: usize,
    total_phrase_bytes: usize,
    owned_bytes: usize,
    working_bytes: usize,
    peak_bytes: usize,
}

impl CompileWork {
    fn new(
        validated_bytes: usize,
        limits: CompilerLimits,
        requirements: Vec<RequirementState>,
        preflight: KnownPreflight,
    ) -> Result<Self, CompilerError> {
        let working_bytes =
            requirements
                .len()
                .checked_mul(size_of::<RequirementState>())
                .ok_or(CompilerError::PeakByteLimit {
                    limit: limits.max_compilation_peak_bytes,
                })?;
        if validated_bytes
            .checked_add(working_bytes)
            .is_none_or(|bytes| bytes > limits.max_compilation_peak_bytes)
        {
            return Err(CompilerError::PeakByteLimit {
                limit: limits.max_compilation_peak_bytes,
            });
        }
        let mut work = Self {
            limits,
            validated_bytes,
            bytes: Vec::new(),
            asset_phrase_bytes: Vec::new(),
            templates: Vec::new(),
            segments: Vec::new(),
            macros: Vec::new(),
            actions: Vec::new(),
            setvars: Vec::new(),
            members: Vec::new(),
            transformations: Vec::new(),
            assets: Vec::new(),
            phrases: Vec::new(),
            requirements,
            runtime_templates: Vec::new(),
            requirement_cursors: [0; 8],
            total_raw_asset_bytes: 0,
            total_phrase_bytes: 0,
            owned_bytes: 0,
            working_bytes,
            peak_bytes: validated_bytes + working_bytes,
        };
        reserve_exact(&mut work.bytes, preflight.metadata_bytes, "compiled bytes")?;
        reserve_exact(&mut work.templates, preflight.templates, "compiled templates")?;
        reserve_exact(&mut work.segments, preflight.segments, "compiled template segments")?;
        reserve_exact(&mut work.macros, preflight.macros, "compiled macros")?;
        reserve_exact(&mut work.actions, preflight.actions, "compiled actions")?;
        reserve_exact(&mut work.setvars, preflight.setvars, "compiled setvars")?;
        reserve_exact(&mut work.members, preflight.members, "compiled members")?;
        reserve_exact(
            &mut work.transformations,
            preflight.transformations,
            "compiled transformations",
        )?;
        reserve_exact(&mut work.assets, preflight.asset_requests, "resolved assets")?;
        reserve_exact(
            &mut work.runtime_templates,
            preflight.max_member_runtime_templates,
            "member template identities",
        )?;
        Ok(work)
    }

    fn resolve_asset(
        &mut self,
        location: SemanticLocation,
        asset: ExternalAssetView<'_>,
        assets: &mut Option<&mut dyn PhraseAssetSource>,
    ) -> Result<(), CompilerError> {
        let request_id = self.assets.len();
        if request_id >= self.limits.max_asset_requests {
            return Err(CompilerError::AssetRequestLimit {
                actual: request_id.saturating_add(1),
                limit: self.limits.max_asset_requests,
            });
        }
        let provider = assets
            .as_deref_mut()
            .ok_or(CompilerError::MissingAssetSource { request_id, location })?;
        let mut raw = Vec::new();
        let mut sink = PhraseAssetSink {
            bytes: &mut raw,
            sticky: None,
            prior_asset_bytes: self.total_raw_asset_bytes,
            working_base_bytes: self
                .validated_bytes
                .checked_add(self.owned_bytes)
                .and_then(|bytes| bytes.checked_add(self.working_bytes))
                .ok_or(CompilerError::PeakByteLimit {
                    limit: self.limits.max_compilation_peak_bytes,
                })?,
            limits: self.limits,
            peak_bytes: &mut self.peak_bytes,
        };
        let provider_result = provider.load(
            PhraseAssetRequest {
                request_id,
                location,
                asset,
            },
            &mut sink,
        );
        if let Some(error) = sink.sticky {
            return Err(CompilerError::AssetSink {
                request_id,
                error,
                location,
            });
        }
        if let Err(failure) = provider_result {
            return Err(CompilerError::AssetSource {
                request_id,
                failure,
                location,
            });
        }
        self.total_raw_asset_bytes =
            self.total_raw_asset_bytes
                .checked_add(raw.len())
                .ok_or(CompilerError::AssetSink {
                    request_id,
                    error: PhraseAssetSinkError::Arithmetic,
                    location,
                })?;
        let phrase_start = self.phrases.len();
        self.normalize_asset(request_id, location, &raw)?;
        reserve_one(&mut self.assets, "resolved asset")?;
        self.push_working_record_charge::<ResolvedAsset>(raw.len())?;
        self.assets.push(ResolvedAsset {
            request_id,
            phrase_start,
            phrase_count: self.phrases.len() - phrase_start,
        });
        Ok(())
    }

    fn normalize_asset(
        &mut self,
        request_id: usize,
        location: SemanticLocation,
        raw: &[u8],
    ) -> Result<(), CompilerError> {
        std::str::from_utf8(raw).map_err(|_| CompilerError::AssetUtf8 { request_id, location })?;
        let initial_count = self.phrases.len();
        let mut line = 1_usize;
        let mut start = 0_usize;
        while start <= raw.len() {
            let end = raw[start..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(raw.len(), |offset| start + offset);
            let mut content = &raw[start..end];
            let has_crlf_terminator = end < raw.len() && content.ends_with(b"\r");
            let content_without_terminator = if has_crlf_terminator {
                &content[..content.len() - 1]
            } else {
                content
            };
            if content_without_terminator.contains(&b'\r') || (!has_crlf_terminator && content.ends_with(b"\r")) {
                return Err(CompilerError::AssetBareCarriageReturn {
                    request_id,
                    line,
                    location,
                });
            }
            if has_crlf_terminator {
                content = &content[..content.len() - 1];
            }
            content = trim_ascii_space_tab(content);
            if !content.is_empty() && content[0] != b'#' {
                let per_asset = self.phrases.len() - initial_count + 1;
                if per_asset > self.limits.max_phrases_per_asset {
                    return Err(CompilerError::AssetPhraseLimit {
                        request_id,
                        actual: per_asset,
                        limit: self.limits.max_phrases_per_asset,
                        location,
                    });
                }
                if self.phrases.len() >= self.limits.max_asset_phrases {
                    return Err(CompilerError::TotalAssetPhraseLimit {
                        actual: self.phrases.len().saturating_add(1),
                        limit: self.limits.max_asset_phrases,
                        location,
                    });
                }
                let total =
                    self.total_phrase_bytes
                        .checked_add(content.len())
                        .ok_or(CompilerError::AssetPhraseByteLimit {
                            actual: usize::MAX,
                            limit: self.limits.max_asset_phrase_bytes,
                            location,
                        })?;
                if total > self.limits.max_asset_phrase_bytes {
                    return Err(CompilerError::AssetPhraseByteLimit {
                        actual: total,
                        limit: self.limits.max_asset_phrase_bytes,
                        location,
                    });
                }
                let bytes = self.push_asset_phrase_bytes(content, raw.len())?;
                reserve_one(&mut self.phrases, "compiled phrase")?;
                self.push_working_record_charge::<CompiledPhrase>(raw.len())?;
                self.phrases.push(CompiledPhrase { bytes });
                self.total_phrase_bytes = total;
            }
            if end == raw.len() {
                break;
            }
            start = end + 1;
            line += 1;
        }
        if self.phrases.len() == initial_count {
            return Err(CompilerError::EmptyAsset { request_id, location });
        }
        Ok(())
    }

    fn compile_member(
        &mut self,
        member: super::ValidatedRuleNodeView<'_, '_>,
        active: bool,
        compatibility_excluded: bool,
    ) -> Result<(), CompilerError> {
        let action_start = self.actions.len();
        debug_assert!(self.runtime_templates.is_empty());
        if compatibility_excluded {
            let state = member.resolved_state();
            let member_location = member
                .operator()
                .map(|operator| operator.location())
                .or_else(|| member.actions().next().map(|action| action.location()))
                .or_else(|| member.final_targets().next().map(|target| target.location()))
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::UnexpectedActionOperand,
                })?;
            reserve_one(&mut self.members, "compiled member")?;
            self.push_record_charge::<CompiledMemberPlan>(member_location)?;
            self.members.push(CompiledMemberPlan {
                active: false,
                expensive_aggregate_actions: false,
                operator_template: None,
                target_plan: None,
                transformation_plan: None,
                phase: state.phase(),
                disruptive: state.disruptive(),
                status: state.status(),
                log: state.log(),
                audit_log: state.audit_log(),
                action_start,
                action_count: 0,
                transformation_start: self.transformations.len(),
                transformation_count: 0,
                runtime_expansion_byte_limit: self.limits.max_rule_runtime_expansion_bytes,
                skip_after_marker: None,
            });
            return Ok(());
        }
        let operator_template = if let Some(template) = member.operator().and_then(|operator| operator.template()) {
            let compiled = self.compile_template(template, member.operator().unwrap().location())?;
            self.push_runtime_template(template.identity(), compiled)?;
            Some(compiled)
        } else {
            None
        };
        let state = member.resolved_state();
        for action in member.actions() {
            if matches!(
                action.operand(),
                ActionOperandView::Ctl(CtlView::ForceRequestBodyVariable(_) | CtlView::RequestBodyProcessor(_))
            ) && state.phase() != Phase::RequestHeaders
            {
                return Err(CompilerError::CtlPhase {
                    location: action.location(),
                });
            }
            self.compile_action(action)?;
        }
        let transformation_start = self.transformations.len();
        for action in member.effective_transformations() {
            let value = action.transformation().ok_or(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::UnexpectedActionOperand,
            })?;
            reserve_one(&mut self.transformations, "compiled transformation")?;
            self.push_record_charge::<TransformationKind>(action.location())?;
            self.transformations.push(value);
        }
        let runtime_bytes = self
            .runtime_templates
            .len()
            .checked_mul(self.limits.max_template_runtime_bytes)
            .ok_or(CompilerError::MemberRuntimeByteLimit {
                actual: usize::MAX,
                limit: self.limits.max_rule_runtime_expansion_bytes,
                location: member.rule().location(),
            })?;
        if runtime_bytes > self.limits.max_rule_runtime_expansion_bytes {
            return Err(CompilerError::MemberRuntimeByteLimit {
                actual: runtime_bytes,
                limit: self.limits.max_rule_runtime_expansion_bytes,
                location: member.rule().location(),
            });
        }
        let member_location = member
            .operator()
            .map(|operator| operator.location())
            .or_else(|| member.actions().next().map(|action| action.location()))
            .or_else(|| member.final_targets().next().map(|target| target.location()))
            .expect("validated member retains an execution node");
        reserve_one(&mut self.members, "compiled member")?;
        self.push_record_charge::<CompiledMemberPlan>(member_location)?;
        self.members.push(CompiledMemberPlan {
            active,
            expensive_aggregate_actions: false,
            operator_template,
            target_plan: None,
            transformation_plan: None,
            phase: state.phase(),
            disruptive: state.disruptive(),
            status: state.status(),
            log: state.log(),
            audit_log: state.audit_log(),
            action_start,
            action_count: self.actions.len() - action_start,
            transformation_start,
            transformation_count: self.transformations.len() - transformation_start,
            runtime_expansion_byte_limit: self.limits.max_rule_runtime_expansion_bytes,
            skip_after_marker: member.skip_after_marker().map(|marker| marker.ordinal()),
        });
        let runtime_working = self
            .runtime_templates
            .len()
            .checked_mul(size_of::<(usize, usize)>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.runtime_templates.clear();
        self.working_bytes = self
            .working_bytes
            .checked_sub(runtime_working)
            .expect("member runtime logical charge was added exactly once");
        Ok(())
    }

    fn compile_action(&mut self, action: SemanticActionView<'_, '_>) -> Result<(), CompilerError> {
        if self.actions.len() >= self.limits.max_action_plans {
            return Err(CompilerError::ActionLimit {
                actual: self.actions.len().saturating_add(1),
                limit: self.limits.max_action_plans,
            });
        }
        let operand = match action.operand() {
            ActionOperandView::NoArgument => CompiledActionOperand::None,
            ActionOperandView::Local(value) => CompiledActionOperand::Local(value),
            ActionOperandView::Transformation(value) => CompiledActionOperand::Transformation(value),
            ActionOperandView::Template(template) => {
                let id = self.compile_template(template, action.location())?;
                self.push_runtime_template(template.identity(), id)?;
                CompiledActionOperand::Template(id)
            }
            ActionOperandView::Literal(value) => {
                CompiledActionOperand::Literal(self.push_bytes(value.text().as_bytes(), action.location())?)
            }
            ActionOperandView::SetVar(setvar) => {
                let key_view = setvar.key();
                let key = self.compile_template_slice(key_view, action.location())?;
                self.push_runtime_template(key_view.template_identity(), key)?;
                let value = setvar
                    .value()
                    .map(|value| {
                        let id = self.compile_template_slice(value, action.location())?;
                        Ok(id)
                    })
                    .transpose()?;
                reserve_one(&mut self.setvars, "compiled setvar")?;
                self.push_record_charge::<CompiledSetVar>(action.location())?;
                let index = self.setvars.len();
                self.setvars.push(CompiledSetVar {
                    collection: setvar.collection(),
                    operation: setvar.operation(),
                    key,
                    value,
                    constant: setvar.constant(),
                });
                if action
                    .template()
                    .is_some_and(|template| template.expansion_count() != 0)
                {
                    self.discharge(CompileRequirement::RuntimeMacro, action.location())?;
                }
                self.discharge(CompileRequirement::TransactionSetVar, action.location())?;
                CompiledActionOperand::SetVar(index)
            }
            ActionOperandView::Ctl(ctl) => {
                let compiled = match ctl {
                    CtlView::AuditEngine(_) => {
                        return Err(CompilerError::PrivateInvariant {
                            kind: CompilerInvariant::UnexpectedActionOperand,
                        });
                    }
                    CtlView::ForceRequestBodyVariable(value) => CompiledCtl::ForceRequestBodyVariable(value),
                    CtlView::RequestBodyProcessor(value) => CompiledCtl::RequestBodyProcessor(value),
                    CtlView::RuleRemoveByTag(tag) => {
                        CompiledCtl::RuleRemoveByTag(self.push_bytes(tag.text().as_bytes(), action.location())?)
                    }
                    CtlView::RuleRemoveTargetByTag { tag, target } => {
                        let tag = self.push_bytes(tag.text().as_bytes(), action.location())?;
                        let selector = match target.selector_kind() {
                            SelectorKind::None => CompiledCtlSelector::None,
                            SelectorKind::Literal | SelectorKind::Path => {
                                let text = target.selector().ok_or(CompilerError::PrivateInvariant {
                                    kind: CompilerInvariant::UnexpectedActionOperand,
                                })?;
                                let bytes = self.push_bytes(text.text().as_bytes(), action.location())?;
                                if target.selector_kind() == SelectorKind::Literal {
                                    CompiledCtlSelector::Literal(bytes)
                                } else {
                                    CompiledCtlSelector::Path(bytes)
                                }
                            }
                            SelectorKind::Regex(_) => {
                                let body = target.regex_body().ok_or(CompilerError::PrivateInvariant {
                                    kind: CompilerInvariant::UnexpectedActionOperand,
                                })?;
                                CompiledCtlSelector::RegexSource(
                                    self.push_bytes(body.text().as_bytes(), action.location())?,
                                )
                            }
                        };
                        CompiledCtl::RuleRemoveTargetByTag {
                            tag,
                            collection: target.collection(),
                            selector,
                        }
                    }
                };
                CompiledActionOperand::Ctl(compiled)
            }
            ActionOperandView::InitCol(_) => {
                return Err(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::UnexpectedActionOperand,
                });
            }
        };
        reserve_one(&mut self.actions, "compiled action")?;
        self.push_record_charge::<CompiledActionPlan>(action.location())?;
        self.actions.push(CompiledActionPlan {
            kind: action.kind(),
            operand,
            location: action.location(),
        });
        Ok(())
    }

    fn push_runtime_template(&mut self, identity: usize, compiled: usize) -> Result<(), CompilerError> {
        if self.runtime_templates.iter().any(|entry| entry.0 == identity) {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::DuplicateRequirement,
            });
        }
        if self.runtime_templates.len() == self.runtime_templates.capacity() {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::MissingRequirement,
            });
        }
        let new_working =
            self.working_bytes
                .checked_add(size_of::<(usize, usize)>())
                .ok_or(CompilerError::PeakByteLimit {
                    limit: self.limits.max_compilation_peak_bytes,
                })?;
        self.check_peak_parts(self.owned_bytes, new_working)?;
        self.runtime_templates.push((identity, compiled));
        self.working_bytes = new_working;
        Ok(())
    }

    fn compile_template(
        &mut self,
        template: OperandTemplateView<'_, '_>,
        location: SemanticLocation,
    ) -> Result<usize, CompilerError> {
        let expansion_count = template.expansion_count();
        let id = self.compile_segments(
            template.segments().map(|segment| {
                (
                    segment.text(),
                    match segment.kind() {
                        TemplateSegmentKind::Literal => None,
                        TemplateSegmentKind::Expansion(expansion) => Some(expansion),
                    },
                )
            }),
            location,
        )?;
        if expansion_count != 0 {
            self.discharge(CompileRequirement::RuntimeMacro, location)?;
        }
        Ok(id)
    }

    fn compile_template_slice(
        &mut self,
        template: TemplateSliceView<'_, '_>,
        location: SemanticLocation,
    ) -> Result<usize, CompilerError> {
        self.compile_segments(
            template.segments().map(|segment| {
                (
                    segment.text(),
                    match segment.kind() {
                        TemplateSegmentKind::Literal => None,
                        TemplateSegmentKind::Expansion(expansion) => Some(expansion),
                    },
                )
            }),
            location,
        )
    }

    fn compile_segments<'text, 'source>(
        &mut self,
        segments: impl Iterator<Item = (&'text str, Option<MacroExpansionView<'text, 'source>>)>,
        location: SemanticLocation,
    ) -> Result<usize, CompilerError>
    where
        'source: 'text,
    {
        if self.templates.len() >= self.limits.max_templates {
            return Err(CompilerError::TemplateLimit {
                limit: self.limits.max_templates,
                location,
            });
        }
        let segment_start = self.segments.len();
        let mut static_bytes = 0_usize;
        for (text, expansion) in segments {
            if self.segments.len() >= self.limits.max_template_segments {
                return Err(CompilerError::TemplateSegmentLimit {
                    limit: self.limits.max_template_segments,
                    location,
                });
            }
            let kind = if let Some(expansion) = expansion {
                let key = self.push_bytes(expansion.key().map_or(&[][..], |key| key.text().as_bytes()), location)?;
                reserve_one(&mut self.macros, "compiled macro")?;
                self.push_record_charge::<CompiledMacroRef>(location)?;
                let index = self.macros.len();
                self.macros.push(CompiledMacroRef {
                    source: expansion.source(),
                    key,
                });
                CompiledSegmentKind::Macro(index)
            } else {
                static_bytes = static_bytes
                    .checked_add(text.len())
                    .ok_or(CompilerError::TemplateStaticByteLimit {
                        actual: usize::MAX,
                        limit: self.limits.max_template_static_bytes,
                        location,
                    })?;
                CompiledSegmentKind::Literal(self.push_bytes(text.as_bytes(), location)?)
            };
            reserve_one(&mut self.segments, "compiled template segment")?;
            self.push_record_charge::<CompiledTemplateSegment>(location)?;
            self.segments.push(CompiledTemplateSegment { kind });
        }
        if static_bytes > self.limits.max_template_static_bytes {
            return Err(CompilerError::TemplateStaticByteLimit {
                actual: static_bytes,
                limit: self.limits.max_template_static_bytes,
                location,
            });
        }
        if static_bytes > self.limits.max_template_runtime_bytes {
            return Err(CompilerError::TemplateRuntimeByteLimit {
                limit: self.limits.max_template_runtime_bytes,
                location,
            });
        }
        reserve_one(&mut self.templates, "compiled template")?;
        self.push_record_charge::<CompiledTemplate>(location)?;
        let id = self.templates.len();
        self.templates.push(CompiledTemplate {
            segment_start,
            segment_count: self.segments.len() - segment_start,
            static_bytes,
            runtime_byte_limit: self.limits.max_template_runtime_bytes,
        });
        Ok(id)
    }

    fn discharge(&mut self, kind: CompileRequirement, location: SemanticLocation) -> Result<(), CompilerError> {
        let category = requirement_category(kind);
        let key = requirement_key(kind, location);
        let Ok(index) = self.requirements.binary_search_by_key(&key, |requirement| {
            requirement_key(requirement.kind, requirement.location)
        }) else {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::MissingRequirement,
            });
        };
        let requirement = &mut self.requirements[index];
        if requirement.discharged {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::DuplicateRequirement,
            });
        }
        requirement.discharged = true;
        self.requirement_cursors[category] = self.requirement_cursors[category].saturating_add(1);
        Ok(())
    }

    fn verify_a_requirements(&self) -> Result<(), CompilerError> {
        for requirement in &self.requirements {
            if matches!(
                requirement.kind,
                CompileRequirement::RuntimeMacro
                    | CompileRequirement::ExternalPhraseAsset
                    | CompileRequirement::TransactionSetVar
            ) && !requirement.discharged
            {
                return Err(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::MissingRequirement,
                });
            }
        }
        Ok(())
    }

    fn push_bytes(&mut self, bytes: &[u8], _location: SemanticLocation) -> Result<ByteRange, CompilerError> {
        let new_owned = self
            .owned_bytes
            .checked_add(bytes.len())
            .ok_or(CompilerError::OwnedByteLimit {
                limit: self.limits.max_compiler_owned_non_matcher_bytes,
            })?;
        if new_owned > self.limits.max_compiler_owned_non_matcher_bytes
            || new_owned > self.limits.max_owned_template_metadata_bytes
        {
            return Err(CompilerError::OwnedByteLimit {
                limit: self
                    .limits
                    .max_compiler_owned_non_matcher_bytes
                    .min(self.limits.max_owned_template_metadata_bytes),
            });
        }
        self.check_peak(new_owned)?;
        self.bytes
            .try_reserve_exact(bytes.len())
            .map_err(|_| CompilerError::Allocation {
                operation: "compiled bytes",
            })?;
        let range = ByteRange {
            start: self.bytes.len(),
            len: bytes.len(),
        };
        self.bytes.extend_from_slice(bytes);
        self.owned_bytes = new_owned;
        Ok(range)
    }

    fn push_record_charge<T>(&mut self, _location: SemanticLocation) -> Result<(), CompilerError> {
        let new_owned = self
            .owned_bytes
            .checked_add(size_of::<T>())
            .ok_or(CompilerError::OwnedByteLimit {
                limit: self.limits.max_compiler_owned_non_matcher_bytes,
            })?;
        if new_owned > self.limits.max_compiler_owned_non_matcher_bytes {
            return Err(CompilerError::OwnedByteLimit {
                limit: self.limits.max_compiler_owned_non_matcher_bytes,
            });
        }
        self.check_peak(new_owned)?;
        self.owned_bytes = new_owned;
        Ok(())
    }

    fn push_asset_phrase_bytes(&mut self, bytes: &[u8], raw_live_bytes: usize) -> Result<ByteRange, CompilerError> {
        let new_working = self
            .working_bytes
            .checked_add(bytes.len())
            .ok_or(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            })?;
        self.check_peak_parts_with_raw(self.owned_bytes, new_working, raw_live_bytes)?;
        self.asset_phrase_bytes
            .try_reserve_exact(bytes.len())
            .map_err(|_| CompilerError::Allocation {
                operation: "asset phrase bytes",
            })?;
        let range = ByteRange {
            start: self.asset_phrase_bytes.len(),
            len: bytes.len(),
        };
        self.asset_phrase_bytes.extend_from_slice(bytes);
        self.working_bytes = new_working;
        Ok(range)
    }

    fn push_working_record_charge<T>(&mut self, raw_live_bytes: usize) -> Result<(), CompilerError> {
        let new_working = self
            .working_bytes
            .checked_add(size_of::<T>())
            .ok_or(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            })?;
        self.check_peak_parts_with_raw(self.owned_bytes, new_working, raw_live_bytes)?;
        self.working_bytes = new_working;
        Ok(())
    }

    fn check_peak(&mut self, owned: usize) -> Result<(), CompilerError> {
        self.check_peak_parts(owned, self.working_bytes)
    }

    fn check_peak_parts(&mut self, owned: usize, working: usize) -> Result<(), CompilerError> {
        self.check_peak_parts_with_raw(owned, working, 0)
    }

    fn check_peak_parts_with_raw(
        &mut self,
        owned: usize,
        working: usize,
        raw_live_bytes: usize,
    ) -> Result<(), CompilerError> {
        let peak = self
            .validated_bytes
            .checked_add(owned)
            .and_then(|bytes| bytes.checked_add(working))
            .and_then(|bytes| bytes.checked_add(raw_live_bytes))
            .ok_or(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            })?;
        self.peak_bytes = self.peak_bytes.max(peak);
        if peak > self.limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        Ok(())
    }
}

fn requirement_category(kind: CompileRequirement) -> usize {
    match kind {
        CompileRequirement::RegexCompilation => 0,
        CompileRequirement::TargetSelectorRegexCompilation => 1,
        CompileRequirement::PhraseMatcherCompilation => 2,
        CompileRequirement::ExternalPhraseAsset => 3,
        CompileRequirement::RuntimeMacro => 4,
        CompileRequirement::BlockDefaultDisruptive => 5,
        CompileRequirement::TransactionSetVar => 6,
        CompileRequirement::DefaultPhase => 7,
    }
}

fn requirement_key(kind: CompileRequirement, location: SemanticLocation) -> (usize, usize, usize, usize, usize) {
    let node_kind = match location.node_kind() {
        SemanticNodeKind::Directive => 0,
        SemanticNodeKind::Target => 1,
        SemanticNodeKind::Operator => 2,
        SemanticNodeKind::Action => 3,
    };
    (
        requirement_category(kind),
        location.document_index(),
        location.directive_index(),
        node_kind,
        location.node_index(),
    )
}

fn trim_ascii_space_tab(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn reserve_one<T>(records: &mut Vec<T>, operation: &'static str) -> Result<(), CompilerError> {
    records
        .try_reserve_exact(1)
        .map_err(|_| CompilerError::Allocation { operation })
}

fn reserve_exact<T>(records: &mut Vec<T>, additional: usize, operation: &'static str) -> Result<(), CompilerError> {
    #[cfg(test)]
    if COMPILER_ALLOCATION_FAILURE.with(|failure| failure.get() == Some(operation)) {
        return Err(CompilerError::Allocation { operation });
    }
    records
        .try_reserve_exact(additional)
        .map_err(|_| CompilerError::Allocation { operation })
}

fn checked_inc(value: usize) -> Result<usize, CompilerError> {
    checked_add(value, 1)
}

fn checked_add(left: usize, right: usize) -> Result<usize, CompilerError> {
    left.checked_add(right).ok_or(CompilerError::PreflightArithmetic)
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/compiler_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/support/seclang/compiler.rs"]
mod test_support;
#[cfg(test)]
use test_support::COMPILER_ALLOCATION_FAILURE;
