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
use super::{CollectionKind, CompiledGeneration, Phase};

pub use json_inspection::ResultLimits as JsonResultLimits;
pub use json_inspection_runtime::{JsonInspectionLimits, JsonInspectionSession, JsonInspectionUsage, JsonResultLimit};

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
    CtlProcessor,
    Json(CandidateId),
    XmlText(CandidateId),
    XmlAttribute(CandidateId),
    XmlMetadata,
    JsonMetadata,
    Count { digits: [u8; 40], start: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectedName {
    Json(CandidateId),
    Xml(CandidateId),
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
        json_budget: Option<&mut InspectionBudget>,
    ) -> Result<&'value [u8], RuntimeError> {
        match &self.value {
            SelectedBytes::Json(id) => {
                let body = request
                    .body
                    .as_ref()
                    .and_then(RequestBodyPublication::json)
                    .ok_or(RuntimeError::JsonInspectionState)?;
                let candidate = body
                    .results()?
                    .candidate(json_budget.ok_or(RuntimeError::JsonInspectionState)?, *id)
                    .map_err(map_store_error)?;
                if self.collection == CollectionKind::ArgsNames {
                    Ok(candidate.name)
                } else {
                    candidate.deferred.ok_or(RuntimeError::JsonInspectionState)
                }
            }
            SelectedBytes::XmlText(_) | SelectedBytes::XmlAttribute(_) => Err(RuntimeError::XmlInspectionState),
            SelectedBytes::XmlMetadata => request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::xml)
                .and_then(|body| body.metadata_value(self.collection))
                .ok_or(RuntimeError::XmlInspectionState),
            SelectedBytes::JsonMetadata => request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::json)
                .and_then(|body| body.metadata_value(self.collection))
                .ok_or(RuntimeError::JsonInspectionState),
            SelectedBytes::Request(range) => Ok(request.bytes_at(*range)),
            SelectedBytes::Body(index) => request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::materialized)
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
                .ok_or(RuntimeError::GenerationInvariant)?
                .value
                .bytes(request, json_budget),
            SelectedBytes::MatchedVar => tx
                .matched_var
                .as_ref()
                .ok_or(RuntimeError::GenerationInvariant)?
                .bytes(request, json_budget),
            SelectedBytes::MatchedVars(index) => tx
                .matched_vars
                .get(*index)
                .ok_or(RuntimeError::GenerationInvariant)?
                .value
                .bytes(request, json_budget),
            SelectedBytes::CtlProcessor => tx
                .request_body_processor
                .map(super::super::BodyProcessorSelection::as_bytes)
                .ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::Count { digits, start } => Ok(&digits[usize::from(*start)..]),
        }
    }

    fn name_bytes<'value>(
        self,
        request: &'value RequestState,
        tx: &'value TxState,
        response: Option<&'value ResponseState>,
        json_budget: Option<&mut InspectionBudget>,
    ) -> Result<Option<&'value [u8]>, RuntimeError> {
        selected_name_bytes(self.name, request, tx, response, json_budget)
    }
}

fn inspection_error(request: &RequestState, error: RuntimeError) -> RuntimeError {
    if request
        .body
        .as_ref()
        .and_then(RequestBodyPublication::structured_results)
        .is_some_and(|results| results.domain() == InspectionDomain::Xml)
        || matches!(request.body, Some(RequestBodyPublication::Xml(_)))
    {
        xml_inspection_runtime::xml_error(error)
    } else {
        error
    }
}

fn selected_name_bytes<'value>(
    name: Option<SelectedName>,
    request: &'value RequestState,
    tx: &'value TxState,
    response: Option<&'value ResponseState>,
    json_budget: Option<&mut InspectionBudget>,
) -> Result<Option<&'value [u8]>, RuntimeError> {
    name.map(|name| match name {
        SelectedName::Json(id) | SelectedName::Xml(id) => request
            .body
            .as_ref()
            .and_then(RequestBodyPublication::structured_results)
            .ok_or(RuntimeError::JsonInspectionState)?
            .candidate(json_budget.ok_or(RuntimeError::JsonInspectionState)?, id)
            .map(|candidate| candidate.name)
            .map_err(map_store_error),
        SelectedName::Request(range) => Ok(request.bytes_at(range)),
        SelectedName::Body(index) => request
            .body
            .as_ref()
            .and_then(RequestBodyPublication::materialized)
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
            .ok_or(RuntimeError::GenerationInvariant)?
            .key
            .bytes(request, json_budget),
        SelectedName::MatchedVars(index) => tx
            .matched_vars
            .get(index)
            .ok_or(RuntimeError::GenerationInvariant)?
            .key
            .bytes(request, json_budget),
    })
    .transpose()
    .map_err(|error| inspection_error(request, error))
}

const COLLECTION_COUNT: usize = CollectionKind::ALL.len();

enum RequestBodyPublication<'generation> {
    Json(Vec<JsonPublication<'generation>>),
    Xml(Vec<xml_inspection_runtime::XmlPublication<'generation>>),
    Materialized(BodyPublication),
}
impl RequestBodyPublication<'_> {
    fn json(&self) -> Option<&JsonPublication<'_>> {
        match self {
            Self::Json(body) => body.first(),
            _ => None,
        }
    }
    fn xml(&self) -> Option<&xml_inspection_runtime::XmlPublication<'_>> {
        match self {
            Self::Xml(body) => body.first(),
            _ => None,
        }
    }
    fn structured_results(&self) -> Option<&SealedResults<'_>> {
        match self {
            Self::Json(body) => body.first().and_then(|body| body.results.as_ref()),
            Self::Xml(body) => body.first().and_then(|body| body.results.as_ref()),
            _ => None,
        }
    }
    fn materialized(&self) -> Option<&BodyPublication> {
        match self {
            Self::Materialized(body) => Some(body),
            Self::Json(_) | Self::Xml(_) => None,
        }
    }
    fn entry_count(&self) -> usize {
        match self {
            Self::Materialized(body) => body.entries.len(),
            Self::Json(body) => body.first().map_or(0, |body| body.summary.entries),
            Self::Xml(body) => body.first().map_or(0, |body| body.summary.entries),
        }
    }
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Materialized(body) => body.retained_bytes,
            Self::Json(body) => body.first().map_or(0, |body| body.summary.retained_bytes),
            Self::Xml(body) => body.first().map_or(0, |body| body.summary.retained_bytes),
        }
    }
}

struct RequestState<'generation> {
    bytes: Vec<u8>,
    collections: [Vec<CollectionValue>; COLLECTION_COUNT],
    body: Option<RequestBodyPublication<'generation>>,
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

struct TxEntry<V = MatchValue, K = MatchValue> {
    key: K,
    value: V,
}

/// JSON handles refer only to the immutable request-owned result arena. Owned
/// values preserve the fallible ordinary matching path and logical byte charges.
#[derive(Debug, PartialEq, Eq)]
enum MatchValue {
    Owned(Vec<u8>),
    Json(SealedMatchValue),
}
impl From<Vec<u8>> for MatchValue {
    fn from(value: Vec<u8>) -> Self {
        Self::Owned(value)
    }
}
impl MatchValue {
    fn len(&self) -> usize {
        match self {
            Self::Owned(value) => value.len(),
            Self::Json(value) => value.len(),
        }
    }
    fn owned_capacity(&self) -> usize {
        match self {
            Self::Owned(value) => value.capacity(),
            Self::Json(_) => 0,
        }
    }
    /// Immutable observation only. Rule execution must use the metered seam.
    fn observed_bytes<'value>(&'value self, request: &'value RequestState) -> Result<&'value [u8], RuntimeError> {
        match self {
            Self::Owned(value) => Ok(value),
            Self::Json(value) => request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::structured_results)
                .ok_or_else(|| inspection_error(request, RuntimeError::JsonInspectionState))?
                .observe_match_value(*value)
                .map_err(|error| inspection_error(request, map_store_error(error))),
        }
    }
    fn try_clone(&self) -> Result<Self, RuntimeError> {
        match self {
            Self::Owned(value) => owned_copy(value).map(Self::Owned),
            Self::Json(value) => Ok(Self::Json(*value)),
        }
    }
    fn bytes<'value>(
        &'value self,
        request: &'value RequestState,
        budget: Option<&mut InspectionBudget>,
    ) -> Result<&'value [u8], RuntimeError> {
        match self {
            Self::Owned(value) => Ok(value),
            Self::Json(value) => request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::structured_results)
                .ok_or_else(|| inspection_error(request, RuntimeError::JsonInspectionState))?
                .match_value(
                    budget.ok_or_else(|| inspection_error(request, RuntimeError::JsonInspectionState))?,
                    *value,
                )
                .map_err(|error| inspection_error(request, map_store_error(error))),
        }
    }
}

#[derive(Default)]
struct TxState {
    check_policy: RuleCheckPolicy,
    #[cfg(test)]
    force_unpruned_inspection: bool,
    entries: Vec<TxEntry>,
    entry_payload_bytes: u128,
    matched_var: Option<MatchValue>,
    matched_var_name: Option<MatchValue>,
    matched_vars: Vec<TxEntry<MatchValue, MatchValue>>,
    matched_vars_payload_bytes: u128,
    removed_units: Vec<usize>,
    target_exclusions: Vec<ActiveTargetExclusion>,
    request_body_processor: Option<super::super::BodyProcessorSelection>,
    force_request_body_variable: Option<super::OnOff>,
    retained_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveTargetExclusion {
    unit: usize,
    collection: CollectionKind,
    selector: super::CompiledTargetSelector,
}

struct CaptureEntry<V = MatchValue, K = MatchValue> {
    key: K,
    value: V,
}

#[derive(Clone, Copy)]
enum CaptureKeySource<'a> {
    Numeric {
        index: usize,
        bytes: &'a [u8],
    },
    Named {
        program: usize,
        group: usize,
        bytes: &'a [u8],
    },
}
impl<'a> CaptureKeySource<'a> {
    fn bytes(self) -> &'a [u8] {
        match self {
            Self::Numeric { bytes, .. } | Self::Named { bytes, .. } => bytes,
        }
    }
}
trait StageCaptureKey: Sized {
    fn allocation_bytes(source: CaptureKeySource<'_>) -> usize;
    fn from_source(source: CaptureKeySource<'_>) -> Result<Self, RuntimeError>;
    fn heap_capacity(&self) -> usize;
}
impl StageCaptureKey for Vec<u8> {
    fn allocation_bytes(source: CaptureKeySource<'_>) -> usize {
        source.bytes().len()
    }
    fn from_source(source: CaptureKeySource<'_>) -> Result<Self, RuntimeError> {
        owned_copy(source.bytes())
    }
    fn heap_capacity(&self) -> usize {
        self.capacity()
    }
}

impl StageCaptureKey for MatchValue {
    fn allocation_bytes(source: CaptureKeySource<'_>) -> usize {
        source.bytes().len()
    }
    fn from_source(source: CaptureKeySource<'_>) -> Result<Self, RuntimeError> {
        owned_copy(source.bytes()).map(Self::Owned)
    }
    fn heap_capacity(&self) -> usize {
        self.owned_capacity()
    }
}

/// Resolved only against the generation bound to the collector's capture scratch.
enum JsonCaptureKey {
    Numeric(usize),
    Named { program: usize, group: usize },
}
impl StageCaptureKey for JsonCaptureKey {
    fn allocation_bytes(_: CaptureKeySource<'_>) -> usize {
        0
    }
    fn from_source(source: CaptureKeySource<'_>) -> Result<Self, RuntimeError> {
        Ok(match source {
            CaptureKeySource::Numeric { index, .. } => Self::Numeric(index),
            CaptureKeySource::Named { program, group, .. } => Self::Named { program, group },
        })
    }
    fn heap_capacity(&self) -> usize {
        0
    }
}
impl JsonCaptureKey {
    fn with_bytes<T, E: From<RuntimeError>>(
        &self,
        generation: &CompiledGeneration,
        operation: impl FnOnce(&[u8]) -> Result<T, E>,
    ) -> Result<T, E> {
        match *self {
            Self::Numeric(index) => {
                let (digits, start) = decimal_bytes(index);
                operation(&digits[start..])
            }
            Self::Named { program, group } => {
                let name = generation
                    .capture_regexes
                    .get(program)
                    .and_then(|regex| regex.capture_name(group))
                    .ok_or(RuntimeError::GenerationInvariant)?;
                operation(name.as_bytes())
            }
        }
    }
}

struct CaptureBuilder<V = MatchValue, K = MatchValue> {
    entries: Vec<CaptureEntry<V, K>>,
    semantic_bytes: usize,
    heap_bytes: usize,
}
impl<V, K> Default for CaptureBuilder<V, K> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            semantic_bytes: 0,
            heap_bytes: 0,
        }
    }
}
impl<V, K> CaptureBuilder<V, K> {
    fn clear(&mut self) {
        self.entries.clear();
        self.semantic_bytes = 0;
        self.heap_bytes = 0;
    }
    fn retained_bytes(&self) -> Result<usize, RuntimeError> {
        self.entries
            .capacity()
            .checked_mul(size_of::<CaptureEntry<V, K>>())
            .and_then(|bytes| bytes.checked_add(self.heap_bytes))
            .ok_or(RuntimeError::Arithmetic)
    }
}

/// Capture sources never escape canonical extraction or borrow persisted storage.
#[derive(Clone, Copy)]
enum CaptureSource<'a> {
    InputRange { input: &'a [u8], start: usize, len: usize },
    Fingerprint(crate::injection::Fingerprint),
}
impl CaptureSource<'_> {
    fn bytes(&self) -> Result<&[u8], RuntimeError> {
        match self {
            Self::InputRange { input, start, len } => input
                .get(*start..start.checked_add(*len).ok_or(RuntimeError::Arithmetic)?)
                .ok_or(RuntimeError::GenerationInvariant),
            Self::Fingerprint(value) => Ok(value.as_bytes()),
        }
    }
}
trait StageCapturePayload: Sized {
    fn allocation_bytes(source: CaptureSource<'_>) -> Result<usize, RuntimeError>;
    fn from_source(source: CaptureSource<'_>) -> Result<Self, RuntimeError>;
    fn heap_capacity(&self) -> usize;
}
impl StageCapturePayload for MatchValue {
    fn allocation_bytes(source: CaptureSource<'_>) -> Result<usize, RuntimeError> {
        Ok(source.bytes()?.len())
    }
    fn from_source(source: CaptureSource<'_>) -> Result<Self, RuntimeError> {
        owned_copy(source.bytes()?).map(Self::Owned)
    }
    fn heap_capacity(&self) -> usize {
        self.owned_capacity()
    }
}
/// Relative to the exact transformed stage subsequently copied into its outcome.
/// Generated SQLi fingerprints remain independent owned bytes, never input ranges.
enum StageCaptureValue {
    InputRange { start: usize, len: usize },
    Fingerprint(crate::injection::Fingerprint),
}
impl StageCapturePayload for StageCaptureValue {
    fn allocation_bytes(source: CaptureSource<'_>) -> Result<usize, RuntimeError> {
        source.bytes()?;
        Ok(0)
    }
    fn from_source(source: CaptureSource<'_>) -> Result<Self, RuntimeError> {
        source.bytes()?;
        Ok(match source {
            CaptureSource::InputRange { start, len, .. } => Self::InputRange { start, len },
            CaptureSource::Fingerprint(value) => Self::Fingerprint(value),
        })
    }
    fn heap_capacity(&self) -> usize {
        0
    }
}

#[derive(Default)]
struct MemberMatch {
    captures: Vec<CaptureEntry>,
    matched_var: Option<MatchValue>,
    matched_var_name: Option<MatchValue>,
}

#[derive(Clone, Copy)]
struct MatchStorage {
    fixed_bytes: usize,
    retained_limit: usize,
    limit: usize,
}

impl MatchStorage {
    fn growth_target(self, capacity: usize) -> Result<usize, RuntimeError> {
        Ok(capacity
            .checked_mul(2)
            .ok_or(RuntimeError::Arithmetic)?
            .max(1)
            .min(self.limit))
    }

    /// Success-only admission for every possible query hit. Actual growth still
    /// validates allocator capacity in push; this never mutates runtime counters.
    fn admits_growth(self, mut capacity: usize, matches: usize) -> bool {
        while capacity < matches {
            let Ok(target) = self.growth_target(capacity) else {
                return false;
            };
            let Some(peak) = capacity
                .checked_add(target)
                .and_then(|count| count.checked_mul(size_of::<MemberMatch>()))
                .and_then(|bytes| self.fixed_bytes.checked_add(bytes))
            else {
                return false;
            };
            if target <= capacity || peak > self.retained_limit {
                return false;
            }
            capacity = target;
        }
        true
    }

    fn push(
        self,
        matches: &mut Vec<MemberMatch>,
        matched: MemberMatch,
        scratch_bytes: &mut usize,
        match_bytes: &mut usize,
    ) -> Result<(), RuntimeError> {
        if matches.len() == matches.capacity() {
            let target = self.growth_target(matches.capacity())?;
            let allocation = target
                .checked_mul(size_of::<MemberMatch>())
                .ok_or(RuntimeError::Arithmetic)?;
            let current = self
                .fixed_bytes
                .checked_add(*match_bytes)
                .ok_or(RuntimeError::Arithmetic)?;
            // Reallocation may temporarily retain the complete old allocation.
            if current.checked_add(allocation).ok_or(RuntimeError::Arithmetic)? > self.retained_limit {
                return Err(RuntimeError::RetainedByteLimit {
                    limit: self.retained_limit,
                });
            }
            matches
                .try_reserve_exact(target.checked_sub(matches.len()).ok_or(RuntimeError::Arithmetic)?)
                .map_err(|_| RuntimeError::Allocation)?;
            let actual = matches
                .capacity()
                .checked_mul(size_of::<MemberMatch>())
                .ok_or(RuntimeError::Arithmetic)?;
            if current.checked_add(actual).ok_or(RuntimeError::Arithmetic)? > self.retained_limit {
                // Drop an allocator overshoot instead of retaining unadmitted space.
                *matches = Vec::new();
                *scratch_bytes = scratch_bytes
                    .checked_sub(*match_bytes)
                    .ok_or(RuntimeError::Arithmetic)?;
                *match_bytes = 0;
                return Err(RuntimeError::RetainedByteLimit {
                    limit: self.retained_limit,
                });
            }
            *scratch_bytes = scratch_bytes
                .checked_sub(*match_bytes)
                .and_then(|bytes| bytes.checked_add(actual))
                .ok_or(RuntimeError::Arithmetic)?;
            *match_bytes = actual;
        }
        matches.push(matched);
        Ok(())
    }
}

struct ChainBranch {
    tx: TxState,
    capture_keys: Vec<MatchValue>,
    setvar_actions: usize,
}

#[derive(Clone)]
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
    ctl_work_bytes: usize,
    tx: TxState,
    capture_keys: Vec<MatchValue>,
    response: Option<ResponseState>,
    request_body_error: Option<BodyError>,
    evaluation_mode: EvaluationMode,
    #[cfg(test)]
    force_json_cursor: bool,
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
        Self::new_shared_with_policy(
            generation,
            request,
            limits,
            evaluation_mode,
            rule_engine_mode,
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
        Self::new_with_policy(
            generation,
            request,
            limits,
            evaluation_mode,
            rule_engine_mode,
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
            ctl_work_bytes: 0,
            tx: TxState {
                check_policy,
                ..TxState::default()
            },
            capture_keys: Vec::new(),
            response: None,
            request_body_error: None,
            evaluation_mode,
            #[cfg(test)]
            force_json_cursor: false,
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
        self.tx
            .observed_get(key, &self.request)
            .ok()
            .flatten()?
            .observed_bytes(&self.request)
            .ok()
    }

    pub const fn request_body_error(&self) -> Option<BodyError> {
        self.request_body_error
    }

    /// Narrow source admission after header controls and before body publication.
    /// Independent parsing, retained-state and execution budgets are unchanged.
    pub fn constrain_request_body_bytes(&mut self, limit: usize) -> Result<(), RuntimeError> {
        self.preflight_request_body(crate::seclang::BodyProcessorSelection::Auto)?;
        if self.request_body_stream.is_some() || self.request_prefix_completion.is_some() || self.json_budget.is_some()
        {
            return Err(RuntimeError::RequestBodyAlreadyPublished);
        }
        if limit > self.limits.body.max_body_bytes {
            return Err(RuntimeError::GenerationInvariant);
        }
        self.limits.body.max_body_bytes = limit;
        Ok(())
    }

    pub fn execute_request_headers(&mut self) -> Result<PhaseOutcome, RuntimeError> {
        self.execute_phase(Phase::RequestHeaders)
    }

    fn publish_non_json_body(&mut self, body: RequestBody) -> Result<(), RuntimeError> {
        self.ensure_json_healthy()?;
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

        let raw_policy = self
            .tx
            .force_request_body_variable
            .map(|value| value == super::OnOff::On);
        let publication = if raw_policy.is_some() {
            build_publication_with_raw_policy(
                &body,
                content_type,
                head_args_combined_size,
                self.limits.body,
                raw_policy,
            )
        } else {
            build_publication(&body, content_type, head_args_combined_size, self.limits.body)
        };
        let (publication, body_error) = match publication {
            Ok(publication) => (publication, None),
            Err(error @ (BodyError::Malformed { .. } | BodyError::MultipartBoundary))
                if self.rule_engine_mode == RuleEngineMode::DetectionOnly =>
            {
                (
                    if raw_policy.is_some() {
                        build_fallback_publication_with_raw_policy(
                            &body,
                            content_type,
                            head_args_combined_size,
                            self.limits.body,
                            raw_policy,
                        )
                    } else {
                        build_fallback_publication(&body, content_type, head_args_combined_size, self.limits.body)
                    }
                    .map_err(|kind| RuntimeError::Body { kind })?,
                    Some(error),
                )
            }
            Err(kind) => return Err(RuntimeError::Body { kind }),
        };
        self.publish_body_state(RequestBodyPublication::Materialized(publication), body_error)
    }

    fn publish_body_state(
        &mut self,
        publication: RequestBodyPublication<'generation>,
        body_error: Option<BodyError>,
    ) -> Result<(), RuntimeError> {
        let entry_count = self
            .request
            .entry_count
            .checked_add(publication.entry_count())
            .ok_or(RuntimeError::Arithmetic)?;
        if entry_count > self.limits.max_collection_entries {
            return Err(RuntimeError::CollectionEntryLimit {
                limit: self.limits.max_collection_entries,
            });
        }
        let request_retained = self
            .request
            .retained_bytes
            .checked_add(publication.retained_bytes())
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

    fn ensure_json_healthy(&self) -> Result<(), RuntimeError> {
        if let Some(error) = self.json_failure {
            return Err(error);
        }
        if let Some(error) = self.json_budget.as_ref().and_then(InspectionBudget::failure) {
            return Err(inspection_error(&self.request, map_store_error(error)));
        }
        Ok(())
    }
    pub fn execute_request_body(&mut self) -> Result<PhaseOutcome, RuntimeError> {
        self.ensure_json_healthy()?;
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
        self.ensure_json_healthy()?;
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
        self.ordinary_scan_cache.clear();
        self.borrowed_transform_memo.clear();
        let member = self.generation.members[id];
        let transformations = member
            .transformation_plan
            .map(|id| {
                let plan = self.generation.transformation_plans[id];
                &self.generation.transformations
                    [plan.transformation_start..plan.transformation_start + plan.transformation_count]
            })
            .unwrap_or_default();
        let transformed = apply_transformations_borrowed(
            prefix,
            transformations,
            TransformLimits {
                max_output_bytes: self.limits.max_transformation_output_bytes,
                max_work_bytes: self.limits.max_transformation_work_bytes,
            },
        )
        .map_err(map_transform_error)?;
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
            None,
            self.json_budget.as_mut(),
        )
        .map(|(matched, _)| matched)
    }

    pub const fn request_prefix_completion(&self) -> Option<RequestBodyPrefixCompletion> {
        self.request_prefix_completion
    }

    /// Prepare the exact constant-memory raw-body lane selected by the compiler.
    pub fn start_request_body_stream(&mut self) -> Result<(), RuntimeError> {
        self.ensure_json_healthy()?;
        if self.next_phase() != Some(Phase::RequestBody) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::RequestBody,
            });
        }
        if self.request.body.is_some() || self.request_body_stream.is_some() {
            return Err(RuntimeError::RequestBodyAlreadyPublished);
        }
        if !self.tx.removed_units.is_empty()
            || !self.tx.target_exclusions.is_empty()
            || self.tx.request_body_processor.is_some()
            || self.tx.force_request_body_variable.is_some()
        {
            return Err(RuntimeError::RequestBodyStreamCtlUnavailable);
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
        self.ensure_json_healthy()?;
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
        self.publish_request_body(RequestBody::new(
            bytes::Bytes::new(),
            super::super::BodyProcessorSelection::Raw,
        ))?;
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
        self.ordinary_scan_cache.clear();
        self.borrowed_transform_memo.clear();
        self.ensure_json_healthy()?;
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
        let policy_prunes = self.generation.has_expensive_checks && !self.tx.check_policy.expensive_checks;
        let mut schedule_ordinal = 0_usize;
        while schedule_ordinal < schedule_len {
            let entry = self.generation.schedules[schedule_index][schedule_ordinal];
            schedule_ordinal = schedule_ordinal.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
            let super::CompiledScheduleEntry::RuleUnit(unit_id) = entry else {
                continue;
            };
            if self.tx.removed_units.binary_search(&unit_id).is_ok()
                || (policy_prunes && self.generation.units[unit_id].expensive_check)
            {
                continue;
            }
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
        // Keep capacity charged while the buffer is loaned to the entire unit.
        let mut matches = std::mem::take(&mut self.match_scratch);
        matches.clear();
        let result = self.evaluate_unit_into(unit_id, prefix_match, &mut matches);
        matches.clear();
        self.match_scratch = matches;
        result
    }

    fn evaluate_unit_into(
        &mut self,
        unit_id: usize,
        prefix_match: Option<bool>,
        matches: &mut Vec<MemberMatch>,
    ) -> Result<bool, RuntimeError> {
        let unit = *self
            .generation
            .units
            .get(unit_id)
            .ok_or(RuntimeError::GenerationInvariant)?;
        let end = unit
            .member_start
            .checked_add(unit.member_count)
            .ok_or(RuntimeError::Arithmetic)?;
        self.evaluate_unit_first_member(
            unit_id,
            unit.member_start,
            if unit.member_count == 1 { prefix_match } else { None },
            matches,
        )?;
        if matches.is_empty() {
            return Ok(false);
        }
        let mut initial = self.tx.try_clone_for_unit()?;
        if unit.member_count == 1 {
            let mut capture_keys = try_clone_keys(&self.capture_keys)?;
            let mut setvar_actions = 0_usize;
            for matched in matches.drain(..) {
                apply_member_match(
                    &mut initial,
                    &mut capture_keys,
                    matched,
                    self.limits,
                    &self.request,
                    self.json_budget.as_mut(),
                )?;
                self.apply_member_actions(unit.member_start, &mut initial, &mut setvar_actions)?;
            }
            return self.commit_unit_match(unit, initial, capture_keys);
        }
        let mut frontier = Vec::new();
        frontier.try_reserve_exact(1).map_err(|_| RuntimeError::Allocation)?;
        frontier.push(ChainBranch {
            tx: initial,
            capture_keys: try_clone_keys(&self.capture_keys)?,
            setvar_actions: 0,
        });
        for member_id in unit.member_start..end {
            let final_member = member_id + 1 == end;
            let current_count = frontier.len();
            let current_bytes = chain_branches_bytes(&frontier)?;
            if final_member {
                // Only staged branches are consumed. Preserve the original
                // frontier checkpoint before selecting children in winner order.
                for mut branch in frontier {
                    self.evaluate_member_matches_into(unit_id, member_id, &branch.tx, true, matches)?;
                    if let Some(matched) = matches.drain(..).next() {
                        apply_member_match(
                            &mut branch.tx,
                            &mut branch.capture_keys,
                            matched,
                            self.limits,
                            &self.request,
                            self.json_budget.as_mut(),
                        )?;
                        self.apply_member_actions(member_id, &mut branch.tx, &mut branch.setvar_actions)?;
                        return self.commit_unit_match(unit, branch.tx, branch.capture_keys);
                    }
                }
                return Ok(false);
            }
            let mut next = Vec::new();
            let mut next_bytes = 0_usize;
            for branch in &frontier {
                if member_id != unit.member_start {
                    self.evaluate_member_matches_into(unit_id, member_id, &branch.tx, final_member, matches)?;
                }
                for matched in matches.drain(..) {
                    let mut candidate = ChainBranch {
                        tx: branch.tx.try_clone()?,
                        capture_keys: try_clone_keys(&branch.capture_keys)?,
                        setvar_actions: branch.setvar_actions,
                    };
                    apply_member_match(
                        &mut candidate.tx,
                        &mut candidate.capture_keys,
                        matched,
                        self.limits,
                        &self.request,
                        self.json_budget.as_mut(),
                    )?;
                    self.apply_member_actions(member_id, &mut candidate.tx, &mut candidate.setvar_actions)?;
                    if current_count.saturating_add(next.len()) >= self.limits.max_operator_evaluations {
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

    /// Loan persistent TX storage while hiding prior-unit match observers.
    /// The accounting placeholder preserves original retained admission. Restore
    /// ownership before propagating any evaluation result or staging actions.
    fn evaluate_unit_first_member(
        &mut self,
        unit_id: usize,
        member_id: usize,
        prefix_match: Option<bool>,
        matches: &mut Vec<MemberMatch>,
    ) -> Result<(), RuntimeError> {
        let mut base = std::mem::take(&mut self.tx);
        self.tx.retained_bytes = base.retained_bytes;
        let previous = (
            base.matched_var.take(),
            base.matched_var_name.take(),
            std::mem::take(&mut base.matched_vars),
            base.matched_vars_payload_bytes,
        );
        base.matched_vars_payload_bytes = 0;
        base.refresh_retained_bytes();
        let result = match prefix_match {
            Some(matched) => self.boolean_member_matches_into(matched, &base, matches),
            None => self.evaluate_member_matches_into(unit_id, member_id, &base, false, matches),
        };
        base.matched_var = previous.0;
        base.matched_var_name = previous.1;
        base.matched_vars = previous.2;
        base.matched_vars_payload_bytes = previous.3;
        base.refresh_retained_bytes();
        self.tx = base;
        result
    }

    fn commit_unit_match(
        &mut self,
        unit: super::CompiledRuleUnit,
        staged: TxState,
        capture_keys: Vec<MatchValue>,
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

    fn boolean_member_matches_into(
        &mut self,
        matched: bool,
        tx: &TxState,
        matches: &mut Vec<MemberMatch>,
    ) -> Result<(), RuntimeError> {
        if matched {
            self.match_storage(tx, 1)?.push(
                matches,
                MemberMatch::default(),
                &mut self.scratch_bytes,
                &mut self.match_scratch_bytes,
            )?;
        }
        Ok(())
    }

    fn match_storage(&self, tx: &TxState, limit: usize) -> Result<MatchStorage, RuntimeError> {
        Ok(MatchStorage {
            fixed_bytes: self
                .retained_bytes()
                .checked_sub(self.match_scratch_bytes)
                .and_then(|bytes| bytes.checked_add(tx.retained_bytes.saturating_sub(self.tx.retained_bytes)))
                .ok_or(RuntimeError::Arithmetic)?,
            retained_limit: self.limits.max_retained_bytes,
            limit,
        })
    }

    #[cfg(test)]
    fn evaluate_member_matches(
        &mut self,
        unit_id: usize,
        member_id: usize,
        tx: &TxState,
        stop_after_first: bool,
    ) -> Result<Vec<MemberMatch>, RuntimeError> {
        let mut matches = std::mem::take(&mut self.match_scratch);
        matches.clear();
        let result = self.evaluate_member_matches_into(unit_id, member_id, tx, stop_after_first, &mut matches);
        // Private tests may inspect the returned matches. Production always
        // drains and returns capacity through evaluate_unit's single loan.
        self.scratch_bytes = self
            .scratch_bytes
            .checked_sub(self.match_scratch_bytes)
            .ok_or(RuntimeError::Arithmetic)?;
        self.match_scratch_bytes = 0;
        result.map(|()| matches)
    }

    fn evaluate_member_matches_into(
        &mut self,
        unit_id: usize,
        member_id: usize,
        tx: &TxState,
        stop_after_first: bool,
        matches: &mut Vec<MemberMatch>,
    ) -> Result<(), RuntimeError> {
        matches.clear();
        if let Some(matched) = self
            .stream_member_matches
            .as_ref()
            .and_then(|members| members.get(member_id))
            .copied()
            .flatten()
        {
            return self.boolean_member_matches_into(matched, tx, matches);
        }
        let member = *self
            .generation
            .members
            .get(member_id)
            .ok_or(RuntimeError::GenerationInvariant)?;
        let Some(operator) = self.generation.operator_plans.get(member_id).copied().flatten() else {
            return self.boolean_member_matches_into(true, tx, matches);
        };
        let target_plan = member.target_plan.ok_or(RuntimeError::GenerationInvariant)?;
        let old_capacity_bytes = self
            .selected_scratch
            .capacity()
            .checked_mul(size_of::<SelectedValue>())
            .ok_or(RuntimeError::Arithmetic)?;
        let other_scratch_bytes = self
            .scratch_bytes
            .checked_sub(old_capacity_bytes)
            .ok_or(RuntimeError::Arithmetic)?;
        let fixed_bytes = self
            .retained_bytes()
            .checked_sub(old_capacity_bytes)
            .and_then(|bytes| bytes.checked_add(tx.retained_bytes.saturating_sub(self.tx.retained_bytes)))
            .ok_or(RuntimeError::Arithmetic)?;
        let capacity_bytes =
            self.limits
                .max_retained_bytes
                .checked_sub(fixed_bytes)
                .ok_or(RuntimeError::RetainedByteLimit {
                    limit: self.limits.max_retained_bytes,
                })?;
        if old_capacity_bytes > capacity_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        let mut json_tail = self.json_summary_no_hit_tail(member_id, tx)?;
        let mut selected = std::mem::take(&mut self.selected_scratch);
        selected.clear();
        let result = (|| {
            let storage = SelectionStorage {
                capacity_bytes,
                retained_limit: self.limits.max_retained_bytes,
            };
            if json_tail.is_some_and(|tail| {
                storage
                    .summary_capacity(
                        selected.capacity(),
                        tail.selected_count,
                        self.limits.max_selected_values,
                    )
                    .is_none()
            }) {
                json_tail = None;
            }
            let mut summary_match_storage = None;
            loop {
                let selection_result = select_values(
                    &self.generation,
                    &self.request,
                    tx,
                    self.response.as_ref(),
                    &mut self.boolean_scratch,
                    &mut self.ctl_work_bytes,
                    SelectionParams {
                        unit_id,
                        plan_id: target_plan,
                        limit: self.limits.max_selected_values,
                        work_limit: self.limits.max_operator_work_bytes,
                        include_json: json_tail.is_none(),
                    },
                    self.json_budget.as_mut(),
                    &mut selected,
                    storage,
                );
                let actual_bytes = selected
                    .capacity()
                    .checked_mul(size_of::<SelectedValue>())
                    .ok_or(RuntimeError::Arithmetic)?;
                self.scratch_bytes = other_scratch_bytes
                    .checked_add(actual_bytes)
                    .ok_or(RuntimeError::Arithmetic)?;
                selection_result?;
                let Some(tail) = json_tail else {
                    break;
                };
                let hypothetical = storage.summary_capacity(
                    selected.capacity(),
                    tail.selected_count,
                    self.limits.max_selected_values,
                );
                if let Some(hypothetical_bytes) = hypothetical.filter(|_| selected.len() == tail.query_count) {
                    let limit = if stop_after_first { 1 } else { tail.selected_count };
                    let mut admission = self.match_storage(tx, limit)?;
                    if let Some(fixed_bytes) = hypothetical_bytes
                        .checked_sub(actual_bytes)
                        .and_then(|extra| admission.fixed_bytes.checked_add(extra))
                    {
                        admission.fixed_bytes = fixed_bytes;
                        let query_matches = if stop_after_first {
                            tail.query_count.min(1)
                        } else {
                            tail.query_count
                        };
                        if admission.admits_growth(matches.capacity(), query_matches) {
                            // Only match admission sees the virtual tail capacity.
                            // Persistent scratch always records actual allocations.
                            summary_match_storage = Some(admission);
                            self.json_budget
                                .as_mut()
                                .ok_or(RuntimeError::JsonInspectionState)?
                                .work(tail.selection_work)
                                .map_err(map_store_error)?;
                            break;
                        }
                    }
                }
                // Query-only selection has no physical or control work under
                // this static singleton proof. Re-select before the first charge
                // if actual allocation or hypothetical match admission is unsure.
                json_tail = None;
                selected.clear();
            }
            let transformations = member
                .transformation_plan
                .and_then(|id| self.generation.transformation_plans.get(id))
                .and_then(|plan| {
                    let end = plan.transformation_start.checked_add(plan.transformation_count)?;
                    self.generation.transformations.get(plan.transformation_start..end)
                })
                .unwrap_or_default();
            let multi_match = member_has_action(&self.generation, member, super::super::ActionKind::MultiMatch)?;
            // The checked nonempty slice above authenticates the canonical plan ID.
            let memo_plan_id =
                if self.evaluation_mode == EvaluationMode::Accelerated && !multi_match && !transformations.is_empty() {
                    member.transformation_plan
                } else {
                    None
                };
            let mut work = 0_usize;
            let match_limit = if stop_after_first { 1 } else { selected.len() };
            let match_storage = match summary_match_storage {
                Some(storage) => storage,
                None => self.match_storage(tx, match_limit)?,
            };
            for selected in selected.iter().copied() {
                let transform_memo =
                    self.borrowed_transform_memo
                        .prepare(selected.value, memo_plan_id, &self.request, self.limits);
                let scan_cache = self.ordinary_scan_cache.prepare(
                    selected.value,
                    member.transformation_plan,
                    !multi_match
                        && self.evaluation_mode == EvaluationMode::Accelerated
                        && matches!(operator.operand, super::CompiledOperatorOperand::Matcher { .. }),
                );
                let mut context = SingleInputContext {
                    generation: &self.generation,
                    request: &self.request,
                    tx,
                    group_scratch: &mut self.group_scratch,
                    boolean_scratch: &mut self.boolean_scratch,
                    capture_scratch: &mut self.capture_scratch,
                    scan_cache,
                    json_budget: self.json_budget.as_mut(),
                };
                let matched = if let SelectedBytes::Json(id)
                | SelectedBytes::XmlText(id)
                | SelectedBytes::XmlAttribute(id) = selected.value
                {
                    json_inspection_runtime::evaluate_json_selected(
                        &mut context,
                        SingleInputPlan {
                            operator,
                            transformations,
                            multi_match,
                            runtime_expansion_byte_limit: member.runtime_expansion_byte_limit,
                            limits: self.limits,
                            evaluation_mode: self.evaluation_mode,
                        },
                        member_id,
                        id,
                        selected.collection,
                        &mut work,
                        &mut self.detector_work_bytes,
                        matches.len(),
                    )
                    .map_err(|error| {
                        if selected.collection == CollectionKind::Xml {
                            xml_inspection_runtime::xml_error(error)
                        } else {
                            error
                        }
                    })?
                } else {
                    let value = selected.bytes(
                        &self.request,
                        tx,
                        self.response.as_ref(),
                        context.json_budget.as_deref_mut(),
                    )?;
                    evaluate_single_input_with_transform_memo(
                        &mut context,
                        SingleInputPlan {
                            operator,
                            transformations,
                            multi_match,
                            runtime_expansion_byte_limit: member.runtime_expansion_byte_limit,
                            limits: self.limits,
                            evaluation_mode: self.evaluation_mode,
                        },
                        value,
                        &mut work,
                        &mut self.detector_work_bytes,
                        matches.len(),
                        |transformed, budget| {
                            matched_variable(&self.request, tx, self.response.as_ref(), selected, transformed, budget)
                        },
                        transform_memo,
                    )?
                };
                if let Some(matched) = matched {
                    match_storage.push(matches, matched, &mut self.scratch_bytes, &mut self.match_scratch_bytes)?;
                    if stop_after_first {
                        return Ok(());
                    }
                }
            }
            if let Some(tail) = json_tail {
                // Combined logical and physical admission was proven before
                // selection. Query errors/early stops never reach this replay.
                charge_operator_work(&mut work, tail.logical_work, self.limits)?;
                self.json_budget
                    .as_mut()
                    .ok_or(RuntimeError::JsonInspectionState)?
                    .work(tail.replay_work)
                    .map_err(map_store_error)?;
            }
            Ok(())
        })();
        selected.clear();
        let actual_bytes = selected
            .capacity()
            .checked_mul(size_of::<SelectedValue>())
            .ok_or(RuntimeError::Arithmetic)?;
        if actual_bytes > capacity_bytes {
            self.scratch_bytes = self
                .scratch_bytes
                .checked_sub(actual_bytes)
                .ok_or(RuntimeError::Arithmetic)?;
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        // Selection was charged before matching; preserve any match-buffer
        // growth recorded while selected_scratch was temporarily loaned.
        self.selected_scratch = selected;
        result
    }

    /// Prove a no-hit JSON tail without selecting or evaluating queries. Any
    /// uncertain bound preserves the cursor's selection-before-evaluation errors.
    fn json_summary_no_hit_tail(
        &mut self,
        member_id: usize,
        tx: &TxState,
    ) -> Result<Option<JsonNoHitTail>, RuntimeError> {
        #[cfg(test)]
        if self.force_json_cursor {
            return Ok(None);
        }
        if self.evaluation_mode != EvaluationMode::Accelerated
            || !tx.target_exclusions.is_empty()
            || !self
                .generation
                .json_inspection
                .members
                .get(member_id)
                .is_some_and(|member| member.summary_no_hit)
        {
            return Ok(None);
        }
        let Some(results) = self
            .request
            .body
            .as_ref()
            .and_then(RequestBodyPublication::json)
            .and_then(|body| body.results.as_ref())
        else {
            return Ok(None);
        };
        if let super::json_inspection_plan::JsonMemberMode::StaticFact(group) = self
            .generation
            .json_inspection
            .members
            .get(member_id)
            .ok_or(RuntimeError::GenerationInvariant)?
            .value
        {
            results
                .guard_group(
                    self.json_budget.as_mut().ok_or(RuntimeError::JsonInspectionState)?,
                    group,
                )
                .map_err(map_store_error)?;
        }
        let queries = self.request.collection_records(CollectionKind::Args)?;
        let bounds = (|| {
            let count = results.json_summary().ok()?.argument_count;
            let selected_count = queries.len().checked_add(count)?;
            let selection_work = count;
            let replay_work = count.checked_mul(3)?;
            let physical_work = selection_work.checked_add(replay_work)?;
            let logical_work = results.json_summary().ok()?.argument_value_bytes.checked_mul(2)?;
            let mut combined_work = logical_work;
            for query in queries {
                // None/lowercase preserve byte length. Charge a possible hit's
                // payload, qualified name and descriptor even for query misses.
                let name_len = CollectionKind::Args.as_str().len().checked_add(match query.name {
                    Some(name) => name.len.checked_add(1)?,
                    None => 0,
                })?;
                let query_work = query
                    .value
                    .len
                    .checked_mul(3)?
                    .checked_add(name_len)?
                    .checked_add(size_of::<MemberMatch>())?;
                combined_work = combined_work.checked_add(query_work)?;
            }
            Some((
                JsonNoHitTail {
                    query_count: queries.len(),
                    selected_count,
                    selection_work,
                    replay_work,
                    logical_work,
                },
                combined_work,
                physical_work,
            ))
        })();
        let Some((tail, combined_work, physical_work)) = bounds else {
            return Ok(None);
        };
        let Some(budget) = self.json_budget.as_ref() else {
            return Ok(None);
        };
        if tail.selected_count > self.limits.max_selected_values
            || combined_work > self.limits.max_operator_work_bytes
            || !results.admits_summary_no_hit(
                budget,
                &self.generation,
                self.limits,
                self.evaluation_mode,
                physical_work,
            )
        {
            return Ok(None);
        }
        Ok(Some(tail))
    }

    fn validate_runtime_actions(&mut self, unit: super::CompiledRuleUnit, tx: &TxState) -> Result<(), RuntimeError> {
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
                    let _expanded = expand_template::<false>(
                        &self.generation,
                        &self.request,
                        tx,
                        template,
                        member.runtime_expansion_byte_limit,
                        self.json_budget.as_mut(),
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
        super::inspection_activation::successor(
            &self.generation,
            unit,
            schedule_index,
            current,
            |_| Ok(()),
            || RuntimeError::GenerationInvariant,
        )
    }

    fn apply_member_actions(
        &mut self,
        member_id: usize,
        staged: &mut TxState,
        setvar_actions: &mut usize,
    ) -> Result<(), RuntimeError> {
        let member = *self
            .generation
            .members
            .get(member_id)
            .ok_or(RuntimeError::GenerationInvariant)?;
        // The exact collector keeps its capture/observer path; only its two
        // aggregate setvar actions are optional, before expansion or allocation.
        if member.expensive_aggregate_actions && !staged.check_policy.expensive_checks {
            return Ok(());
        }
        let action_end = member
            .action_start
            .checked_add(member.action_count)
            .ok_or(RuntimeError::Arithmetic)?;
        for action_index in member.action_start..action_end {
            let action = *self
                .generation
                .actions
                .get(action_index)
                .ok_or(RuntimeError::GenerationInvariant)?;
            if let super::CompiledActionOperand::Ctl(ctl) = action.operand {
                self.apply_ctl(ctl, staged)?;
                continue;
            }
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
            let key = if setvar.operation == super::SetVarOperation::Delete {
                template_operand(
                    &self.generation,
                    &self.request,
                    staged,
                    setvar.key,
                    member.runtime_expansion_byte_limit,
                    self.json_budget.as_mut(),
                )?
            } else {
                Cow::Owned(expand_template::<true>(
                    &self.generation,
                    &self.request,
                    staged,
                    setvar.key,
                    member.runtime_expansion_byte_limit,
                    self.json_budget.as_mut(),
                )?)
            };
            match setvar.operation {
                super::SetVarOperation::Delete => staged.delete(&key, &self.request, self.json_budget.as_mut())?,
                super::SetVarOperation::Assign => {
                    let value = setvar_value(
                        &self.generation,
                        &self.request,
                        staged,
                        setvar,
                        member.runtime_expansion_byte_limit,
                        self.json_budget.as_mut(),
                    )?;
                    staged.set(
                        key.into_owned(),
                        value,
                        self.limits,
                        &self.request,
                        self.json_budget.as_mut(),
                    )?;
                }
                super::SetVarOperation::Add | super::SetVarOperation::Subtract => {
                    let current = staged
                        .get(&key, &self.request, self.json_budget.as_mut())?
                        .map(|value| value.bytes(&self.request, self.json_budget.as_mut()))
                        .transpose()?
                        .map_or(Ok(0_i64), parse_i64)?;
                    let delta = if let Some(constant) = setvar.constant {
                        constant
                    } else {
                        let value = setvar_value(
                            &self.generation,
                            &self.request,
                            staged,
                            setvar,
                            member.runtime_expansion_byte_limit,
                            self.json_budget.as_mut(),
                        )?;
                        parse_i64(&value)?
                    };
                    let result = if setvar.operation == super::SetVarOperation::Add {
                        current.checked_add(delta)
                    } else {
                        current.checked_sub(delta)
                    }
                    .ok_or(RuntimeError::IntegerConversion)?;
                    let (digits, start) = signed_decimal_bytes(result);
                    staged.set(
                        key.into_owned(),
                        owned_copy(&digits[start..])?,
                        self.limits,
                        &self.request,
                        self.json_budget.as_mut(),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn apply_ctl(&mut self, ctl: super::CompiledCtl, staged: &mut TxState) -> Result<(), RuntimeError> {
        charge_ctl_work(&mut self.ctl_work_bytes, 1, self.limits.max_operator_work_bytes)?;
        match ctl {
            super::CompiledCtl::ForceRequestBodyVariable(value) => {
                staged.force_request_body_variable = Some(value);
            }
            super::CompiledCtl::RequestBodyProcessor(value) => {
                if self.next_phase() != Some(Phase::RequestHeaders) || self.request.body.is_some() {
                    return Err(RuntimeError::GenerationInvariant);
                }
                staged.request_body_processor = Some(match value {
                    super::RequestBodyProcessor::UrlEncoded => super::super::BodyProcessorSelection::UrlEncoded,
                    super::RequestBodyProcessor::Multipart => super::super::BodyProcessorSelection::Multipart,
                    super::RequestBodyProcessor::Json => super::super::BodyProcessorSelection::Json,
                    super::RequestBodyProcessor::Xml => super::super::BodyProcessorSelection::Xml,
                });
            }
            super::CompiledCtl::RuleRemoveByTag(tag) => {
                for entry in tagged_units(&self.generation, tag)? {
                    charge_ctl_work(
                        &mut self.ctl_work_bytes,
                        staged.removed_units.len().saturating_add(1),
                        self.limits.max_operator_work_bytes,
                    )?;
                    staged.add_removed_unit(entry.unit, self.limits)?;
                }
            }
            super::CompiledCtl::RuleRemoveTargetByTag {
                tag,
                collection,
                selector,
            } => {
                let super::CompiledCtlSelector::Resolved(selector) = selector else {
                    return Err(RuntimeError::GenerationInvariant);
                };
                for entry in tagged_units(&self.generation, tag)? {
                    charge_ctl_work(
                        &mut self.ctl_work_bytes,
                        staged.target_exclusions.len().saturating_add(1),
                        self.limits.max_operator_work_bytes,
                    )?;
                    staged.add_target_exclusion(
                        ActiveTargetExclusion {
                            unit: entry.unit,
                            collection,
                            selector,
                        },
                        self.limits,
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn charge_ctl_work(work: &mut usize, amount: usize, limit: usize) -> Result<(), RuntimeError> {
    *work = work.checked_add(amount).ok_or(RuntimeError::Arithmetic)?;
    if *work > limit {
        return Err(RuntimeError::OperatorWorkLimit { limit });
    }
    Ok(())
}

fn tagged_units(
    generation: &CompiledGeneration,
    tag: super::ByteRange,
) -> Result<&[super::CompiledTagRuleRef], RuntimeError> {
    let tag = generation_bytes(&generation.bytes, tag)?;
    let refs = &generation.tag_rule_refs;
    let start = refs.partition_point(|entry| {
        generation_bytes(&generation.bytes, entry.tag).expect("compiled tag reference has a valid range") < tag
    });
    let end = refs.partition_point(|entry| {
        generation_bytes(&generation.bytes, entry.tag).expect("compiled tag reference has a valid range") <= tag
    });
    Ok(&refs[start..end])
}

// Shared by ordinary selected values and the upcoming provisional JSON
// collector. Context supplies existing programs and scratch, never a second
// operator implementation. Matched identity construction runs only on a match.
struct SingleInputContext<'a> {
    generation: &'a CompiledGeneration,
    request: &'a RequestState<'a>,
    tx: &'a TxState,
    group_scratch: &'a mut [Option<MatchScratch>],
    boolean_scratch: &'a mut [Option<RegexBooleanScratch>],
    capture_scratch: &'a mut [Option<RegexCaptureScratch>],
    scan_cache: Option<&'a mut StageScanCache>,
    json_budget: Option<&'a mut InspectionBudget>,
}

struct SingleInputPlan<'a> {
    operator: super::CompiledOperatorPlan,
    transformations: &'a [super::super::TransformationKind],
    multi_match: bool,
    runtime_expansion_byte_limit: usize,
    limits: RuntimeLimits,
    evaluation_mode: EvaluationMode,
}

fn evaluate_single_input(
    context: &mut SingleInputContext<'_>,
    plan: SingleInputPlan<'_>,
    value: &[u8],
    work: &mut usize,
    detector_work_bytes: &mut usize,
    matched_count: usize,
    make_matched: impl Fn(&[u8], Option<&mut InspectionBudget>) -> Result<(Vec<u8>, Vec<u8>), RuntimeError>,
) -> Result<Option<MemberMatch>, RuntimeError> {
    evaluate_single_input_with_transform_memo(
        context,
        plan,
        value,
        work,
        detector_work_bytes,
        matched_count,
        make_matched,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_single_input_with_transform_memo(
    context: &mut SingleInputContext<'_>,
    plan: SingleInputPlan<'_>,
    value: &[u8],
    work: &mut usize,
    detector_work_bytes: &mut usize,
    matched_count: usize,
    make_matched: impl Fn(&[u8], Option<&mut InspectionBudget>) -> Result<(Vec<u8>, Vec<u8>), RuntimeError>,
    transform_memo: Option<(&mut BorrowedTransformMemo, BorrowedTransformKey)>,
) -> Result<Option<MemberMatch>, RuntimeError> {
    let transform_limits = TransformLimits {
        max_output_bytes: plan.limits.max_transformation_output_bytes,
        max_work_bytes: plan.limits.max_transformation_work_bytes,
    };
    if !plan.multi_match {
        let transformed = match transform_memo {
            Some((memo, key)) if memo.key == Some(key) => Cow::Borrowed(value),
            Some((memo, key)) => {
                memo.clear();
                let transformed = apply_transformations_borrowed(value, plan.transformations, transform_limits)
                    .map_err(map_transform_error)?;
                // Borrowed proves complete validation and identity for every step.
                // Local transform limits are part of the key; downstream charges
                // still execute below even when a subsequent call reuses this proof.
                if matches!(&transformed, Cow::Borrowed(_)) {
                    memo.key = Some(key);
                }
                transformed
            }
            None => apply_transformations_borrowed(value, plan.transformations, transform_limits)
                .map_err(map_transform_error)?,
        };
        return evaluate_transformed_input(
            context,
            &plan,
            &transformed,
            value.len(),
            work,
            detector_work_bytes,
            matched_count,
            &make_matched,
        );
    }
    let transformed_values =
        apply_transformations_multimatch(value, plan.transformations, transform_limits).map_err(map_transform_error)?;
    for transformed in transformed_values {
        if let Some(matched) = evaluate_transformed_input(
            context,
            &plan,
            &transformed,
            value.len(),
            work,
            detector_work_bytes,
            matched_count,
            &make_matched,
        )? {
            return Ok(Some(matched));
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn evaluate_transformed_input(
    context: &mut SingleInputContext<'_>,
    plan: &SingleInputPlan<'_>,
    transformed: &[u8],
    original_bytes: usize,
    work: &mut usize,
    detector_work_bytes: &mut usize,
    matched_count: usize,
    make_matched: impl FnOnce(&[u8], Option<&mut InspectionBudget>) -> Result<(Vec<u8>, Vec<u8>), RuntimeError>,
) -> Result<Option<MemberMatch>, RuntimeError> {
    let mut accounting = OrderedStageAccounting {
        original_bytes,
        limits: plan.limits,
        work,
        detector_work_bytes,
    };
    let captures =
        inspect_predicate_stage(context, plan, transformed, &mut accounting).map_err(|error| match error {
            PredicateStageFailure::Semantic { error, .. }
            | PredicateStageFailure::Physical(error)
            | PredicateStageFailure::Observer(error) => error,
        })?;
    captures
        .map(|captures| {
            let (value, name) = make_matched(transformed, context.json_budget.as_deref_mut())?;
            finish_member_match(captures, value, name, matched_count, work, plan.limits)
        })
        .transpose()
}
fn finish_member_match(
    captures: Vec<CaptureEntry>,
    value: impl Into<MatchValue>,
    name: impl Into<MatchValue>,
    matched_count: usize,
    work: &mut usize,
    limits: RuntimeLimits,
) -> Result<MemberMatch, RuntimeError> {
    if matched_count == limits.max_operator_evaluations {
        return Err(RuntimeError::OperatorEvaluationLimit {
            limit: limits.max_operator_evaluations,
        });
    }
    let matched = MemberMatch {
        captures,
        matched_var: Some(value.into()),
        matched_var_name: Some(name.into()),
    };
    charge_operator_work(work, member_match_bytes(&matched)?, limits)?;
    Ok(matched)
}

#[derive(Clone, Copy)]
enum PredicateCheckpoint {
    DetectorInput,
    Operator,
    Captures,
}

enum PredicateStageFailure<E> {
    Semantic {
        checkpoint: PredicateCheckpoint,
        error: RuntimeError,
    },
    Physical(RuntimeError),
    Observer(E),
}

impl<E> PredicateStageFailure<E> {
    fn at(checkpoint: PredicateCheckpoint, error: RuntimeError) -> Self {
        if matches!(error, RuntimeError::Allocation | RuntimeError::Arithmetic) {
            Self::Physical(error)
        } else {
            Self::Semantic { checkpoint, error }
        }
    }
}

impl<E> From<RuntimeError> for PredicateStageFailure<E> {
    fn from(error: RuntimeError) -> Self {
        Self::at(PredicateCheckpoint::Captures, error)
    }
}

/// Observer failures are physical inspection failures. Predicate/capture errors
/// retain their semantic checkpoint so a provisional collector can defer them.
trait PredicateStageObserver {
    type Error;
    type CaptureValue: StageCapturePayload;
    type CaptureKey: StageCaptureKey;
    fn observes_physical_work(&self) -> bool {
        false
    }
    fn operator_work(&mut self, _work: usize) -> Result<(), Self::Error> {
        Ok(())
    }
    fn charge_stage(&mut self, transformed_bytes: usize, detector_work: usize) -> Result<(), Self::Error>;
    fn before_captures(&mut self, transformed_bytes: usize) -> Result<(), Self::Error>;
    fn captures(&mut self, captures: &[CaptureEntry<Self::CaptureValue, Self::CaptureKey>]) -> Result<(), Self::Error>;
    fn admit_capture_allocation(&mut self, current_bytes: usize, required_bytes: usize) -> Result<(), Self::Error>;
    fn capture_capacity(&mut self, actual_bytes: usize) -> Result<(), Self::Error>;
}
type PredicateStageResult<O> = Result<
    Option<Vec<CaptureEntry<<O as PredicateStageObserver>::CaptureValue, <O as PredicateStageObserver>::CaptureKey>>>,
    PredicateStageFailure<<O as PredicateStageObserver>::Error>,
>;

struct OrderedStageAccounting<'a> {
    original_bytes: usize,
    limits: RuntimeLimits,
    work: &'a mut usize,
    detector_work_bytes: &'a mut usize,
}

impl PredicateStageObserver for OrderedStageAccounting<'_> {
    type Error = RuntimeError;
    type CaptureValue = MatchValue;
    type CaptureKey = MatchValue;
    fn charge_stage(&mut self, transformed_bytes: usize, detector_work: usize) -> Result<(), RuntimeError> {
        if detector_work != 0 {
            *self.detector_work_bytes = self
                .detector_work_bytes
                .checked_add(detector_work)
                .ok_or(RuntimeError::Arithmetic)?;
            if *self.detector_work_bytes > self.limits.max_operator_work_bytes {
                return Err(RuntimeError::OperatorWorkLimit {
                    limit: self.limits.max_operator_work_bytes,
                });
            }
        }
        charge_operator_work(
            self.work,
            self.original_bytes
                .checked_add(transformed_bytes)
                .and_then(|bytes| bytes.checked_add(detector_work))
                .ok_or(RuntimeError::Arithmetic)?,
            self.limits,
        )
    }
    fn before_captures(&mut self, _transformed_bytes: usize) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn captures(&mut self, _captures: &[CaptureEntry]) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn admit_capture_allocation(&mut self, _current_bytes: usize, _required_bytes: usize) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn capture_capacity(&mut self, _actual_bytes: usize) -> Result<(), RuntimeError> {
        Ok(())
    }
}

fn detector_work_for_stage(plan: super::CompiledOperatorPlan, len: usize) -> Result<usize, RuntimeError> {
    match plan.operand {
        super::CompiledOperatorOperand::DetectSqli { .. } => {
            crate::injection::work_cost(crate::injection::Kind::Sqli, len)
        }
        super::CompiledOperatorOperand::DetectXss => crate::injection::work_cost(crate::injection::Kind::Xss, len),
        _ => Some(0),
    }
    .ok_or(RuntimeError::DetectorInputLimit {
        limit: crate::injection::MAX_INPUT_BYTES,
    })
}

fn inspect_predicate_stage<O: PredicateStageObserver>(
    context: &mut SingleInputContext<'_>,
    plan: &SingleInputPlan<'_>,
    transformed: &[u8],
    observer: &mut O,
) -> PredicateStageResult<O> {
    let mut captures = CaptureBuilder::default();
    if inspect_predicate_stage_into(context, plan, transformed, observer, &mut captures)? {
        Ok(Some(captures.entries))
    } else {
        Ok(None)
    }
}

fn inspect_predicate_stage_into<O: PredicateStageObserver>(
    context: &mut SingleInputContext<'_>,
    plan: &SingleInputPlan<'_>,
    transformed: &[u8],
    observer: &mut O,
    captures: &mut CaptureBuilder<O::CaptureValue, O::CaptureKey>,
) -> Result<bool, PredicateStageFailure<O::Error>> {
    inspect_predicate_stage_with_hit_matcher(context, plan, transformed, observer, captures, None)
}

fn inspect_predicate_stage_with_hit_matcher<O: PredicateStageObserver>(
    context: &mut SingleInputContext<'_>,
    plan: &SingleInputPlan<'_>,
    transformed: &[u8],
    observer: &mut O,
    captures: &mut CaptureBuilder<O::CaptureValue, O::CaptureKey>,
    hit_matcher: Option<super::CompiledOperatorPlan>,
) -> Result<bool, PredicateStageFailure<O::Error>> {
    if !captures.entries.is_empty() || captures.semantic_bytes != 0 || captures.heap_bytes != 0 {
        return Err(RuntimeError::GenerationInvariant.into());
    }
    if hit_matcher.is_some()
        && (!matches!(
            plan.operator.operand,
            super::CompiledOperatorOperand::Regex(super::RegexPlanRef::Capture(_))
        ) || !matches!(
            hit_matcher.map(|p| p.operand),
            Some(super::CompiledOperatorOperand::Matcher { pattern_count: 1, .. })
        ))
    {
        return Err(RuntimeError::GenerationInvariant.into());
    }
    let detection = hit_matcher.unwrap_or(plan.operator);
    let detector_work = detector_work_for_stage(plan.operator, transformed.len())
        .map_err(|error| PredicateStageFailure::at(PredicateCheckpoint::DetectorInput, error))?;
    observer
        .charge_stage(transformed.len(), detector_work)
        .map_err(PredicateStageFailure::Observer)?;
    if observer.observes_physical_work() {
        let work =
            physical_operator_work(context, detection, transformed.len()).map_err(PredicateStageFailure::Physical)?;
        observer.operator_work(work).map_err(PredicateStageFailure::Observer)?;
    }
    let (positive, capture) = evaluate_operator(
        context.generation,
        context.request,
        context.tx,
        context.group_scratch,
        context.boolean_scratch,
        context.capture_scratch,
        detection,
        transformed,
        plan.runtime_expansion_byte_limit,
        plan.evaluation_mode,
        context.scan_cache.as_deref_mut(),
        context.json_budget.as_deref_mut(),
    )
    .map_err(|error| PredicateStageFailure::at(PredicateCheckpoint::Operator, error))?;
    if positive == plan.operator.negated {
        return Ok(false);
    }
    if !plan.operator.negated {
        observer
            .before_captures(transformed.len())
            .map_err(PredicateStageFailure::Observer)?;
        extract_captures_into(
            context.generation,
            context.capture_scratch,
            plan.operator,
            transformed,
            plan.limits,
            capture,
            observer,
            captures,
        )?;
    }
    observer
        .captures(&captures.entries)
        .map_err(PredicateStageFailure::Observer)?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn extract_captures_into<O: PredicateStageObserver>(
    generation: &CompiledGeneration,
    scratch: &mut [Option<RegexCaptureScratch>],
    plan: super::CompiledOperatorPlan,
    input: &[u8],
    limits: RuntimeLimits,
    capture: Option<OperatorCapture>,
    observer: &mut O,
    captures: &mut CaptureBuilder<O::CaptureValue, O::CaptureKey>,
) -> Result<(), PredicateStageFailure<O::Error>> {
    if !matches!(
        plan.operand,
        super::CompiledOperatorOperand::Regex(super::RegexPlanRef::Capture(_))
            | super::CompiledOperatorOperand::CapturePhrase(_)
            | super::CompiledOperatorOperand::DetectSqli { capture: true }
    ) {
        return Ok(());
    }
    let bound = match plan.operand {
        super::CompiledOperatorOperand::Regex(super::RegexPlanRef::Capture(program)) => generation
            .capture_regexes
            .get(program)
            .ok_or(RuntimeError::GenerationInvariant)?
            .capture_len()
            .checked_mul(2)
            .ok_or(RuntimeError::Arithmetic)?,
        // Each regex group may produce a numeric key and a named alias;
        // phrase and SQLi fingerprints produce only capture 0.
        _ => 1,
    };
    let slots = bound.min(limits.max_capture_entries).min(64);
    let initial = slots
        .checked_mul(size_of::<CaptureEntry<O::CaptureValue, O::CaptureKey>>())
        .ok_or(RuntimeError::Arithmetic)?;
    if slots > captures.entries.capacity() {
        let current = captures.retained_bytes()?;
        observer
            .admit_capture_allocation(current, current.checked_add(initial).ok_or(RuntimeError::Arithmetic)?)
            .map_err(PredicateStageFailure::Observer)?;
        captures
            .entries
            .try_reserve_exact(slots)
            .map_err(|_| RuntimeError::Allocation)?;
    }
    observer
        .capture_capacity(captures.retained_bytes()?)
        .map_err(PredicateStageFailure::Observer)?;
    match plan.operand {
        super::CompiledOperatorOperand::Regex(super::RegexPlanRef::Capture(program)) => {
            let regex = generation
                .capture_regexes
                .get(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            let slot = scratch.get_mut(program).ok_or(RuntimeError::GenerationInvariant)?;
            let scratch = lazy_capture_scratch(slot, regex);
            let results = regex.captures(input, scratch).map_err(map_regex_scratch_error)?;
            for index in 0..regex.capture_len() {
                let Some(range) = results.group(index) else {
                    continue;
                };
                let value = CaptureSource::InputRange {
                    input,
                    start: range.start,
                    len: range
                        .end
                        .checked_sub(range.start)
                        .ok_or(RuntimeError::GenerationInvariant)?,
                };
                let (digits, start) = decimal_bytes(index);
                push_capture(
                    captures,
                    CaptureKeySource::Numeric {
                        index,
                        bytes: &digits[start..],
                    },
                    value,
                    limits,
                    observer,
                )?;
                if let Some(name) = regex.capture_name(index) {
                    push_capture(
                        captures,
                        CaptureKeySource::Named {
                            program,
                            group: index,
                            bytes: name.as_bytes(),
                        },
                        value,
                        limits,
                        observer,
                    )?;
                }
            }
        }
        super::CompiledOperatorOperand::CapturePhrase(_) => {
            let Some(OperatorCapture::Phrase(matched)) = capture else {
                return Err(RuntimeError::GenerationInvariant.into());
            };
            let value = CaptureSource::InputRange {
                input,
                start: matched.start(),
                len: matched
                    .end()
                    .checked_sub(matched.start())
                    .ok_or(RuntimeError::GenerationInvariant)?,
            };
            push_capture(
                captures,
                CaptureKeySource::Numeric { index: 0, bytes: b"0" },
                value,
                limits,
                observer,
            )?;
        }
        super::CompiledOperatorOperand::DetectSqli { capture: true } => {
            let Some(OperatorCapture::Fingerprint(fingerprint)) = capture else {
                return Err(RuntimeError::GenerationInvariant.into());
            };
            push_capture(
                captures,
                CaptureKeySource::Numeric { index: 0, bytes: b"0" },
                CaptureSource::Fingerprint(fingerprint),
                limits,
                observer,
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn push_capture<O: PredicateStageObserver>(
    captures: &mut CaptureBuilder<O::CaptureValue, O::CaptureKey>,
    key_source: CaptureKeySource<'_>,
    source: CaptureSource<'_>,
    limits: RuntimeLimits,
    observer: &mut O,
) -> Result<(), PredicateStageFailure<O::Error>> {
    let value_len = source.bytes()?.len();
    let key_len = key_source.bytes().len();
    let key_allocation = O::CaptureKey::allocation_bytes(key_source);
    let allocation_bytes = O::CaptureValue::allocation_bytes(source)?;
    if captures.entries.len() == limits.max_capture_entries {
        return Err(RuntimeError::CaptureEntryLimit {
            limit: limits.max_capture_entries,
        }
        .into());
    }
    let actual = captures
        .semantic_bytes
        .checked_add(key_len)
        .and_then(|total| total.checked_add(value_len))
        .ok_or(RuntimeError::Arithmetic)?;
    if actual > limits.max_capture_bytes {
        return Err(RuntimeError::CaptureByteLimit {
            limit: limits.max_capture_bytes,
        }
        .into());
    }
    let retained = captures.retained_bytes()?;
    let extra_slot = if captures.entries.len() == captures.entries.capacity() {
        size_of::<CaptureEntry<O::CaptureValue, O::CaptureKey>>()
    } else {
        0
    };
    let required = retained
        .checked_add(extra_slot)
        .and_then(|bytes| bytes.checked_add(key_allocation))
        .and_then(|bytes| bytes.checked_add(allocation_bytes))
        .ok_or(RuntimeError::Arithmetic)?;
    let peak = if extra_slot != 0 {
        required
            .checked_add(
                captures
                    .entries
                    .capacity()
                    .checked_mul(size_of::<CaptureEntry<O::CaptureValue, O::CaptureKey>>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .ok_or(RuntimeError::Arithmetic)?
    } else {
        required
    };
    observer
        .admit_capture_allocation(retained, peak)
        .map_err(PredicateStageFailure::Observer)?;
    captures
        .entries
        .try_reserve_exact(1)
        .map_err(|_| RuntimeError::Allocation)?;
    let descriptors = captures.retained_bytes()?;
    observer
        .capture_capacity(descriptors)
        .map_err(PredicateStageFailure::Observer)?;
    observer
        .admit_capture_allocation(
            descriptors,
            descriptors
                .checked_add(key_allocation)
                .ok_or(RuntimeError::Arithmetic)?,
        )
        .map_err(PredicateStageFailure::Observer)?;
    let key = O::CaptureKey::from_source(key_source)?;
    #[cfg(test)]
    measurements::record(Event::TemporaryCaptureCopyBytes, key_allocation);
    let with_key = descriptors
        .checked_add(key.heap_capacity())
        .ok_or(RuntimeError::Arithmetic)?;
    observer
        .capture_capacity(with_key)
        .map_err(PredicateStageFailure::Observer)?;
    observer
        .admit_capture_allocation(
            with_key,
            with_key.checked_add(allocation_bytes).ok_or(RuntimeError::Arithmetic)?,
        )
        .map_err(PredicateStageFailure::Observer)?;
    let value = O::CaptureValue::from_source(source)?;
    #[cfg(test)]
    measurements::record(Event::TemporaryCaptureCopyBytes, allocation_bytes);
    observer
        .capture_capacity(
            with_key
                .checked_add(value.heap_capacity())
                .ok_or(RuntimeError::Arithmetic)?,
        )
        .map_err(PredicateStageFailure::Observer)?;
    let heap_bytes = captures
        .heap_bytes
        .checked_add(key.heap_capacity())
        .and_then(|bytes| bytes.checked_add(value.heap_capacity()))
        .ok_or(RuntimeError::Arithmetic)?;
    captures.entries.push(CaptureEntry { key, value });
    captures.semantic_bytes = actual;
    captures.heap_bytes = heap_bytes;
    Ok(())
}

fn matched_variable(
    request: &RequestState,
    tx: &TxState,
    response: Option<&ResponseState>,
    selected: SelectedValue,
    transformed: &[u8],
    budget: Option<&mut InspectionBudget>,
) -> Result<(Vec<u8>, Vec<u8>), RuntimeError> {
    let value = owned_copy(transformed)?;
    let selected_name = selected.name_bytes(request, tx, response, budget)?;
    let name = matched_variable_name(selected.collection, selected_name)?;
    Ok((value, name))
}

fn matched_variable_name(collection: CollectionKind, selected_name: Option<&[u8]>) -> Result<Vec<u8>, RuntimeError> {
    let collection = collection.as_str().as_bytes();
    let name_len = selected_name.map_or(0, |name| name.len().saturating_add(1));
    let capacity = collection.len().checked_add(name_len).ok_or(RuntimeError::Arithmetic)?;
    let mut name = Vec::new();
    name.try_reserve_exact(capacity).map_err(|_| RuntimeError::Allocation)?;
    name.extend_from_slice(collection);
    if let Some(selected_name) = selected_name {
        name.push(b':');
        name.extend_from_slice(selected_name);
    }
    Ok(name)
}

fn apply_member_match(
    staged: &mut TxState,
    capture_keys: &mut Vec<MatchValue>,
    matched: MemberMatch,
    limits: RuntimeLimits,
    request: &RequestState,
    mut json_budget: Option<&mut InspectionBudget>,
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
        let entry = TxEntry {
            key: name.try_clone()?,
            value: value.try_clone()?,
        };
        let payload = tx_payload_bytes(entry.key.len(), entry.value.len())?;
        let total = tx_payload_delta(staged.matched_vars_payload_bytes, 0, payload)?;
        staged.matched_vars.push(entry);
        staged.matched_vars_payload_bytes = total;
    }
    staged.matched_var = matched.matched_var;
    staged.matched_var_name = matched.matched_var_name;
    staged.refresh_retained_bytes();
    if staged.retained_bytes > limits.max_tx_bytes {
        return Err(RuntimeError::TxByteLimit {
            limit: limits.max_tx_bytes,
        });
    }
    if matched.captures.is_empty() {
        return Ok(());
    }
    for key in capture_keys.iter() {
        staged.delete(
            key.bytes(request, json_budget.as_deref_mut())?,
            request,
            json_budget.as_deref_mut(),
        )?;
    }
    capture_keys.clear();
    capture_keys
        .try_reserve_exact(matched.captures.len())
        .map_err(|_| RuntimeError::Allocation)?;
    for capture in matched.captures {
        let stored_key = capture.key.try_clone()?;
        staged.set(capture.key, capture.value, limits, request, json_budget.as_deref_mut())?;
        capture_keys.push(stored_key);
    }
    Ok(())
}

fn try_clone_keys(values: &[MatchValue]) -> Result<Vec<MatchValue>, RuntimeError> {
    let mut cloned = Vec::new();
    cloned
        .try_reserve_exact(values.len())
        .map_err(|_| RuntimeError::Allocation)?;
    for value in values {
        cloned.push(value.try_clone()?);
    }
    Ok(cloned)
}

fn chain_branch_bytes(branch: &ChainBranch) -> Result<usize, RuntimeError> {
    branch.capture_keys.iter().try_fold(
        branch
            .capture_keys
            .len()
            .checked_mul(size_of::<MatchValue>())
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
        .checked_add(matched.matched_var.as_ref().map_or(0, MatchValue::len))
        .and_then(|value| value.checked_add(matched.matched_var_name.as_ref().map_or(0, MatchValue::len)))
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
    fn add_removed_unit(&mut self, unit: usize, limits: RuntimeLimits) -> Result<(), RuntimeError> {
        let Err(index) = self.removed_units.binary_search(&unit) else {
            return Ok(());
        };
        if self.removed_units.len().saturating_add(self.target_exclusions.len()) >= limits.max_ctl_entries {
            return Err(RuntimeError::CtlEntryLimit {
                limit: limits.max_ctl_entries,
            });
        }
        self.removed_units
            .try_reserve(1)
            .map_err(|_| RuntimeError::Allocation)?;
        self.removed_units.insert(index, unit);
        self.refresh_retained_bytes();
        if self.retained_bytes > limits.max_tx_bytes {
            return Err(RuntimeError::TxByteLimit {
                limit: limits.max_tx_bytes,
            });
        }
        Ok(())
    }

    fn add_target_exclusion(
        &mut self,
        entry: ActiveTargetExclusion,
        limits: RuntimeLimits,
    ) -> Result<(), RuntimeError> {
        let start = self
            .target_exclusions
            .partition_point(|existing| existing.unit < entry.unit);
        let end = self
            .target_exclusions
            .partition_point(|existing| existing.unit <= entry.unit);
        if self.target_exclusions[start..end].contains(&entry) {
            return Ok(());
        }
        if self.removed_units.len().saturating_add(self.target_exclusions.len()) >= limits.max_ctl_entries {
            return Err(RuntimeError::CtlEntryLimit {
                limit: limits.max_ctl_entries,
            });
        }
        self.target_exclusions
            .try_reserve(1)
            .map_err(|_| RuntimeError::Allocation)?;
        self.target_exclusions.insert(end, entry);
        self.refresh_retained_bytes();
        if self.retained_bytes > limits.max_tx_bytes {
            return Err(RuntimeError::TxByteLimit {
                limit: limits.max_tx_bytes,
            });
        }
        Ok(())
    }

    fn find_entry(
        &self,
        key: &[u8],
        request: &RequestState,
        mut json_budget: Option<&mut InspectionBudget>,
    ) -> Result<Option<usize>, RuntimeError> {
        for (index, entry) in self.entries.iter().enumerate() {
            if entry
                .key
                .bytes(request, json_budget.as_deref_mut())?
                .eq_ignore_ascii_case(key)
            {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    fn get(
        &self,
        key: &[u8],
        request: &RequestState,
        json_budget: Option<&mut InspectionBudget>,
    ) -> Result<Option<&MatchValue>, RuntimeError> {
        Ok(self
            .find_entry(key, request, json_budget)?
            .map(|index| &self.entries[index].value))
    }

    /// Public observation validates shared keys without consuming runtime work.
    fn observed_get(&self, key: &[u8], request: &RequestState) -> Result<Option<&MatchValue>, RuntimeError> {
        for entry in &self.entries {
            if entry.key.observed_bytes(request)?.eq_ignore_ascii_case(key) {
                return Ok(Some(&entry.value));
            }
        }
        Ok(None)
    }

    fn delete(
        &mut self,
        key: &[u8],
        request: &RequestState,
        json_budget: Option<&mut InspectionBudget>,
    ) -> Result<(), RuntimeError> {
        if let Some(index) = self.find_entry(key, request, json_budget)? {
            let entry = &self.entries[index];
            let removed = tx_payload_bytes(entry.key.len(), entry.value.len())?;
            let total = tx_payload_delta(self.entry_payload_bytes, removed, 0)?;
            self.entries.remove(index);
            self.entry_payload_bytes = total;
            self.refresh_retained_bytes();
        }
        Ok(())
    }

    fn set(
        &mut self,
        key: impl Into<MatchValue>,
        value: impl Into<MatchValue>,
        limits: RuntimeLimits,
        request: &RequestState,
        mut json_budget: Option<&mut InspectionBudget>,
    ) -> Result<(), RuntimeError> {
        let key = key.into();
        let value = value.into();
        if let Some(index) = self.find_entry(key.bytes(request, json_budget.as_deref_mut())?, request, json_budget)? {
            let entry = &mut self.entries[index];
            let total = tx_payload_delta(self.entry_payload_bytes, entry.value.len() as u128, value.len() as u128)?;
            entry.value = value;
            self.entry_payload_bytes = total;
            self.refresh_retained_bytes();
        } else {
            if self.entries.len() == limits.max_tx_variables {
                return Err(RuntimeError::TxVariableLimit {
                    limit: limits.max_tx_variables,
                });
            }
            self.entries.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
            let payload = tx_payload_bytes(key.len(), value.len())?;
            let total = tx_payload_delta(self.entry_payload_bytes, 0, payload)?;
            self.entries.push(TxEntry { key, value });
            self.entry_payload_bytes = total;
            self.refresh_retained_bytes();
        }
        if self.retained_bytes > limits.max_tx_bytes {
            return Err(RuntimeError::TxByteLimit {
                limit: limits.max_tx_bytes,
            });
        }
        Ok(())
    }

    fn try_clone(&self) -> Result<Self, RuntimeError> {
        self.try_clone_with_matches(true)
    }

    fn try_clone_for_unit(&self) -> Result<Self, RuntimeError> {
        #[cfg(test)]
        measurements::record(Event::UnitTxClones, 1);
        self.try_clone_with_matches(false)
    }

    fn try_clone_with_matches(&self, keep_matches: bool) -> Result<Self, RuntimeError> {
        let entries = try_clone_entries(&self.entries)?;
        let matched_vars = if keep_matches {
            let mut cloned = Vec::new();
            cloned
                .try_reserve_exact(self.matched_vars.len())
                .map_err(|_| RuntimeError::Allocation)?;
            for entry in &self.matched_vars {
                cloned.push(TxEntry {
                    key: entry.key.try_clone()?,
                    value: entry.value.try_clone()?,
                });
            }
            cloned
        } else {
            Vec::new()
        };
        let mut removed_units = Vec::new();
        removed_units
            .try_reserve_exact(self.removed_units.len())
            .map_err(|_| RuntimeError::Allocation)?;
        removed_units.extend_from_slice(&self.removed_units);
        let mut target_exclusions = Vec::new();
        target_exclusions
            .try_reserve_exact(self.target_exclusions.len())
            .map_err(|_| RuntimeError::Allocation)?;
        target_exclusions.extend_from_slice(&self.target_exclusions);
        let mut cloned = Self {
            check_policy: self.check_policy,
            #[cfg(test)]
            force_unpruned_inspection: self.force_unpruned_inspection,
            entries,
            entry_payload_bytes: self.entry_payload_bytes,
            matched_var: if keep_matches {
                self.matched_var.as_ref().map(MatchValue::try_clone).transpose()?
            } else {
                None
            },
            matched_var_name: if keep_matches {
                self.matched_var_name.as_ref().map(MatchValue::try_clone).transpose()?
            } else {
                None
            },
            matched_vars,
            matched_vars_payload_bytes: if keep_matches {
                self.matched_vars_payload_bytes
            } else {
                0
            },
            removed_units,
            target_exclusions,
            request_body_processor: self.request_body_processor,
            force_request_body_variable: self.force_request_body_variable,
            retained_bytes: self.retained_bytes,
        };
        cloned.refresh_retained_bytes();
        Ok(cloned)
    }

    /// Payload totals stay exact; only the public semantic charge saturates.
    fn refresh_retained_bytes(&mut self) {
        self.retained_bytes = self
            .entries
            .len()
            .saturating_mul(size_of::<TxEntry>())
            .saturating_add(usize::try_from(self.entry_payload_bytes).unwrap_or(usize::MAX))
            .saturating_add(self.matched_var.as_ref().map_or(0, MatchValue::len))
            .saturating_add(self.matched_var_name.as_ref().map_or(0, MatchValue::len))
            .saturating_add(
                self.matched_vars
                    .len()
                    .saturating_mul(size_of::<TxEntry<MatchValue, MatchValue>>()),
            )
            .saturating_add(self.removed_units.len().saturating_mul(size_of::<usize>()))
            .saturating_add(
                self.target_exclusions
                    .len()
                    .saturating_mul(size_of::<ActiveTargetExclusion>()),
            )
            .saturating_add(usize::try_from(self.matched_vars_payload_bytes).unwrap_or(usize::MAX));
    }
}

fn tx_payload_bytes(key: usize, value: usize) -> Result<u128, RuntimeError> {
    (key as u128).checked_add(value as u128).ok_or(RuntimeError::Arithmetic)
}

fn tx_payload_delta(current: u128, removed: u128, added: u128) -> Result<u128, RuntimeError> {
    current
        .checked_sub(removed)
        .and_then(|bytes| bytes.checked_add(added))
        .ok_or(RuntimeError::Arithmetic)
}

fn try_clone_entries(entries: &[TxEntry]) -> Result<Vec<TxEntry>, RuntimeError> {
    let mut cloned = Vec::new();
    cloned
        .try_reserve_exact(entries.len())
        .map_err(|_| RuntimeError::Allocation)?;
    for entry in entries {
        cloned.push(TxEntry {
            key: entry.key.try_clone()?,
            value: entry.value.try_clone()?,
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
    json_budget: Option<&mut InspectionBudget>,
) -> Result<Vec<u8>, RuntimeError> {
    if let Some(constant) = setvar.constant {
        let (digits, start) = signed_decimal_bytes(constant);
        return owned_copy(&digits[start..]);
    }
    if let Some(template) = setvar.value {
        return expand_template::<true>(generation, request, tx, template, member_limit, json_budget);
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

fn matcher_program_scratch_bytes(generation: &CompiledGeneration) -> Result<usize, RuntimeError> {
    let mut total = 0_usize;
    for matcher in generation.all_matchers() {
        total = total
            .checked_add(
                matcher
                    .required_scratch_bytes()
                    .checked_sub(size_of::<MatchScratch>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .ok_or(RuntimeError::Arithmetic)?;
    }
    for program in &generation.boolean_regexes {
        total = total
            .checked_add(
                program
                    .required_scratch_bytes()
                    .checked_sub(size_of::<RegexBooleanScratch>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .ok_or(RuntimeError::Arithmetic)?;
    }
    for program in &generation.capture_regexes {
        total = total
            .checked_add(
                program
                    .required_scratch_bytes()
                    .checked_sub(size_of::<RegexCaptureScratch>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .ok_or(RuntimeError::Arithmetic)?;
    }
    Ok(total)
}

fn matcher_scratch_capacity(
    program_bytes: usize,
    groups: usize,
    booleans: usize,
    captures: usize,
) -> Result<usize, RuntimeError> {
    let mut total = program_bytes;
    for (count, unit) in [
        (groups, size_of::<Option<MatchScratch>>()),
        (booleans, size_of::<Option<RegexBooleanScratch>>()),
        (captures, size_of::<Option<RegexCaptureScratch>>()),
    ] {
        total = total
            .checked_add(count.checked_mul(unit).ok_or(RuntimeError::Arithmetic)?)
            .ok_or(RuntimeError::Arithmetic)?;
    }
    Ok(total)
}
fn lazy_group_scratch<'a>(
    slot: &'a mut Option<MatchScratch>,
    program: &crate::matcher::CompiledMatcher,
) -> &'a mut MatchScratch {
    slot.get_or_insert_with(|| {
        #[cfg(test)]
        measurements::record(Event::GroupScratchInitializations, 1);
        program.create_scratch()
    })
}
fn lazy_boolean_scratch<'a>(
    slot: &'a mut Option<RegexBooleanScratch>,
    program: &crate::matcher::CompiledRegexProgram,
) -> &'a mut RegexBooleanScratch {
    slot.get_or_insert_with(|| {
        #[cfg(test)]
        measurements::record(Event::BooleanScratchInitializations, 1);
        program.create_scratch()
    })
}
fn lazy_capture_scratch<'a>(
    slot: &'a mut Option<RegexCaptureScratch>,
    program: &crate::matcher::CompiledCaptureRegexProgram,
) -> &'a mut RegexCaptureScratch {
    slot.get_or_insert_with(|| {
        #[cfg(test)]
        measurements::record(Event::CaptureScratchInitializations, 1);
        program.create_scratch()
    })
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

#[derive(Clone, Copy)]
struct JsonNoHitTail {
    query_count: usize,
    selected_count: usize,
    selection_work: usize,
    replay_work: usize,
    logical_work: usize,
}

struct SelectionParams {
    unit_id: usize,
    plan_id: usize,
    limit: usize,
    work_limit: usize,
    include_json: bool,
}

#[allow(clippy::too_many_arguments)]
fn select_values(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    response: Option<&ResponseState>,
    scratch: &mut [Option<RegexBooleanScratch>],
    ctl_work: &mut usize,
    params: SelectionParams,
    mut json_budget: Option<&mut InspectionBudget>,
    selected: &mut Vec<SelectedValue>,
    storage: SelectionStorage,
) -> Result<(), RuntimeError> {
    let SelectionParams {
        unit_id,
        plan_id,
        limit,
        work_limit,
        include_json,
    } = params;
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
    let exclude_start = tx.target_exclusions.partition_point(|entry| entry.unit < unit_id);
    let exclude_end = tx.target_exclusions.partition_point(|entry| entry.unit <= unit_id);
    let exclusions = &tx.target_exclusions[exclude_start..exclude_end];
    for target in records {
        let json_count = if target.modifier == super::TargetModifier::Count
            && target.selector == super::CompiledTargetSelector::None
            && exclusions.is_empty()
            && matches!(target.collection, CollectionKind::Args | CollectionKind::ArgsNames)
        {
            request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::json)
                .map(JsonPublication::argument_count)
        } else {
            None
        };
        let mut candidates = CandidateCursor::new(target.collection, include_json && json_count.is_none());
        match target.modifier {
            super::TargetModifier::None => {
                while let Some(candidate) = candidates.next(request, tx, response, json_budget.as_deref_mut())? {
                    if selector_matches(
                        generation,
                        request,
                        tx,
                        response,
                        scratch,
                        target.collection,
                        target.selector,
                        candidate,
                        json_budget.as_deref_mut(),
                    )? && !ctl_excludes_value(
                        generation,
                        request,
                        tx,
                        response,
                        scratch,
                        exclusions,
                        candidate,
                        ctl_work,
                        work_limit,
                        json_budget.as_deref_mut(),
                    )? {
                        if selected.len() == limit {
                            return Err(RuntimeError::SelectedValueLimit { limit });
                        }
                        storage.reserve(selected, limit)?;
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
                            json_budget.as_deref_mut(),
                        )?;
                    if !remove {
                        selected[write] = candidate;
                        write += 1;
                    }
                }
                selected.truncate(write);
            }
            super::TargetModifier::Count => {
                let mut count = json_count.unwrap_or(0);
                while let Some(candidate) = candidates.next(request, tx, response, json_budget.as_deref_mut())? {
                    if selector_matches(
                        generation,
                        request,
                        tx,
                        response,
                        scratch,
                        target.collection,
                        target.selector,
                        candidate,
                        json_budget.as_deref_mut(),
                    )? && !ctl_excludes_value(
                        generation,
                        request,
                        tx,
                        response,
                        scratch,
                        exclusions,
                        candidate,
                        ctl_work,
                        work_limit,
                        json_budget.as_deref_mut(),
                    )? {
                        count = count.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
                    }
                }
                if selected.len() == limit {
                    return Err(RuntimeError::SelectedValueLimit { limit });
                }
                let (digits, start) = decimal_bytes(count);
                storage.reserve(selected, limit)?;
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
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn ctl_excludes_value(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    response: Option<&ResponseState>,
    scratch: &mut [Option<RegexBooleanScratch>],
    exclusions: &[ActiveTargetExclusion],
    candidate: SelectedValue,
    work: &mut usize,
    limit: usize,
    mut json_budget: Option<&mut InspectionBudget>,
) -> Result<bool, RuntimeError> {
    for exclusion in exclusions {
        *work = work.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
        if *work > limit {
            return Err(RuntimeError::OperatorWorkLimit { limit });
        }
        if exclusion.collection != candidate.collection {
            continue;
        }
        *work = work
            .checked_add(
                candidate
                    .name_bytes(request, tx, response, json_budget.as_deref_mut())?
                    .map_or(1, |name| name.len().saturating_add(1)),
            )
            .ok_or(RuntimeError::Arithmetic)?;
        if *work > limit {
            return Err(RuntimeError::OperatorWorkLimit { limit });
        }
        if selector_matches(
            generation,
            request,
            tx,
            response,
            scratch,
            exclusion.collection,
            exclusion.selector,
            candidate,
            json_budget.as_deref_mut(),
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Clone, Copy)]
struct SelectionStorage {
    capacity_bytes: usize,
    retained_limit: usize,
}

impl SelectionStorage {
    /// Simulate complete geometric selection growth with conservative realloc
    /// overlap admission. Near-limit uncertainty falls back to actual selection.
    fn summary_capacity(self, mut capacity: usize, count: usize, limit: usize) -> Option<usize> {
        let maximum = limit.min(self.capacity_bytes / size_of::<SelectedValue>());
        while capacity < count {
            if capacity >= maximum {
                return None;
            }
            let target = capacity.checked_mul(2)?.max(4).min(maximum);
            let overlap = capacity.checked_add(target)?.checked_mul(size_of::<SelectedValue>())?;
            if target <= capacity || overlap > self.capacity_bytes {
                return None;
            }
            capacity = target;
        }
        let bytes = capacity.checked_mul(size_of::<SelectedValue>())?;
        (bytes <= self.capacity_bytes).then_some(bytes)
    }

    fn reserve(self, selected: &mut Vec<SelectedValue>, limit: usize) -> Result<(), RuntimeError> {
        if selected.len() < selected.capacity() {
            return Ok(());
        }
        let maximum = limit.min(self.capacity_bytes / size_of::<SelectedValue>());
        if selected.len() >= maximum {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.retained_limit,
            });
        }
        let target = selected.capacity().saturating_mul(2).max(4).min(maximum);
        selected
            .try_reserve_exact(target - selected.len())
            .map_err(|_| RuntimeError::Allocation)?;
        if selected
            .capacity()
            .checked_mul(size_of::<SelectedValue>())
            .ok_or(RuntimeError::Arithmetic)?
            > self.capacity_bytes
        {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.retained_limit,
            });
        }
        Ok(())
    }
}

struct CandidateCursor {
    collection: CollectionKind,
    include_json: bool,
    stage: u8,
    index: usize,
    next_json: Option<CandidateId>,
}

impl CandidateCursor {
    fn new(collection: CollectionKind, include_json: bool) -> Self {
        Self {
            collection,
            include_json,
            stage: 0,
            index: 0,
            next_json: None,
        }
    }

    fn next(
        &mut self,
        request: &RequestState,
        tx: &TxState,
        response: Option<&ResponseState>,
        json_budget: Option<&mut InspectionBudget>,
    ) -> Result<Option<SelectedValue>, RuntimeError> {
        let collection = self.collection;
        let selected = |name, value| {
            Some(SelectedValue {
                collection,
                name,
                value,
            })
        };
        let index = self.index;
        match collection {
            CollectionKind::ReqbodyProcessor if request.body.is_none() && tx.request_body_processor.is_some() => {
                self.index = 1;
                Ok((index == 0).then_some(SelectedValue {
                    collection,
                    name: None,
                    value: SelectedBytes::CtlProcessor,
                }))
            }
            CollectionKind::Tx | CollectionKind::MatchedVars => {
                let count = if collection == CollectionKind::Tx {
                    tx.entries.len()
                } else {
                    tx.matched_vars.len()
                };
                if index == count {
                    return Ok(None);
                }
                self.index += 1;
                Ok(if collection == CollectionKind::Tx {
                    selected(Some(SelectedName::Tx(index)), SelectedBytes::Tx(index))
                } else {
                    selected(
                        Some(SelectedName::MatchedVars(index)),
                        SelectedBytes::MatchedVars(index),
                    )
                })
            }
            CollectionKind::MatchedVar => {
                self.index = 1;
                Ok((index == 0 && tx.matched_var.is_some()).then_some(SelectedValue {
                    collection,
                    name: None,
                    value: SelectedBytes::MatchedVar,
                }))
            }
            CollectionKind::ResponseHeaders | CollectionKind::ResponseStatus => {
                if let Some(response) = response {
                    while let Some(entry) = response.entries.get(self.index) {
                        let index = self.index;
                        self.index += 1;
                        if entry.collection == collection {
                            return Ok(selected(
                                entry.name.as_ref().map(|_| SelectedName::Response(index)),
                                SelectedBytes::Response(index),
                            ));
                        }
                    }
                }
                Ok(None)
            }
            CollectionKind::ResponseBody => {
                self.index = 1;
                Ok(
                    (index == 0 && response.and_then(|response| response.window.as_ref()).is_some()).then_some(
                        SelectedValue {
                            collection,
                            name: None,
                            value: SelectedBytes::ResponseWindow,
                        },
                    ),
                )
            }
            _ => {
                if self.stage == 0 {
                    let overrides = matches!(
                        collection,
                        CollectionKind::ArgsCombinedSize | CollectionKind::ReqbodyProcessor
                    ) && request.body.is_some();
                    let records = request.collection_records(collection)?;
                    if !overrides {
                        if let Some(candidate) = records.get(self.index) {
                            self.index += 1;
                            return Ok(selected(
                                candidate.name.map(SelectedName::Request),
                                SelectedBytes::Request(candidate.value),
                            ));
                        }
                    }
                    self.stage = 1;
                    self.index = 0;
                    self.next_json = request
                        .body
                        .as_ref()
                        .and_then(RequestBodyPublication::structured_results)
                        .and_then(|results| results.receipt)
                        .map(|receipt| receipt.head);
                }
                if self.stage == 1 {
                    if let Some(body) = request.body.as_ref().and_then(RequestBodyPublication::xml) {
                        if self.include_json && collection == CollectionKind::Xml {
                            if let Some(id) = self.next_json {
                                let results = body.results.as_ref().ok_or(RuntimeError::XmlInspectionState)?;
                                if results.domain() != InspectionDomain::Xml {
                                    return Err(RuntimeError::XmlInspectionState);
                                }
                                let candidate = results
                                    .candidate(json_budget.ok_or(RuntimeError::XmlInspectionState)?, id)
                                    .map_err(map_store_error)
                                    .map_err(xml_inspection_runtime::xml_error)?;
                                self.next_json = candidate.next;
                                return match candidate.kind {
                                    CandidateKind::XmlText => Ok(selected(None, SelectedBytes::XmlText(id))),
                                    CandidateKind::XmlAttribute => {
                                        Ok(selected(Some(SelectedName::Xml(id)), SelectedBytes::XmlAttribute(id)))
                                    }
                                    _ => Err(RuntimeError::XmlInspectionState),
                                };
                            }
                        } else if body.metadata_value(collection).is_some() {
                            self.stage = 2;
                            return Ok(selected(None, SelectedBytes::XmlMetadata));
                        }
                    }
                    if let Some(body) = request.body.as_ref().and_then(RequestBodyPublication::json) {
                        if self.include_json && matches!(collection, CollectionKind::Args | CollectionKind::ArgsNames) {
                            if let Some(id) = self.next_json {
                                let results = body.results.as_ref().ok_or(RuntimeError::JsonInspectionState)?;
                                let budget = json_budget.ok_or(RuntimeError::JsonInspectionState)?;
                                self.next_json = results.candidate(budget, id).map_err(map_store_error)?.next;
                                return Ok(selected(Some(SelectedName::Json(id)), SelectedBytes::Json(id)));
                            }
                        } else if body.metadata_value(collection).is_some() {
                            self.stage = 2;
                            return Ok(selected(None, SelectedBytes::JsonMetadata));
                        }
                    }
                    self.stage = 2;
                }
                if let Some(body) = request.body.as_ref().and_then(RequestBodyPublication::materialized) {
                    while let Some(entry) = body.entries.get(self.index) {
                        let index = self.index;
                        self.index += 1;
                        if entry.collection == collection {
                            return Ok(selected(
                                entry.name.as_ref().map(|_| SelectedName::Body(index)),
                                SelectedBytes::Body(index),
                            ));
                        }
                    }
                }
                Ok(None)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn selector_matches(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    response: Option<&ResponseState>,
    scratch: &mut [Option<RegexBooleanScratch>],
    collection: CollectionKind,
    selector: super::CompiledTargetSelector,
    candidate: SelectedValue,
    json_budget: Option<&mut InspectionBudget>,
) -> Result<bool, RuntimeError> {
    match selector {
        super::CompiledTargetSelector::None => Ok(true),
        super::CompiledTargetSelector::Literal(expected) => {
            let expected = generation_bytes(&generation.target_bytes, expected)?;
            let name = candidate.name_bytes(request, tx, response, json_budget)?;
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
            if matches!(
                candidate.value,
                SelectedBytes::XmlText(_) | SelectedBytes::XmlAttribute(_)
            ) {
                return match path {
                    b"/*" => Ok(matches!(candidate.value, SelectedBytes::XmlText(_))),
                    b"//@*" => Ok(matches!(candidate.value, SelectedBytes::XmlAttribute(_))),
                    _ => Err(RuntimeError::XmlSelectorUnavailable),
                };
            }
            let SelectedBytes::Body(index) = candidate.value else {
                return Ok(false);
            };
            let entry = request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::materialized)
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
            let name = candidate.name_bytes(request, tx, response, json_budget)?;
            let Some(name) = name else {
                return Ok(false);
            };
            let regex = generation
                .boolean_regexes
                .get(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            let slot = scratch.get_mut(program).ok_or(RuntimeError::GenerationInvariant)?;
            let scratch = lazy_boolean_scratch(slot, regex);
            regex.is_match(name, scratch).map_err(map_regex_scratch_error)
        }
    }
}

fn charge_operator_work(work: &mut usize, added: usize, limits: RuntimeLimits) -> Result<(), RuntimeError> {
    *work = work.checked_add(added).ok_or(RuntimeError::Arithmetic)?;
    if *work > limits.max_operator_work_bytes {
        return Err(RuntimeError::OperatorWorkLimit {
            limit: limits.max_operator_work_bytes,
        });
    }
    Ok(())
}
/// Valid only during one borrowed operator stage; never keyed by buffer address.
#[derive(Default)]
struct StageScanCache {
    storage: ScanCacheStorage,
    #[cfg(test)]
    scans: usize,
}

/// Dense small namespaces avoid heap storage; larger namespaces keep the
/// existing per-domain admitted capacity instead of a global bitmap.
enum ScanCacheStorage {
    Single(Option<usize>),
    Inline { bits: u64, capacity: usize },
    Overflow(Vec<usize>),
}
impl Default for ScanCacheStorage {
    fn default() -> Self {
        Self::Inline { bits: 0, capacity: 0 }
    }
}
impl StageScanCache {
    fn with_storage(mut self, storage: ScanCacheStorage) -> Self {
        self.storage = storage;
        self
    }
    fn bit(group: usize) -> Option<u64> {
        u32::try_from(group).ok().and_then(|shift| 1_u64.checked_shl(shift))
    }
    fn contains(&self, group: usize) -> bool {
        match &self.storage {
            ScanCacheStorage::Single(last) => *last == Some(group),
            ScanCacheStorage::Inline { bits, .. } => Self::bit(group).is_some_and(|bit| bits & bit != 0),
            ScanCacheStorage::Overflow(scanned) => scanned.contains(&group),
        }
    }
    fn len(&self) -> usize {
        match &self.storage {
            ScanCacheStorage::Single(last) => usize::from(last.is_some()),
            ScanCacheStorage::Inline { bits, .. } => bits.count_ones() as usize,
            ScanCacheStorage::Overflow(scanned) => scanned.len(),
        }
    }
    fn heap_capacity(&self) -> usize {
        match &self.storage {
            ScanCacheStorage::Overflow(scanned) => scanned.capacity(),
            _ => 0,
        }
    }
    fn full(&self) -> bool {
        match &self.storage {
            ScanCacheStorage::Single(_) => false,
            ScanCacheStorage::Inline { capacity, .. } => self.len() == *capacity,
            ScanCacheStorage::Overflow(scanned) => scanned.len() == scanned.capacity(),
        }
    }
    fn insert(&mut self, group: usize) -> Result<(), RuntimeError> {
        if self.contains(group) {
            return Ok(());
        }
        if self.full() {
            return Err(RuntimeError::GenerationInvariant);
        }
        match &mut self.storage {
            ScanCacheStorage::Single(last) => *last = Some(group),
            ScanCacheStorage::Inline { bits, .. } => {
                *bits |= Self::bit(group).ok_or(RuntimeError::GenerationInvariant)?;
            }
            ScanCacheStorage::Overflow(scanned) => scanned.push(group),
        }
        Ok(())
    }
    fn clear(&mut self) {
        match &mut self.storage {
            ScanCacheStorage::Single(last) => *last = None,
            ScanCacheStorage::Inline { bits, .. } => *bits = 0,
            ScanCacheStorage::Overflow(scanned) => scanned.clear(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BorrowedTransformKey {
    range: ByteRange,
    plan_id: usize,
    max_output_bytes: usize,
    max_work_bytes: usize,
}

/// One successful identity walk over immutable request bytes. No payload is retained.
#[derive(Default)]
struct BorrowedTransformMemo {
    key: Option<BorrowedTransformKey>,
}

impl BorrowedTransformMemo {
    fn clear(&mut self) {
        self.key = None;
    }

    fn prepare(
        &mut self,
        value: SelectedBytes,
        plan_id: Option<usize>,
        request: &RequestState<'_>,
        limits: RuntimeLimits,
    ) -> Option<(&mut Self, BorrowedTransformKey)> {
        let key = (|| {
            let SelectedBytes::Request(range) = value else {
                return None;
            };
            let plan_id = plan_id?;
            let end = range.start.checked_add(range.len)?;
            request.bytes.get(range.start..end)?;
            Some(BorrowedTransformKey {
                range,
                plan_id,
                max_output_bytes: limits.max_transformation_output_bytes,
                max_work_bytes: limits.max_transformation_work_bytes,
            })
        })();
        if key.is_none() {
            self.clear();
        }
        key.map(|key| (self, key))
    }
}

/// One immutable request range/profile and one group's existing result bits.
/// This retains no input bytes and never allocates cache storage.
struct OrdinaryScanCache {
    key: Option<(ByteRange, Option<usize>)>,
    cache: StageScanCache,
}
impl OrdinaryScanCache {
    fn new() -> Self {
        Self {
            key: None,
            cache: StageScanCache::default().with_storage(ScanCacheStorage::Single(None)),
        }
    }
    fn clear(&mut self) {
        self.key = None;
        self.cache.clear();
    }
    fn prepare(&mut self, value: SelectedBytes, profile: Option<usize>, eligible: bool) -> Option<&mut StageScanCache> {
        if let SelectedBytes::Request(range) = value {
            if eligible {
                let key = Some((range, profile));
                if self.key != key {
                    self.cache.clear();
                    self.key = key;
                }
                return Some(&mut self.cache);
            }
        }
        self.clear();
        None
    }
}

fn physical_operator_work(
    context: &SingleInputContext<'_>,
    plan: super::CompiledOperatorPlan,
    len: usize,
) -> Result<usize, RuntimeError> {
    let mut work = len.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
    match plan.operand {
        super::CompiledOperatorOperand::Matcher {
            group, pattern_count, ..
        } => {
            let cache = context.scan_cache.as_deref().ok_or(RuntimeError::GenerationInvariant)?;
            work = cache
                .len()
                .checked_mul(2)
                .and_then(|v| v.checked_add(pattern_count))
                .and_then(|v| v.checked_add(1))
                .ok_or(RuntimeError::Arithmetic)?;
            if !cache.contains(group) {
                let count = context
                    .generation
                    .matcher_at(group)
                    .ok_or(RuntimeError::GenerationInvariant)?
                    .pattern_count();
                work = work
                    .checked_add(
                        len.checked_add(1)
                            .and_then(|v| v.checked_mul(count.checked_add(1)?))
                            .ok_or(RuntimeError::Arithmetic)?,
                    )
                    .ok_or(RuntimeError::Arithmetic)?;
            }
        }
        super::CompiledOperatorOperand::String { template, .. }
        | super::CompiledOperatorOperand::Integer {
            operand: super::CompiledIntegerOperand::Template(template),
            ..
        } => {
            let template = context
                .generation
                .templates
                .get(template)
                .ok_or(RuntimeError::GenerationInvariant)?;
            work = work
                .checked_add(template.static_bytes)
                .and_then(|v| v.checked_add(template.segment_count.checked_mul(2)?))
                .ok_or(RuntimeError::Arithmetic)?;
            if let super::CompiledOperatorOperand::String { relation, .. } = plan.operand {
                let (haystack, needle) = match relation {
                    super::StringRelation::Contains => (len, template.static_bytes),
                    super::StringRelation::Within => (template.static_bytes, len),
                    _ => (0, 0),
                };
                if needle != 0 && needle <= haystack {
                    let comparisons = (haystack - needle + 1)
                        .checked_mul(needle)
                        .ok_or(RuntimeError::Arithmetic)?;
                    work = work.checked_add(comparisons).ok_or(RuntimeError::Arithmetic)?;
                }
            }
        }
        super::CompiledOperatorOperand::IpNetworks { count, .. } => {
            work = work.checked_add(count).ok_or(RuntimeError::Arithmetic)?;
        }
        _ => {}
    }
    Ok(work)
}

#[expect(
    clippy::too_many_arguments,
    reason = "shared matcher operands and optional stage cache stay explicit"
)]
fn evaluate_matcher_operand_cached(
    generation: &CompiledGeneration,
    group_scratch: &mut [Option<MatchScratch>],
    group: usize,
    pattern_start: usize,
    pattern_count: usize,
    input: &[u8],
    evaluation_mode: EvaluationMode,
    cache: Option<&mut StageScanCache>,
) -> Result<bool, RuntimeError> {
    let matcher = generation.matcher_at(group).ok_or(RuntimeError::GenerationInvariant)?;
    let scratch = group_scratch.get_mut(group).ok_or(RuntimeError::GenerationInvariant)?;
    let end = pattern_start
        .checked_add(pattern_count)
        .ok_or(RuntimeError::Arithmetic)?;
    if cache.as_ref().is_some_and(|cache| cache.contains(group)) {
        let scratch = scratch.as_mut().ok_or(RuntimeError::GenerationInvariant)?;
        let result = scratch.contains_any_in_range(matcher.pattern_count(), pattern_start..end);
        #[cfg(test)]
        measurements::record(Event::RangeQueries, 1);
        #[cfg(test)]
        measurements::record(Event::RangeQueryFailures, usize::from(result.is_none()));
        return result.ok_or(RuntimeError::GenerationInvariant);
    }
    // Capacity was admitted by the collector before this callback. No hidden
    // growth is permitted in the canonical matcher seam.
    if cache.as_ref().is_some_and(|cache| cache.full()) {
        return Err(RuntimeError::GenerationInvariant);
    }
    let scratch = lazy_group_scratch(scratch, matcher);
    let matches = match evaluation_mode {
        EvaluationMode::Accelerated => matcher.scan(input, scratch),
        EvaluationMode::Reference => matcher.scan_reference(input, scratch),
    }
    .map_err(map_matcher_error)?;
    #[cfg(test)]
    measurements::record(Event::FullScans, 1);
    let result = matches.contains_any_in_range(pattern_start..end);
    #[cfg(test)]
    measurements::record(Event::RangeQueries, 1);
    #[cfg(test)]
    measurements::record(Event::RangeQueryFailures, usize::from(result.is_none()));
    let result = result.ok_or(RuntimeError::GenerationInvariant)?;
    if let Some(cache) = cache {
        cache.insert(group)?;
        #[cfg(test)]
        {
            cache.scans += 1;
        }
    }
    Ok(result)
}

enum OperatorCapture {
    Fingerprint(crate::injection::Fingerprint),
    Phrase(CapturePhraseMatch),
}

#[allow(clippy::too_many_arguments)]
fn evaluate_operator(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    group_scratch: &mut [Option<MatchScratch>],
    boolean_scratch: &mut [Option<RegexBooleanScratch>],
    capture_scratch: &mut [Option<RegexCaptureScratch>],
    plan: super::CompiledOperatorPlan,
    input: &[u8],
    member_runtime_limit: usize,
    evaluation_mode: EvaluationMode,
    scan_cache: Option<&mut StageScanCache>,
    json_budget: Option<&mut InspectionBudget>,
) -> Result<(bool, Option<OperatorCapture>), RuntimeError> {
    let mut capture = None;
    let positive = match plan.operand {
        super::CompiledOperatorOperand::String { relation, template } => {
            let operand = template_operand(generation, request, tx, template, member_runtime_limit, json_budget)?;
            Ok(match relation {
                super::StringRelation::BeginsWith => input.starts_with(&operand),
                super::StringRelation::Contains => contains(input, &operand),
                super::StringRelation::EndsWith => input.ends_with(&operand),
                super::StringRelation::Equals => input == operand.as_ref(),
                super::StringRelation::Within => contains(&operand, input),
            })
        }
        super::CompiledOperatorOperand::Integer { relation, operand } => {
            let expected = match operand {
                super::CompiledIntegerOperand::Static(value) => value,
                super::CompiledIntegerOperand::Template(template) => parse_i64(&template_operand(
                    generation,
                    request,
                    tx,
                    template,
                    member_runtime_limit,
                    json_budget,
                )?)?,
            };
            let actual = parse_i64(input)?;
            Ok(integer_relation(relation, actual, expected))
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
            capture = crate::injection::detect_sqli(input).map(OperatorCapture::Fingerprint);
            Ok(capture.is_some())
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
                regex
                    .is_match(input, lazy_boolean_scratch(scratch, regex))
                    .map_err(map_regex_scratch_error)
            }
            super::RegexPlanRef::Capture(program) => {
                #[cfg(test)]
                measurements::record(Event::CaptureBooleanSearches, 1);
                let regex = generation
                    .capture_regexes
                    .get(program)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                let scratch = capture_scratch
                    .get_mut(program)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                regex
                    .is_match(input, lazy_capture_scratch(scratch, regex))
                    .map_err(map_regex_scratch_error)
            }
        },
        super::CompiledOperatorOperand::Matcher {
            group,
            pattern_start,
            pattern_count,
        } => evaluate_matcher_operand_cached(
            generation,
            group_scratch,
            group,
            pattern_start,
            pattern_count,
            input,
            evaluation_mode,
            scan_cache,
        ),
        super::CompiledOperatorOperand::CapturePhrase(program) => {
            let matcher = generation
                .capture_phrases
                .get(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            capture = match evaluation_mode {
                EvaluationMode::Accelerated => matcher.find(input),
                EvaluationMode::Reference => matcher.find_reference(input),
            }
            .map(OperatorCapture::Phrase);
            Ok(capture.is_some())
        }
    }?;
    Ok((positive, capture))
}

fn static_template_operand(
    generation: &CompiledGeneration,
    template_id: usize,
    member_limit: usize,
) -> Result<Option<&[u8]>, RuntimeError> {
    let template = generation
        .templates
        .get(template_id)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let end = template
        .segment_start
        .checked_add(template.segment_count)
        .ok_or(RuntimeError::Arithmetic)?;
    let segments = generation
        .segments
        .get(template.segment_start..end)
        .ok_or(RuntimeError::GenerationInvariant)?;
    if segments
        .iter()
        .any(|segment| matches!(segment.kind, super::CompiledSegmentKind::Macro(_)))
    {
        return Ok(None);
    }
    let limit = template.runtime_byte_limit.min(member_limit);
    let mut start = None;
    let mut len = 0usize;
    for segment in segments {
        let super::CompiledSegmentKind::Literal(range) = segment.kind else {
            return Err(RuntimeError::GenerationInvariant);
        };
        let bytes = generation_bytes(&generation.bytes, range)?;
        let base = *start.get_or_insert(range.start);
        if base.checked_add(len).ok_or(RuntimeError::Arithmetic)? != range.start {
            return Err(RuntimeError::GenerationInvariant);
        }
        len = checked_runtime_append(len, bytes.len(), limit)?;
    }
    match start {
        Some(start) => generation_bytes(&generation.bytes, super::ByteRange { start, len }).map(Some),
        None => Ok(Some(&[])),
    }
}

fn template_operand<'g>(
    generation: &'g CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    template: usize,
    limit: usize,
    json_budget: Option<&mut InspectionBudget>,
) -> Result<Cow<'g, [u8]>, RuntimeError> {
    match static_template_operand(generation, template, limit)? {
        Some(value) => Ok(Cow::Borrowed(value)),
        None => expand_template::<true>(generation, request, tx, template, limit, json_budget).map(Cow::Owned),
    }
}

// Validation still resolves and meters every macro, but need not retain its bytes.
fn expand_template<const RETAIN_OUTPUT: bool>(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    template_id: usize,
    member_limit: usize,
    mut json_budget: Option<&mut InspectionBudget>,
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
    if RETAIN_OUTPUT {
        output
            .try_reserve(template.static_bytes.min(limit))
            .map_err(|_| RuntimeError::Allocation)?;
    }
    let mut expanded_len = 0_usize;
    for segment in segments {
        let value = match segment.kind {
            super::CompiledSegmentKind::Literal(range) => generation_bytes(&generation.bytes, range)?,
            super::CompiledSegmentKind::Macro(macro_id) => {
                let reference = *generation
                    .macros
                    .get(macro_id)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                let key = generation_bytes(&generation.bytes, reference.key)?;
                let value = macro_value(
                    generation,
                    macro_id,
                    request,
                    tx,
                    reference.source,
                    key,
                    json_budget.as_deref_mut(),
                )?;
                value.unwrap_or_default()
            }
        };
        expanded_len = checked_runtime_append(expanded_len, value.len(), limit)?;
        if RETAIN_OUTPUT {
            output.extend_from_slice(value);
        }
    }
    Ok(output)
}

fn macro_value<'request>(
    generation: &CompiledGeneration,
    macro_id: usize,
    request: &'request RequestState,
    tx: &'request TxState,
    source: super::MacroSource,
    key: &[u8],
    mut json_budget: Option<&mut InspectionBudget>,
) -> Result<Option<&'request [u8]>, RuntimeError> {
    Ok(match source {
        super::MacroSource::Args => {
            if let Some(value) = request.first_named(CollectionKind::Args, key, false) {
                Some(value)
            } else if let Some(results) = request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::json)
                .and_then(|body| body.results.as_ref())
            {
                let demand = generation
                    .json_inspection
                    .macro_demands
                    .get(macro_id)
                    .copied()
                    .flatten()
                    .ok_or(RuntimeError::JsonInspectionState)?;
                results
                    .named_value(json_budget.ok_or(RuntimeError::JsonInspectionState)?, demand)
                    .map_err(map_store_error)?
            } else {
                None
            }
        }
        super::MacroSource::RequestHeaders => request.first_named(CollectionKind::RequestHeaders, key, true),
        super::MacroSource::RemoteAddr => request.first_value(CollectionKind::RemoteAddr),
        super::MacroSource::RequestLine => request.first_value(CollectionKind::RequestLine),
        super::MacroSource::Tx => tx
            .get(key, request, json_budget.as_deref_mut())?
            .map(|value| value.bytes(request, json_budget))
            .transpose()?,
        super::MacroSource::MatchedVar => tx
            .matched_var
            .as_ref()
            .map(|value| value.bytes(request, json_budget))
            .transpose()?,
        super::MacroSource::MatchedVarName => tx
            .matched_var_name
            .as_ref()
            .map(|value| value.bytes(request, json_budget))
            .transpose()?,
    })
}

fn generation_bytes(bytes: &[u8], range: super::ByteRange) -> Result<&[u8], RuntimeError> {
    let end = range.start.checked_add(range.len).ok_or(RuntimeError::Arithmetic)?;
    bytes.get(range.start..end).ok_or(RuntimeError::GenerationInvariant)
}

fn checked_runtime_append(current: usize, added: usize, limit: usize) -> Result<usize, RuntimeError> {
    let actual = current.checked_add(added).ok_or(RuntimeError::Arithmetic)?;
    if actual > limit {
        return Err(RuntimeError::ValueByteLimit { actual, limit });
    }
    Ok(actual)
}

pub(super) fn parse_i64(value: &[u8]) -> Result<i64, RuntimeError> {
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
    memchr::memmem::find(haystack, needle).is_some()
}

fn ip_matches(address: std::net::IpAddr, network: super::CompiledIpNetwork) -> bool {
    match (address, network.address) {
        (std::net::IpAddr::V4(address), std::net::IpAddr::V4(base)) => {
            let prefix = u32::from(network.prefix.min(32));
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            u32::from(address) & mask == u32::from(base)
        }
        (std::net::IpAddr::V6(address), std::net::IpAddr::V6(base)) => {
            let prefix = u32::from(network.prefix.min(128));
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) };
            u128::from(address) & mask == u128::from(base)
        }
        _ => false,
    }
}

fn invalid_url_encoding(input: &[u8]) -> bool {
    let mut index = 0_usize;
    while index < input.len() {
        let Some(offset) = memchr::memchr(b'%', &input[index..]) else {
            return false;
        };
        index += offset;
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
            if memchr::memchr3(b'\r', b'\n', 0, header.value).is_some() {
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
        let (status_digits, status_start) = decimal_bytes(usize::from(response.status));
        entries.push(ResponseEntry {
            collection: CollectionKind::ResponseStatus,
            name: None,
            value: owned_copy(&status_digits[status_start..])?,
        });
        for header in response.headers {
            entries.push(ResponseEntry {
                collection: CollectionKind::ResponseHeaders,
                name: Some(owned_copy(header.name)?),
                value: owned_copy(header.value)?,
            });
        }
        let retained_bytes = header_bytes
            .checked_add(status_digits.len() - status_start)
            .ok_or(RuntimeError::Arithmetic)?
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

impl RequestState<'_> {
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
            if memchr::memchr3(b'\r', b'\n', 0, header.value).is_some() {
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
            .nominal
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
            .try_reserve_exact(derived_bytes.arena)
            .map_err(|_| RuntimeError::Allocation)?;
        let (digits, digit_start) = decimal_bytes(derived_bytes.query_combined_size);
        state.push_scalar(CollectionKind::ArgsCombinedSize, &digits[digit_start..])?;
        state.push_query_collection(query)?;
        state.push_collection_alias(CollectionKind::Args, CollectionKind::ArgsGet, false)?;
        state.push_collection_alias(CollectionKind::Args, CollectionKind::ArgsNames, true)?;
        state.push_collection_alias(CollectionKind::Args, CollectionKind::ArgsGetNames, true)?;
        state.push_scalar(CollectionKind::QueryString, query)?;
        state.push_scalar(CollectionKind::RemoteAddr, request.remote_addr)?;
        state.push_scalar(
            CollectionKind::ReqbodyProcessor,
            derived_bytes.body_processor.as_bytes(),
        )?;
        state.push_cookie_collection(request.headers)?;
        state.push_collection_alias(
            CollectionKind::RequestCookies,
            CollectionKind::RequestCookiesNames,
            true,
        )?;
        state.push_request_path_collections(request.uri)?;
        state.push_header_collection(request.headers)?;
        state.push_collection_alias(
            CollectionKind::RequestHeaders,
            CollectionKind::RequestHeadersNames,
            true,
        )?;
        state.push_scalar(CollectionKind::RequestLine, request.request_line)?;
        state.push_scalar(CollectionKind::RequestMethod, request.method)?;
        state.push_scalar(CollectionKind::RequestProtocol, request.protocol)?;
        state.push_scalar(CollectionKind::RequestUri, request.uri)?;
        state.push_scalar(CollectionKind::RequestUriRaw, request.uri_raw)?;
        state.push_scalar(CollectionKind::UniqueId, request.unique_id)?;

        // Logical alias payload remains charged even when immutable ranges share bytes.
        state.retained_bytes = derived_bytes
            .nominal
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

    fn push_collection_alias(
        &mut self,
        source: CollectionKind,
        alias: CollectionKind,
        names: bool,
    ) -> Result<(), RuntimeError> {
        // Replay records in collection order, preserving fallible metadata growth order.
        let source = collection_index(source);
        for index in 0..self.collections[source].len() {
            let mut record = self.collections[source][index];
            if names {
                record.value = record.name.ok_or(RuntimeError::GenerationInvariant)?;
            }
            self.push_record(alias, record)?;
        }
        Ok(())
    }

    fn push_header_collection(&mut self, headers: &[RequestHeader<'_>]) -> Result<(), RuntimeError> {
        for header in headers {
            let name = self.copy_bytes(header.name)?;
            let value = self.copy_bytes(header.value)?;
            self.push_record(
                CollectionKind::RequestHeaders,
                CollectionValue {
                    name: Some(name),
                    value,
                },
            )?;
        }
        Ok(())
    }

    fn push_query_collection(&mut self, query: &[u8]) -> Result<(), RuntimeError> {
        for segment in nonempty_segments(query, b'&') {
            let (name, value) = split_once(segment, b'=');
            let name = self.copy_percent_decoded(name)?;
            let value = self.copy_percent_decoded(value)?;
            self.push_record(
                CollectionKind::Args,
                CollectionValue {
                    name: Some(name),
                    value,
                },
            )?;
        }
        Ok(())
    }

    fn push_cookie_collection(&mut self, headers: &[RequestHeader<'_>]) -> Result<(), RuntimeError> {
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
                let value = self.copy_bytes(trim_ascii(value))?;
                self.push_record(
                    CollectionKind::RequestCookies,
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
        if memchr::memchr2(b'+', b'%', value).is_none() {
            return self.copy_bytes(value);
        }
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
        if memchr::memchr(b'%', value).is_none() {
            return self.copy_bytes(value);
        }
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
            self.body.as_ref()?.materialized()?.entries.iter().find_map(|entry| {
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
        if let Some(value) = self
            .body
            .as_ref()
            .and_then(RequestBodyPublication::json)
            .and_then(|body| body.metadata_value(collection))
        {
            return Some(value);
        }
        if collection == CollectionKind::ArgsCombinedSize {
            if let Some(body) = self.body.as_ref().and_then(RequestBodyPublication::materialized) {
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
                    .materialized()?
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
    memchr::memchr(b'?', uri).map_or(&[], |index| &uri[index + 1..])
}

fn request_filename(uri: &[u8]) -> &[u8] {
    let end = memchr::memchr(b'?', uri).unwrap_or(uri.len());
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
    memchr::memchr(delimiter, value).map_or((value, &[]), |index| (&value[..index], &value[index + 1..]))
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
    if memchr::memchr(b'%', value).is_none() {
        return value.len();
    }
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

pub(super) fn decimal_bytes(mut value: usize) -> ([u8; 40], usize) {
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

struct RequestHeadProjection {
    arena: usize,
    nominal: usize,
    query_combined_size: usize,
    body_processor: super::super::BodyProcessorSelection,
}

fn projected_derived_bytes(request: RequestHead<'_>, query: &[u8]) -> Result<RequestHeadProjection, RuntimeError> {
    let query_combined_size = query_argument_combined_size(query)?;
    let (_, combined_start) = decimal_bytes(query_combined_size);
    let mut total = 40 - combined_start;
    let body_processor = request_body_processor(request.headers);
    for value in [
        query,
        request.remote_addr,
        body_processor.as_bytes(),
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
    let mut arena = total;
    for segment in nonempty_segments(query, b'&') {
        let (name, value) = split_once(segment, b'=');
        let name = percent_decoded_len(name);
        let value = percent_decoded_len(value);
        total = checked_add(total, checked_mul(name, 4)?)?;
        arena = checked_add(arena, name)?;
        total = checked_add(total, checked_mul(value, 2)?)?;
        arena = checked_add(arena, value)?;
    }
    for header in request.headers {
        total = checked_add(total, checked_mul(header.name.len(), 2)?)?;
        arena = checked_add(arena, header.name.len())?;
        total = checked_add(total, header.value.len())?;
        arena = checked_add(arena, header.value.len())?;
        if header.name.eq_ignore_ascii_case(b"cookie") {
            for segment in nonempty_segments(header.value, b';') {
                let (name, value) = split_once(trim_ascii(segment), b'=');
                let name = trim_ascii(name);
                if name.is_empty() {
                    continue;
                }
                total = checked_add(total, checked_mul(name.len(), 2)?)?;
                arena = checked_add(arena, name.len())?;
                total = checked_add(total, trim_ascii(value).len())?;
                arena = checked_add(arena, trim_ascii(value).len())?;
            }
        }
    }
    Ok(RequestHeadProjection {
        arena,
        nominal: total,
        query_combined_size,
        body_processor,
    })
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
#[path = "../../tests/unit/seclang/transaction_test.rs"]
mod tests;

#[cfg(test)]
use tests::matcher_scratch_bytes;

#[path = "json_inspection.rs"]
mod json_inspection;
#[path = "json_inspection_collector.rs"]
mod json_inspection_collector;
#[path = "json_inspection_runtime.rs"]
mod json_inspection_runtime;

#[path = "xml_inspection_runtime.rs"]
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
