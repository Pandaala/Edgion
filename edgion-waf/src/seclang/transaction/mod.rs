//! Runtime transaction: limits, public request/response views, errors, findings and the `Transaction` owner.

use crate::seclang::decimal_bytes;
#[cfg(test)]
use crate::seclang::measurements::{self, Event};
use std::borrow::Cow;
use std::mem::size_of;
use std::ops::Deref;
use std::sync::Arc;

use super::InspectionDomain;
use json_inspection::{CandidateId, CandidateKind, InspectionBudget, SealedMatchValue, SealedResults};
use json_inspection_runtime::{map_store_error, JsonPublication};
use thiserror::Error;

use crate::matcher::{
    CapturePhraseMatch, MatchScratch, MatcherScanError, MatcherStreamState, RegexBooleanScratch, RegexCaptureScratch,
    RegexScratchError,
};

use super::super::body::{
    build_fallback_publication, build_fallback_publication_with_raw_policy, build_publication,
    build_publication_with_raw_policy, processor_from_content_type, BodyError, BodyLimits, BodyPublication,
    RequestBody,
};
use super::super::transform::{
    apply_transformations_borrowed, apply_transformations_multimatch, TransformError, TransformLimits,
};
use super::super::{ActionKind, BodyProcessorSelection, Severity, TransformationKind};
use super::inspection_activation;
use super::json_inspection_plan::JsonMemberMode;
// `CompilerByteRange` is aliased because `storage::ByteRange` (a runtime range) shares the name.
use super::{
    is_streaming_raw_member, ActionEnvelopeKind, ByteRange as CompilerByteRange, CollectionKind, CompiledActionOperand,
    CompiledCtl, CompiledCtlSelector, CompiledGeneration, CompiledIntegerOperand, CompiledIpNetwork,
    CompiledMemberPlan, CompiledOperatorOperand, CompiledOperatorPlan, CompiledRuleUnit, CompiledScheduleEntry,
    CompiledSegmentKind, CompiledSetVar, CompiledTagRuleRef, CompiledTargetSelector, LocalActionValue, MacroSource,
    OnOff, Phase, RegexPlanRef, RequestBodyProcessor, ResolvedDisruptive, SetVarOperation, StringRelation,
    TargetModifier,
};

pub use json_inspection::ResultLimits as JsonResultLimits;
pub use json_inspection_runtime::{JsonInspectionLimits, JsonInspectionSession, JsonInspectionUsage, JsonResultLimit};
mod operators;
mod phases;
mod predicate;
mod selection;
mod state;
mod storage;
mod units;

use operators::*;
use predicate::*;
use selection::*;
use state::*;
use storage::*;

pub const DEFAULT_MAX_RUNTIME_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_HEADERS: usize = 1_024;
pub const DEFAULT_MAX_RUNTIME_HEADER_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_QUERY_ARGUMENTS: usize = 4_096;
pub const DEFAULT_MAX_RUNTIME_COOKIES: usize = 4_096;
pub const DEFAULT_MAX_RUNTIME_COLLECTION_ENTRIES: usize = 65_536;
pub const DEFAULT_MAX_RUNTIME_VALUE_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_RETAINED_BYTES: usize = 32 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_SELECTED_VALUES: usize = 65_536;
pub const DEFAULT_MAX_RUNTIME_FINDINGS: usize = 1_024;
pub const DEFAULT_MAX_RUNTIME_OPERATOR_EVALUATIONS: usize = 1_000_000;
pub const DEFAULT_MAX_RUNTIME_OPERATOR_WORK_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_TRANSFORMATION_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_TRANSFORMATION_WORK_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_MATCHER_SCRATCH_BYTES: usize = 256 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_TX_VARIABLES: usize = 4_096;
pub const DEFAULT_MAX_RUNTIME_TX_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_SETVAR_ACTIONS: usize = 100_000;
pub const DEFAULT_MAX_RUNTIME_CTL_ENTRIES: usize = 4_096;
pub const DEFAULT_MAX_RUNTIME_CAPTURE_ENTRIES: usize = 64;
pub const DEFAULT_MAX_RUNTIME_CAPTURE_BYTES: usize = 256 * 1024;
pub const DEFAULT_MAX_RUNTIME_RESPONSE_HEADERS: usize = 1_024;
pub const DEFAULT_MAX_RUNTIME_RESPONSE_HEADER_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_RESPONSE_WINDOW_BYTES: usize = 256 * 1024;

/// Optional rule computations for one transaction. Gateway supplies its pinned policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleCheckPolicy {
    pub expensive_checks: bool,
}

impl Default for RuleCheckPolicy {
    /// Direct engine callers execute the original complete rules by default.
    fn default() -> Self {
        Self { expensive_checks: true }
    }
}

/// Select the matcher execution path used by one transaction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EvaluationMode {
    /// Use the compiled matcher's bounded candidate acceleration.
    #[default]
    Accelerated,
    /// Bypass optional phrase candidate acceleration for differential checks.
    Reference,
}

/// Select whether disruptive actions enforce or only report their decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RuleEngineMode {
    #[default]
    On,
    DetectionOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimits {
    pub max_input_bytes: usize,
    pub max_headers: usize,
    pub max_header_bytes: usize,
    pub max_query_arguments: usize,
    pub max_cookies: usize,
    pub max_collection_entries: usize,
    pub max_value_bytes: usize,
    pub max_retained_bytes: usize,
    pub max_selected_values: usize,
    pub max_findings: usize,
    pub max_operator_evaluations: usize,
    pub max_operator_work_bytes: usize,
    pub max_transformation_output_bytes: usize,
    pub max_transformation_work_bytes: usize,
    pub max_matcher_scratch_bytes: usize,
    pub max_tx_variables: usize,
    pub max_tx_bytes: usize,
    pub max_setvar_actions: usize,
    pub max_ctl_entries: usize,
    pub max_capture_entries: usize,
    pub max_capture_bytes: usize,
    pub max_response_headers: usize,
    pub max_response_header_bytes: usize,
    pub max_response_window_bytes: usize,
    pub body: BodyLimits,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: DEFAULT_MAX_RUNTIME_INPUT_BYTES,
            max_headers: DEFAULT_MAX_RUNTIME_HEADERS,
            max_header_bytes: DEFAULT_MAX_RUNTIME_HEADER_BYTES,
            max_query_arguments: DEFAULT_MAX_RUNTIME_QUERY_ARGUMENTS,
            max_cookies: DEFAULT_MAX_RUNTIME_COOKIES,
            max_collection_entries: DEFAULT_MAX_RUNTIME_COLLECTION_ENTRIES,
            max_value_bytes: DEFAULT_MAX_RUNTIME_VALUE_BYTES,
            max_retained_bytes: DEFAULT_MAX_RUNTIME_RETAINED_BYTES,
            max_selected_values: DEFAULT_MAX_RUNTIME_SELECTED_VALUES,
            max_findings: DEFAULT_MAX_RUNTIME_FINDINGS,
            max_operator_evaluations: DEFAULT_MAX_RUNTIME_OPERATOR_EVALUATIONS,
            max_operator_work_bytes: DEFAULT_MAX_RUNTIME_OPERATOR_WORK_BYTES,
            max_transformation_output_bytes: DEFAULT_MAX_RUNTIME_TRANSFORMATION_OUTPUT_BYTES,
            max_transformation_work_bytes: DEFAULT_MAX_RUNTIME_TRANSFORMATION_WORK_BYTES,
            max_matcher_scratch_bytes: DEFAULT_MAX_RUNTIME_MATCHER_SCRATCH_BYTES,
            max_tx_variables: DEFAULT_MAX_RUNTIME_TX_VARIABLES,
            max_tx_bytes: DEFAULT_MAX_RUNTIME_TX_BYTES,
            max_setvar_actions: DEFAULT_MAX_RUNTIME_SETVAR_ACTIONS,
            max_ctl_entries: DEFAULT_MAX_RUNTIME_CTL_ENTRIES,
            max_capture_entries: DEFAULT_MAX_RUNTIME_CAPTURE_ENTRIES,
            max_capture_bytes: DEFAULT_MAX_RUNTIME_CAPTURE_BYTES,
            max_response_headers: DEFAULT_MAX_RUNTIME_RESPONSE_HEADERS,
            max_response_header_bytes: DEFAULT_MAX_RUNTIME_RESPONSE_HEADER_BYTES,
            max_response_window_bytes: DEFAULT_MAX_RUNTIME_RESPONSE_WINDOW_BYTES,
            body: BodyLimits::default(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct RequestHeader<'input> {
    name: &'input [u8],
    value: &'input [u8],
}

impl std::fmt::Debug for RequestHeader<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestHeader")
            .field("name_bytes", &self.name.len())
            .field("value_bytes", &self.value.len())
            .finish()
    }
}

impl<'input> RequestHeader<'input> {
    pub const fn new(name: &'input [u8], value: &'input [u8]) -> Self {
        Self { name, value }
    }

    pub const fn name(self) -> &'input [u8] {
        self.name
    }

    pub const fn value(self) -> &'input [u8] {
        self.value
    }
}

#[derive(Clone, Copy)]
pub struct ResponseHeader<'input> {
    name: &'input [u8],
    value: &'input [u8],
}

impl std::fmt::Debug for ResponseHeader<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResponseHeader")
            .field("name_bytes", &self.name.len())
            .field("value_bytes", &self.value.len())
            .finish()
    }
}

impl<'input> ResponseHeader<'input> {
    pub const fn new(name: &'input [u8], value: &'input [u8]) -> Self {
        Self { name, value }
    }

    pub const fn name(self) -> &'input [u8] {
        self.name
    }

    pub const fn value(self) -> &'input [u8] {
        self.value
    }
}

#[derive(Clone, Copy)]
pub struct ResponseHead<'input> {
    status: u16,
    headers: &'input [ResponseHeader<'input>],
}

impl std::fmt::Debug for ResponseHead<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResponseHead")
            .field("status", &self.status)
            .field("headers", &self.headers.len())
            .finish()
    }
}

impl<'input> ResponseHead<'input> {
    pub const fn new(status: u16, headers: &'input [ResponseHeader<'input>]) -> Self {
        Self { status, headers }
    }
}

#[derive(Clone, Copy)]
pub struct ResponseWindow<'input> {
    bytes: &'input [u8],
    end_of_stream: bool,
}

impl std::fmt::Debug for ResponseWindow<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResponseWindow")
            .field("bytes", &self.bytes.len())
            .field("end_of_stream", &self.end_of_stream)
            .finish()
    }
}

impl<'input> ResponseWindow<'input> {
    pub const fn new(bytes: &'input [u8], end_of_stream: bool) -> Self {
        Self { bytes, end_of_stream }
    }
}

#[derive(Clone, Copy)]
pub struct RequestHead<'input> {
    request_line: &'input [u8],
    method: &'input [u8],
    uri: &'input [u8],
    uri_raw: &'input [u8],
    protocol: &'input [u8],
    remote_addr: &'input [u8],
    unique_id: &'input [u8],
    headers: &'input [RequestHeader<'input>],
}

impl std::fmt::Debug for RequestHead<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestHead")
            .field("request_line_bytes", &self.request_line.len())
            .field("method_bytes", &self.method.len())
            .field("uri_bytes", &self.uri.len())
            .field("uri_raw_bytes", &self.uri_raw.len())
            .field("protocol_bytes", &self.protocol.len())
            .field("remote_addr_bytes", &self.remote_addr.len())
            .field("unique_id_bytes", &self.unique_id.len())
            .field("headers", &self.headers.len())
            .finish()
    }
}

impl<'input> RequestHead<'input> {
    // Public constructor consumed outside the crate; the eight slices are independent request fields.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        request_line: &'input [u8],
        method: &'input [u8],
        uri: &'input [u8],
        uri_raw: &'input [u8],
        protocol: &'input [u8],
        remote_addr: &'input [u8],
        unique_id: &'input [u8],
        headers: &'input [RequestHeader<'input>],
    ) -> Self {
        Self {
            request_line,
            method,
            uri,
            uri_raw,
            protocol,
            remote_addr,
            unique_id,
            headers,
        }
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("XML inspection parser failed: {kind}")]
    XmlStream { kind: crate::seclang::XmlStreamError },
    #[error("XML inspection result limit exceeded")]
    XmlResultLimit { kind: JsonResultLimit },
    #[error("XML inspection state is inconsistent")]
    XmlInspectionState,
    #[error("XML inspection dependency is unsupported for rule {rule_id}")]
    XmlUnsupported { rule_id: u64 },
    #[error("JSON checker failed: {kind}")]
    JsonChecker { kind: crate::seclang::JsonCheckerError },
    #[error("JSON inspection result limit exceeded")]
    JsonResultLimit {
        kind: json_inspection_runtime::JsonResultLimit,
    },
    #[error("JSON inspection state is inconsistent")]
    JsonInspectionState,
    #[error("JSON inspection dependency is unsupported for rule {rule_id}")]
    JsonUnsupported { rule_id: u64 },
    #[error("request input has {actual} bytes, exceeding the limit of {limit}")]
    InputByteLimit { actual: usize, limit: usize },
    #[error("request has {actual} headers, exceeding the limit of {limit}")]
    HeaderLimit { actual: usize, limit: usize },
    #[error("request headers have {actual} bytes, exceeding the limit of {limit}")]
    HeaderByteLimit { actual: usize, limit: usize },
    #[error("request value has {actual} bytes, exceeding the limit of {limit}")]
    ValueByteLimit { actual: usize, limit: usize },
    #[error("request has more query arguments than the limit of {limit}")]
    QueryArgumentLimit { limit: usize },
    #[error("request has more cookies than the limit of {limit}")]
    CookieLimit { limit: usize },
    #[error("transaction collections exceed the entry limit of {limit}")]
    CollectionEntryLimit { limit: usize },
    #[error("transaction retained bytes exceed the limit of {limit}")]
    RetainedByteLimit { limit: usize },
    #[error("transaction matcher scratch exceeds the byte limit of {limit}")]
    MatcherScratchByteLimit { limit: usize },
    #[error("selected values exceed the configured limit of {limit}")]
    SelectedValueLimit { limit: usize },
    #[error("operator evaluations exceed the configured limit of {limit}")]
    OperatorEvaluationLimit { limit: usize },
    #[error("operator work exceeds the configured byte limit of {limit}")]
    OperatorWorkLimit { limit: usize },
    #[error("detector input exceeds the fixed byte limit of {limit}")]
    DetectorInputLimit { limit: usize },
    #[error("transaction variables exceed the configured count limit of {limit}")]
    TxVariableLimit { limit: usize },
    #[error("transaction variables exceed the configured byte limit of {limit}")]
    TxByteLimit { limit: usize },
    #[error("setvar actions exceed the configured limit of {limit}")]
    SetVarActionLimit { limit: usize },
    #[error("request-scoped control entries exceed the configured limit of {limit}")]
    CtlEntryLimit { limit: usize },
    #[error("capture entries exceed the configured limit of {limit}")]
    CaptureEntryLimit { limit: usize },
    #[error("capture bytes exceed the configured limit of {limit}")]
    CaptureByteLimit { limit: usize },
    #[error("request body processing failed with bounded category {kind}")]
    Body { kind: BodyError },
    #[error("request body state has already been published")]
    RequestBodyAlreadyPublished,
    #[error("request body state must be published before phase 2 executes")]
    RequestBodyNotPublished,
    #[error("raw request-body streaming cannot apply request-scoped control actions")]
    RequestBodyStreamCtlUnavailable,
    #[error("compiled XML target selector is unavailable in the bounded runtime profile")]
    XmlSelectorUnavailable,
    #[error("response has {actual} headers, exceeding the limit of {limit}")]
    ResponseHeaderLimit { actual: usize, limit: usize },
    #[error("response headers have {actual} bytes, exceeding the limit of {limit}")]
    ResponseHeaderByteLimit { actual: usize, limit: usize },
    #[error("response status {status} is outside the supported HTTP range")]
    InvalidResponseStatus { status: u16 },
    #[error("response state has already been published")]
    ResponseAlreadyPublished,
    #[error("response state must be published before phase 3 executes")]
    ResponseNotPublished,
    #[error("response observation window has {actual} bytes, exceeding the limit of {limit}")]
    ResponseWindowByteLimit { actual: usize, limit: usize },
    #[error("compiled generation contains an invalid runtime reference")]
    GenerationInvariant,
    #[error("matcher execution failed with bounded category {kind:?}")]
    Matcher { kind: RuntimeMatcherError },
    #[error("transformation execution failed with bounded category {kind:?}")]
    Transformation { kind: RuntimeTransformError },
    #[error("runtime action {kind:?} is not implemented by the current transaction slice")]
    RuntimeActionUnavailable { kind: ActionKind },
    #[error("runtime macro source {kind:?} is not implemented by the current transaction slice")]
    RuntimeMacroUnavailable { kind: MacroSource },
    #[error("request header name is invalid at ordinal {ordinal}")]
    InvalidHeaderName { ordinal: usize },
    #[error("request header value is invalid at ordinal {ordinal}")]
    InvalidHeaderValue { ordinal: usize },
    #[error("response header name is invalid at ordinal {ordinal}")]
    InvalidResponseHeaderName { ordinal: usize },
    #[error("response header value is invalid at ordinal {ordinal}")]
    InvalidResponseHeaderValue { ordinal: usize },
    #[error("transaction phase {actual:?} cannot run while {expected:?} is required")]
    PhaseOrder { expected: Option<Phase>, actual: Phase },
    #[error("transaction accounting arithmetic overflowed")]
    Arithmetic,
    #[error("cannot allocate bounded transaction state")]
    Allocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMatcherError {
    ScratchShape,
    PatternShape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTransformError {
    OutputLimit,
    WorkLimit,
    Allocation,
    Arithmetic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleFinding {
    rule_id: u64,
    log: bool,
    audit_log: bool,
    severity: Option<Severity>,
    tag_count: usize,
}

impl RuleFinding {
    pub const fn rule_id(self) -> u64 {
        self.rule_id
    }

    pub const fn log(self) -> bool {
        self.log
    }

    pub const fn audit_log(self) -> bool {
        self.audit_log
    }

    pub const fn severity(self) -> Option<Severity> {
        self.severity
    }

    pub const fn tag_count(self) -> usize {
        self.tag_count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intervention {
    rule_id: u64,
    status: u16,
}

impl Intervention {
    pub const fn rule_id(self) -> u64 {
        self.rule_id
    }

    pub const fn status(self) -> u16 {
        self.status
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseOutcome {
    findings: Vec<RuleFinding>,
    omitted_findings: usize,
    omitted_loggable_findings: usize,
    intervention: Option<Intervention>,
    evaluated_units: usize,
}

impl PhaseOutcome {
    pub fn findings(&self) -> &[RuleFinding] {
        &self.findings
    }

    pub const fn omitted_findings(&self) -> usize {
        self.omitted_findings
    }

    /// Findings omitted by `max_findings` that retained at least one logging
    /// action. This lets callers honor `nolog,noauditlog` without retaining
    /// the full finding payload beyond the configured runtime bound.
    pub const fn omitted_loggable_findings(&self) -> usize {
        self.omitted_loggable_findings
    }

    pub const fn intervention(&self) -> Option<Intervention> {
        self.intervention
    }

    pub const fn evaluated_units(&self) -> usize {
        self.evaluated_units
    }
}

/// Default payload-byte inspection window.
pub const DEFAULT_REQUEST_BODY_PREFIX_BYTES: usize = 64 * 1024;
/// Hard upper bound for configurable prefix inspection.
pub const MAX_REQUEST_BODY_PREFIX_BYTES: usize = 1024 * 1024;

/// Why prefix inspection stopped; this is separate from body publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestBodyPrefixCompletion {
    EndOfStream,
    WindowFilled,
    /// No phase-two rules exist, so no payload inspection was needed.
    NotRequired,
}

pub struct Transaction<'generation> {
    ordinary_scan_cache: OrdinaryScanCache,
    borrowed_transform_memo: BorrowedTransformMemo,
    generation: GenerationHandle<'generation>,
    request: RequestState<'generation>,
    json_budget: Option<InspectionBudget>,
    json_failure: Option<RuntimeError>,
    next_phase: u8,
    limits: RuntimeLimits,
    group_scratch: Vec<Option<MatchScratch>>,
    boolean_scratch: Vec<Option<RegexBooleanScratch>>,
    capture_scratch: Vec<Option<RegexCaptureScratch>>,
    scratch_bytes: usize,
    selected_scratch: Vec<SelectedValue>,
    match_scratch: Vec<MemberMatch>,
    match_scratch_bytes: usize,
    detector_work_bytes: usize,
    /// Ordinary operator work of the current rule unit. It survives every
    /// chain member and frontier branch, including failed ones, so branch
    /// fan-out cannot multiply the per-unit work bound.
    unit_work_bytes: usize,
    ctl_work_bytes: usize,
    tx: TxState,
    /// Undo log of the rule unit staged in place in `tx`; reused across units.
    tx_journal: TxJournal,
    capture_keys: Vec<MatchValue>,
    response: Option<ResponseState>,
    request_body_error: Option<BodyError>,
    request_body_cutoff: bool,
    evaluation_mode: EvaluationMode,
    #[cfg(test)]
    force_json_cursor: bool,
    /// Test oracle switch: select TX targets through the generic cursor path.
    #[cfg(test)]
    force_generic_selection: bool,
    rule_engine_mode: RuleEngineMode,
    request_body_stream: Option<RequestBodyStreamState>,
    stream_member_matches: Option<Vec<Option<bool>>>,
    // Cold 0/1 owner: ordinary transactions keep no completed JSON allocation.
    // The fixed owner allocation is physical execution state, separate from
    // reference logical retention counters and independently bounded fact arenas.
    request_prefix_completion: Option<RequestBodyPrefixCompletion>,
}

struct RequestBodyStreamState {
    groups: Vec<Option<MatcherStreamState>>,
    member_positive: Vec<bool>,
    consumed_bytes: usize,
    operator_work_bytes: usize,
    early_positive_member: Option<usize>,
    retained_bytes: usize,
}

impl std::fmt::Debug for Transaction<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Transaction")
            .field("next_phase", &self.next_phase())
            .field("evaluation_mode", &self.evaluation_mode)
            .field("retained_bytes", &self.retained_bytes())
            .field("collection_entries", &self.request.entry_count)
            .finish_non_exhaustive()
    }
}

impl Transaction<'static> {
    /// Create a transaction that owns a shared immutable generation.
    pub fn new_shared(
        generation: Arc<CompiledGeneration>,
        request: RequestHead<'_>,
        limits: RuntimeLimits,
    ) -> Result<Self, RuntimeError> {
        Self::new_shared_with_policy(
            generation,
            request,
            limits,
            EvaluationMode::Accelerated,
            RuleEngineMode::On,
            RuleCheckPolicy::default(),
        )
    }

    /// Create a shared transaction with immutable optional-check policy.
    pub fn new_shared_with_policy(
        generation: Arc<CompiledGeneration>,
        request: RequestHead<'_>,
        limits: RuntimeLimits,
        evaluation_mode: EvaluationMode,
        rule_engine_mode: RuleEngineMode,
        check_policy: RuleCheckPolicy,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_handle(
            GenerationHandle::Shared(generation),
            request,
            limits,
            evaluation_mode,
            rule_engine_mode,
            check_policy,
        )
    }
}

impl<'generation> Transaction<'generation> {
    pub fn new(
        generation: &'generation CompiledGeneration,
        request: RequestHead<'_>,
        limits: RuntimeLimits,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_policy(
            generation,
            request,
            limits,
            EvaluationMode::Accelerated,
            RuleEngineMode::On,
            RuleCheckPolicy::default(),
        )
    }

    /// Create a borrowed transaction with immutable optional-check policy.
    pub fn new_with_policy(
        generation: &'generation CompiledGeneration,
        request: RequestHead<'_>,
        limits: RuntimeLimits,
        evaluation_mode: EvaluationMode,
        rule_engine_mode: RuleEngineMode,
        check_policy: RuleCheckPolicy,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_handle(
            GenerationHandle::Borrowed(generation),
            request,
            limits,
            evaluation_mode,
            rule_engine_mode,
            check_policy,
        )
    }

    fn new_with_handle(
        generation: GenerationHandle<'generation>,
        request: RequestHead<'_>,
        limits: RuntimeLimits,
        evaluation_mode: EvaluationMode,
        rule_engine_mode: RuleEngineMode,
        check_policy: RuleCheckPolicy,
    ) -> Result<Self, RuntimeError> {
        let request = RequestState::build(request, limits)?;
        let program_scratch_bytes = matcher_program_scratch_bytes(&generation)?;
        let scratch_bytes = matcher_scratch_capacity(
            program_scratch_bytes,
            generation.matcher_count(),
            generation.boolean_regexes.len(),
            generation.capture_regexes.len(),
        )?;
        if scratch_bytes > limits.max_matcher_scratch_bytes {
            return Err(RuntimeError::MatcherScratchByteLimit {
                limit: limits.max_matcher_scratch_bytes,
            });
        }
        let total_retained = request
            .retained_bytes
            .checked_add(scratch_bytes)
            .ok_or(RuntimeError::Arithmetic)?;
        if total_retained > limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: limits.max_retained_bytes,
            });
        }
        let mut group_scratch = Vec::new();
        group_scratch
            .try_reserve_exact(generation.matcher_count())
            .map_err(|_| RuntimeError::Allocation)?;
        group_scratch.resize_with(generation.matcher_count(), || None);
        let mut boolean_scratch = Vec::new();
        boolean_scratch
            .try_reserve_exact(generation.boolean_regexes.len())
            .map_err(|_| RuntimeError::Allocation)?;
        boolean_scratch.resize_with(generation.boolean_regexes.len(), || None);
        let mut capture_scratch = Vec::new();
        capture_scratch
            .try_reserve_exact(generation.capture_regexes.len())
            .map_err(|_| RuntimeError::Allocation)?;
        capture_scratch.resize_with(generation.capture_regexes.len(), || None);
        let actual_scratch = matcher_scratch_capacity(
            program_scratch_bytes,
            group_scratch.capacity(),
            boolean_scratch.capacity(),
            capture_scratch.capacity(),
        )?;
        if actual_scratch > limits.max_matcher_scratch_bytes {
            return Err(RuntimeError::MatcherScratchByteLimit {
                limit: limits.max_matcher_scratch_bytes,
            });
        }
        if request
            .retained_bytes
            .checked_add(actual_scratch)
            .ok_or(RuntimeError::Arithmetic)?
            > limits.max_retained_bytes
        {
            return Err(RuntimeError::RetainedByteLimit {
                limit: limits.max_retained_bytes,
            });
        }
        let scratch_bytes = actual_scratch;
        Ok(Self {
            ordinary_scan_cache: OrdinaryScanCache::new(),
            borrowed_transform_memo: BorrowedTransformMemo::default(),
            generation,
            request,
            json_budget: None,
            json_failure: None,
            next_phase: Phase::RequestHeaders as u8,
            limits,
            group_scratch,
            boolean_scratch,
            capture_scratch,
            scratch_bytes,
            selected_scratch: Vec::new(),
            match_scratch: Vec::new(),
            match_scratch_bytes: 0,
            detector_work_bytes: 0,
            unit_work_bytes: 0,
            ctl_work_bytes: 0,
            tx: TxState {
                check_policy,
                ..TxState::default()
            },
            tx_journal: TxJournal::default(),
            capture_keys: Vec::new(),
            response: None,
            request_body_error: None,
            request_body_cutoff: false,
            evaluation_mode,
            #[cfg(test)]
            force_json_cursor: false,
            #[cfg(test)]
            force_generic_selection: false,
            rule_engine_mode,
            request_body_stream: None,
            stream_member_matches: None,
            request_prefix_completion: None,
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/seclang/transaction/mod_test.rs"]
mod tests;

#[cfg(test)]
use tests::matcher_scratch_bytes;

#[path = "../json_inspection.rs"]
mod json_inspection;
#[path = "../json_inspection_collector.rs"]
mod json_inspection_collector;
#[path = "../json_inspection_runtime.rs"]
mod json_inspection_runtime;

#[path = "../xml_inspection_runtime.rs"]
mod xml_inspection_runtime;
pub use xml_inspection_runtime::{XmlInspectionLimits, XmlInspectionSession, XmlInspectionUsage};

pub(super) fn integer_relation(relation: super::IntegerRelation, actual: i64, expected: i64) -> bool {
    match relation {
        super::IntegerRelation::Equal => actual == expected,
        super::IntegerRelation::GreaterOrEqual => actual >= expected,
        super::IntegerRelation::Greater => actual > expected,
        super::IntegerRelation::Less => actual < expected,
    }
}
