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
#[path = "../xml_inspection_plan.rs"]
mod xml_inspection_plan;
pub use xml_inspection_plan::XmlInspectionUnsupported;
#[cfg(test)]
pub(crate) use xml_inspection_plan::XmlInspectionUnsupportedReason;
#[path = "../capture_hit_plan.rs"]
mod capture_hit_plan;
#[path = "../expensive_checks.rs"]
mod expensive_checks;
#[path = "../inspection_activation.rs"]
mod inspection_activation;
#[path = "../json_inspection_plan.rs"]
mod json_inspection_plan;
pub use json_inspection_plan::JsonInspectionUnsupported;

mod canonical;
mod generation;
mod lowering;
mod operator_plan;
mod publish;
mod work;

use canonical::*;
use generation::*;
use lowering::*;
use operator_plan::*;
use work::*;

pub(crate) use canonical::CanonicalWork;
pub use generation::{CompiledGeneration, CompiledScheduleEntryKind};

#[path = "../transaction.rs"]
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
#[path = "../../../tests/unit/seclang/compiler/mod_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/support/seclang/compiler/mod.rs"]
mod test_support;
#[cfg(test)]
use test_support::COMPILER_ALLOCATION_FAILURE;
