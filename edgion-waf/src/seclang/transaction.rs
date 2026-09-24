use std::borrow::Cow;
use std::mem::size_of;
use std::ops::Deref;
use std::sync::Arc;

use thiserror::Error;

use crate::matcher::{
    MatchScratch, MatcherScanError, MatcherStreamState, RegexBooleanScratch, RegexCaptureScratch, RegexScratchError,
};

use super::super::body::{
    build_fallback_publication, build_publication, processor_from_content_type, BodyError, BodyLimits, BodyPublication,
    RequestBody,
};
use super::super::transform::{
    apply_transformations, apply_transformations_multimatch, TransformError, TransformLimits,
};
use super::{CollectionKind, CompiledGeneration, Phase};

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
pub const DEFAULT_MAX_RUNTIME_CAPTURE_ENTRIES: usize = 64;
pub const DEFAULT_MAX_RUNTIME_CAPTURE_BYTES: usize = 256 * 1024;
pub const DEFAULT_MAX_RUNTIME_RESPONSE_HEADERS: usize = 1_024;
pub const DEFAULT_MAX_RUNTIME_RESPONSE_HEADER_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_RUNTIME_RESPONSE_WINDOW_BYTES: usize = 256 * 1024;

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
    #[error("runtime integer conversion failed")]
    IntegerConversion,
    #[error("compiled generation contains an invalid runtime reference")]
    GenerationInvariant,
    #[error("matcher execution failed with bounded category {kind:?}")]
    Matcher { kind: RuntimeMatcherError },
    #[error("transformation execution failed with bounded category {kind:?}")]
    Transformation { kind: RuntimeTransformError },
    #[error("runtime action {kind:?} is not implemented by the current transaction slice")]
    RuntimeActionUnavailable { kind: super::super::ActionKind },
    #[error("runtime macro source {kind:?} is not implemented by the current transaction slice")]
    RuntimeMacroUnavailable { kind: super::MacroSource },
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
    severity: Option<super::super::Severity>,
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

    pub const fn severity(self) -> Option<super::super::Severity> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: usize,
    len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CollectionValue {
    name: Option<ByteRange>,
    value: ByteRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectedValue {
    collection: CollectionKind,
    name: Option<SelectedName>,
    value: SelectedBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectedBytes {
    Request(ByteRange),
    Body(usize),
    Response(usize),
    ResponseWindow,
    Tx(usize),
    MatchedVar,
    MatchedVars(usize),
    Count { digits: [u8; 40], start: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectedName {
    Request(ByteRange),
    Body(usize),
    Response(usize),
    Tx(usize),
    MatchedVars(usize),
}

impl SelectedValue {
    fn bytes<'value>(
        &'value self,
        request: &'value RequestState,
        tx: &'value TxState,
        response: Option<&'value ResponseState>,
    ) -> Result<&'value [u8], RuntimeError> {
        match &self.value {
            SelectedBytes::Request(range) => Ok(request.bytes_at(*range)),
            SelectedBytes::Body(index) => request
                .body
                .as_ref()
                .and_then(|body| body.entries.get(*index))
                .map(|entry| entry.value.as_slice())
                .ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::Response(index) => response
                .and_then(|response| response.entries.get(*index))
                .map(|entry| entry.value.as_slice())
                .ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::ResponseWindow => response
                .and_then(|response| response.window.as_deref())
                .ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::Tx(index) => tx
                .entries
                .get(*index)
                .map(|entry| entry.value.as_slice())
                .ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::MatchedVar => tx.matched_var.as_deref().ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::MatchedVars(index) => tx
                .matched_vars
                .get(*index)
                .map(|entry| entry.value.as_slice())
                .ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::Count { digits, start } => Ok(&digits[usize::from(*start)..]),
        }
    }

    fn name_bytes<'value>(
        self,
        request: &'value RequestState,
        tx: &'value TxState,
        response: Option<&'value ResponseState>,
    ) -> Result<Option<&'value [u8]>, RuntimeError> {
        selected_name_bytes(self.name, request, tx, response)
    }
}

fn selected_name_bytes<'value>(
    name: Option<SelectedName>,
    request: &'value RequestState,
    tx: &'value TxState,
    response: Option<&'value ResponseState>,
) -> Result<Option<&'value [u8]>, RuntimeError> {
    name.map(|name| match name {
        SelectedName::Request(range) => Ok(request.bytes_at(range)),
        SelectedName::Body(index) => request
            .body
            .as_ref()
            .and_then(|body| body.entries.get(index))
            .and_then(|entry| entry.name.as_deref())
            .ok_or(RuntimeError::GenerationInvariant),
        SelectedName::Response(index) => response
            .and_then(|response| response.entries.get(index))
            .and_then(|entry| entry.name.as_deref())
            .ok_or(RuntimeError::GenerationInvariant),
        SelectedName::Tx(index) => tx
            .entries
            .get(index)
            .map(|entry| entry.key.as_slice())
            .ok_or(RuntimeError::GenerationInvariant),
        SelectedName::MatchedVars(index) => tx
            .matched_vars
            .get(index)
            .map(|entry| entry.key.as_slice())
            .ok_or(RuntimeError::GenerationInvariant),
    })
    .transpose()
}

const COLLECTION_COUNT: usize = CollectionKind::ALL.len();

struct RequestState {
    bytes: Vec<u8>,
    collections: [Vec<CollectionValue>; COLLECTION_COUNT],
    body: Option<BodyPublication>,
    entry_count: usize,
    retained_bytes: usize,
}

struct ResponseEntry {
    collection: CollectionKind,
    name: Option<Vec<u8>>,
    value: Vec<u8>,
}

struct ResponseState {
    entries: Vec<ResponseEntry>,
    window: Option<Vec<u8>>,
    retained_bytes: usize,
}

struct TxEntry {
    key: Vec<u8>,
    value: Vec<u8>,
}

#[derive(Default)]
struct TxState {
    entries: Vec<TxEntry>,
    matched_var: Option<Vec<u8>>,
    matched_var_name: Option<Vec<u8>>,
    matched_vars: Vec<TxEntry>,
    retained_bytes: usize,
}

struct CaptureEntry {
    key: Vec<u8>,
    value: Vec<u8>,
}

#[derive(Default)]
struct MemberMatch {
    captures: Vec<CaptureEntry>,
    matched_var: Option<Vec<u8>>,
    matched_var_name: Option<Vec<u8>>,
}

struct ChainBranch {
    tx: TxState,
    capture_keys: Vec<Vec<u8>>,
    setvar_actions: usize,
}

enum GenerationHandle<'generation> {
    Borrowed(&'generation CompiledGeneration),
    Shared(Arc<CompiledGeneration>),
}

impl Deref for GenerationHandle<'_> {
    type Target = CompiledGeneration;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(generation) => generation,
            Self::Shared(generation) => generation,
        }
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
    generation: GenerationHandle<'generation>,
    request: RequestState,
    next_phase: u8,
    limits: RuntimeLimits,
    group_scratch: Vec<MatchScratch>,
    boolean_scratch: Vec<RegexBooleanScratch>,
    capture_scratch: Vec<RegexCaptureScratch>,
    scratch_bytes: usize,
    detector_work_bytes: usize,
    tx: TxState,
    capture_keys: Vec<Vec<u8>>,
    response: Option<ResponseState>,
    request_body_error: Option<BodyError>,
    evaluation_mode: EvaluationMode,
    rule_engine_mode: RuleEngineMode,
    request_body_stream: Option<RequestBodyStreamState>,
    stream_member_matches: Option<Vec<Option<bool>>>,
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
        Self::new_shared_with_modes(
            generation,
            request,
            limits,
            EvaluationMode::Accelerated,
            RuleEngineMode::On,
        )
    }

    /// Create a shared-generation transaction with explicit execution modes.
    pub fn new_shared_with_modes(
        generation: Arc<CompiledGeneration>,
        request: RequestHead<'_>,
        limits: RuntimeLimits,
        evaluation_mode: EvaluationMode,
        rule_engine_mode: RuleEngineMode,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_handle(
            GenerationHandle::Shared(generation),
            request,
            limits,
            evaluation_mode,
            rule_engine_mode,
        )
    }
}

impl<'generation> Transaction<'generation> {
    pub fn new(
        generation: &'generation CompiledGeneration,
        request: RequestHead<'_>,
        limits: RuntimeLimits,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_evaluation(generation, request, limits, EvaluationMode::Accelerated)
    }

    /// Create a transaction with an explicit matcher evaluation path.
    pub fn new_with_evaluation(
        generation: &'generation CompiledGeneration,
        request: RequestHead<'_>,
        limits: RuntimeLimits,
        evaluation_mode: EvaluationMode,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_modes(generation, request, limits, evaluation_mode, RuleEngineMode::On)
    }

    /// Create a transaction with explicit matcher and disruptive-action modes.
    pub fn new_with_modes(
        generation: &'generation CompiledGeneration,
        request: RequestHead<'_>,
        limits: RuntimeLimits,
        evaluation_mode: EvaluationMode,
        rule_engine_mode: RuleEngineMode,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_handle(
            GenerationHandle::Borrowed(generation),
            request,
            limits,
            evaluation_mode,
            rule_engine_mode,
        )
    }

    fn new_with_handle(
        generation: GenerationHandle<'generation>,
        request: RequestHead<'_>,
        limits: RuntimeLimits,
        evaluation_mode: EvaluationMode,
        rule_engine_mode: RuleEngineMode,
    ) -> Result<Self, RuntimeError> {
        let request = RequestState::build(request, limits)?;
        let scratch_bytes = matcher_scratch_bytes(&generation)?;
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
            .try_reserve_exact(generation.groups.len())
            .map_err(|_| RuntimeError::Allocation)?;
        for group in &generation.groups {
            group_scratch.push(group.matcher.create_scratch());
        }
        let mut boolean_scratch = Vec::new();
        boolean_scratch
            .try_reserve_exact(generation.boolean_regexes.len())
            .map_err(|_| RuntimeError::Allocation)?;
        for program in &generation.boolean_regexes {
            boolean_scratch.push(program.create_scratch());
        }
        let mut capture_scratch = Vec::new();
        capture_scratch
            .try_reserve_exact(generation.capture_regexes.len())
            .map_err(|_| RuntimeError::Allocation)?;
        for program in &generation.capture_regexes {
            capture_scratch.push(program.create_scratch());
        }
        Ok(Self {
            generation,
            request,
            next_phase: Phase::RequestHeaders as u8,
            limits,
            group_scratch,
            boolean_scratch,
            capture_scratch,
            scratch_bytes,
            detector_work_bytes: 0,
            tx: TxState::default(),
            capture_keys: Vec::new(),
            response: None,
            request_body_error: None,
            evaluation_mode,
            rule_engine_mode,
            request_body_stream: None,
            stream_member_matches: None,
            request_prefix_completion: None,
        })
    }

    pub fn generation(&self) -> &CompiledGeneration {
        &self.generation
    }

    pub fn retained_bytes(&self) -> usize {
        self.request
            .retained_bytes
            .saturating_add(self.scratch_bytes)
            .saturating_add(self.tx.retained_bytes)
            .saturating_add(self.response.as_ref().map_or(0, ResponseState::total_retained_bytes))
            .saturating_add(
                self.request_body_stream
                    .as_ref()
                    .map_or(0, |state| state.retained_bytes),
            )
    }

    pub fn next_phase(&self) -> Option<Phase> {
        phase_from_number(self.next_phase)
    }

    pub fn tx_value(&self, key: &[u8]) -> Option<&[u8]> {
        self.tx.get(key)
    }

    pub const fn request_body_error(&self) -> Option<BodyError> {
        self.request_body_error
    }

    pub fn execute_request_headers(&mut self) -> Result<PhaseOutcome, RuntimeError> {
        self.execute_phase(Phase::RequestHeaders)
    }

    pub fn publish_request_body(&mut self, body: RequestBody<'_>) -> Result<(), RuntimeError> {
        if self.next_phase() != Some(Phase::RequestBody) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::RequestBody,
            });
        }
        if self.request.body.is_some() {
            return Err(RuntimeError::RequestBodyAlreadyPublished);
        }
        let content_type = self
            .request
            .first_named(CollectionKind::RequestHeaders, b"content-type", true);
        let head_args_combined_size = self
            .request
            .first_value(CollectionKind::ArgsCombinedSize)
            .ok_or(RuntimeError::GenerationInvariant)
            .and_then(parse_usize)?;
        let (publication, body_error) =
            match build_publication(body, content_type, head_args_combined_size, self.limits.body) {
                Ok(publication) => (publication, None),
                Err(error @ (BodyError::Malformed { .. } | BodyError::MultipartBoundary))
                    if self.rule_engine_mode == RuleEngineMode::DetectionOnly =>
                {
                    (
                        build_fallback_publication(body, content_type, head_args_combined_size, self.limits.body)
                            .map_err(|kind| RuntimeError::Body { kind })?,
                        Some(error),
                    )
                }
                Err(kind) => return Err(RuntimeError::Body { kind }),
            };
        let entry_count = self
            .request
            .entry_count
            .checked_add(publication.entries.len())
            .ok_or(RuntimeError::Arithmetic)?;
        if entry_count > self.limits.max_collection_entries {
            return Err(RuntimeError::CollectionEntryLimit {
                limit: self.limits.max_collection_entries,
            });
        }
        let request_retained = self
            .request
            .retained_bytes
            .checked_add(publication.retained_bytes)
            .ok_or(RuntimeError::Arithmetic)?;
        let total_retained = request_retained
            .checked_add(self.scratch_bytes)
            .and_then(|value| value.checked_add(self.tx.retained_bytes))
            .ok_or(RuntimeError::Arithmetic)?;
        if total_retained > self.limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        self.request.body = Some(publication);
        self.request_body_error = body_error;
        self.request.entry_count = entry_count;
        self.request.retained_bytes = request_retained;
        Ok(())
    }

    pub fn execute_request_body(&mut self) -> Result<PhaseOutcome, RuntimeError> {
        if self.request.body.is_none() {
            return Err(RuntimeError::RequestBodyNotPublished);
        }
        self.execute_phase(Phase::RequestBody)
    }

    /// Decide phase two over borrowed prefix bytes without publishing a complete body.
    pub fn execute_request_body_prefix(
        &mut self,
        prefix: &[u8],
        limit: usize,
        completion: RequestBodyPrefixCompletion,
    ) -> Result<PhaseOutcome, RuntimeError> {
        if self.next_phase() != Some(Phase::RequestBody) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::RequestBody,
            });
        }
        self.generation
            .validate_request_body_prefix()
            .map_err(|_| RuntimeError::GenerationInvariant)?;
        if self.request.body.is_some()
            || self.request_body_stream.is_some()
            || limit == 0
            || limit > MAX_REQUEST_BODY_PREFIX_BYTES
            || prefix.len() > limit
            || (completion == RequestBodyPrefixCompletion::WindowFilled && prefix.len() != limit)
            || (completion == RequestBodyPrefixCompletion::NotRequired
                && (!prefix.is_empty() || self.generation.requires_complete_request_body()))
        {
            return Err(RuntimeError::GenerationInvariant);
        }
        if prefix.len() > self.limits.body.max_body_bytes {
            return Err(RuntimeError::Body {
                kind: BodyError::ByteLimit {
                    actual: prefix.len(),
                    limit: self.limits.body.max_body_bytes,
                },
            });
        }
        if completion == RequestBodyPrefixCompletion::NotRequired {
            let outcome = self.execute_phase(Phase::RequestBody)?;
            self.request_prefix_completion = Some(completion);
            return Ok(outcome);
        }
        let outcome = self.execute_phase_with_prefix(Phase::RequestBody, true, Some(prefix));
        if outcome.is_ok() {
            self.request_prefix_completion = Some(completion);
        }
        outcome
    }

    fn evaluate_prefix_member(&mut self, id: usize, prefix: &[u8], work: &mut usize) -> Result<bool, RuntimeError> {
        let member = self.generation.members[id];
        let transformations = member
            .transformation_plan
            .map(|id| {
                let plan = self.generation.transformation_plans[id];
                &self.generation.transformations
                    [plan.transformation_start..plan.transformation_start + plan.transformation_count]
            })
            .unwrap_or_default();
        let transformed = if transformations.is_empty() {
            if prefix.len() > self.limits.max_transformation_output_bytes {
                return Err(map_transform_error(TransformError::OutputLimit {
                    limit: self.limits.max_transformation_output_bytes,
                }));
            }
            Cow::Borrowed(prefix)
        } else {
            Cow::Owned(
                apply_transformations(
                    prefix,
                    transformations,
                    TransformLimits {
                        max_output_bytes: self.limits.max_transformation_output_bytes,
                        max_work_bytes: self.limits.max_transformation_work_bytes,
                    },
                )
                .map_err(map_transform_error)?,
            )
        };
        *work = work
            .checked_add(prefix.len())
            .and_then(|work| work.checked_add(transformed.len()))
            .ok_or(RuntimeError::Arithmetic)?;
        if *work > self.limits.max_operator_work_bytes {
            return Err(RuntimeError::OperatorWorkLimit {
                limit: self.limits.max_operator_work_bytes,
            });
        }
        evaluate_operator(
            &self.generation,
            &self.request,
            &self.tx,
            &mut self.group_scratch,
            &mut self.boolean_scratch,
            &mut self.capture_scratch,
            self.generation.operator_plans[id].ok_or(RuntimeError::GenerationInvariant)?,
            &transformed,
            member.runtime_expansion_byte_limit,
            self.evaluation_mode,
        )
        .map(|(matched, _)| matched)
    }

    pub const fn request_prefix_completion(&self) -> Option<RequestBodyPrefixCompletion> {
        self.request_prefix_completion
    }

    /// Prepare the exact constant-memory raw-body lane selected by the compiler.
    pub fn start_request_body_stream(&mut self) -> Result<(), RuntimeError> {
        if self.next_phase() != Some(Phase::RequestBody) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::RequestBody,
            });
        }
        if self.request.body.is_some() || self.request_body_stream.is_some() {
            return Err(RuntimeError::RequestBodyAlreadyPublished);
        }
        let summary = self.generation.request_body_execution_summary();
        if summary.streaming_structured_rule_units() != 0
            || summary.bounded_snapshot_rule_units() != 0
            || summary.unsupported_rule_units() != 0
        {
            return Err(RuntimeError::GenerationInvariant);
        }
        let mut groups = Vec::new();
        groups
            .try_reserve_exact(self.generation.groups.len())
            .map_err(|_| RuntimeError::Allocation)?;
        let mut retained_bytes = 0_usize;
        for group in &self.generation.groups {
            let mapping_end = group
                .mapping_start
                .checked_add(group.mapping_count)
                .ok_or(RuntimeError::Arithmetic)?;
            let mappings = self
                .generation
                .pattern_mappings
                .get(group.mapping_start..mapping_end)
                .ok_or(RuntimeError::GenerationInvariant)?;
            let streaming = group.key.phase == Phase::RequestBody
                && !mappings.is_empty()
                && mappings
                    .iter()
                    .all(|mapping| super::is_streaming_raw_member(&self.generation, mapping.member));
            if streaming {
                let state = group.matcher.create_stream_state().map_err(map_matcher_error)?;
                retained_bytes = retained_bytes
                    .checked_add(state.memory_usage())
                    .ok_or(RuntimeError::Arithmetic)?;
                groups.push(Some(state));
            } else {
                groups.push(None);
            }
        }
        let member_positive = vec![false; self.generation.members.len()];
        let mut phase_two_units = self.generation.schedules[phase_index(Phase::RequestBody)]
            .iter()
            .filter_map(|entry| match entry {
                super::CompiledScheduleEntry::RuleUnit(unit_index) => Some(*unit_index),
                super::CompiledScheduleEntry::MarkerSentinel(_) => None,
            });
        let early_positive_member = phase_two_units
            .next()
            .and_then(|unit_index| phase_two_units.next().is_none().then_some(unit_index));
        let early_positive_member = early_positive_member.and_then(|unit_index| {
            let unit = self.generation.units.get(unit_index)?;
            if unit.member_count != 1 || !super::is_streaming_raw_member(&self.generation, unit.member_start) {
                return None;
            }
            self.generation
                .operator_plans
                .get(unit.member_start)
                .and_then(|plan| *plan)
                .is_some_and(|plan| !plan.negated)
                .then_some(unit.member_start)
        });
        retained_bytes = retained_bytes
            .checked_add(member_positive.capacity().saturating_mul(size_of::<bool>()))
            .and_then(|value| {
                value.checked_add(
                    groups
                        .capacity()
                        .saturating_mul(size_of::<Option<MatcherStreamState>>()),
                )
            })
            .ok_or(RuntimeError::Arithmetic)?;
        if self.retained_bytes().saturating_add(retained_bytes) > self.limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        self.request_body_stream = Some(RequestBodyStreamState {
            groups,
            member_positive,
            consumed_bytes: 0,
            operator_work_bytes: 0,
            early_positive_member,
            retained_bytes,
        });
        Ok(())
    }

    /// Scan one raw request-body chunk. `Some` is returned exactly once at EOS.
    pub fn execute_request_body_stream_chunk(
        &mut self,
        chunk: &[u8],
        end_of_stream: bool,
    ) -> Result<Option<PhaseOutcome>, RuntimeError> {
        let mut early_finalize = false;
        {
            let stream = self
                .request_body_stream
                .as_mut()
                .ok_or(RuntimeError::RequestBodyNotPublished)?;
            let consumed_bytes = stream
                .consumed_bytes
                .checked_add(chunk.len())
                .ok_or(RuntimeError::Arithmetic)?;
            if consumed_bytes > self.limits.body.max_body_bytes {
                return Err(RuntimeError::Body {
                    kind: BodyError::ByteLimit {
                        actual: consumed_bytes,
                        limit: self.limits.body.max_body_bytes,
                    },
                });
            }
            stream.consumed_bytes = consumed_bytes;
            for (group_index, state) in stream.groups.iter_mut().enumerate() {
                let Some(state) = state else { continue };
                stream.operator_work_bytes = stream
                    .operator_work_bytes
                    .checked_add(chunk.len())
                    .ok_or(RuntimeError::Arithmetic)?;
                if stream.operator_work_bytes > self.limits.max_operator_work_bytes {
                    return Err(RuntimeError::OperatorWorkLimit {
                        limit: self.limits.max_operator_work_bytes,
                    });
                }
                let group = self
                    .generation
                    .groups
                    .get(group_index)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                let results = group
                    .matcher
                    .scan_stream_chunk(state, chunk, end_of_stream)
                    .map_err(map_matcher_error)?;
                for pattern_id in results.iter() {
                    let mapping_index = group
                        .mapping_start
                        .checked_add(pattern_id.as_usize())
                        .ok_or(RuntimeError::Arithmetic)?;
                    let mapping = self
                        .generation
                        .pattern_mappings
                        .get(mapping_index)
                        .ok_or(RuntimeError::GenerationInvariant)?;
                    let matched = stream
                        .member_positive
                        .get_mut(mapping.member)
                        .ok_or(RuntimeError::GenerationInvariant)?;
                    *matched = true;
                    early_finalize |= stream.early_positive_member == Some(mapping.member);
                }
            }
        }
        if !end_of_stream && !early_finalize {
            return Ok(None);
        }
        let stream = self
            .request_body_stream
            .take()
            .ok_or(RuntimeError::RequestBodyNotPublished)?;
        let mut overrides = vec![None; self.generation.members.len()];
        for (member_index, positive) in stream.member_positive.into_iter().enumerate() {
            if !super::is_streaming_raw_member(&self.generation, member_index) {
                continue;
            }
            let operator = self
                .generation
                .operator_plans
                .get(member_index)
                .and_then(|plan| *plan)
                .ok_or(RuntimeError::GenerationInvariant)?;
            overrides[member_index] = Some(positive != operator.negated);
        }
        self.stream_member_matches = Some(overrides);
        self.publish_request_body(RequestBody::new(&[], super::super::BodyProcessorSelection::Raw))?;
        let outcome = self.execute_request_body();
        self.stream_member_matches = None;
        outcome.map(Some)
    }

    pub fn publish_response_head(&mut self, response: ResponseHead<'_>) -> Result<(), RuntimeError> {
        if self.next_phase() != Some(Phase::ResponseHeaders) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::ResponseHeaders,
            });
        }
        if self.response.is_some() {
            return Err(RuntimeError::ResponseAlreadyPublished);
        }
        let response = ResponseState::build(response, self.limits)?;
        let total_retained = self
            .retained_bytes()
            .checked_add(response.total_retained_bytes())
            .ok_or(RuntimeError::Arithmetic)?;
        if total_retained > self.limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        self.response = Some(response);
        Ok(())
    }

    pub fn execute_response_headers(&mut self) -> Result<PhaseOutcome, RuntimeError> {
        if self.response.is_none() {
            return Err(RuntimeError::ResponseNotPublished);
        }
        self.execute_phase(Phase::ResponseHeaders)
    }

    pub fn execute_response_window(&mut self, window: ResponseWindow<'_>) -> Result<PhaseOutcome, RuntimeError> {
        if self.next_phase() != Some(Phase::ResponseBody) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::ResponseBody,
            });
        }
        if window.bytes.len() > self.limits.max_response_window_bytes {
            return Err(RuntimeError::ResponseWindowByteLimit {
                actual: window.bytes.len(),
                limit: self.limits.max_response_window_bytes,
            });
        }
        if self.response.is_none() {
            return Err(RuntimeError::ResponseNotPublished);
        }
        let bytes = owned_copy(window.bytes)?;
        let total_retained = self
            .retained_bytes()
            .checked_add(bytes.len())
            .ok_or(RuntimeError::Arithmetic)?;
        if total_retained > self.limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        self.response.as_mut().ok_or(RuntimeError::ResponseNotPublished)?.window = Some(bytes);
        let result = self.execute_phase_with_advance(Phase::ResponseBody, window.end_of_stream);
        if let Some(response) = &mut self.response {
            response.window = None;
        }
        result
    }

    pub fn execute_logging(&mut self) -> Result<PhaseOutcome, RuntimeError> {
        self.execute_phase(Phase::Logging)
    }

    fn execute_phase(&mut self, phase: Phase) -> Result<PhaseOutcome, RuntimeError> {
        self.execute_phase_with_advance(phase, true)
    }

    fn execute_phase_with_advance(&mut self, phase: Phase, advance: bool) -> Result<PhaseOutcome, RuntimeError> {
        self.execute_phase_with_prefix(phase, advance, None)
    }

    fn execute_phase_with_prefix(
        &mut self,
        phase: Phase,
        advance: bool,
        prefix: Option<&[u8]>,
    ) -> Result<PhaseOutcome, RuntimeError> {
        let mut prefix_work = 0_usize;
        let expected = self.next_phase();
        if expected != Some(phase) {
            return Err(RuntimeError::PhaseOrder {
                expected,
                actual: phase,
            });
        }
        let mut outcome = PhaseOutcome {
            findings: Vec::new(),
            omitted_findings: 0,
            omitted_loggable_findings: 0,
            intervention: None,
            evaluated_units: 0,
        };
        let schedule_index = phase_index(phase);
        let schedule_len = self.generation.schedules[schedule_index].len();
        let mut schedule_ordinal = 0_usize;
        while schedule_ordinal < schedule_len {
            let entry = self.generation.schedules[schedule_index][schedule_ordinal];
            schedule_ordinal = schedule_ordinal.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
            let super::CompiledScheduleEntry::RuleUnit(unit_id) = entry else {
                continue;
            };
            outcome.evaluated_units = outcome.evaluated_units.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
            if outcome.evaluated_units > self.limits.max_operator_evaluations {
                return Err(RuntimeError::OperatorEvaluationLimit {
                    limit: self.limits.max_operator_evaluations,
                });
            }
            let prefix_match = if let Some(prefix) = prefix {
                let unit = self
                    .generation
                    .units
                    .get(unit_id)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                if unit.member_count != 1 {
                    return Err(RuntimeError::GenerationInvariant);
                }
                if self
                    .generation
                    .member_requires_request_body(self.generation.members[unit.member_start])
                {
                    Some(self.evaluate_prefix_member(unit.member_start, prefix, &mut prefix_work)?)
                } else {
                    None
                }
            } else {
                None
            };
            if self.evaluate_unit(unit_id, prefix_match)? {
                let unit = *self
                    .generation
                    .units
                    .get(unit_id)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                if outcome.findings.len() < self.limits.max_findings {
                    if outcome.findings.len() == outcome.findings.capacity() {
                        let remaining = self.limits.max_findings.saturating_sub(outcome.findings.len());
                        let additional = remaining.min(outcome.findings.capacity().max(4));
                        outcome
                            .findings
                            .try_reserve_exact(additional)
                            .map_err(|_| RuntimeError::Allocation)?;
                    }
                    let (severity, tag_count) = self.finding_metadata(unit)?;
                    outcome.findings.push(RuleFinding {
                        rule_id: unit.id,
                        log: unit.log,
                        audit_log: unit.audit_log,
                        severity,
                        tag_count,
                    });
                } else {
                    outcome.omitted_findings = outcome
                        .omitted_findings
                        .checked_add(1)
                        .ok_or(RuntimeError::Arithmetic)?;
                    if unit.log || unit.audit_log {
                        outcome.omitted_loggable_findings = outcome
                            .omitted_loggable_findings
                            .checked_add(1)
                            .ok_or(RuntimeError::Arithmetic)?;
                    }
                }
                if unit.disruptive == super::ResolvedDisruptive::Deny && self.rule_engine_mode == RuleEngineMode::On {
                    outcome.intervention = Some(Intervention {
                        rule_id: unit.id,
                        status: unit.status.unwrap_or(403),
                    });
                    break;
                }
                schedule_ordinal = self.next_schedule_ordinal(unit, schedule_index, schedule_ordinal)?;
            }
        }
        if advance {
            self.next_phase = self.next_phase.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
        }
        Ok(outcome)
    }

    fn evaluate_unit(&mut self, unit_id: usize, prefix_match: Option<bool>) -> Result<bool, RuntimeError> {
        let unit = *self
            .generation
            .units
            .get(unit_id)
            .ok_or(RuntimeError::GenerationInvariant)?;
        let end = unit
            .member_start
            .checked_add(unit.member_count)
            .ok_or(RuntimeError::Arithmetic)?;
        let mut initial = self.tx.try_clone()?;
        initial.clear_matches();
        if unit.member_count == 1 {
            let matches = match prefix_match {
                Some(matched) => Self::boolean_member_matches(matched)?,
                None => self.evaluate_member_matches(unit.member_start, &initial, false)?,
            };
            if matches.is_empty() {
                return Ok(false);
            }
            let mut capture_keys = try_clone_byte_vectors(&self.capture_keys)?;
            let mut setvar_actions = 0_usize;
            for matched in matches {
                apply_member_match(&mut initial, &mut capture_keys, matched, self.limits)?;
                self.apply_member_setvars(unit.member_start, &mut initial, &mut setvar_actions)?;
            }
            return self.commit_unit_match(unit, initial, capture_keys);
        }
        let mut frontier = Vec::new();
        frontier.try_reserve_exact(1).map_err(|_| RuntimeError::Allocation)?;
        frontier.push(ChainBranch {
            tx: initial,
            capture_keys: try_clone_byte_vectors(&self.capture_keys)?,
            setvar_actions: 0,
        });
        for member_id in unit.member_start..end {
            let final_member = member_id + 1 == end;
            let current_bytes = chain_branches_bytes(&frontier)?;
            let mut next = Vec::new();
            let mut next_bytes = 0_usize;
            for branch in &frontier {
                for matched in self.evaluate_member_matches(member_id, &branch.tx, final_member)? {
                    let mut candidate = ChainBranch {
                        tx: branch.tx.try_clone()?,
                        capture_keys: try_clone_byte_vectors(&branch.capture_keys)?,
                        setvar_actions: branch.setvar_actions,
                    };
                    apply_member_match(&mut candidate.tx, &mut candidate.capture_keys, matched, self.limits)?;
                    self.apply_member_setvars(member_id, &mut candidate.tx, &mut candidate.setvar_actions)?;
                    if final_member {
                        return self.commit_unit_match(unit, candidate.tx, candidate.capture_keys);
                    }
                    if frontier.len().saturating_add(next.len()) >= self.limits.max_operator_evaluations {
                        return Err(RuntimeError::OperatorEvaluationLimit {
                            limit: self.limits.max_operator_evaluations,
                        });
                    }
                    next_bytes = next_bytes
                        .checked_add(chain_branch_bytes(&candidate)?)
                        .ok_or(RuntimeError::Arithmetic)?;
                    if current_bytes.checked_add(next_bytes).ok_or(RuntimeError::Arithmetic)?
                        > self.limits.max_operator_work_bytes
                    {
                        return Err(RuntimeError::OperatorWorkLimit {
                            limit: self.limits.max_operator_work_bytes,
                        });
                    }
                    next.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
                    next.push(candidate);
                }
            }
            if next.is_empty() {
                return Ok(false);
            }
            frontier = next;
        }
        Ok(false)
    }

    fn commit_unit_match(
        &mut self,
        unit: super::CompiledRuleUnit,
        staged: TxState,
        capture_keys: Vec<Vec<u8>>,
    ) -> Result<bool, RuntimeError> {
        self.validate_runtime_actions(unit, &staged)?;
        let total_retained = self
            .request
            .retained_bytes
            .checked_add(self.scratch_bytes)
            .and_then(|value| value.checked_add(staged.retained_bytes))
            .and_then(|value| value.checked_add(self.response.as_ref().map_or(0, ResponseState::total_retained_bytes)))
            .ok_or(RuntimeError::Arithmetic)?;
        if total_retained > self.limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        self.tx = staged;
        self.capture_keys = capture_keys;
        Ok(true)
    }

    fn boolean_member_matches(matched: bool) -> Result<Vec<MemberMatch>, RuntimeError> {
        let mut matches = Vec::new();
        if matched {
            matches.try_reserve_exact(1).map_err(|_| RuntimeError::Allocation)?;
            matches.push(MemberMatch::default());
        }
        Ok(matches)
    }

    fn evaluate_member_matches(
        &mut self,
        member_id: usize,
        tx: &TxState,
        stop_after_first: bool,
    ) -> Result<Vec<MemberMatch>, RuntimeError> {
        if let Some(matched) = self
            .stream_member_matches
            .as_ref()
            .and_then(|members| members.get(member_id))
            .copied()
            .flatten()
        {
            return Self::boolean_member_matches(matched);
        }
        let member = *self
            .generation
            .members
            .get(member_id)
            .ok_or(RuntimeError::GenerationInvariant)?;
        let Some(operator) = self.generation.operator_plans.get(member_id).copied().flatten() else {
            let mut matches = Vec::new();
            matches.try_reserve_exact(1).map_err(|_| RuntimeError::Allocation)?;
            matches.push(MemberMatch::default());
            return Ok(matches);
        };
        let target_plan = member.target_plan.ok_or(RuntimeError::GenerationInvariant)?;
        let selected = select_values(
            &self.generation,
            &self.request,
            tx,
            self.response.as_ref(),
            &mut self.boolean_scratch,
            target_plan,
            self.limits.max_selected_values,
        )?;
        let transformations = member
            .transformation_plan
            .and_then(|id| self.generation.transformation_plans.get(id))
            .and_then(|plan| {
                let end = plan.transformation_start.checked_add(plan.transformation_count)?;
                self.generation.transformations.get(plan.transformation_start..end)
            })
            .unwrap_or_default();
        let multi_match = member_has_action(&self.generation, member, super::super::ActionKind::MultiMatch)?;
        let mut work = 0_usize;
        let mut matches = Vec::new();
        matches
            .try_reserve_exact(if stop_after_first {
                1
            } else {
                self.limits.max_selected_values.min(selected.len())
            })
            .map_err(|_| RuntimeError::Allocation)?;
        for selected in selected {
            let value = selected.bytes(&self.request, tx, self.response.as_ref())?;
            let transform_limits = TransformLimits {
                max_output_bytes: self.limits.max_transformation_output_bytes,
                max_work_bytes: self.limits.max_transformation_work_bytes,
            };
            let transformed_values = if multi_match {
                apply_transformations_multimatch(value, transformations, transform_limits)
                    .map_err(map_transform_error)?
            } else {
                let mut values = Vec::new();
                values.try_reserve_exact(1).map_err(|_| RuntimeError::Allocation)?;
                values.push(
                    apply_transformations(value, transformations, transform_limits).map_err(map_transform_error)?,
                );
                values
            };
            for transformed in transformed_values {
                let detector_work = match operator.operand {
                    super::CompiledOperatorOperand::DetectSqli { .. } => {
                        crate::injection::work_cost(crate::injection::Kind::Sqli, transformed.len())
                    }
                    super::CompiledOperatorOperand::DetectXss => {
                        crate::injection::work_cost(crate::injection::Kind::Xss, transformed.len())
                    }
                    _ => Some(0),
                }
                .ok_or(RuntimeError::DetectorInputLimit {
                    limit: crate::injection::MAX_INPUT_BYTES,
                })?;
                if detector_work != 0 {
                    self.detector_work_bytes = self
                        .detector_work_bytes
                        .checked_add(detector_work)
                        .ok_or(RuntimeError::Arithmetic)?;
                    if self.detector_work_bytes > self.limits.max_operator_work_bytes {
                        return Err(RuntimeError::OperatorWorkLimit {
                            limit: self.limits.max_operator_work_bytes,
                        });
                    }
                }
                work = work
                    .checked_add(value.len())
                    .and_then(|value| value.checked_add(transformed.len()))
                    .and_then(|value| value.checked_add(detector_work))
                    .ok_or(RuntimeError::Arithmetic)?;
                if work > self.limits.max_operator_work_bytes {
                    return Err(RuntimeError::OperatorWorkLimit {
                        limit: self.limits.max_operator_work_bytes,
                    });
                }
                let (positive, fingerprint) = evaluate_operator(
                    &self.generation,
                    &self.request,
                    tx,
                    &mut self.group_scratch,
                    &mut self.boolean_scratch,
                    &mut self.capture_scratch,
                    operator,
                    &transformed,
                    member.runtime_expansion_byte_limit,
                    self.evaluation_mode,
                )?;
                if positive != operator.negated {
                    let captures = if operator.negated {
                        Vec::new()
                    } else {
                        extract_captures(
                            &self.generation,
                            &mut self.capture_scratch,
                            operator,
                            &transformed,
                            self.limits,
                            self.evaluation_mode,
                            fingerprint,
                        )?
                    };
                    let (matched_var, matched_var_name) =
                        matched_variable(&self.request, tx, self.response.as_ref(), selected, &transformed)?;
                    if matches.len() == self.limits.max_operator_evaluations {
                        return Err(RuntimeError::OperatorEvaluationLimit {
                            limit: self.limits.max_operator_evaluations,
                        });
                    }
                    let matched = MemberMatch {
                        captures,
                        matched_var: Some(matched_var),
                        matched_var_name: Some(matched_var_name),
                    };
                    work = work
                        .checked_add(member_match_bytes(&matched)?)
                        .ok_or(RuntimeError::Arithmetic)?;
                    if work > self.limits.max_operator_work_bytes {
                        return Err(RuntimeError::OperatorWorkLimit {
                            limit: self.limits.max_operator_work_bytes,
                        });
                    }
                    matches.push(matched);
                    if stop_after_first {
                        return Ok(matches);
                    }
                    break;
                }
            }
        }
        Ok(matches)
    }

    fn validate_runtime_actions(&self, unit: super::CompiledRuleUnit, tx: &TxState) -> Result<(), RuntimeError> {
        let end = unit
            .member_start
            .checked_add(unit.member_count)
            .ok_or(RuntimeError::Arithmetic)?;
        for member in self
            .generation
            .members
            .get(unit.member_start..end)
            .ok_or(RuntimeError::GenerationInvariant)?
        {
            let action_end = member
                .action_start
                .checked_add(member.action_count)
                .ok_or(RuntimeError::Arithmetic)?;
            for action in self
                .generation
                .actions
                .get(member.action_start..action_end)
                .ok_or(RuntimeError::GenerationInvariant)?
            {
                if matches!(
                    action.kind,
                    super::ActionEnvelopeKind::Action(
                        super::super::ActionKind::LogData | super::super::ActionKind::Msg
                    )
                ) {
                    let super::CompiledActionOperand::Template(template) = action.operand else {
                        return Err(RuntimeError::GenerationInvariant);
                    };
                    let _expanded = expand_template(
                        &self.generation,
                        &self.request,
                        tx,
                        template,
                        member.runtime_expansion_byte_limit,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn finding_metadata(
        &self,
        unit: super::CompiledRuleUnit,
    ) -> Result<(Option<super::super::Severity>, usize), RuntimeError> {
        let end = unit
            .member_start
            .checked_add(unit.member_count)
            .ok_or(RuntimeError::Arithmetic)?;
        let mut severity = None;
        let mut tag_count = 0_usize;
        for member in self
            .generation
            .members
            .get(unit.member_start..end)
            .ok_or(RuntimeError::GenerationInvariant)?
        {
            let action_end = member
                .action_start
                .checked_add(member.action_count)
                .ok_or(RuntimeError::Arithmetic)?;
            for action in self
                .generation
                .actions
                .get(member.action_start..action_end)
                .ok_or(RuntimeError::GenerationInvariant)?
            {
                match (action.kind, action.operand) {
                    (
                        super::ActionEnvelopeKind::Action(super::super::ActionKind::Severity),
                        super::CompiledActionOperand::Local(super::LocalActionValue::Severity(value)),
                    ) => severity = Some(value),
                    (
                        super::ActionEnvelopeKind::Action(super::super::ActionKind::Tag),
                        super::CompiledActionOperand::Literal(_),
                    ) => tag_count = tag_count.checked_add(1).ok_or(RuntimeError::Arithmetic)?,
                    _ => {}
                }
            }
        }
        Ok((severity, tag_count))
    }

    fn next_schedule_ordinal(
        &self,
        unit: super::CompiledRuleUnit,
        schedule_index: usize,
        current: usize,
    ) -> Result<usize, RuntimeError> {
        let end = unit
            .member_start
            .checked_add(unit.member_count)
            .ok_or(RuntimeError::Arithmetic)?;
        let members = self
            .generation
            .members
            .get(unit.member_start..end)
            .ok_or(RuntimeError::GenerationInvariant)?;
        if let Some(target) = members.iter().find_map(|member| member.skip_after_marker) {
            return target.checked_add(1).ok_or(RuntimeError::Arithmetic);
        }
        let mut skip = None;
        for member in members {
            let action_end = member
                .action_start
                .checked_add(member.action_count)
                .ok_or(RuntimeError::Arithmetic)?;
            for action in self
                .generation
                .actions
                .get(member.action_start..action_end)
                .ok_or(RuntimeError::GenerationInvariant)?
            {
                if let super::CompiledActionOperand::Local(super::LocalActionValue::Skip(count)) = action.operand {
                    skip = Some(usize::try_from(count).map_err(|_| RuntimeError::Arithmetic)?);
                }
            }
        }
        let Some(mut remaining) = skip else {
            return Ok(current);
        };
        let schedule = self
            .generation
            .schedules
            .get(schedule_index)
            .ok_or(RuntimeError::GenerationInvariant)?;
        let mut cursor = current;
        while cursor < schedule.len() && remaining != 0 {
            if matches!(schedule[cursor], super::CompiledScheduleEntry::RuleUnit(_)) {
                remaining -= 1;
            }
            cursor += 1;
        }
        Ok(cursor)
    }

    fn apply_member_setvars(
        &self,
        member_id: usize,
        staged: &mut TxState,
        setvar_actions: &mut usize,
    ) -> Result<(), RuntimeError> {
        let member = *self
            .generation
            .members
            .get(member_id)
            .ok_or(RuntimeError::GenerationInvariant)?;
        let action_end = member
            .action_start
            .checked_add(member.action_count)
            .ok_or(RuntimeError::Arithmetic)?;
        for action in self
            .generation
            .actions
            .get(member.action_start..action_end)
            .ok_or(RuntimeError::GenerationInvariant)?
        {
            let super::CompiledActionOperand::SetVar(setvar_id) = action.operand else {
                continue;
            };
            *setvar_actions = (*setvar_actions).checked_add(1).ok_or(RuntimeError::Arithmetic)?;
            if *setvar_actions > self.limits.max_setvar_actions {
                return Err(RuntimeError::SetVarActionLimit {
                    limit: self.limits.max_setvar_actions,
                });
            }
            let setvar = *self
                .generation
                .setvars
                .get(setvar_id)
                .ok_or(RuntimeError::GenerationInvariant)?;
            if setvar.collection != CollectionKind::Tx {
                return Err(RuntimeError::GenerationInvariant);
            }
            let key = expand_template(
                &self.generation,
                &self.request,
                staged,
                setvar.key,
                member.runtime_expansion_byte_limit,
            )?;
            match setvar.operation {
                super::SetVarOperation::Delete => staged.delete(&key),
                super::SetVarOperation::Assign => {
                    let value = setvar_value(
                        &self.generation,
                        &self.request,
                        staged,
                        setvar,
                        member.runtime_expansion_byte_limit,
                    )?;
                    staged.set(key, value, self.limits)?;
                }
                super::SetVarOperation::Add | super::SetVarOperation::Subtract => {
                    let current = staged.get(&key).map_or(Ok(0_i64), parse_i64)?;
                    let value = setvar_value(
                        &self.generation,
                        &self.request,
                        staged,
                        setvar,
                        member.runtime_expansion_byte_limit,
                    )?;
                    let delta = parse_i64(&value)?;
                    let result = if setvar.operation == super::SetVarOperation::Add {
                        current.checked_add(delta)
                    } else {
                        current.checked_sub(delta)
                    }
                    .ok_or(RuntimeError::IntegerConversion)?;
                    let (digits, start) = signed_decimal_bytes(result);
                    staged.set(key, owned_copy(&digits[start..])?, self.limits)?;
                }
            }
        }
        Ok(())
    }
}

fn extract_captures(
    generation: &CompiledGeneration,
    scratch: &mut [RegexCaptureScratch],
    plan: super::CompiledOperatorPlan,
    input: &[u8],
    limits: RuntimeLimits,
    evaluation_mode: EvaluationMode,
    fingerprint: Option<crate::injection::Fingerprint>,
) -> Result<Vec<CaptureEntry>, RuntimeError> {
    if !matches!(
        plan.operand,
        super::CompiledOperatorOperand::Regex(super::RegexPlanRef::Capture(_))
            | super::CompiledOperatorOperand::CapturePhrase(_)
            | super::CompiledOperatorOperand::DetectSqli { capture: true }
    ) {
        return Ok(Vec::new());
    }
    let mut captures = Vec::new();
    captures
        .try_reserve_exact(limits.max_capture_entries.min(64))
        .map_err(|_| RuntimeError::Allocation)?;
    match plan.operand {
        super::CompiledOperatorOperand::Regex(super::RegexPlanRef::Capture(program)) => {
            let regex = generation
                .capture_regexes
                .get(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            let scratch = scratch.get_mut(program).ok_or(RuntimeError::GenerationInvariant)?;
            let results = regex.captures(input, scratch).map_err(map_regex_scratch_error)?;
            for index in 0..regex.capture_len() {
                let Some(range) = results.group(index) else {
                    continue;
                };
                let value = input.get(range).ok_or(RuntimeError::GenerationInvariant)?;
                let (digits, start) = decimal_bytes(index);
                push_capture(&mut captures, &digits[start..], value, limits)?;
                if let Some(name) = regex.capture_name(index) {
                    push_capture(&mut captures, name.as_bytes(), value, limits)?;
                }
            }
        }
        super::CompiledOperatorOperand::CapturePhrase(program) => {
            let matcher = generation
                .capture_phrases
                .get(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            let matched = match evaluation_mode {
                EvaluationMode::Accelerated => matcher.find(input),
                EvaluationMode::Reference => matcher.find_reference(input),
            }
            .ok_or(RuntimeError::GenerationInvariant)?;
            let value = input
                .get(matched.start()..matched.end())
                .ok_or(RuntimeError::GenerationInvariant)?;
            push_capture(&mut captures, b"0", value, limits)?;
        }
        super::CompiledOperatorOperand::DetectSqli { capture: true } => {
            let fingerprint = fingerprint.ok_or(RuntimeError::GenerationInvariant)?;
            push_capture(&mut captures, b"0", fingerprint.as_bytes(), limits)?;
        }
        _ => {}
    }
    Ok(captures)
}

fn push_capture(
    captures: &mut Vec<CaptureEntry>,
    key: &[u8],
    value: &[u8],
    limits: RuntimeLimits,
) -> Result<(), RuntimeError> {
    if captures.len() == limits.max_capture_entries {
        return Err(RuntimeError::CaptureEntryLimit {
            limit: limits.max_capture_entries,
        });
    }
    let current = captures.iter().try_fold(0_usize, |total, capture| {
        total
            .checked_add(capture.key.len())
            .and_then(|value| value.checked_add(capture.value.len()))
            .ok_or(RuntimeError::Arithmetic)
    })?;
    let actual = current
        .checked_add(key.len())
        .and_then(|total| total.checked_add(value.len()))
        .ok_or(RuntimeError::Arithmetic)?;
    if actual > limits.max_capture_bytes {
        return Err(RuntimeError::CaptureByteLimit {
            limit: limits.max_capture_bytes,
        });
    }
    captures.push(CaptureEntry {
        key: owned_copy(key)?,
        value: owned_copy(value)?,
    });
    Ok(())
}

fn matched_variable(
    request: &RequestState,
    tx: &TxState,
    response: Option<&ResponseState>,
    selected: SelectedValue,
    transformed: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), RuntimeError> {
    let value = owned_copy(transformed)?;
    let collection = selected.collection.as_str().as_bytes();
    let selected_name = selected.name_bytes(request, tx, response)?;
    let name_len = selected_name.map_or(0, |name| name.len().saturating_add(1));
    let capacity = collection.len().checked_add(name_len).ok_or(RuntimeError::Arithmetic)?;
    let mut name = Vec::new();
    name.try_reserve_exact(capacity).map_err(|_| RuntimeError::Allocation)?;
    name.extend_from_slice(collection);
    if let Some(selected_name) = selected_name {
        name.push(b':');
        name.extend_from_slice(selected_name);
    }
    Ok((value, name))
}

fn apply_member_match(
    staged: &mut TxState,
    capture_keys: &mut Vec<Vec<u8>>,
    matched: MemberMatch,
    limits: RuntimeLimits,
) -> Result<(), RuntimeError> {
    if let (Some(value), Some(name)) = (&matched.matched_var, &matched.matched_var_name) {
        if staged.matched_vars.len() == limits.max_tx_variables {
            return Err(RuntimeError::TxVariableLimit {
                limit: limits.max_tx_variables,
            });
        }
        staged
            .matched_vars
            .try_reserve(1)
            .map_err(|_| RuntimeError::Allocation)?;
        staged.matched_vars.push(TxEntry {
            key: owned_copy(name)?,
            value: owned_copy(value)?,
        });
    }
    staged.matched_var = matched.matched_var;
    staged.matched_var_name = matched.matched_var_name;
    staged.recount();
    if staged.retained_bytes > limits.max_tx_bytes {
        return Err(RuntimeError::TxByteLimit {
            limit: limits.max_tx_bytes,
        });
    }
    if matched.captures.is_empty() {
        return Ok(());
    }
    for key in capture_keys.iter() {
        staged.delete(key);
    }
    capture_keys.clear();
    capture_keys
        .try_reserve_exact(matched.captures.len())
        .map_err(|_| RuntimeError::Allocation)?;
    for capture in matched.captures {
        let stored_key = owned_copy(&capture.key)?;
        staged.set(capture.key, capture.value, limits)?;
        capture_keys.push(stored_key);
    }
    Ok(())
}

fn try_clone_byte_vectors(values: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, RuntimeError> {
    let mut cloned = Vec::new();
    cloned
        .try_reserve_exact(values.len())
        .map_err(|_| RuntimeError::Allocation)?;
    for value in values {
        cloned.push(owned_copy(value)?);
    }
    Ok(cloned)
}

fn chain_branch_bytes(branch: &ChainBranch) -> Result<usize, RuntimeError> {
    branch.capture_keys.iter().try_fold(
        branch
            .capture_keys
            .len()
            .checked_mul(size_of::<Vec<u8>>())
            .and_then(|value| value.checked_add(branch.tx.retained_bytes))
            .ok_or(RuntimeError::Arithmetic)?,
        |total, key| total.checked_add(key.len()).ok_or(RuntimeError::Arithmetic),
    )
}

fn member_match_bytes(matched: &MemberMatch) -> Result<usize, RuntimeError> {
    let capture_bytes = matched.captures.iter().try_fold(
        matched
            .captures
            .len()
            .checked_mul(size_of::<CaptureEntry>())
            .ok_or(RuntimeError::Arithmetic)?,
        |total, capture| {
            total
                .checked_add(capture.key.len())
                .and_then(|value| value.checked_add(capture.value.len()))
                .ok_or(RuntimeError::Arithmetic)
        },
    )?;
    capture_bytes
        .checked_add(matched.matched_var.as_ref().map_or(0, Vec::len))
        .and_then(|value| value.checked_add(matched.matched_var_name.as_ref().map_or(0, Vec::len)))
        .and_then(|value| value.checked_add(size_of::<MemberMatch>()))
        .ok_or(RuntimeError::Arithmetic)
}

fn chain_branches_bytes(branches: &[ChainBranch]) -> Result<usize, RuntimeError> {
    branches.iter().try_fold(
        branches
            .len()
            .checked_mul(size_of::<ChainBranch>())
            .ok_or(RuntimeError::Arithmetic)?,
        |total, branch| {
            total
                .checked_add(chain_branch_bytes(branch)?)
                .ok_or(RuntimeError::Arithmetic)
        },
    )
}

impl TxState {
    fn clear_matches(&mut self) {
        self.matched_var = None;
        self.matched_var_name = None;
        self.matched_vars.clear();
        self.recount();
    }

    fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|entry| entry.key.eq_ignore_ascii_case(key))
            .map(|entry| entry.value.as_slice())
    }

    fn delete(&mut self, key: &[u8]) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.key.eq_ignore_ascii_case(key))
        {
            self.entries.remove(index);
            self.recount();
        }
    }

    fn set(&mut self, key: Vec<u8>, value: Vec<u8>, limits: RuntimeLimits) -> Result<(), RuntimeError> {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.key.eq_ignore_ascii_case(&key))
        {
            entry.value = value;
            self.recount();
        } else {
            if self.entries.len() == limits.max_tx_variables {
                return Err(RuntimeError::TxVariableLimit {
                    limit: limits.max_tx_variables,
                });
            }
            self.entries.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
            self.entries.push(TxEntry { key, value });
            self.recount();
        }
        if self.retained_bytes > limits.max_tx_bytes {
            return Err(RuntimeError::TxByteLimit {
                limit: limits.max_tx_bytes,
            });
        }
        Ok(())
    }

    fn try_clone(&self) -> Result<Self, RuntimeError> {
        let entries = try_clone_entries(&self.entries)?;
        let matched_vars = try_clone_entries(&self.matched_vars)?;
        Ok(Self {
            entries,
            matched_var: self.matched_var.as_deref().map(owned_copy).transpose()?,
            matched_var_name: self.matched_var_name.as_deref().map(owned_copy).transpose()?,
            matched_vars,
            retained_bytes: self.retained_bytes,
        })
    }

    fn recount(&mut self) {
        self.retained_bytes = self
            .entries
            .len()
            .saturating_mul(size_of::<TxEntry>())
            .saturating_add(
                self.entries
                    .iter()
                    .map(|entry| entry.key.len().saturating_add(entry.value.len()))
                    .sum::<usize>(),
            )
            .saturating_add(self.matched_var.as_ref().map_or(0, Vec::len))
            .saturating_add(self.matched_var_name.as_ref().map_or(0, Vec::len))
            .saturating_add(self.matched_vars.len().saturating_mul(size_of::<TxEntry>()))
            .saturating_add(
                self.matched_vars
                    .iter()
                    .map(|entry| entry.key.len().saturating_add(entry.value.len()))
                    .sum::<usize>(),
            );
    }
}

fn try_clone_entries(entries: &[TxEntry]) -> Result<Vec<TxEntry>, RuntimeError> {
    let mut cloned = Vec::new();
    cloned
        .try_reserve_exact(entries.len())
        .map_err(|_| RuntimeError::Allocation)?;
    for entry in entries {
        cloned.push(TxEntry {
            key: owned_copy(&entry.key)?,
            value: owned_copy(&entry.value)?,
        });
    }
    Ok(cloned)
}

fn setvar_value(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    setvar: super::CompiledSetVar,
    member_limit: usize,
) -> Result<Vec<u8>, RuntimeError> {
    if let Some(constant) = setvar.constant {
        let (digits, start) = signed_decimal_bytes(constant);
        return owned_copy(&digits[start..]);
    }
    if let Some(template) = setvar.value {
        return expand_template(generation, request, tx, template, member_limit);
    }
    Ok(Vec::new())
}

fn owned_copy(value: &[u8]) -> Result<Vec<u8>, RuntimeError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(value.len())
        .map_err(|_| RuntimeError::Allocation)?;
    output.extend_from_slice(value);
    Ok(output)
}

fn signed_decimal_bytes(value: i64) -> ([u8; 40], usize) {
    let negative = value < 0;
    let mut magnitude = value.unsigned_abs();
    let mut bytes = [0_u8; 40];
    let mut start = bytes.len();
    loop {
        start -= 1;
        bytes[start] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    if negative {
        start -= 1;
        bytes[start] = b'-';
    }
    (bytes, start)
}

fn matcher_scratch_bytes(generation: &CompiledGeneration) -> Result<usize, RuntimeError> {
    let mut total = 0_usize;
    for group in &generation.groups {
        total = total
            .checked_add(group.matcher.required_scratch_bytes())
            .ok_or(RuntimeError::Arithmetic)?;
    }
    for program in &generation.boolean_regexes {
        total = total
            .checked_add(program.required_scratch_bytes())
            .ok_or(RuntimeError::Arithmetic)?;
    }
    for program in &generation.capture_regexes {
        total = total
            .checked_add(program.required_scratch_bytes())
            .ok_or(RuntimeError::Arithmetic)?;
    }
    Ok(total)
}

fn member_has_action(
    generation: &CompiledGeneration,
    member: super::CompiledMemberPlan,
    expected: super::super::ActionKind,
) -> Result<bool, RuntimeError> {
    let end = member
        .action_start
        .checked_add(member.action_count)
        .ok_or(RuntimeError::Arithmetic)?;
    Ok(generation
        .actions
        .get(member.action_start..end)
        .ok_or(RuntimeError::GenerationInvariant)?
        .iter()
        .any(|action| action.kind == super::ActionEnvelopeKind::Action(expected)))
}

fn select_values(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    response: Option<&ResponseState>,
    scratch: &mut [RegexBooleanScratch],
    plan_id: usize,
    limit: usize,
) -> Result<Vec<SelectedValue>, RuntimeError> {
    let plan = *generation
        .target_plans
        .get(plan_id)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let end = plan
        .record_start
        .checked_add(plan.record_count)
        .ok_or(RuntimeError::Arithmetic)?;
    let records = generation
        .target_records
        .get(plan.record_start..end)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let mut selected = Vec::new();
    selected
        .try_reserve(limit.min(request.entry_count.saturating_add(records.len())))
        .map_err(|_| RuntimeError::Allocation)?;
    for target in records {
        let candidates = collection_candidates(request, tx, response, target.collection)?;
        match target.modifier {
            super::TargetModifier::None => {
                for candidate in candidates {
                    if selector_matches(
                        generation,
                        request,
                        tx,
                        response,
                        scratch,
                        target.collection,
                        target.selector,
                        candidate,
                    )? {
                        if selected.len() == limit {
                            return Err(RuntimeError::SelectedValueLimit { limit });
                        }
                        selected.push(candidate);
                    }
                }
            }
            super::TargetModifier::Exclude => {
                let mut write = 0_usize;
                for read in 0..selected.len() {
                    let candidate = selected[read];
                    let remove = candidate.collection == target.collection
                        && selector_matches(
                            generation,
                            request,
                            tx,
                            response,
                            scratch,
                            target.collection,
                            target.selector,
                            candidate,
                        )?;
                    if !remove {
                        selected[write] = candidate;
                        write += 1;
                    }
                }
                selected.truncate(write);
            }
            super::TargetModifier::Count => {
                let mut count = 0_usize;
                for candidate in candidates {
                    if selector_matches(
                        generation,
                        request,
                        tx,
                        response,
                        scratch,
                        target.collection,
                        target.selector,
                        candidate,
                    )? {
                        count = count.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
                    }
                }
                if selected.len() == limit {
                    return Err(RuntimeError::SelectedValueLimit { limit });
                }
                let (digits, start) = decimal_bytes(count);
                selected.push(SelectedValue {
                    collection: target.collection,
                    name: None,
                    value: SelectedBytes::Count {
                        digits,
                        start: u8::try_from(start).map_err(|_| RuntimeError::Arithmetic)?,
                    },
                });
            }
        }
    }
    Ok(selected)
}

fn collection_candidates(
    request: &RequestState,
    tx: &TxState,
    response: Option<&ResponseState>,
    collection: CollectionKind,
) -> Result<Vec<SelectedValue>, RuntimeError> {
    let mut candidates = Vec::new();
    match collection {
        CollectionKind::Tx => {
            candidates
                .try_reserve_exact(tx.entries.len())
                .map_err(|_| RuntimeError::Allocation)?;
            for index in 0..tx.entries.len() {
                candidates.push(SelectedValue {
                    collection,
                    name: Some(SelectedName::Tx(index)),
                    value: SelectedBytes::Tx(index),
                });
            }
        }
        CollectionKind::MatchedVar => {
            if tx.matched_var.is_some() {
                candidates.push(SelectedValue {
                    collection,
                    name: None,
                    value: SelectedBytes::MatchedVar,
                });
            }
        }
        CollectionKind::MatchedVars => {
            candidates
                .try_reserve_exact(tx.matched_vars.len())
                .map_err(|_| RuntimeError::Allocation)?;
            for index in 0..tx.matched_vars.len() {
                candidates.push(SelectedValue {
                    collection,
                    name: Some(SelectedName::MatchedVars(index)),
                    value: SelectedBytes::MatchedVars(index),
                });
            }
        }
        CollectionKind::ResponseHeaders | CollectionKind::ResponseStatus => {
            if let Some(response) = response {
                let count = response
                    .entries
                    .iter()
                    .filter(|entry| entry.collection == collection)
                    .count();
                candidates
                    .try_reserve_exact(count)
                    .map_err(|_| RuntimeError::Allocation)?;
                for (index, entry) in response.entries.iter().enumerate() {
                    if entry.collection != collection {
                        continue;
                    }
                    candidates.push(SelectedValue {
                        collection,
                        name: entry.name.as_ref().map(|_| SelectedName::Response(index)),
                        value: SelectedBytes::Response(index),
                    });
                }
            }
        }
        CollectionKind::ResponseBody => {
            if response.and_then(|response| response.window.as_ref()).is_some() {
                candidates.push(SelectedValue {
                    collection,
                    name: None,
                    value: SelectedBytes::ResponseWindow,
                });
            }
        }
        _ => {
            let records = request.collection_records(collection)?;
            let body_overrides_head = matches!(
                collection,
                CollectionKind::ArgsCombinedSize | CollectionKind::ReqbodyProcessor
            ) && request.body.is_some();
            let body_count = request.body.as_ref().map_or(0, |body| {
                body.entries
                    .iter()
                    .filter(|entry| entry.collection == collection)
                    .count()
            });
            candidates
                .try_reserve_exact(records.len().saturating_add(body_count))
                .map_err(|_| RuntimeError::Allocation)?;
            if !body_overrides_head {
                for candidate in records {
                    candidates.push(SelectedValue {
                        collection,
                        name: candidate.name.map(SelectedName::Request),
                        value: SelectedBytes::Request(candidate.value),
                    });
                }
            }
            if let Some(body) = &request.body {
                for (index, entry) in body.entries.iter().enumerate() {
                    if entry.collection != collection {
                        continue;
                    }
                    candidates.push(SelectedValue {
                        collection,
                        name: entry.name.as_ref().map(|_| SelectedName::Body(index)),
                        value: SelectedBytes::Body(index),
                    });
                }
            }
        }
    }
    Ok(candidates)
}

#[allow(clippy::too_many_arguments)]
fn selector_matches(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    response: Option<&ResponseState>,
    scratch: &mut [RegexBooleanScratch],
    collection: CollectionKind,
    selector: super::CompiledTargetSelector,
    candidate: SelectedValue,
) -> Result<bool, RuntimeError> {
    let name = candidate.name_bytes(request, tx, response)?;
    match selector {
        super::CompiledTargetSelector::None => Ok(true),
        super::CompiledTargetSelector::Literal(expected) => {
            let expected = generation_bytes(&generation.target_bytes, expected)?;
            let Some(name) = name else {
                return Ok(false);
            };
            Ok(
                if matches!(
                    collection,
                    CollectionKind::RequestHeaders
                        | CollectionKind::RequestHeadersNames
                        | CollectionKind::ResponseHeaders
                        | CollectionKind::Tx
                ) {
                    name.eq_ignore_ascii_case(expected)
                } else {
                    name == expected
                },
            )
        }
        super::CompiledTargetSelector::Path(path) => {
            let path = generation_bytes(&generation.target_bytes, path)?;
            let SelectedBytes::Body(index) = candidate.value else {
                return Ok(false);
            };
            let entry = request
                .body
                .as_ref()
                .and_then(|body| body.entries.get(index))
                .ok_or(RuntimeError::GenerationInvariant)?;
            if entry.collection != CollectionKind::Xml {
                return Ok(false);
            }
            match path {
                b"/*" => Ok(!entry.xml_attribute),
                b"//@*" => Ok(entry.xml_attribute),
                _ => Err(RuntimeError::XmlSelectorUnavailable),
            }
        }
        super::CompiledTargetSelector::Regex { program, .. } => {
            let Some(name) = name else {
                return Ok(false);
            };
            let regex = generation
                .boolean_regexes
                .get(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            let scratch = scratch.get_mut(program).ok_or(RuntimeError::GenerationInvariant)?;
            regex.is_match(name, scratch).map_err(map_regex_scratch_error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn evaluate_operator(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    group_scratch: &mut [MatchScratch],
    boolean_scratch: &mut [RegexBooleanScratch],
    capture_scratch: &mut [RegexCaptureScratch],
    plan: super::CompiledOperatorPlan,
    input: &[u8],
    member_runtime_limit: usize,
    evaluation_mode: EvaluationMode,
) -> Result<(bool, Option<crate::injection::Fingerprint>), RuntimeError> {
    let mut fingerprint = None;
    let positive = match plan.operand {
        super::CompiledOperatorOperand::String { relation, template } => {
            let operand = expand_template(generation, request, tx, template, member_runtime_limit)?;
            Ok(match relation {
                super::StringRelation::BeginsWith => input.starts_with(&operand),
                super::StringRelation::Contains => contains(input, &operand),
                super::StringRelation::EndsWith => input.ends_with(&operand),
                super::StringRelation::Equals => input == operand,
                super::StringRelation::Within => contains(&operand, input),
            })
        }
        super::CompiledOperatorOperand::Integer { relation, operand } => {
            let expected = match operand {
                super::CompiledIntegerOperand::Static(value) => value,
                super::CompiledIntegerOperand::Template(template) => parse_i64(&expand_template(
                    generation,
                    request,
                    tx,
                    template,
                    member_runtime_limit,
                )?)?,
            };
            let actual = parse_i64(input)?;
            Ok(match relation {
                super::IntegerRelation::Equal => actual == expected,
                super::IntegerRelation::GreaterOrEqual => actual >= expected,
                super::IntegerRelation::Greater => actual > expected,
                super::IntegerRelation::Less => actual < expected,
            })
        }
        super::CompiledOperatorOperand::IpNetworks { start, count } => {
            let Some(address) = std::str::from_utf8(input)
                .ok()
                .and_then(|value| value.parse::<std::net::IpAddr>().ok())
            else {
                return Ok((false, None));
            };
            let end = start.checked_add(count).ok_or(RuntimeError::Arithmetic)?;
            let networks = generation
                .ip_networks
                .get(start..end)
                .ok_or(RuntimeError::GenerationInvariant)?;
            Ok(networks.iter().any(|network| ip_matches(address, *network)))
        }
        super::CompiledOperatorOperand::ByteSet { words } => Ok(input.iter().copied().any(|byte| {
            let index = usize::from(byte);
            words[index / 64] & (1_u64 << (index % 64)) == 0
        })),
        super::CompiledOperatorOperand::ValidateUrlEncoding => Ok(invalid_url_encoding(input)),
        super::CompiledOperatorOperand::ValidateUtf8Encoding => Ok(std::str::from_utf8(input).is_err()),
        super::CompiledOperatorOperand::Unconditional => Ok(true),
        super::CompiledOperatorOperand::DetectSqli { .. } => {
            fingerprint = crate::injection::detect_sqli(input);
            Ok(fingerprint.is_some())
        }
        super::CompiledOperatorOperand::DetectXss => Ok(crate::injection::detect_xss(input)),
        super::CompiledOperatorOperand::Regex(reference) => match reference {
            super::RegexPlanRef::Boolean(program) => {
                let regex = generation
                    .boolean_regexes
                    .get(program)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                let scratch = boolean_scratch
                    .get_mut(program)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                regex.is_match(input, scratch).map_err(map_regex_scratch_error)
            }
            super::RegexPlanRef::Capture(program) => {
                let regex = generation
                    .capture_regexes
                    .get(program)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                let scratch = capture_scratch
                    .get_mut(program)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                regex.is_match(input, scratch).map_err(map_regex_scratch_error)
            }
        },
        super::CompiledOperatorOperand::Matcher {
            group,
            pattern_start,
            pattern_count,
        } => {
            let matcher = generation.groups.get(group).ok_or(RuntimeError::GenerationInvariant)?;
            let scratch = group_scratch.get_mut(group).ok_or(RuntimeError::GenerationInvariant)?;
            let end = pattern_start
                .checked_add(pattern_count)
                .ok_or(RuntimeError::Arithmetic)?;
            let matches = match evaluation_mode {
                EvaluationMode::Accelerated => matcher.matcher.scan(input, scratch),
                EvaluationMode::Reference => matcher.matcher.scan_reference(input, scratch),
            }
            .map_err(map_matcher_error)?;
            let found = matches
                .iter()
                .any(|pattern| (pattern_start..end).contains(&pattern.as_usize()));
            Ok(found)
        }
        super::CompiledOperatorOperand::CapturePhrase(program) => {
            let matcher = generation
                .capture_phrases
                .get(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            Ok(match evaluation_mode {
                EvaluationMode::Accelerated => matcher.find(input),
                EvaluationMode::Reference => matcher.find_reference(input),
            }
            .is_some())
        }
    }?;
    Ok((positive, fingerprint))
}

fn expand_template(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    template_id: usize,
    member_limit: usize,
) -> Result<Vec<u8>, RuntimeError> {
    let template = *generation
        .templates
        .get(template_id)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let limit = template.runtime_byte_limit.min(member_limit);
    let end = template
        .segment_start
        .checked_add(template.segment_count)
        .ok_or(RuntimeError::Arithmetic)?;
    let segments = generation
        .segments
        .get(template.segment_start..end)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let mut output = Vec::new();
    output
        .try_reserve(template.static_bytes.min(limit))
        .map_err(|_| RuntimeError::Allocation)?;
    for segment in segments {
        match segment.kind {
            super::CompiledSegmentKind::Literal(range) => {
                append_runtime(&mut output, generation_bytes(&generation.bytes, range)?, limit)?;
            }
            super::CompiledSegmentKind::Macro(macro_id) => {
                let reference = *generation
                    .macros
                    .get(macro_id)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                let key = generation_bytes(&generation.bytes, reference.key)?;
                let value = macro_value(request, tx, reference.source, key)?;
                append_runtime(&mut output, value.unwrap_or_default(), limit)?;
            }
        }
    }
    Ok(output)
}

fn macro_value<'request>(
    request: &'request RequestState,
    tx: &'request TxState,
    source: super::MacroSource,
    key: &[u8],
) -> Result<Option<&'request [u8]>, RuntimeError> {
    Ok(match source {
        super::MacroSource::Args => request.first_named(CollectionKind::Args, key, false),
        super::MacroSource::RequestHeaders => request.first_named(CollectionKind::RequestHeaders, key, true),
        super::MacroSource::RemoteAddr => request.first_value(CollectionKind::RemoteAddr),
        super::MacroSource::RequestLine => request.first_value(CollectionKind::RequestLine),
        super::MacroSource::Tx => tx.get(key),
        super::MacroSource::MatchedVar => tx.matched_var.as_deref(),
        super::MacroSource::MatchedVarName => tx.matched_var_name.as_deref(),
    })
}

fn generation_bytes(bytes: &[u8], range: super::ByteRange) -> Result<&[u8], RuntimeError> {
    let end = range.start.checked_add(range.len).ok_or(RuntimeError::Arithmetic)?;
    bytes.get(range.start..end).ok_or(RuntimeError::GenerationInvariant)
}

fn append_runtime(output: &mut Vec<u8>, value: &[u8], limit: usize) -> Result<(), RuntimeError> {
    let actual = output.len().checked_add(value.len()).ok_or(RuntimeError::Arithmetic)?;
    if actual > limit {
        return Err(RuntimeError::ValueByteLimit { actual, limit });
    }
    output.extend_from_slice(value);
    Ok(())
}

fn parse_i64(value: &[u8]) -> Result<i64, RuntimeError> {
    let text = std::str::from_utf8(value).map_err(|_| RuntimeError::IntegerConversion)?;
    if text.is_empty()
        || text
            .bytes()
            .enumerate()
            .any(|(index, byte)| !(byte.is_ascii_digit() || index == 0 && matches!(byte, b'+' | b'-')))
    {
        return Err(RuntimeError::IntegerConversion);
    }
    text.parse().map_err(|_| RuntimeError::IntegerConversion)
}

fn parse_usize(value: &[u8]) -> Result<usize, RuntimeError> {
    let text = std::str::from_utf8(value).map_err(|_| RuntimeError::GenerationInvariant)?;
    if text.is_empty() || text.bytes().any(|byte| !byte.is_ascii_digit()) {
        return Err(RuntimeError::GenerationInvariant);
    }
    text.parse().map_err(|_| RuntimeError::GenerationInvariant)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|window| window == needle)
}

fn ip_matches(address: std::net::IpAddr, network: super::CompiledIpNetwork) -> bool {
    match (address, network.address) {
        (std::net::IpAddr::V4(address), std::net::IpAddr::V4(base)) => {
            let prefix = u32::from(network.prefix.min(32));
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            u32::from(address) & mask == u32::from(base) & mask
        }
        (std::net::IpAddr::V6(address), std::net::IpAddr::V6(base)) => {
            let prefix = u32::from(network.prefix.min(128));
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) };
            u128::from(address) & mask == u128::from(base) & mask
        }
        _ => false,
    }
}

fn invalid_url_encoding(input: &[u8]) -> bool {
    let mut index = 0_usize;
    while index < input.len() {
        if input[index] != b'%' {
            index += 1;
            continue;
        }
        if index + 2 >= input.len() || hex(input[index + 1]).is_none() || hex(input[index + 2]).is_none() {
            return true;
        }
        index += 3;
    }
    false
}

fn map_transform_error(error: TransformError) -> RuntimeError {
    let kind = match error {
        TransformError::OutputLimit { .. } => RuntimeTransformError::OutputLimit,
        TransformError::WorkLimit { .. } => RuntimeTransformError::WorkLimit,
        TransformError::Allocation => RuntimeTransformError::Allocation,
        TransformError::Arithmetic => RuntimeTransformError::Arithmetic,
    };
    RuntimeError::Transformation { kind }
}

fn map_regex_scratch_error(error: RegexScratchError) -> RuntimeError {
    match error {
        RegexScratchError::ScratchShape => RuntimeError::Matcher {
            kind: RuntimeMatcherError::ScratchShape,
        },
    }
}

fn map_matcher_error(error: MatcherScanError) -> RuntimeError {
    let kind = match error {
        MatcherScanError::ScratchShape => RuntimeMatcherError::ScratchShape,
        MatcherScanError::PhraseSearchMode | MatcherScanError::InvalidPatternId => RuntimeMatcherError::PatternShape,
    };
    RuntimeError::Matcher { kind }
}

impl ResponseState {
    fn build(response: ResponseHead<'_>, limits: RuntimeLimits) -> Result<Self, RuntimeError> {
        if !(100..=999).contains(&response.status) {
            return Err(RuntimeError::InvalidResponseStatus {
                status: response.status,
            });
        }
        if response.headers.len() > limits.max_response_headers {
            return Err(RuntimeError::ResponseHeaderLimit {
                actual: response.headers.len(),
                limit: limits.max_response_headers,
            });
        }
        let mut header_bytes = 0_usize;
        for (ordinal, header) in response.headers.iter().copied().enumerate() {
            if header.name.is_empty() || !header.name.iter().copied().all(is_header_name_byte) {
                return Err(RuntimeError::InvalidResponseHeaderName { ordinal });
            }
            if header.value.iter().any(|byte| matches!(byte, b'\r' | b'\n' | 0)) {
                return Err(RuntimeError::InvalidResponseHeaderValue { ordinal });
            }
            check_value(header.name, limits.max_value_bytes)?;
            check_value(header.value, limits.max_value_bytes)?;
            header_bytes = checked_add(header_bytes, checked_add(header.name.len(), header.value.len())?)?;
            if header_bytes > limits.max_response_header_bytes {
                return Err(RuntimeError::ResponseHeaderByteLimit {
                    actual: header_bytes,
                    limit: limits.max_response_header_bytes,
                });
            }
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(response.headers.len().saturating_add(1))
            .map_err(|_| RuntimeError::Allocation)?;
        entries.push(ResponseEntry {
            collection: CollectionKind::ResponseStatus,
            name: None,
            value: owned_copy(response.status.to_string().as_bytes())?,
        });
        for header in response.headers {
            entries.push(ResponseEntry {
                collection: CollectionKind::ResponseHeaders,
                name: Some(owned_copy(header.name)?),
                value: owned_copy(header.value)?,
            });
        }
        let retained_bytes = entries
            .iter()
            .try_fold(0_usize, |total, entry| {
                total
                    .checked_add(entry.name.as_ref().map_or(0, Vec::len))
                    .and_then(|value| value.checked_add(entry.value.len()))
                    .ok_or(RuntimeError::Arithmetic)
            })?
            .checked_add(
                entries
                    .len()
                    .checked_mul(size_of::<ResponseEntry>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .ok_or(RuntimeError::Arithmetic)?;
        Ok(Self {
            entries,
            window: None,
            retained_bytes,
        })
    }

    fn total_retained_bytes(&self) -> usize {
        self.retained_bytes
            .saturating_add(self.window.as_ref().map_or(0, Vec::len))
    }
}

fn phase_index(phase: Phase) -> usize {
    phase as usize - 1
}

impl RequestState {
    fn build(request: RequestHead<'_>, limits: RuntimeLimits) -> Result<Self, RuntimeError> {
        if request.headers.len() > limits.max_headers {
            return Err(RuntimeError::HeaderLimit {
                actual: request.headers.len(),
                limit: limits.max_headers,
            });
        }
        let fixed = [
            request.request_line,
            request.method,
            request.uri,
            request.uri_raw,
            request.protocol,
            request.remote_addr,
            request.unique_id,
        ];
        let mut input_bytes = 0_usize;
        for value in fixed {
            check_value(value, limits.max_value_bytes)?;
            input_bytes = checked_add(input_bytes, value.len())?;
        }
        let mut header_bytes = 0_usize;
        for (ordinal, header) in request.headers.iter().copied().enumerate() {
            if header.name.is_empty() || !header.name.iter().copied().all(is_header_name_byte) {
                return Err(RuntimeError::InvalidHeaderName { ordinal });
            }
            if header.value.iter().any(|byte| matches!(byte, b'\r' | b'\n' | 0)) {
                return Err(RuntimeError::InvalidHeaderValue { ordinal });
            }
            check_value(header.name, limits.max_value_bytes)?;
            check_value(header.value, limits.max_value_bytes)?;
            header_bytes = checked_add(header_bytes, checked_add(header.name.len(), header.value.len())?)?;
            if header_bytes > limits.max_header_bytes {
                return Err(RuntimeError::HeaderByteLimit {
                    actual: header_bytes,
                    limit: limits.max_header_bytes,
                });
            }
        }
        input_bytes = checked_add(input_bytes, header_bytes)?;
        if input_bytes > limits.max_input_bytes {
            return Err(RuntimeError::InputByteLimit {
                actual: input_bytes,
                limit: limits.max_input_bytes,
            });
        }

        let query = query_part(request.uri);
        let query_count = count_delimited(query, b'&', limits.max_query_arguments)?;
        let cookie_count = count_cookies(request.headers, limits.max_cookies)?;
        let scalar_count = 12_usize;
        let header_records = checked_mul(request.headers.len(), 2)?;
        let query_records = checked_mul(query_count, 4)?;
        let cookie_records = checked_mul(cookie_count, 2)?;
        let record_count = scalar_count
            .checked_add(header_records)
            .and_then(|value| value.checked_add(query_records))
            .and_then(|value| value.checked_add(cookie_records))
            .ok_or(RuntimeError::Arithmetic)?;
        if record_count > limits.max_collection_entries {
            return Err(RuntimeError::CollectionEntryLimit {
                limit: limits.max_collection_entries,
            });
        }

        let derived_bytes = projected_derived_bytes(request, query)?;
        let retained_bytes = derived_bytes
            .checked_add(
                record_count
                    .checked_mul(size_of::<CollectionValue>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .and_then(|value| value.checked_add(size_of::<[Vec<CollectionValue>; COLLECTION_COUNT]>()))
            .ok_or(RuntimeError::Arithmetic)?;
        if retained_bytes > limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: limits.max_retained_bytes,
            });
        }

        let mut state = Self {
            bytes: Vec::new(),
            collections: std::array::from_fn(|_| Vec::new()),
            body: None,
            entry_count: 0,
            retained_bytes: 0,
        };
        state
            .bytes
            .try_reserve_exact(derived_bytes)
            .map_err(|_| RuntimeError::Allocation)?;
        let (digits, digit_start) = decimal_bytes(query_argument_combined_size(query)?);
        state.push_scalar(CollectionKind::ArgsCombinedSize, &digits[digit_start..])?;
        state.push_query_collection(CollectionKind::Args, query, false)?;
        state.push_query_collection(CollectionKind::ArgsGet, query, false)?;
        state.push_query_collection(CollectionKind::ArgsNames, query, true)?;
        state.push_query_collection(CollectionKind::ArgsGetNames, query, true)?;
        state.push_scalar(CollectionKind::QueryString, query)?;
        state.push_scalar(CollectionKind::RemoteAddr, request.remote_addr)?;
        state.push_scalar(
            CollectionKind::ReqbodyProcessor,
            request_body_processor(request.headers).as_bytes(),
        )?;
        state.push_cookie_collection(CollectionKind::RequestCookies, request.headers, false)?;
        state.push_cookie_collection(CollectionKind::RequestCookiesNames, request.headers, true)?;
        state.push_request_path_collections(request.uri)?;
        state.push_header_collection(CollectionKind::RequestHeaders, request.headers, false)?;
        state.push_header_collection(CollectionKind::RequestHeadersNames, request.headers, true)?;
        state.push_scalar(CollectionKind::RequestLine, request.request_line)?;
        state.push_scalar(CollectionKind::RequestMethod, request.method)?;
        state.push_scalar(CollectionKind::RequestProtocol, request.protocol)?;
        state.push_scalar(CollectionKind::RequestUri, request.uri)?;
        state.push_scalar(CollectionKind::RequestUriRaw, request.uri_raw)?;
        state.push_scalar(CollectionKind::UniqueId, request.unique_id)?;

        state.retained_bytes = state
            .bytes
            .len()
            .checked_add(
                state
                    .entry_count
                    .checked_mul(size_of::<CollectionValue>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .and_then(|value| value.checked_add(size_of::<[Vec<CollectionValue>; COLLECTION_COUNT]>()))
            .ok_or(RuntimeError::Arithmetic)?;
        if state.retained_bytes > limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: limits.max_retained_bytes,
            });
        }
        debug_assert!(state.entry_count <= record_count);
        Ok(state)
    }

    fn push_record(&mut self, collection: CollectionKind, record: CollectionValue) -> Result<(), RuntimeError> {
        let records = &mut self.collections[collection_index(collection)];
        records.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
        records.push(record);
        self.entry_count = self.entry_count.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
        Ok(())
    }

    fn push_scalar(&mut self, collection: CollectionKind, value: &[u8]) -> Result<(), RuntimeError> {
        let value = self.copy_bytes(value)?;
        self.push_record(collection, CollectionValue { name: None, value })
    }

    fn push_header_collection(
        &mut self,
        collection: CollectionKind,
        headers: &[RequestHeader<'_>],
        names: bool,
    ) -> Result<(), RuntimeError> {
        for header in headers {
            let name = self.copy_bytes(header.name)?;
            let value = if names { name } else { self.copy_bytes(header.value)? };
            self.push_record(
                collection,
                CollectionValue {
                    name: Some(name),
                    value,
                },
            )?;
        }
        Ok(())
    }

    fn push_query_collection(
        &mut self,
        collection: CollectionKind,
        query: &[u8],
        names: bool,
    ) -> Result<(), RuntimeError> {
        for segment in nonempty_segments(query, b'&') {
            let (name, value) = split_once(segment, b'=');
            let name = self.copy_percent_decoded(name)?;
            let value = if names { name } else { self.copy_percent_decoded(value)? };
            self.push_record(
                collection,
                CollectionValue {
                    name: Some(name),
                    value,
                },
            )?;
        }
        Ok(())
    }

    fn push_cookie_collection(
        &mut self,
        collection: CollectionKind,
        headers: &[RequestHeader<'_>],
        names: bool,
    ) -> Result<(), RuntimeError> {
        for header in headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case(b"cookie"))
        {
            for segment in nonempty_segments(header.value, b';') {
                let (name, value) = split_once(trim_ascii(segment), b'=');
                let name = trim_ascii(name);
                if name.is_empty() {
                    continue;
                }
                let name = self.copy_bytes(name)?;
                let value = if names {
                    name
                } else {
                    self.copy_bytes(trim_ascii(value))?
                };
                self.push_record(
                    collection,
                    CollectionValue {
                        name: Some(name),
                        value,
                    },
                )?;
            }
        }
        Ok(())
    }

    fn push_request_path_collections(&mut self, uri: &[u8]) -> Result<(), RuntimeError> {
        let filename = self.copy_percent_decoded_path(request_filename(uri))?;
        let filename_end = filename
            .start
            .checked_add(filename.len)
            .ok_or(RuntimeError::Arithmetic)?;
        let decoded = self
            .bytes
            .get(filename.start..filename_end)
            .ok_or(RuntimeError::GenerationInvariant)?;
        let basename_offset = decoded
            .iter()
            .rposition(|byte| matches!(byte, b'/' | b'\\'))
            .map_or(0, |index| index + 1);
        let basename = ByteRange {
            start: filename
                .start
                .checked_add(basename_offset)
                .ok_or(RuntimeError::Arithmetic)?,
            len: filename.len - basename_offset,
        };
        self.push_record(
            CollectionKind::RequestBasename,
            CollectionValue {
                name: None,
                value: basename,
            },
        )?;
        self.push_record(
            CollectionKind::RequestFilename,
            CollectionValue {
                name: None,
                value: filename,
            },
        )
    }

    fn copy_bytes(&mut self, value: &[u8]) -> Result<ByteRange, RuntimeError> {
        let start = self.bytes.len();
        self.bytes.extend_from_slice(value);
        Ok(ByteRange {
            start,
            len: value.len(),
        })
    }

    fn copy_percent_decoded(&mut self, value: &[u8]) -> Result<ByteRange, RuntimeError> {
        let start = self.bytes.len();
        let mut index = 0_usize;
        while index < value.len() {
            match value[index] {
                b'+' => {
                    self.bytes.push(b' ');
                    index += 1;
                }
                b'%' if index + 2 < value.len() => {
                    if let (Some(high), Some(low)) = (hex(value[index + 1]), hex(value[index + 2])) {
                        self.bytes.push((high << 4) | low);
                        index += 3;
                    } else {
                        self.bytes.push(b'%');
                        index += 1;
                    }
                }
                byte => {
                    self.bytes.push(byte);
                    index += 1;
                }
            }
        }
        Ok(ByteRange {
            start,
            len: self.bytes.len() - start,
        })
    }

    fn copy_percent_decoded_path(&mut self, value: &[u8]) -> Result<ByteRange, RuntimeError> {
        let start = self.bytes.len();
        let mut index = 0_usize;
        while index < value.len() {
            if value[index] == b'%' && index + 2 < value.len() {
                if let (Some(high), Some(low)) = (hex(value[index + 1]), hex(value[index + 2])) {
                    self.bytes.push((high << 4) | low);
                    index += 3;
                    continue;
                }
            }
            self.bytes.push(value[index]);
            index += 1;
        }
        Ok(ByteRange {
            start,
            len: self.bytes.len() - start,
        })
    }

    #[cfg(test)]
    fn collection(&self, collection: CollectionKind) -> impl Iterator<Item = (Option<&[u8]>, &[u8])> {
        self.collections[collection_index(collection)].iter().map(|record| {
            let name = record.name.map(|range| self.bytes_at(range));
            (name, self.bytes_at(record.value))
        })
    }

    fn bytes_at(&self, range: ByteRange) -> &[u8] {
        range
            .start
            .checked_add(range.len)
            .and_then(|end| self.bytes.get(range.start..end))
            .unwrap_or_default()
    }

    fn collection_records(&self, collection: CollectionKind) -> Result<&[CollectionValue], RuntimeError> {
        Ok(&self.collections[collection_index(collection)])
    }

    fn first_named(&self, collection: CollectionKind, key: &[u8], ascii_insensitive: bool) -> Option<&[u8]> {
        let head = self.collection_records(collection).ok()?.iter().find_map(|record| {
            let name = self.bytes_at(record.name?);
            let matched = if ascii_insensitive {
                name.eq_ignore_ascii_case(key)
            } else {
                name == key
            };
            matched.then(|| self.bytes_at(record.value))
        });
        head.or_else(|| {
            self.body.as_ref()?.entries.iter().find_map(|entry| {
                if entry.collection != collection {
                    return None;
                }
                let name = entry.name.as_deref()?;
                let matched = if ascii_insensitive {
                    name.eq_ignore_ascii_case(key)
                } else {
                    name == key
                };
                matched.then_some(entry.value.as_slice())
            })
        })
    }

    fn first_value(&self, collection: CollectionKind) -> Option<&[u8]> {
        if collection == CollectionKind::ArgsCombinedSize {
            if let Some(body) = &self.body {
                if let Some(value) = body
                    .entries
                    .iter()
                    .find_map(|entry| (entry.collection == collection).then_some(entry.value.as_slice()))
                {
                    return Some(value);
                }
            }
        }
        self.collection_records(collection)
            .ok()?
            .first()
            .map(|record| self.bytes_at(record.value))
            .or_else(|| {
                self.body
                    .as_ref()?
                    .entries
                    .iter()
                    .find_map(|entry| (entry.collection == collection).then_some(entry.value.as_slice()))
            })
    }
}

fn phase_from_number(value: u8) -> Option<Phase> {
    match value {
        1 => Some(Phase::RequestHeaders),
        2 => Some(Phase::RequestBody),
        3 => Some(Phase::ResponseHeaders),
        4 => Some(Phase::ResponseBody),
        5 => Some(Phase::Logging),
        _ => None,
    }
}

fn collection_index(collection: CollectionKind) -> usize {
    match collection {
        CollectionKind::Args => 0,
        CollectionKind::ArgsCombinedSize => 1,
        CollectionKind::ArgsGet => 2,
        CollectionKind::ArgsGetNames => 3,
        CollectionKind::ArgsNames => 4,
        CollectionKind::Files => 5,
        CollectionKind::FilesCombinedSize => 6,
        CollectionKind::FilesNames => 7,
        CollectionKind::MatchedVar => 8,
        CollectionKind::MatchedVars => 9,
        CollectionKind::MultipartPartHeaders => 10,
        CollectionKind::QueryString => 11,
        CollectionKind::RemoteAddr => 12,
        CollectionKind::ReqbodyProcessor => 13,
        CollectionKind::RequestBasename => 14,
        CollectionKind::RequestBody => 15,
        CollectionKind::RequestBodyLength => 16,
        CollectionKind::RequestCookies => 17,
        CollectionKind::RequestCookiesNames => 18,
        CollectionKind::RequestFilename => 19,
        CollectionKind::RequestHeaders => 20,
        CollectionKind::RequestHeadersNames => 21,
        CollectionKind::RequestLine => 22,
        CollectionKind::RequestMethod => 23,
        CollectionKind::RequestProtocol => 24,
        CollectionKind::RequestUri => 25,
        CollectionKind::RequestUriRaw => 26,
        CollectionKind::ResponseBody => 27,
        CollectionKind::ResponseHeaders => 28,
        CollectionKind::ResponseStatus => 29,
        CollectionKind::Tx => 30,
        CollectionKind::UniqueId => 31,
        CollectionKind::Xml => 32,
    }
}

fn check_value(value: &[u8], limit: usize) -> Result<(), RuntimeError> {
    if value.len() > limit {
        return Err(RuntimeError::ValueByteLimit {
            actual: value.len(),
            limit,
        });
    }
    Ok(())
}

fn checked_add(left: usize, right: usize) -> Result<usize, RuntimeError> {
    left.checked_add(right).ok_or(RuntimeError::Arithmetic)
}

fn checked_mul(left: usize, right: usize) -> Result<usize, RuntimeError> {
    left.checked_mul(right).ok_or(RuntimeError::Arithmetic)
}

fn is_header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn query_part(uri: &[u8]) -> &[u8] {
    uri.iter()
        .position(|byte| *byte == b'?')
        .map_or(&[], |index| &uri[index + 1..])
}

fn request_filename(uri: &[u8]) -> &[u8] {
    let end = uri.iter().position(|byte| *byte == b'?').unwrap_or(uri.len());
    &uri[..end]
}

fn nonempty_segments(value: &[u8], delimiter: u8) -> impl Iterator<Item = &[u8]> {
    value
        .split(move |byte| *byte == delimiter)
        .filter(|segment| !segment.is_empty())
}

fn count_delimited(value: &[u8], delimiter: u8, limit: usize) -> Result<usize, RuntimeError> {
    let mut count = 0_usize;
    for _ in nonempty_segments(value, delimiter) {
        if count == limit {
            return Err(RuntimeError::QueryArgumentLimit { limit });
        }
        count += 1;
    }
    Ok(count)
}

fn count_cookies(headers: &[RequestHeader<'_>], limit: usize) -> Result<usize, RuntimeError> {
    let mut count = 0_usize;
    for header in headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case(b"cookie"))
    {
        for segment in nonempty_segments(header.value, b';') {
            let (name, _) = split_once(trim_ascii(segment), b'=');
            if trim_ascii(name).is_empty() {
                continue;
            }
            if count == limit {
                return Err(RuntimeError::CookieLimit { limit });
            }
            count += 1;
        }
    }
    Ok(count)
}

fn split_once(value: &[u8], delimiter: u8) -> (&[u8], &[u8]) {
    value
        .iter()
        .position(|byte| *byte == delimiter)
        .map_or((value, &[]), |index| (&value[..index], &value[index + 1..]))
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn query_argument_combined_size(query: &[u8]) -> Result<usize, RuntimeError> {
    nonempty_segments(query, b'&').try_fold(0_usize, |total, segment| {
        let (name, value) = split_once(segment, b'=');
        checked_add(
            total,
            checked_add(percent_decoded_len(name), percent_decoded_len(value))?,
        )
    })
}

fn percent_decoded_len(value: &[u8]) -> usize {
    let mut length = 0_usize;
    let mut index = 0_usize;
    while index < value.len() {
        index += if value[index] == b'%'
            && index + 2 < value.len()
            && hex(value[index + 1]).is_some()
            && hex(value[index + 2]).is_some()
        {
            3
        } else {
            1
        };
        length += 1;
    }
    length
}

fn decimal_bytes(mut value: usize) -> ([u8; 40], usize) {
    let mut bytes = [0_u8; 40];
    let mut start = bytes.len();
    loop {
        start -= 1;
        bytes[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return (bytes, start);
        }
    }
}

fn projected_derived_bytes(request: RequestHead<'_>, query: &[u8]) -> Result<usize, RuntimeError> {
    let (_, combined_start) = decimal_bytes(query_argument_combined_size(query)?);
    let mut total = 40 - combined_start;
    for value in [
        query,
        request.remote_addr,
        request_body_processor(request.headers).as_bytes(),
        request.request_line,
        request.method,
        request.protocol,
        request.uri,
        request.uri_raw,
        request.unique_id,
    ] {
        total = checked_add(total, value.len())?;
    }
    total = checked_add(total, percent_decoded_len(request_filename(request.uri)))?;
    for segment in nonempty_segments(query, b'&') {
        let (name, value) = split_once(segment, b'=');
        total = checked_add(total, checked_mul(percent_decoded_len(name), 4)?)?;
        total = checked_add(total, checked_mul(percent_decoded_len(value), 2)?)?;
    }
    for header in request.headers {
        total = checked_add(total, checked_mul(header.name.len(), 2)?)?;
        total = checked_add(total, header.value.len())?;
        if header.name.eq_ignore_ascii_case(b"cookie") {
            for segment in nonempty_segments(header.value, b';') {
                let (name, value) = split_once(trim_ascii(segment), b'=');
                let name = trim_ascii(name);
                if name.is_empty() {
                    continue;
                }
                total = checked_add(total, checked_mul(name.len(), 2)?)?;
                total = checked_add(total, trim_ascii(value).len())?;
            }
        }
    }
    Ok(total)
}

fn request_body_processor(headers: &[RequestHeader<'_>]) -> super::super::BodyProcessorSelection {
    processor_from_content_type(
        headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(b"content-type"))
            .map(|header| header.value),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seclang::{
        compile_with_includes, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink, IncludeSource,
        PhaseDefaults, SemanticValidationContext,
    };

    struct NoIncludes;

    impl IncludeSource for NoIncludes {
        fn load(&mut self, _request: IncludeRequest<'_>, _sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
            Err(IncludeFailure::NotFound)
        }
    }

    fn generation(source: &str) -> CompiledGeneration {
        let defaults = [
            Phase::RequestHeaders,
            Phase::RequestBody,
            Phase::ResponseHeaders,
            Phase::ResponseBody,
            Phase::Logging,
        ]
        .map(|phase| PhaseDefaults::pass(phase, true, true));
        let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap();
        compile_with_includes(
            "runtime-fixture",
            source.as_bytes(),
            &mut NoIncludes,
            None,
            context,
            FrontendLimits::default(),
        )
        .unwrap()
        .into_parts()
        .0
    }

    fn request<'a>(headers: &'a [RequestHeader<'a>], uri: &'a [u8]) -> RequestHead<'a> {
        RequestHead::new(
            b"GET /search?a=one&a=two%20words HTTP/1.1",
            b"GET",
            uri,
            uri,
            b"HTTP/1.1",
            b"192.0.2.1",
            b"request-1",
            headers,
        )
    }

    #[test]
    fn shared_generation_transaction_outlives_the_callers_handle() {
        let generation = Arc::new(generation(
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:1,phase:1,pass\"\n",
        ));
        let mut transaction =
            Transaction::new_shared(Arc::clone(&generation), request(&[], b"/"), RuntimeLimits::default()).unwrap();
        drop(generation);

        assert_eq!(
            transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
            1
        );
    }

    #[test]
    fn finding_storage_is_lazy_and_grows_only_after_matches() {
        let mut clean_source = String::new();
        let mut matching_source = String::new();
        for id in 1..=64 {
            clean_source.push_str(&format!(
                "SecRule REQUEST_URI \"@streq /never-{id}\" \"id:{id},phase:1,pass\"\n"
            ));
            matching_source.push_str(&format!(
                "SecRule REQUEST_URI \"@contains /\" \"id:{id},phase:1,pass\"\n"
            ));
        }

        let clean_generation = generation(&clean_source);
        let mut clean = Transaction::new(&clean_generation, request(&[], b"/clean"), RuntimeLimits::default()).unwrap();
        let clean_outcome = clean.execute_request_headers().unwrap();
        assert!(clean_outcome.findings.is_empty());
        assert_eq!(clean_outcome.findings.capacity(), 0);

        let matching_generation = generation(&matching_source);
        let limits = RuntimeLimits {
            max_findings: 17,
            ..RuntimeLimits::default()
        };
        let mut matching = Transaction::new(&matching_generation, request(&[], b"/match"), limits).unwrap();
        let matching_outcome = matching.execute_request_headers().unwrap();
        assert_eq!(matching_outcome.findings.len(), 17);
        assert_eq!(matching_outcome.omitted_findings, 47);
        assert_eq!(matching_outcome.omitted_loggable_findings, 47);
        assert!(matching_outcome.findings.capacity() >= matching_outcome.findings.len());
        assert!(matching_outcome.findings.capacity() <= 32);

        let silent_generation =
            generation("SecRule REQUEST_URI \"@contains /\" \"id:100,phase:1,pass,nolog,noauditlog\"\n");
        let silent_limits = RuntimeLimits {
            max_findings: 0,
            ..RuntimeLimits::default()
        };
        let mut silent = Transaction::new(&silent_generation, request(&[], b"/match"), silent_limits).unwrap();
        let silent_outcome = silent.execute_request_headers().unwrap();
        assert_eq!(silent_outcome.omitted_findings, 1);
        assert_eq!(silent_outcome.omitted_loggable_findings, 0);
        assert_eq!(silent_outcome.findings.capacity(), 0);
    }

    #[test]
    fn request_state_preserves_query_header_and_cookie_order() {
        let headers = [
            RequestHeader::new(b"X-Test", b"first"),
            RequestHeader::new(b"Cookie", b"a=one; b=two"),
            RequestHeader::new(b"X-Test", b"second"),
            RequestHeader::new(b"cookie", b"a=three"),
        ];
        let state = RequestState::build(
            request(&headers, b"/search?a=one&a=two%20words"),
            RuntimeLimits::default(),
        )
        .unwrap();
        assert_eq!(
            state.collection(CollectionKind::ArgsGet).collect::<Vec<_>>(),
            [
                (Some(b"a".as_slice()), b"one".as_slice()),
                (Some(b"a".as_slice()), b"two words".as_slice()),
            ]
        );
        assert_eq!(
            state.collection(CollectionKind::RequestCookies).collect::<Vec<_>>(),
            [
                (Some(b"a".as_slice()), b"one".as_slice()),
                (Some(b"b".as_slice()), b"two".as_slice()),
                (Some(b"a".as_slice()), b"three".as_slice()),
            ]
        );
        assert_eq!(
            state.collection(CollectionKind::RequestHeaders).collect::<Vec<_>>(),
            [
                (Some(b"X-Test".as_slice()), b"first".as_slice()),
                (Some(b"Cookie".as_slice()), b"a=one; b=two".as_slice()),
                (Some(b"X-Test".as_slice()), b"second".as_slice()),
                (Some(b"cookie".as_slice()), b"a=three".as_slice()),
            ]
        );
        let decoded_path = RequestState::build(
            request(&[], b"/get/php%20call%28%24x%29+tail?value=1"),
            RuntimeLimits::default(),
        )
        .unwrap();
        assert_eq!(
            decoded_path
                .collection(CollectionKind::RequestFilename)
                .collect::<Vec<_>>(),
            [(None, b"/get/php call($x)+tail".as_slice())]
        );
        assert_eq!(
            decoded_path
                .collection(CollectionKind::RequestBasename)
                .collect::<Vec<_>>(),
            [(None, b"php call($x)+tail".as_slice())]
        );
    }

    #[test]
    fn every_count_and_byte_limit_rejects_first_excess() {
        let headers = [RequestHeader::new(b"X", b"1"), RequestHeader::new(b"Y", b"2")];
        assert!(matches!(
            RequestState::build(
                request(&headers, b"/?a=1"),
                RuntimeLimits {
                    max_headers: 1,
                    ..RuntimeLimits::default()
                }
            ),
            Err(RuntimeError::HeaderLimit { actual: 2, limit: 1 })
        ));
        assert!(matches!(
            RequestState::build(
                request(&[], b"/?a=1&b=2"),
                RuntimeLimits {
                    max_query_arguments: 1,
                    ..RuntimeLimits::default()
                }
            ),
            Err(RuntimeError::QueryArgumentLimit { limit: 1 })
        ));
        let cookies = [RequestHeader::new(b"Cookie", b"a=1;b=2")];
        assert!(matches!(
            RequestState::build(
                request(&cookies, b"/"),
                RuntimeLimits {
                    max_cookies: 1,
                    ..RuntimeLimits::default()
                }
            ),
            Err(RuntimeError::CookieLimit { limit: 1 })
        ));
        let exact = RequestState::build(request(&headers[..1], b"/"), RuntimeLimits::default()).unwrap();
        assert!(matches!(
            RequestState::build(
                request(&headers[..1], b"/"),
                RuntimeLimits {
                    max_retained_bytes: exact.retained_bytes - 1,
                    ..RuntimeLimits::default()
                }
            ),
            Err(RuntimeError::RetainedByteLimit { .. })
        ));
    }

    #[test]
    fn invalid_headers_and_errors_are_content_free() {
        let private = [RequestHeader::new(b"Bad Name", b"private-value")];
        let Err(error) = RequestState::build(request(&private, b"/private-path"), RuntimeLimits::default()) else {
            panic!("private fixture must fail header validation");
        };
        assert_eq!(error, RuntimeError::InvalidHeaderName { ordinal: 0 });
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("private"));
        assert!(!rendered.contains("Bad Name"));
    }

    #[test]
    fn phase_cursor_is_monotonic() {
        assert_eq!(phase_from_number(1), Some(Phase::RequestHeaders));
        assert_eq!(phase_from_number(5), Some(Phase::Logging));
        assert_eq!(phase_from_number(6), None);
    }

    #[test]
    fn request_header_phase_executes_targets_transforms_and_operators_in_order() {
        let generation = generation(concat!(
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:1,phase:1,pass\"\n",
            "SecRule REQUEST_HEADERS:X-Test \"@pm first other\" \"id:2,phase:1,pass,t:lowercase\"\n",
            "SecRule &REQUEST_HEADERS \"@ge 2\" \"id:3,phase:1,pass\"\n",
            "SecRule REQUEST_URI \"@rx ^/private\" \"id:4,phase:1,deny,status:451\"\n",
        ));
        let headers = [
            RequestHeader::new(b"x-test", b"FIRST"),
            RequestHeader::new(b"Host", b"example.test"),
        ];
        let mut transaction = Transaction::new(
            &generation,
            request(&headers, b"/private?a=1"),
            RuntimeLimits::default(),
        )
        .unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert_eq!(outcome.intervention().map(Intervention::status), Some(451));
        assert_eq!(outcome.evaluated_units(), 4);
        assert_eq!(transaction.next_phase(), Some(Phase::RequestBody));
        assert!(matches!(
            transaction.execute_request_headers(),
            Err(RuntimeError::PhaseOrder {
                expected: Some(Phase::RequestBody),
                actual: Phase::RequestHeaders,
            })
        ));
    }

    #[test]
    fn detection_only_reports_deny_rules_without_short_circuiting_the_phase() {
        let generation = generation(concat!(
            "SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,deny,status:451\"\n",
            "SecRule REQUEST_URI \"@contains private\" \"id:2,phase:1,deny\"\n",
            "SecRule REQUEST_URI \"@contains private\" \"id:3,phase:1,pass\"\n",
        ));
        let mut transaction = Transaction::new_with_modes(
            &generation,
            request(&[], b"/private"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            RuleEngineMode::DetectionOnly,
        )
        .unwrap();

        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(outcome.intervention(), None);
        assert_eq!(outcome.evaluated_units(), 3);
    }

    #[test]
    fn negation_selector_exclusion_and_validation_operators_are_exact() {
        let generation = generation(concat!(
            "SecRule REQUEST_HEADERS|!REQUEST_HEADERS:Ignore \"!@contains forbidden\" \"id:10,phase:1,pass\"\n",
            "SecRule REQUEST_URI \"@validateUrlEncoding\" \"id:11,phase:1,pass\"\n",
            "SecRule REMOTE_ADDR \"@ipMatch 192.0.2.0/24\" \"id:12,phase:1,pass\"\n",
        ));
        let headers = [
            RequestHeader::new(b"Ignore", b"forbidden"),
            RequestHeader::new(b"Keep", b"safe"),
        ];
        let mut transaction = Transaction::new(
            &generation,
            request(&headers, b"/path?bad=%Q0"),
            RuntimeLimits::default(),
        )
        .unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [10, 11, 12]
        );
    }

    #[test]
    fn setvar_actions_are_ordered_visible_and_case_insensitive() {
        let generation = generation(concat!(
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:20,phase:1,pass,setvar:'tx.score=1',setvar:'tx.score=+2',setvar:'tx.copy=%{tx.score}'\"\n",
            "SecRule REQUEST_HEADERS:X-Score \"@eq %{TX.score}\" \"id:21,phase:1,pass,setvar:'!tx.copy'\"\n",
        ));
        let headers = [RequestHeader::new(b"X-Score", b"3")];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [20, 21]
        );
        assert_eq!(transaction.tx_value(b"SCORE"), Some(b"3".as_slice()));
        assert_eq!(transaction.tx_value(b"copy"), None);
    }

    #[test]
    fn failed_chain_and_failed_action_batch_do_not_leak_tx_mutations() {
        let failed_chain = generation(concat!(
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:22,phase:1,pass,chain,setvar:'tx.leak=1'\"\n",
            "SecRule REQUEST_URI \"@streq /never\"\n",
        ));
        let mut transaction =
            Transaction::new(&failed_chain, request(&[], b"/actual"), RuntimeLimits::default()).unwrap();
        assert!(transaction.execute_request_headers().unwrap().findings().is_empty());
        assert_eq!(transaction.tx_value(b"leak"), None);

        let overflow = generation(
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:23,phase:1,pass,setvar:'tx.score=1',setvar:'tx.score=+9223372036854775807'\"\n",
        );
        let mut transaction = Transaction::new(&overflow, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        assert!(matches!(
            transaction.execute_request_headers(),
            Err(RuntimeError::IntegerConversion)
        ));
        assert_eq!(transaction.tx_value(b"score"), None);
        assert_eq!(transaction.next_phase(), Some(Phase::RequestHeaders));
    }

    #[test]
    fn matcher_scratch_limit_is_checked_before_transaction_publication() {
        let generation = generation("SecRule REQUEST_URI \"@pm one two\" \"id:30,phase:1,pass\"\n");
        let baseline = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        assert!(baseline.scratch_bytes > 0);
        assert!(matches!(
            Transaction::new(
                &generation,
                request(&[], b"/"),
                RuntimeLimits {
                    max_matcher_scratch_bytes: baseline.scratch_bytes - 1,
                    ..RuntimeLimits::default()
                },
            ),
            Err(RuntimeError::MatcherScratchByteLimit { .. })
        ));
    }

    #[test]
    fn skip_and_skip_after_move_only_the_phase_schedule_cursor() {
        let generation = generation(concat!(
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:40,phase:1,pass,skip:1\"\n",
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:41,phase:1,pass\"\n",
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:42,phase:1,pass,skipAfter:END\"\n",
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:43,phase:1,pass\"\n",
            "SecMarker END\n",
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:44,phase:1,pass\"\n",
        ));
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [40, 42, 44]
        );
        assert_eq!(outcome.evaluated_units(), 3);
    }

    #[test]
    fn regex_and_phrase_captures_publish_bounded_tx_values_and_replace_prior_keys() {
        let generation = generation(concat!(
            "SecRule REQUEST_URI \"@rx ^/user/([a-z]+)-(?P<num>[0-9]+)\" \"id:50,phase:1,pass,capture,setvar:'tx.copy=%{TX.1}'\"\n",
            "SecRule REQUEST_HEADERS:X-Token \"@pm token\" \"id:51,phase:1,pass,capture\"\n",
        ));
        let headers = [RequestHeader::new(b"X-Token", b"prefix TOKEN suffix")];
        let mut transaction = Transaction::new(
            &generation,
            request(&headers, b"/user/alice-42"),
            RuntimeLimits::default(),
        )
        .unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [50, 51]
        );
        assert_eq!(transaction.tx_value(b"copy"), Some(b"alice".as_slice()));
        assert_eq!(transaction.tx_value(b"0"), Some(b"TOKEN".as_slice()));
        assert_eq!(transaction.tx_value(b"1"), None);
        assert_eq!(transaction.tx_value(b"num"), None);
    }

    #[test]
    fn injection_operators_compile_transform_capture_and_chain_atomically() {
        let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:101,phase:1,pass,t:urlDecodeUni,capture,chain,setvar:'tx.fingerprint=%{TX.0}'\"\n",
            "SecRule TX:0 \"@streq 1&1\"\n",
            "SecRule REQUEST_HEADERS:X-XSS \"@detectXSS\" \"id:102,phase:1,pass,capture,setvar:'tx.previous=%{TX.0}'\"\n",
        ));
        let headers = [
            RequestHeader::new(b"X-SQL", b"1%20OR%201%3D1"),
            RequestHeader::new(b"X-XSS", b"<script>alert(1)</script>"),
        ];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [101, 102]
        );
        assert_eq!(transaction.tx_value(b"0"), Some(b"1&1".as_slice()));
        assert_eq!(transaction.tx_value(b"fingerprint"), Some(b"1&1".as_slice()));
        assert_eq!(transaction.tx_value(b"previous"), Some(b"1&1".as_slice()));

        let failed = self::generation(concat!(
            "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:103,phase:1,pass,capture,chain\"\n",
            "SecRule REQUEST_METHOD \"@streq POST\"\n",
        ));
        let mut transaction = Transaction::new(&failed, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        assert!(transaction.execute_request_headers().unwrap().findings().is_empty());
        assert_eq!(transaction.tx_value(b"0"), None);
    }

    #[test]
    fn negated_detection_does_not_capture_and_detection_work_is_bounded() {
        let generation = generation("SecRule REQUEST_HEADERS:X-SQL \"!@detectSQLi\" \"id:104,phase:1,pass,capture\"\n");
        let headers = [RequestHeader::new(b"X-SQL", b"safe")];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        assert_eq!(
            transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
            104
        );
        assert_eq!(transaction.tx_value(b"0"), None);

        let budget_generation =
            self::generation("SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:106,phase:1,pass\"\n");
        let exact_work = 4096 + 10 * b"safe".len();
        let mut transaction = Transaction::new(
            &budget_generation,
            request(&headers, b"/"),
            RuntimeLimits {
                max_operator_work_bytes: exact_work,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        transaction.execute_request_headers().unwrap();
        let mut transaction = Transaction::new(
            &budget_generation,
            request(&headers, b"/"),
            RuntimeLimits {
                max_operator_work_bytes: exact_work - 1,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        assert!(matches!(
            transaction.execute_request_headers(),
            Err(RuntimeError::OperatorWorkLimit { .. })
        ));
    }

    #[test]
    fn decoded_query_high_byte_does_not_hide_later_xss() {
        let generation = generation("SecRule ARGS_GET:x \"@detectXSS\" \"id:107,phase:1,pass\"\n");
        let mut transaction = Transaction::new(
            &generation,
            request(&[], b"/?x=%3Ca%20%FFid%3Dx%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E"),
            RuntimeLimits::default(),
        )
        .unwrap();
        assert_eq!(
            transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
            107
        );

        let generation = self::generation("SecRule ARGS_GET:x \"@detectSQLi\" \"id:108,phase:1,pass\"\n");
        let mut transaction = Transaction::new(
            &generation,
            request(&[], b"/?x=%27-(f=q%27%BF"),
            RuntimeLimits::default(),
        )
        .unwrap();
        assert_eq!(
            transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
            108
        );

        let mut transaction = Transaction::new(
            &generation,
            request(&[], b"/?x=1%20OR%20q%27%BF%C2%BF%27"),
            RuntimeLimits::default(),
        )
        .unwrap();
        assert_eq!(
            transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
            108
        );
    }

    #[test]
    fn detector_work_accumulates_across_rules_and_phases() {
        let detector_work = crate::injection::work_cost(crate::injection::Kind::Sqli, b"safe".len()).unwrap();
        let one_rule_work = detector_work + 2 * b"safe".len();
        let headers = [RequestHeader::new(b"X-SQL", b"safe")];
        for rule_count in [1, 2, 100] {
            let source = (1..=rule_count)
                .map(|id| format!("SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:{id},phase:1,pass\"\n"))
                .collect::<String>();
            let generation = generation(&source);
            let limit = if rule_count == 1 {
                one_rule_work
            } else {
                (rule_count - 1) * detector_work + 2 * b"safe".len()
            };
            let mut transaction = Transaction::new(
                &generation,
                request(&headers, b"/"),
                RuntimeLimits {
                    max_operator_work_bytes: limit,
                    ..RuntimeLimits::default()
                },
            )
            .unwrap();
            let result = transaction.execute_request_headers();
            if rule_count == 1 {
                assert!(result.is_ok(), "{result:?}");
            } else {
                assert_eq!(result, Err(RuntimeError::OperatorWorkLimit { limit }));
            }
        }

        let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:201,phase:1,pass\"\n",
            "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:202,phase:2,pass\"\n",
        ));
        let mut transaction = Transaction::new(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits {
                max_operator_work_bytes: one_rule_work,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        transaction.execute_request_headers().unwrap();
        assert_eq!(
            transaction.execute_request_body_prefix(b"", 1, RequestBodyPrefixCompletion::NotRequired),
            Err(RuntimeError::OperatorWorkLimit { limit: one_rule_work })
        );
    }

    #[test]
    fn detector_work_accumulates_across_chain_members() {
        let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:203,phase:1,pass,chain\"\n",
            "SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\"\n",
        ));
        let value = b"1 OR 1=1";
        let headers = [RequestHeader::new(b"X-SQL", value)];
        let limit = crate::injection::work_cost(crate::injection::Kind::Sqli, value.len()).unwrap() + 2 * value.len();
        let mut transaction = Transaction::new(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits {
                max_operator_work_bytes: limit,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        assert_eq!(
            transaction.execute_request_headers(),
            Err(RuntimeError::OperatorWorkLimit { limit })
        );
        assert_eq!(transaction.tx_value(b"0"), None);
    }

    #[test]
    fn detector_input_and_capture_failures_do_not_publish_tx_state() {
        let generation = generation("SecRule REQUEST_HEADERS:X-SQL \"@detectSQLi\" \"id:105,phase:1,pass,capture\"\n");
        let headers = [RequestHeader::new(b"X-SQL", b"1 OR 1=1")];
        let mut transaction = Transaction::new(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits {
                max_capture_entries: 0,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        assert!(matches!(
            transaction.execute_request_headers(),
            Err(RuntimeError::CaptureEntryLimit { .. })
        ));
        assert_eq!(transaction.tx_value(b"0"), None);

        let large = vec![b'a'; crate::injection::MAX_INPUT_BYTES + 1];
        let headers = [RequestHeader::new(b"X-SQL", &large)];
        let mut transaction = Transaction::new(
            &generation,
            request(&headers, b"/"),
            RuntimeLimits {
                max_operator_work_bytes: usize::MAX,
                max_header_bytes: crate::injection::MAX_INPUT_BYTES + 1024,
                max_value_bytes: crate::injection::MAX_INPUT_BYTES + 1,
                max_transformation_output_bytes: crate::injection::MAX_INPUT_BYTES + 1,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        let result = transaction.execute_request_headers();
        assert!(
            matches!(
                &result,
                Err(RuntimeError::DetectorInputLimit { limit }) if *limit == crate::injection::MAX_INPUT_BYTES
            ),
            "{result:?}"
        );
        assert_eq!(transaction.tx_value(b"0"), None);
    }

    #[test]
    fn crs_extension_capture_is_visible_to_its_chain_child() {
        let direct = crate::matcher::CompiledCaptureRegexProgram::compile_one(
            r"\.([^.]+)$",
            crate::matcher::RegexProgramLimits::default(),
        )
        .unwrap();
        let mut direct_scratch = direct.create_scratch();
        let direct_captures = direct.captures(b"foo.bak", &mut direct_scratch).unwrap();
        assert_eq!(direct_captures.overall(), Some(3..7));
        assert_eq!(direct_captures.group(1), Some(4..7));
        let generation = generation(concat!(
            "SecAction \"id:1,phase:1,pass,nolog,setvar:'tx.restricted_extensions=.bak/ .sql/'\"\n",
            "SecRule REQUEST_BASENAME \"@rx \\.([^.]+)$\" \"id:2,phase:1,pass,capture,setvar:'tx.extension=.%{tx.1}/',chain\"\n",
            "SecRule TX:EXTENSION \"@within %{tx.restricted_extensions}\" \"t:none,t:lowercase\"\n",
        ));
        let mut transaction =
            Transaction::new(&generation, request(&[], b"/get/foo.bak"), RuntimeLimits::default()).unwrap();

        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(transaction.tx_value(b"extension"), Some(b".bak/".as_slice()));
    }

    #[test]
    fn chain_retries_later_root_values_after_an_earlier_child_failure() {
        let generation = generation(concat!(
            "SecAction \"id:1,phase:1,pass,nolog,setvar:'tx.restricted=/content-range/'\"\n",
            "SecRule REQUEST_HEADERS_NAMES \"@rx ^.*$\" \"id:2,phase:1,pass,capture,t:lowercase,setvar:'tx.header_%{tx.0}=/%{tx.0}/',chain\"\n",
            "SecRule TX:/^header_/ \"@within %{tx.restricted}\"\n",
        ));
        let headers = [
            RequestHeader::new(b"Accept", b"text/plain"),
            RequestHeader::new(b"Content-Range", b"bytes 0-1/2"),
        ];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();

        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(transaction.tx_value(b"header_accept"), None);
        assert_eq!(
            transaction.tx_value(b"header_content-range"),
            Some(b"/content-range/".as_slice())
        );
    }

    #[test]
    fn tx_regex_selectors_are_case_insensitive_like_literal_tx_selectors() {
        let generation = generation(concat!(
            "SecAction \"id:1,phase:1,pass,nolog,setvar:'tx.lowercase=value'\"\n",
            "SecRule TX:/^LOWERCASE$/ \"@streq value\" \"id:2,phase:1,pass\"\n",
        ));
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();

        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn seclang_regex_hex_escapes_match_raw_bytes_without_unicode_reencoding() {
        let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-Braced \"@rx \\x{bc}[^\\x{be}]*\\x{be}\" \"id:1,phase:1,pass\"\n",
            "SecRule REQUEST_HEADERS:X-Magic \"@rx \\xac\\xed\" \"id:2,phase:1,pass\"\n",
        ));
        let headers = [
            RequestHeader::new(b"X-Braced", b"\xbcvalue\xbe"),
            RequestHeader::new(b"X-Magic", b"\xac\xedpayload"),
        ];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();

        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn standalone_rule_applies_nondisruptive_actions_for_every_matching_collection_value() {
        let generation = generation(concat!(
            "SecAction \"id:1,phase:1,pass,nolog,setvar:'tx.counter=0'\"\n",
            "SecRule REQUEST_HEADERS_NAMES \"@rx ^X-Collect-.+$\" \"id:2,phase:1,pass,capture,setvar:'tx.name_%{tx.counter}=%{tx.0}',setvar:'tx.counter=+1'\"\n",
        ));
        let headers = [
            RequestHeader::new(b"X-Collect-One", b"first"),
            RequestHeader::new(b"X-Collect-Two", b"second"),
        ];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();

        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(transaction.tx_value(b"counter"), Some(b"2".as_slice()));
        assert_eq!(transaction.tx_value(b"name_0"), Some(b"X-Collect-One".as_slice()));
        assert_eq!(transaction.tx_value(b"name_1"), Some(b"X-Collect-Two".as_slice()));
    }

    #[test]
    fn chain_children_observe_staged_captures_and_setvars_before_atomic_commit() {
        let generation = generation(concat!(
            "SecRule REQUEST_URI \"@rx ^/user/([a-z]+)$\" \"id:52,phase:1,pass,capture,chain,setvar:'tx.root=%{TX.1}'\"\n",
            "SecRule REQUEST_HEADERS:X-User \"@streq %{TX.root}\"\n",
        ));
        let headers = [RequestHeader::new(b"X-User", b"alice")];
        let mut transaction =
            Transaction::new(&generation, request(&headers, b"/user/alice"), RuntimeLimits::default()).unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(outcome.findings().len(), 1);
        assert_eq!(transaction.tx_value(b"root"), Some(b"alice".as_slice()));
        assert_eq!(transaction.tx_value(b"1"), Some(b"alice".as_slice()));
    }

    #[test]
    fn matched_variable_macros_use_transformed_value_and_are_visible_inside_chains() {
        let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-User \"@streq alice\" \"id:55,phase:1,pass,t:lowercase,chain,setvar:'tx.value=%{MATCHED_VAR}',setvar:'tx.name=%{MATCHED_VAR_NAME}'\"\n",
            "SecRule TX:value \"@streq alice\" \"setvar:'tx.child_name=%{MATCHED_VAR_NAME}'\"\n",
        ));
        let headers = [RequestHeader::new(b"X-User", b"ALICE")];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(outcome.findings().len(), 1);
        assert_eq!(transaction.tx_value(b"value"), Some(b"alice".as_slice()));
        assert_eq!(
            transaction.tx_value(b"name"),
            Some(b"REQUEST_HEADERS:X-User".as_slice())
        );
        assert_eq!(transaction.tx_value(b"child_name"), Some(b"TX:value".as_slice()));
    }

    #[test]
    fn negated_operator_matches_the_first_nonmatching_selected_value() {
        let generation = generation(
            "SecRule REQUEST_HEADERS:X-Test \"!@streq forbidden\" \"id:56,phase:1,pass,setvar:'tx.value=%{MATCHED_VAR}'\"\n",
        );
        let headers = [
            RequestHeader::new(b"X-Test", b"forbidden"),
            RequestHeader::new(b"X-Test", b"allowed"),
        ];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(outcome.findings().len(), 1);
        assert_eq!(transaction.tx_value(b"value"), Some(b"allowed".as_slice()));

        let mut empty = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        assert!(empty.execute_request_headers().unwrap().findings().is_empty());
    }

    #[test]
    fn message_and_logdata_templates_are_validated_without_exposing_payloads() {
        let generation = generation(
            "SecRule REQUEST_HEADERS:X-Secret \"@contains token\" \"id:57,phase:1,pass,msg:'matched %{MATCHED_VAR_NAME}',logdata:'value %{MATCHED_VAR}'\"\n",
        );
        let headers = [RequestHeader::new(b"X-Secret", b"private-token")];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(outcome.findings().len(), 1);
        let rendered = format!("{outcome:?} {transaction:?}");
        assert!(!rendered.contains("private"));
        assert!(!rendered.contains("X-Secret"));
    }

    #[test]
    fn matched_vars_collection_accumulates_only_within_the_current_chain() {
        let generation = generation(concat!(
            "SecRule ARGS_GET:first \"@streq one\" \"id:58,phase:1,pass,chain\"\n",
            "SecRule ARGS_GET:second \"@streq two\" \"chain\"\n",
            "SecRule MATCHED_VARS:ARGS_GET:first \"@streq one\"\n",
            "SecRule MATCHED_VARS \"@streq one\" \"id:59,phase:1,pass\"\n",
        ));
        let mut transaction = Transaction::new(
            &generation,
            request(&[], b"/?first=one&second=two"),
            RuntimeLimits::default(),
        )
        .unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [58]
        );
    }

    #[test]
    fn multi_match_evaluates_raw_and_each_changed_transformation_prefix() {
        let generation = generation(
            "SecRule REQUEST_HEADERS:X-Test \"@streq ABC\" \"id:53,phase:1,pass,multiMatch,t:lowercase,t:length\"\n",
        );
        let headers = [RequestHeader::new(b"X-Test", b"ABC")];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(outcome.findings().len(), 1);
        assert_eq!(outcome.findings()[0].rule_id(), 53);
    }

    #[test]
    fn findings_expose_only_bounded_safe_metadata_shape() {
        let generation = generation(
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:54,phase:1,pass,severity:'CRITICAL',tag:'private-category',tag:'second'\"\n",
        );
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        let outcome = transaction.execute_request_headers().unwrap();
        assert_eq!(
            outcome.findings()[0].severity(),
            Some(super::super::super::Severity::Critical)
        );
        assert_eq!(outcome.findings()[0].tag_count(), 2);
        let rendered = format!("{:?}", outcome.findings()[0]);
        assert!(!rendered.contains("private-category"));
    }

    #[test]
    fn request_body_publication_extends_args_and_executes_phase_two_once() {
        let generation = generation(concat!(
            "SecRule REQUEST_URI \"@contains item=from-query\" \"id:60,phase:1,pass\"\n",
            "SecRule ARGS:item \"@streq from-body\" \"id:61,phase:2,pass\"\n",
            "SecRule REQUEST_BODY \"@contains item=from-body\" \"id:62,phase:2,pass\"\n",
            "SecRule REQBODY_PROCESSOR \"@streq URLENCODED\" \"id:63,phase:2,pass\"\n",
            "SecRule ARGS_COMBINED_SIZE \"@eq 27\" \"id:68,phase:2,pass\"\n",
        ));
        let headers = [RequestHeader::new(
            b"Content-Type",
            b"application/x-www-form-urlencoded",
        )];
        let mut transaction = Transaction::new(
            &generation,
            request(&headers, b"/?item=from-query"),
            RuntimeLimits::default(),
        )
        .unwrap();
        assert_eq!(
            transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
            60
        );
        transaction
            .publish_request_body(RequestBody::new(
                b"item=from-body",
                super::super::super::BodyProcessorSelection::Auto,
            ))
            .unwrap();
        let outcome = transaction.execute_request_body().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [61, 62, 63, 68]
        );
        assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
    }

    #[test]
    fn prefix_mixes_header_and_body_rules_in_schedule_order() {
        let headers = [RequestHeader::new(b"X-Test", b"blocked")];
        for targets in [
            ["REQUEST_HEADERS:X-Test", "REQUEST_BODY"],
            ["REQUEST_BODY", "REQUEST_HEADERS:X-Test"],
        ] {
            let rules = format!("SecRule {} \"@contains blocked\" \"id:1,phase:2,deny,status:409\"\nSecRule {} \"@contains blocked\" \"id:2,phase:2,deny,status:403\"\n", targets[0], targets[1]);
            let generation = generation(&rules);
            assert!(generation.requires_complete_request_body());
            assert!(generation.request_body_execution_summary().requires_request_body());
            for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                let mut transaction = Transaction::new_with_modes(
                    &generation,
                    request(&headers, b"/"),
                    RuntimeLimits::default(),
                    EvaluationMode::Accelerated,
                    mode,
                )
                .unwrap();
                transaction.execute_request_headers().unwrap();
                let outcome = transaction
                    .execute_request_body_prefix(b"blocked", 7, RequestBodyPrefixCompletion::EndOfStream)
                    .unwrap();
                if mode == RuleEngineMode::On {
                    assert_eq!(
                        outcome.intervention,
                        Some(Intervention {
                            rule_id: 1,
                            status: 409
                        })
                    );
                    assert_eq!(outcome.evaluated_units, 1);
                } else {
                    assert_eq!(
                        outcome
                            .findings
                            .iter()
                            .map(|finding| finding.rule_id)
                            .collect::<Vec<_>>(),
                        vec![1, 2]
                    );
                    assert_eq!(outcome.intervention, None);
                }
            }
        }
    }

    #[test]
    fn prefix_not_required_still_executes_phase_two_header_rules() {
        let generation = generation("SecRule REQUEST_HEADERS:X-Test \"@streq blocked\" \"id:1,phase:2,deny\"\n");
        let headers = [RequestHeader::new(b"X-Test", b"blocked")];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        let outcome = transaction
            .execute_request_body_prefix(b"", 64, RequestBodyPrefixCompletion::NotRequired)
            .unwrap();
        assert_eq!(
            outcome.intervention,
            Some(Intervention {
                rule_id: 1,
                status: 403
            })
        );
        assert!(transaction.request.body.is_none());
        assert_eq!(
            transaction.request_prefix_completion(),
            Some(RequestBodyPrefixCompletion::NotRequired)
        );
        assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
    }

    #[test]
    fn prefix_short_circuits_deny_before_unreachable_work() {
        let rules: String = (1..=33)
            .map(|id| format!("SecRule REQUEST_BODY \"@contains blocked\" \"id:{id},phase:2,deny,status:403\"\n"))
            .collect();
        let generation = generation(&rules);
        let mut payload = vec![b'x'; MAX_REQUEST_BODY_PREFIX_BYTES];
        payload[..7].copy_from_slice(b"blocked");
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        let outcome = transaction
            .execute_request_body_prefix(&payload, payload.len(), RequestBodyPrefixCompletion::WindowFilled)
            .unwrap();
        assert_eq!(outcome.evaluated_units, 1);
        let mut full = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        full.execute_request_headers().unwrap();
        full.publish_request_body(RequestBody::new(
            &payload,
            super::super::super::BodyProcessorSelection::Auto,
        ))
        .unwrap();
        let full_outcome = full.execute_request_body().unwrap();
        assert_eq!(outcome.intervention, full_outcome.intervention);
        assert_eq!(outcome.findings, full_outcome.findings);
        assert_eq!(
            outcome.intervention,
            Some(Intervention {
                rule_id: 1,
                status: 403
            })
        );

        for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
            let mut transaction = Transaction::new_with_modes(
                &generation,
                request(&[], b"/"),
                RuntimeLimits::default(),
                EvaluationMode::Accelerated,
                mode,
            )
            .unwrap();
            transaction.execute_request_headers().unwrap();
            if mode == RuleEngineMode::On {
                payload[..7].fill(b'x');
            } else {
                payload[..7].copy_from_slice(b"blocked");
            }
            assert!(matches!(
                transaction.execute_request_body_prefix(
                    &payload,
                    payload.len(),
                    RequestBodyPrefixCompletion::WindowFilled
                ),
                Err(RuntimeError::OperatorWorkLimit { .. })
            ));
            assert_eq!(transaction.request_prefix_completion(), None);
        }
    }

    #[test]
    fn prefix_borrowed_input_preserves_output_limit_and_detection_findings() {
        let generation = generation(concat!(
            "SecRule REQUEST_BODY \"@contains blocked\" \"id:1,phase:2,deny\"\n",
            "SecRule REQUEST_BODY \"@contains blocked\" \"id:2,phase:2,deny\"\n",
        ));
        let limits = RuntimeLimits {
            max_transformation_output_bytes: 6,
            ..RuntimeLimits::default()
        };
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), limits).unwrap();
        transaction.execute_request_headers().unwrap();
        assert!(matches!(
            transaction.execute_request_body_prefix(b"blocked", 7, RequestBodyPrefixCompletion::EndOfStream),
            Err(RuntimeError::Transformation {
                kind: RuntimeTransformError::OutputLimit
            })
        ));
        let mut transaction = Transaction::new_with_modes(
            &generation,
            request(&[], b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            RuleEngineMode::DetectionOnly,
        )
        .unwrap();
        transaction.execute_request_headers().unwrap();
        let outcome = transaction
            .execute_request_body_prefix(b"blocked", 7, RequestBodyPrefixCompletion::EndOfStream)
            .unwrap();
        assert_eq!(
            outcome
                .findings
                .iter()
                .map(|finding| finding.rule_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(outcome.evaluated_units, 2);
        assert_eq!(outcome.intervention, None);
    }

    #[test]
    fn prefix_configurable_window_matches_at_the_boundary_up_to_one_mib() {
        let generation = generation("SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,t:lowercase\"\n");
        for limit in [16 * 1024, 100 * 1024, MAX_REQUEST_BODY_PREFIX_BYTES] {
            let mut payload = vec![b'x'; limit];
            payload[limit - 9..].copy_from_slice(b"FORBIDDEN");
            let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
            transaction.execute_request_headers().unwrap();
            let outcome = transaction
                .execute_request_body_prefix(&payload, limit, RequestBodyPrefixCompletion::WindowFilled)
                .unwrap();
            assert!(outcome.intervention().is_some());
            assert_eq!(outcome.findings().len(), 1);
        }
        for limit in [0, MAX_REQUEST_BODY_PREFIX_BYTES + 1] {
            let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
            transaction.execute_request_headers().unwrap();
            assert!(transaction
                .execute_request_body_prefix(b"", limit, RequestBodyPrefixCompletion::EndOfStream)
                .is_err());
        }
    }

    #[test]
    fn prefix_completion_preserves_body_absence_and_advances_phase_once() {
        let generation = generation(concat!(
            "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,status:403\"\n",
            "SecRule RESPONSE_STATUS \"@eq 200\" \"id:2,phase:3,pass\"\n",
        ));
        assert_eq!(generation.validate_request_body_prefix(), Ok(()));
        for completion in [
            RequestBodyPrefixCompletion::EndOfStream,
            RequestBodyPrefixCompletion::WindowFilled,
        ] {
            let mut bytes = vec![b'x'; DEFAULT_REQUEST_BODY_PREFIX_BYTES];
            bytes[DEFAULT_REQUEST_BODY_PREFIX_BYTES - 9..].copy_from_slice(b"forbidden");
            let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
            transaction.execute_request_headers().unwrap();
            let outcome = transaction
                .execute_request_body_prefix(&bytes, DEFAULT_REQUEST_BODY_PREFIX_BYTES, completion)
                .unwrap();
            assert_eq!(outcome.findings().len(), 1);
            assert!(outcome.intervention().is_some());
            assert!(transaction.request.body.is_none());
            assert_eq!(transaction.request_prefix_completion(), Some(completion));
            assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
            assert!(transaction
                .execute_request_body_prefix(&bytes, DEFAULT_REQUEST_BODY_PREFIX_BYTES, completion)
                .is_err());
            transaction.publish_response_head(ResponseHead::new(200, &[])).unwrap();
            assert_eq!(
                transaction.execute_response_headers().unwrap().findings()[0].rule_id(),
                2
            );
        }
    }

    #[test]
    fn prefix_detection_only_uses_pm_transform_and_short_body_completion() {
        let generation =
            generation("SecRule REQUEST_BODY \"@pm forbidden blocked\" \"id:1,phase:2,deny,t:lowercase\"\n");
        let mut transaction = Transaction::new_with_modes(
            &generation,
            request(&[], b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            RuleEngineMode::DetectionOnly,
        )
        .unwrap();
        transaction.execute_request_headers().unwrap();
        let outcome = transaction
            .execute_request_body_prefix(
                b"BLOCKED",
                DEFAULT_REQUEST_BODY_PREFIX_BYTES,
                RequestBodyPrefixCompletion::EndOfStream,
            )
            .unwrap();
        assert_eq!(outcome.findings().len(), 1);
        assert!(outcome.intervention().is_none());
        assert!(transaction.request.body.is_none());
    }

    #[test]
    fn prefix_rejects_false_window_completion_and_supports_no_body_rules() {
        let generation = generation("SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,deny\"\n");
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        assert!(transaction
            .execute_request_body_prefix(
                b"short",
                DEFAULT_REQUEST_BODY_PREFIX_BYTES,
                RequestBodyPrefixCompletion::WindowFilled
            )
            .is_err());
        assert!(transaction
            .execute_request_body_prefix(
                &[],
                DEFAULT_REQUEST_BODY_PREFIX_BYTES,
                RequestBodyPrefixCompletion::NotRequired
            )
            .is_ok());
        assert!(transaction.request.body.is_none());
    }

    #[test]
    fn prefix_admission_checks_only_the_selected_window_and_enforces_limits() {
        let generation = generation("SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny\"\n");
        let mut payload = vec![b'x'; DEFAULT_REQUEST_BODY_PREFIX_BYTES];
        payload.extend_from_slice(b"forbidden");
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        assert!(transaction
            .execute_request_body_prefix(
                &payload,
                DEFAULT_REQUEST_BODY_PREFIX_BYTES,
                RequestBodyPrefixCompletion::EndOfStream
            )
            .is_err());
        assert!(transaction
            .execute_request_body_prefix(
                &[],
                DEFAULT_REQUEST_BODY_PREFIX_BYTES,
                RequestBodyPrefixCompletion::NotRequired
            )
            .is_err());
        let outcome = transaction
            .execute_request_body_prefix(
                &payload[..DEFAULT_REQUEST_BODY_PREFIX_BYTES],
                DEFAULT_REQUEST_BODY_PREFIX_BYTES,
                RequestBodyPrefixCompletion::WindowFilled,
            )
            .unwrap();
        assert!(outcome.findings().is_empty());
        assert!(outcome.intervention().is_none());
        assert!(transaction.request.body.is_none());
    }

    #[test]
    fn raw_phrase_body_stream_is_chunk_exact_and_constant_memory() {
        let generation = generation(concat!(
            "SecRule REQUEST_BODY \"@pm blocked\" \"id:70,phase:2,deny,status:406,t:lowercase\"\n",
            "SecRule REQUEST_BODY \"!@pm present\" \"id:71,phase:2,pass\"\n",
            "SecRule REQUEST_METHOD \"@streq GET\" \"id:72,phase:2,pass\"\n",
        ));
        let mut transaction = Transaction::new_with_modes(
            &generation,
            request(&[], b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            RuleEngineMode::DetectionOnly,
        )
        .unwrap();
        transaction.execute_request_headers().unwrap();
        transaction.start_request_body_stream().unwrap();
        let retained = transaction.retained_bytes();
        assert_eq!(
            transaction
                .execute_request_body_stream_chunk(b"prefix-BLO", false)
                .unwrap(),
            None
        );
        assert_eq!(transaction.retained_bytes(), retained);
        assert_eq!(
            transaction.execute_request_body_stream_chunk(b"CKED-", false).unwrap(),
            None
        );
        assert_eq!(
            transaction
                .execute_request_body_stream_chunk(&vec![b'z'; 1024 * 1024], false)
                .unwrap(),
            None
        );
        assert_eq!(transaction.retained_bytes(), retained);
        let outcome = transaction
            .execute_request_body_stream_chunk(b"suffix", true)
            .unwrap()
            .expect("EOS completes Phase 2");
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [70, 71, 72]
        );
        assert_eq!(outcome.intervention(), None);
        assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
    }

    #[test]
    fn raw_phrase_body_stream_preserves_blocking_status() {
        let generation = generation("SecRule REQUEST_BODY \"@pm cross-boundary\" \"id:73,phase:2,deny,status:409\"\n");
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        transaction.start_request_body_stream().unwrap();
        transaction.execute_request_body_stream_chunk(b"cross-", false).unwrap();
        let outcome = transaction
            .execute_request_body_stream_chunk(b"boundary", false)
            .unwrap()
            .expect("the sole positive raw rule completes Phase 2 immediately");
        assert_eq!(outcome.intervention().map(Intervention::status), Some(409));
        assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
    }

    #[test]
    fn raw_phrase_body_stream_enforces_body_and_work_limits_before_scanning_excess() {
        let generation = generation("SecRule REQUEST_BODY \"@pm bounded\" \"id:74,phase:2,pass\"\n");
        let mut body_limited = Transaction::new(
            &generation,
            request(&[], b"/"),
            RuntimeLimits {
                body: super::super::super::BodyLimits {
                    max_body_bytes: 3,
                    ..super::super::super::BodyLimits::default()
                },
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        body_limited.execute_request_headers().unwrap();
        body_limited.start_request_body_stream().unwrap();
        assert_eq!(
            body_limited.execute_request_body_stream_chunk(b"four", false),
            Err(RuntimeError::Body {
                kind: BodyError::ByteLimit { actual: 4, limit: 3 }
            })
        );

        let mut work_limited = Transaction::new(
            &generation,
            request(&[], b"/"),
            RuntimeLimits {
                max_operator_work_bytes: 3,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        work_limited.execute_request_headers().unwrap();
        work_limited.start_request_body_stream().unwrap();
        assert_eq!(
            work_limited.execute_request_body_stream_chunk(b"four", false),
            Err(RuntimeError::OperatorWorkLimit { limit: 3 })
        );
    }

    #[test]
    fn request_body_processor_is_available_from_content_type_in_phase_one() {
        let generation = generation("SecRule REQBODY_PROCESSOR \"@streq MULTIPART\" \"id:1,phase:1,pass\"\n");
        let headers = [RequestHeader::new(
            b"Content-Type",
            b"multipart/form-data; boundary=example",
        )];
        let mut transaction = Transaction::new(&generation, request(&headers, b"/"), RuntimeLimits::default()).unwrap();

        assert_eq!(
            transaction.execute_request_headers().unwrap().findings()[0].rule_id(),
            1
        );
    }

    #[test]
    fn json_and_xml_body_views_feed_args_and_selected_xml_paths() {
        let json_generation = generation(concat!(
            "SecRule ARGS:json.user.name \"@streq alice\" \"id:64,phase:2,pass\"\n",
            "SecRule ARGS:json.roles \"@eq 2\" \"id:65,phase:2,pass\"\n",
        ));
        let mut json = Transaction::new(&json_generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        json.execute_request_headers().unwrap();
        json.publish_request_body(RequestBody::new(
            br#"{"user":{"name":"alice"},"roles":["a","b"]}"#,
            super::super::super::BodyProcessorSelection::Json,
        ))
        .unwrap();
        assert_eq!(json.execute_request_body().unwrap().findings().len(), 2);

        let xml_generation = generation(concat!(
            "SecRule XML:/* \"@contains attack\" \"id:66,phase:2,pass\"\n",
            "SecRule XML://@* \"@streq dangerous\" \"id:67,phase:2,pass\"\n",
        ));
        let mut xml = Transaction::new(&xml_generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        xml.execute_request_headers().unwrap();
        xml.publish_request_body(RequestBody::new(
            br#"<root mode="dangerous"><item>attack</item></root>"#,
            super::super::super::BodyProcessorSelection::Xml,
        ))
        .unwrap();
        let outcome = xml.execute_request_body().unwrap();
        assert_eq!(
            outcome
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [66, 67]
        );
    }

    #[test]
    fn failed_body_publication_is_atomic_and_can_be_retried() {
        let generation = generation("SecRule ARGS:a \"@streq ok\" \"id:69,phase:2,pass\"\n");
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        assert!(matches!(
            transaction.publish_request_body(RequestBody::new(
                b"{private",
                super::super::super::BodyProcessorSelection::Json,
            )),
            Err(RuntimeError::Body {
                kind: BodyError::Malformed {
                    processor: super::super::super::BodyProcessorSelection::Json
                }
            })
        ));
        assert!(transaction.request.body.is_none());
        assert_eq!(transaction.next_phase(), Some(Phase::RequestBody));
        transaction
            .publish_request_body(RequestBody::new(
                b"a=ok",
                super::super::super::BodyProcessorSelection::UrlEncoded,
            ))
            .unwrap();
        assert_eq!(transaction.execute_request_body().unwrap().findings()[0].rule_id(), 69);
    }

    #[test]
    fn detection_only_retains_malformed_body_signal_and_executes_raw_phase_two_rules() {
        let generation = generation(concat!(
            "SecRule REQBODY_PROCESSOR \"@streq JSON\" \"id:1,phase:2,pass,chain\"\n",
            "SecRule REQUEST_BODY \"@contains private\"\n",
        ));
        let mut transaction = Transaction::new_with_modes(
            &generation,
            request(&[], b"/"),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            RuleEngineMode::DetectionOnly,
        )
        .unwrap();
        transaction.execute_request_headers().unwrap();
        transaction
            .publish_request_body(RequestBody::new(
                b"{private",
                super::super::super::BodyProcessorSelection::Json,
            ))
            .unwrap();

        assert!(matches!(
            transaction.request_body_error(),
            Some(BodyError::Malformed {
                processor: super::super::super::BodyProcessorSelection::Json
            })
        ));
        assert_eq!(transaction.execute_request_body().unwrap().findings()[0].rule_id(), 1);
    }

    #[test]
    fn response_headers_windows_and_logging_advance_without_body_aggregation() {
        let generation = generation(concat!(
            "SecRule RESPONSE_STATUS \"@eq 404\" \"id:70,phase:3,pass\"\n",
            "SecRule RESPONSE_HEADERS:X-Test \"@streq value\" \"id:71,phase:3,pass\"\n",
            "SecAction \"id:72,phase:4,pass,setvar:'tx.windows=+1'\"\n",
            "SecRule TX:windows \"@eq 2\" \"id:73,phase:5,pass\"\n",
        ));
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        transaction
            .publish_request_body(RequestBody::new(b"", super::super::super::BodyProcessorSelection::Raw))
            .unwrap();
        transaction.execute_request_body().unwrap();
        let response_headers = [ResponseHeader::new(b"x-test", b"value")];
        transaction
            .publish_response_head(ResponseHead::new(404, &response_headers))
            .unwrap();
        let headers = transaction.execute_response_headers().unwrap();
        assert_eq!(
            headers
                .findings()
                .iter()
                .map(|finding| finding.rule_id())
                .collect::<Vec<_>>(),
            [70, 71]
        );
        let first = transaction
            .execute_response_window(ResponseWindow::new(b"private-window", false))
            .unwrap();
        assert_eq!(first.findings()[0].rule_id(), 72);
        assert_eq!(transaction.next_phase(), Some(Phase::ResponseBody));
        assert!(transaction.response.as_ref().unwrap().window.is_none());
        let last = transaction
            .execute_response_window(ResponseWindow::new(b"", true))
            .unwrap();
        assert_eq!(last.findings()[0].rule_id(), 72);
        assert_eq!(transaction.next_phase(), Some(Phase::Logging));
        assert!(transaction.response.as_ref().unwrap().window.is_none());
        assert_eq!(transaction.execute_logging().unwrap().findings()[0].rule_id(), 73);
        assert_eq!(transaction.next_phase(), None);
        assert!(!format!("{transaction:?}").contains("private-window"));
    }

    #[test]
    fn response_limits_are_content_free_and_window_failure_does_not_retain_bytes() {
        let generation = generation("");
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), RuntimeLimits::default()).unwrap();
        transaction.execute_request_headers().unwrap();
        transaction
            .publish_request_body(RequestBody::new(b"", super::super::super::BodyProcessorSelection::Raw))
            .unwrap();
        transaction.execute_request_body().unwrap();
        let private = [ResponseHeader::new(b"X-Private", b"private-value")];
        assert!(matches!(
            transaction.publish_response_head(ResponseHead::new(99, &private)),
            Err(RuntimeError::InvalidResponseStatus { status: 99 })
        ));
        transaction
            .publish_response_head(ResponseHead::new(200, &private))
            .unwrap();
        transaction.execute_response_headers().unwrap();
        let error = transaction
            .execute_response_window(ResponseWindow::new(b"private", false))
            .unwrap();
        assert!(error.findings().is_empty());
        assert!(transaction.response.as_ref().unwrap().window.is_none());

        let limited = RuntimeLimits {
            max_response_window_bytes: 6,
            ..RuntimeLimits::default()
        };
        let mut transaction = Transaction::new(&generation, request(&[], b"/"), limited).unwrap();
        transaction.execute_request_headers().unwrap();
        transaction
            .publish_request_body(RequestBody::new(b"", super::super::super::BodyProcessorSelection::Raw))
            .unwrap();
        transaction.execute_request_body().unwrap();
        transaction.publish_response_head(ResponseHead::new(200, &[])).unwrap();
        transaction.execute_response_headers().unwrap();
        let error = transaction
            .execute_response_window(ResponseWindow::new(b"private", false))
            .unwrap_err();
        assert_eq!(error, RuntimeError::ResponseWindowByteLimit { actual: 7, limit: 6 });
        assert!(!format!("{error:?} {error}").contains("private"));
        assert!(transaction.response.as_ref().unwrap().window.is_none());
    }

    #[test]
    fn accelerated_and_reference_transactions_are_outcome_equivalent() {
        let generation = generation(concat!(
            "SecRule REQUEST_HEADERS:X-Probe \"@pm alpha beta gamma\" \"id:80,phase:1,pass,t:lowercase,setvar:'tx.score=+1'\"\n",
            "SecRule REQUEST_HEADERS:X-Probe \"@pm delta omega theta\" \"id:81,phase:1,pass,t:lowercase,setvar:'tx.score=+2'\"\n",
            "SecRule REQUEST_HEADERS:X-Probe \"@rx (alpha|delta)[0-9]+\" \"id:82,phase:1,pass,t:lowercase,setvar:'tx.score=+4'\"\n",
            "SecRule REQUEST_HEADERS:X-Probe \"@rx (omega|theta)-[a-z]+\" \"id:83,phase:1,pass,t:lowercase,setvar:'tx.score=+8'\"\n",
            "SecRule REQUEST_HEADERS:X-Probe \"@pm alpha delta\" \"id:84,phase:1,pass,t:lowercase,capture,setvar:'tx.capture=%{TX.0}'\"\n",
        ));
        let mut seed = 0x07c3_19e2_d830_2026_u64;
        for case in 0..512 {
            let mut value = Vec::with_capacity(192);
            for _ in 0..128 {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                value.push(b'a' + ((seed >> 33) % 26) as u8);
            }
            match case % 8 {
                0 => value.extend_from_slice(b" ALPHA42"),
                1 => value.extend_from_slice(b" delta7"),
                2 => value.extend_from_slice(b" omega-value"),
                3 => value.extend_from_slice(b" theta-test"),
                4 => value.extend_from_slice(b" gamma"),
                5 => value.extend_from_slice(b" ALPHA delta"),
                _ => {}
            }
            let headers = [RequestHeader::new(b"X-Probe", &value)];
            let mut accelerated = Transaction::new_with_evaluation(
                &generation,
                request(&headers, b"/"),
                RuntimeLimits::default(),
                EvaluationMode::Accelerated,
            )
            .unwrap();
            let mut reference = Transaction::new_with_evaluation(
                &generation,
                request(&headers, b"/"),
                RuntimeLimits::default(),
                EvaluationMode::Reference,
            )
            .unwrap();
            let accelerated_outcome = accelerated.execute_request_headers().unwrap();
            let reference_outcome = reference.execute_request_headers().unwrap();
            assert_eq!(accelerated_outcome, reference_outcome, "case={case}");
            assert_eq!(
                accelerated.tx_value(b"score"),
                reference.tx_value(b"score"),
                "case={case}"
            );
            assert_eq!(
                accelerated.tx_value(b"capture"),
                reference.tx_value(b"capture"),
                "case={case}"
            );
        }
    }
}
