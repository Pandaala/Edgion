use crate::ai::terminal::{AiClientStreamDialect, FinalResponseEligibility, ResponseTerminalState};
use crate::ai::{AiAttemptPlan, AiRequestContext, GenAiSummary};
use crate::backends::{SelectedHttpBackend, ServiceBackendKind};
use crate::lb::backend::LbBackend;
use crate::observe::logs::LogBuffer;
use crate::observe::RequestDiagnostics;
use crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseProcessor;
use crate::plugins::runtime::traits::UpstreamResponseTrailer;
use crate::plugins::runtime::PluginRuntime;
use crate::plugins::{EdgionPluginsLog, StageLogs};
use crate::request_body::RequestBodyState;
use crate::routes::grpc::{GrpcRouteRuleUnit, SelectedGrpcBackend};
use crate::routes::HttpRouteRuleUnit;
use crate::runtime::GatewayInfo;
use bytes::Bytes;
use edgion_resources::resources::http_route::SessionPersistence;
use edgion_resources::resources::lb_policy::ParsedLBPolicy;
use edgion_resources::uri_utils;
use edgion_resources::{EdgionErrorCode, EdgionInfoCode, HTTPRouteMatch};
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_http::ResponseHeader;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const PLUGIN_ENTRY_NAME_CAPACITY: usize = crate::plugins::runtime::log::NAME_CAPACITY;
const PLUGIN_ENTRY_ALIAS_CAPACITY: usize = 32;
const MAX_WAF_ACCESS_LOG_FINDINGS: usize = 16;

pub(crate) type AttachedExtProcMap = HashMap<String, Arc<crate::plugins::http::ext_proc::AttachedExtProc>>;

/// Lazily decoded request parameters shared by routing, plugins, templates,
/// and access-log projections.
///
/// Each view keeps its own init-once gate because query matching is a routing
/// hot path while path captures are plugin-only. Empty results use `None` as a
/// sentinel and allocate nothing; non-empty maps box only their 48-byte map
/// header in addition to the buckets they inherently own.
#[derive(Default)]
struct ParsedRequestCaches {
    path: std::sync::OnceLock<Option<Box<HashMap<String, String>>>>,
    query: std::sync::OnceLock<Option<Box<HashMap<String, String>>>>,
}

static EMPTY_PARSED_REQUEST_PARAMS: std::sync::LazyLock<HashMap<String, String>> =
    std::sync::LazyLock::new(HashMap::new);

pub(crate) fn parse_path_params(pattern: &str, actual_path: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for (pattern_segment, actual_segment) in pattern
        .split('/')
        .filter(|segment| !segment.is_empty())
        .zip(actual_path.split('/').filter(|segment| !segment.is_empty()))
    {
        if pattern_segment.starts_with(':') && !pattern_segment.starts_with("::") {
            let name = &pattern_segment[1..];
            if !name.is_empty() {
                params.insert(name.to_string(), actual_segment.to_string());
            }
        }
    }
    params
}

/// Request-local exactly-once gate for the canonical response-trailer phase.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum UpstreamTrailerLifecycle {
    #[default]
    Open,
    AwaitingTrailers,
    Dispatched,
}

impl UpstreamTrailerLifecycle {
    #[inline]
    pub(crate) fn mark_before_trailers(&mut self) -> bool {
        if !matches!(self, Self::Open) {
            return false;
        }
        *self = Self::AwaitingTrailers;
        true
    }

    #[inline]
    pub(crate) fn claim_without_trailers(&mut self) -> bool {
        if !matches!(self, Self::Open) {
            return false;
        }
        *self = Self::Dispatched;
        true
    }

    #[inline]
    pub(crate) fn claim_real_trailers(&mut self) -> bool {
        if matches!(self, Self::Dispatched) {
            return false;
        }
        *self = Self::Dispatched;
        true
    }

    #[inline]
    pub(crate) fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    #[inline]
    pub(crate) fn claim_cleanup(&mut self) -> bool {
        if matches!(self, Self::Dispatched) {
            return false;
        }
        *self = Self::Dispatched;
        true
    }
}

/// Allocation-free identity of the request-stage plugin entry currently being
/// invoked. The runtime owns this slot; body observers may only snapshot it at
/// registration time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CurrentPluginEntryIdentity {
    name: [u8; PLUGIN_ENTRY_NAME_CAPACITY],
    name_len: u8,
    alias: [u8; PLUGIN_ENTRY_ALIAS_CAPACITY],
    alias_len: u8,
    has_alias: bool,
}

/// Request-owned native WAF state.
///
/// The box keeps the large, optional transaction and its reusable matcher /
/// regex scratch out of the base HTTP context. Future streaming body parser
/// and window state belongs here as well; it must not use thread-local storage
/// because a request may move between executor workers.
pub(crate) struct WafRequestContext {
    owner: Arc<()>,
    access_log: WafAccessLog,
    claimed_phases: u8,
    transaction: Option<edgion_waf::seclang::Transaction<'static>>,
    request_body_streaming: bool,
    request_body_streamed_bytes: u64,
    request_body_stream_limit: u64,
    request_body_execution_micros: u64,
    charged_runtime_bytes: i64,
}

impl WafRequestContext {
    fn selected(selection: WafSelection) -> Self {
        Self {
            owner: selection.owner,
            access_log: WafAccessLog::new(selection.source, selection.priority, selection.mode),
            claimed_phases: 0,
            transaction: None,
            request_body_streaming: false,
            request_body_streamed_bytes: 0,
            request_body_stream_limit: 0,
            request_body_execution_micros: 0,
            charged_runtime_bytes: 0,
        }
    }

    fn claim_phase(&mut self, owner: &Arc<()>, phase: WafPhase) -> WafPhaseClaim {
        if !Arc::ptr_eq(&self.owner, owner) || self.claimed_phases & phase.bit() != 0 {
            return WafPhaseClaim::Skip;
        }
        self.claimed_phases |= phase.bit();
        WafPhaseClaim::Execute
    }

    pub(crate) fn install(&mut self, owner: Arc<()>, transaction: edgion_waf::seclang::Transaction<'static>) -> bool {
        if !Arc::ptr_eq(&self.owner, &owner) || self.transaction.is_some() {
            return false;
        }
        self.transaction = Some(transaction);
        self.refresh_runtime_charge();
        true
    }

    pub(crate) fn take(&mut self, owner: &Arc<()>) -> Option<edgion_waf::seclang::Transaction<'static>> {
        Arc::ptr_eq(&self.owner, owner)
            .then(|| self.transaction.take())
            .flatten()
    }

    pub(crate) fn enable_request_body_streaming(&mut self, owner: &Arc<()>, body_limit: u64) -> bool {
        if !Arc::ptr_eq(&self.owner, owner) || self.transaction.is_none() {
            return false;
        }
        self.request_body_streaming = true;
        self.request_body_stream_limit = body_limit;
        true
    }

    pub(crate) const fn request_body_streaming(&self) -> bool {
        self.request_body_streaming
    }

    pub(crate) fn execute_request_body_stream_chunk(
        &mut self,
        chunk: &[u8],
        end_of_stream: bool,
    ) -> Result<Option<edgion_waf::seclang::PhaseOutcome>, edgion_waf::seclang::RuntimeError> {
        let chunk_bytes = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        let next_bytes = self
            .request_body_streamed_bytes
            .checked_add(chunk_bytes)
            .ok_or(edgion_waf::seclang::RuntimeError::Arithmetic)?;
        if next_bytes > self.request_body_stream_limit {
            return Err(edgion_waf::seclang::RuntimeError::Body {
                kind: edgion_waf::seclang::BodyError::ByteLimit {
                    actual: usize::try_from(next_bytes).unwrap_or(usize::MAX),
                    limit: usize::try_from(self.request_body_stream_limit).unwrap_or(usize::MAX),
                },
            });
        }
        let transaction = self
            .transaction
            .as_mut()
            .ok_or(edgion_waf::seclang::RuntimeError::RequestBodyNotPublished)?;
        let outcome = transaction.execute_request_body_stream_chunk(chunk, end_of_stream)?;
        self.request_body_streamed_bytes = next_bytes;
        if end_of_stream || outcome.is_some() {
            self.request_body_streaming = false;
        }
        self.refresh_runtime_charge();
        Ok(outcome)
    }

    pub(crate) fn abandon_request_body_stream(&mut self) {
        self.request_body_streaming = false;
    }

    pub(crate) fn add_request_body_execution_micros(&mut self, elapsed_micros: u64) {
        self.request_body_execution_micros = self.request_body_execution_micros.saturating_add(elapsed_micros);
    }

    pub(crate) fn take_request_body_execution_micros(&mut self) -> u64 {
        std::mem::take(&mut self.request_body_execution_micros)
    }

    fn refresh_runtime_charge(&mut self) {
        let next = self.transaction.as_ref().map_or(0, |transaction| {
            i64::try_from(transaction.retained_bytes()).unwrap_or(i64::MAX)
        });
        let delta = next.saturating_sub(self.charged_runtime_bytes);
        if delta != 0 {
            crate::observe::metrics::adjust_waf_runtime_bytes_active(delta);
            self.charged_runtime_bytes = next;
        }
    }

    fn record_phase_outcome(
        &mut self,
        owner: &Arc<()>,
        phase: WafLogPhase,
        execution_us: u64,
        outcome: &edgion_waf::seclang::PhaseOutcome,
    ) {
        if Arc::ptr_eq(&self.owner, owner) {
            self.access_log.record_phase(phase, execution_us, outcome);
        }
    }

    fn record_terminal_error(&mut self, owner: &Arc<()>, phase: WafLogPhase, code: WafErrorCode, status: u16) {
        if Arc::ptr_eq(&self.owner, owner) {
            self.access_log.record_error(phase, code, status);
        }
    }
}

impl Drop for WafRequestContext {
    fn drop(&mut self) {
        if self.charged_runtime_bytes != 0 {
            crate::observe::metrics::adjust_waf_runtime_bytes_active(-self.charged_runtime_bytes);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WafPhase {
    Request,
    ResponseHeaders,
    ResponseComplete,
}

impl WafPhase {
    const fn bit(self) -> u8 {
        match self {
            Self::Request => 1 << 0,
            Self::ResponseHeaders => 1 << 1,
            Self::ResponseComplete => 1 << 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WafPhaseClaim {
    Execute,
    Skip,
    Pending,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WafLogSource {
    Global,
    Gateway,
    Route,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WafLogMode {
    On,
    DetectionOnly,
}

#[derive(Debug)]
pub(crate) struct WafSelection {
    owner: Arc<()>,
    source: WafLogSource,
    priority: u16,
    mode: WafLogMode,
}

impl WafSelection {
    pub(crate) fn new(owner: Arc<()>, source: WafLogSource, priority: u16, mode: WafLogMode) -> Self {
        Self {
            owner,
            source,
            priority,
            mode,
        }
    }

    #[cfg(test)]
    pub(crate) fn owner(&self) -> &Arc<()> {
        &self.owner
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WafLogPhase {
    RequestHeaders,
    RequestBody,
    ResponseHeaders,
    ResponseBody,
    Logging,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WafLogOutcome {
    NotRun,
    Running,
    Incomplete,
    Allowed,
    Detected,
    Blocked,
    Error,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WafFindingSeverity {
    Emergency,
    Alert,
    Critical,
    Error,
    Warning,
    Notice,
    Info,
    Debug,
}

impl From<edgion_waf::seclang::Severity> for WafFindingSeverity {
    fn from(value: edgion_waf::seclang::Severity) -> Self {
        use edgion_waf::seclang::Severity;
        match value {
            Severity::Emergency => Self::Emergency,
            Severity::Alert => Self::Alert,
            Severity::Critical => Self::Critical,
            Severity::Error => Self::Error,
            Severity::Warning => Self::Warning,
            Severity::Notice => Self::Notice,
            Severity::Info => Self::Info,
            Severity::Debug => Self::Debug,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct WafFindingLog {
    phase: WafLogPhase,
    rule_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<WafFindingSeverity>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct WafInterventionLog {
    phase: WafLogPhase,
    rule_id: u64,
    status: u16,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WafErrorCode {
    InputLimit,
    BodyLimit,
    BodyProcessing,
    RuntimeLimit,
    ResponseLimit,
    PhaseState,
    Unsupported,
    RuntimeInvariant,
    Matcher,
    Transformation,
    Allocation,
    Arithmetic,
    WinnerNotSelected,
    TransactionOwnerConflict,
    TransactionMissing,
    RequestBodyReadFailed,
    BodyStreamInstallFailed,
    BodyStreamAbandoned,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct WafTerminalErrorLog {
    phase: WafLogPhase,
    code: WafErrorCode,
    status: u16,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct WafPhaseLog {
    execution_us: u64,
    evaluated_rules: u64,
    findings: u64,
    omitted_findings: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct WafPhaseLogs {
    #[serde(skip_serializing_if = "Option::is_none")]
    request_headers: Option<WafPhaseLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_body: Option<WafPhaseLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_headers: Option<WafPhaseLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_body: Option<WafPhaseLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logging: Option<WafPhaseLog>,
}

#[derive(Debug, Serialize)]
pub struct WafAccessLog {
    source: WafLogSource,
    priority: u16,
    mode: WafLogMode,
    outcome: WafLogOutcome,
    phases: WafPhaseLogs,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    findings: Vec<WafFindingLog>,
    #[serde(skip_serializing_if = "is_zero_u64")]
    findings_omitted: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    intervention: Option<WafInterventionLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_error: Option<WafTerminalErrorLog>,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

impl WafAccessLog {
    fn new(source: WafLogSource, priority: u16, mode: WafLogMode) -> Self {
        Self {
            source,
            priority,
            mode,
            outcome: WafLogOutcome::NotRun,
            phases: WafPhaseLogs::default(),
            findings: Vec::new(),
            findings_omitted: 0,
            intervention: None,
            terminal_error: None,
        }
    }

    fn phase_slot(&mut self, phase: WafLogPhase) -> &mut Option<WafPhaseLog> {
        match phase {
            WafLogPhase::RequestHeaders => &mut self.phases.request_headers,
            WafLogPhase::RequestBody => &mut self.phases.request_body,
            WafLogPhase::ResponseHeaders => &mut self.phases.response_headers,
            WafLogPhase::ResponseBody => &mut self.phases.response_body,
            WafLogPhase::Logging => &mut self.phases.logging,
        }
    }

    fn record_phase(
        &mut self,
        phase: WafLogPhase,
        execution_us: u64,
        phase_outcome: &edgion_waf::seclang::PhaseOutcome,
    ) {
        if self.phase_slot(phase).is_some() {
            return;
        }
        let recordable = saturating_u64(
            phase_outcome
                .findings()
                .iter()
                .filter(|finding| finding.log() || finding.audit_log())
                .count(),
        );
        let runtime_omitted = saturating_u64(phase_outcome.omitted_loggable_findings());
        *self.phase_slot(phase) = Some(WafPhaseLog {
            execution_us,
            evaluated_rules: saturating_u64(phase_outcome.evaluated_units()),
            findings: recordable,
            omitted_findings: runtime_omitted,
        });

        for finding in phase_outcome.findings() {
            if !finding.log() && !finding.audit_log() {
                continue;
            }
            if self.findings.len() < MAX_WAF_ACCESS_LOG_FINDINGS {
                self.findings.push(WafFindingLog {
                    phase,
                    rule_id: finding.rule_id(),
                    severity: finding.severity().map(Into::into),
                });
            } else {
                self.findings_omitted = self.findings_omitted.saturating_add(1);
            }
        }
        self.findings_omitted = self.findings_omitted.saturating_add(runtime_omitted);

        if let Some(intervention) = phase_outcome.intervention() {
            if self.intervention.is_none() {
                self.intervention = Some(WafInterventionLog {
                    phase,
                    rule_id: intervention.rule_id(),
                    status: intervention.status(),
                });
            }
            self.outcome = WafLogOutcome::Blocked;
        } else if recordable != 0 || runtime_omitted != 0 {
            if !matches!(self.outcome, WafLogOutcome::Blocked | WafLogOutcome::Error) {
                self.outcome = WafLogOutcome::Detected;
            }
        } else if phase == WafLogPhase::Logging
            && matches!(self.outcome, WafLogOutcome::NotRun | WafLogOutcome::Running)
        {
            self.outcome = WafLogOutcome::Allowed;
        } else if matches!(self.outcome, WafLogOutcome::NotRun) {
            self.outcome = WafLogOutcome::Running;
        }
    }

    fn record_error(&mut self, phase: WafLogPhase, code: WafErrorCode, status: u16) {
        if self.terminal_error.is_none() {
            self.terminal_error = Some(WafTerminalErrorLog { phase, code, status });
        }
        self.outcome = WafLogOutcome::Error;
    }

    fn finalize(&mut self) {
        if self.phases.logging.is_none() && matches!(self.outcome, WafLogOutcome::Running | WafLogOutcome::Detected) {
            self.outcome = WafLogOutcome::Incomplete;
        }
    }
}

#[cfg(test)]
mod waf_context_tests {
    use super::*;
    use edgion_resources::resources::edgion_plugins::{WafConfig, WafMode};
    use edgion_waf::seclang::{RequestHead, RuleEngineMode, RuntimeLimits, Transaction};

    fn request_headers_outcome(rules: &str, uri: &'static [u8]) -> edgion_waf::seclang::PhaseOutcome {
        let config = WafConfig {
            rules: rules.to_string(),
            mode: WafMode::On,
            priority: 0,
        };
        let generation = Arc::new(config.compile_generation().expect("test WAF compiles"));
        let request = RequestHead::new(
            b"GET /audit HTTP/1.1",
            b"GET",
            uri,
            uri,
            b"HTTP/1.1",
            b"192.0.2.1",
            b"request-1",
            &[],
        );
        let mut transaction = Transaction::new_shared_with_modes(
            generation,
            request,
            RuntimeLimits::default(),
            edgion_waf::seclang::EvaluationMode::Accelerated,
            RuleEngineMode::On,
        )
        .unwrap();
        transaction.execute_request_headers().unwrap()
    }

    #[test]
    fn winner_selection_and_phase_claims_are_exact_once() {
        let selected = Arc::new(());
        let other = Arc::new(());
        let mut ctx = EdgionHttpContext::new();

        assert!(ctx.waf.is_none());
        assert!(ctx.select_waf(Some(WafSelection::new(
            Arc::clone(&selected),
            WafLogSource::Route,
            10,
            WafLogMode::On,
        ))));
        assert!(!ctx.select_waf(Some(WafSelection::new(
            Arc::clone(&other),
            WafLogSource::Global,
            20,
            WafLogMode::DetectionOnly,
        ))));
        assert_eq!(ctx.claim_waf_phase(&other, WafPhase::Request), WafPhaseClaim::Skip);
        for phase in [WafPhase::Request, WafPhase::ResponseHeaders, WafPhase::ResponseComplete] {
            assert_eq!(ctx.claim_waf_phase(&selected, phase), WafPhaseClaim::Execute);
            assert_eq!(ctx.claim_waf_phase(&selected, phase), WafPhaseClaim::Skip);
        }
    }

    #[test]
    fn no_winner_keeps_the_unified_waf_state_unallocated() {
        let owner = Arc::new(());
        let mut ctx = EdgionHttpContext::new();

        assert!(ctx.select_waf(None));
        assert!(!ctx.select_waf(None));
        assert!(!ctx.select_waf(Some(WafSelection::new(
            Arc::clone(&owner),
            WafLogSource::Route,
            0,
            WafLogMode::On,
        ))));
        assert!(ctx.waf.is_none());
        assert_eq!(ctx.claim_waf_phase(&owner, WafPhase::Request), WafPhaseClaim::Pending);
        assert_eq!(
            std::mem::size_of::<Option<Box<WafRequestContext>>>(),
            std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn selected_waf_allocates_one_request_owned_box_and_no_winner_allocates_none() {
        let owner = Arc::new(());
        let (selected, selected_allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            let mut ctx = EdgionHttpContext::new();
            assert!(ctx.select_waf(Some(WafSelection::new(
                Arc::clone(&owner),
                WafLogSource::Route,
                0,
                WafLogMode::On,
            ))));
            ctx
        });
        assert!(selected.waf_access_log().is_some());
        assert_eq!(selected_allocations.calls, 1);
        assert_eq!(
            selected_allocations.requested_bytes,
            std::mem::size_of::<WafRequestContext>()
        );

        let (unselected, unselected_allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            let mut ctx = EdgionHttpContext::new();
            assert!(ctx.select_waf(None));
            ctx
        });
        assert!(unselected.waf_access_log().is_none());
        assert_eq!(unselected_allocations.calls, 0);
        assert_eq!(unselected_allocations.requested_bytes, 0);
    }

    #[test]
    fn access_summary_is_bounded_and_retains_intervention_outside_the_finding_cap() {
        let mut rules = String::new();
        for id in 1..=20 {
            rules.push_str(&format!(
                "SecRule REQUEST_URI \"@contains /audit\" \"id:{id},phase:1,pass,log,auditlog\"\n"
            ));
        }
        rules.push_str("SecRule REQUEST_URI \"@contains /audit\" \"id:999,phase:1,deny,status:409,log,auditlog\"\n");
        let outcome = request_headers_outcome(&rules, b"/audit-private-input");
        let mut access = WafAccessLog::new(WafLogSource::Route, 17, WafLogMode::On);
        access.record_phase(WafLogPhase::RequestHeaders, 23, &outcome);

        let value = serde_json::to_value(&access).unwrap();
        assert_eq!(value["source"], "route");
        assert_eq!(value["priority"], 17);
        assert_eq!(value["mode"], "on");
        assert_eq!(value["outcome"], "blocked");
        assert_eq!(value["findings"].as_array().unwrap().len(), MAX_WAF_ACCESS_LOG_FINDINGS);
        assert_eq!(value["findings_omitted"], 5);
        assert_eq!(value["intervention"]["rule_id"], 999);
        assert_eq!(value["intervention"]["status"], 409);
        let json = value.to_string();
        assert!(!json.contains("audit-private-input"));
    }

    #[test]
    fn nolog_finding_is_excluded_and_first_terminal_error_is_stable() {
        let outcome = request_headers_outcome(
            concat!(
                "SecRule REQUEST_URI \"@contains /audit\" \"id:1,phase:1,pass,nolog,noauditlog\"\n",
                "SecRule REQUEST_URI \"@contains /audit\" \"id:2,phase:1,pass,log\"\n",
            ),
            b"/audit",
        );
        let mut access = WafAccessLog::new(WafLogSource::Gateway, 3, WafLogMode::DetectionOnly);
        access.record_phase(WafLogPhase::RequestHeaders, 7, &outcome);
        access.record_error(WafLogPhase::RequestBody, WafErrorCode::BodyLimit, 413);
        access.record_error(WafLogPhase::Logging, WafErrorCode::RuntimeInvariant, 500);

        let value = serde_json::to_value(&access).unwrap();
        let findings = value["findings"].as_array().unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0]["rule_id"], 2);
        assert_eq!(value["terminal_error"]["phase"], "request_body");
        assert_eq!(value["terminal_error"]["code"], "body_limit");
        assert_eq!(value["terminal_error"]["status"], 413);
    }

    #[test]
    fn finalization_distinguishes_not_run_incomplete_and_allowed() {
        // The rule set must be non-empty to compile; `/clean` simply does not match it.
        let clean = request_headers_outcome(
            "SecRule REQUEST_URI \"@contains /detected\" \"id:1,phase:1,pass,log\"\n",
            b"/clean",
        );
        let detected = request_headers_outcome(
            "SecRule REQUEST_URI \"@contains /detected\" \"id:1,phase:1,pass,log\"\n",
            b"/detected",
        );

        let mut not_run = WafAccessLog::new(WafLogSource::Global, 0, WafLogMode::On);
        not_run.finalize();
        assert_eq!(serde_json::to_value(&not_run).unwrap()["outcome"], "not_run");

        let mut incomplete = WafAccessLog::new(WafLogSource::Global, 0, WafLogMode::On);
        incomplete.record_phase(WafLogPhase::RequestHeaders, 1, &clean);
        incomplete.finalize();
        assert_eq!(serde_json::to_value(&incomplete).unwrap()["outcome"], "incomplete");

        let mut incomplete_after_finding = WafAccessLog::new(WafLogSource::Global, 0, WafLogMode::On);
        incomplete_after_finding.record_phase(WafLogPhase::RequestHeaders, 1, &detected);
        incomplete_after_finding.finalize();
        assert_eq!(
            serde_json::to_value(&incomplete_after_finding).unwrap()["outcome"],
            "incomplete"
        );

        let mut allowed = WafAccessLog::new(WafLogSource::Global, 0, WafLogMode::On);
        allowed.record_phase(WafLogPhase::Logging, 1, &clean);
        allowed.finalize();
        assert_eq!(serde_json::to_value(&allowed).unwrap()["outcome"], "allowed");

        let mut fully_detected = WafAccessLog::new(WafLogSource::Global, 0, WafLogMode::DetectionOnly);
        fully_detected.record_phase(WafLogPhase::Logging, 1, &detected);
        fully_detected.finalize();
        assert_eq!(serde_json::to_value(&fully_detected).unwrap()["outcome"], "detected");
    }
}

impl CurrentPluginEntryIdentity {
    pub(crate) fn new(name: &str, alias: Option<&str>) -> Option<Self> {
        if name.is_empty() || name.len() > PLUGIN_ENTRY_NAME_CAPACITY {
            return None;
        }
        let alias = alias.unwrap_or_default();
        if alias.len() > PLUGIN_ENTRY_ALIAS_CAPACITY {
            return None;
        }
        let mut identity = Self {
            name: [0; PLUGIN_ENTRY_NAME_CAPACITY],
            name_len: name.len() as u8,
            alias: [0; PLUGIN_ENTRY_ALIAS_CAPACITY],
            alias_len: alias.len() as u8,
            has_alias: !alias.is_empty(),
        };
        identity.name[..name.len()].copy_from_slice(name.as_bytes());
        identity.alias[..alias.len()].copy_from_slice(alias.as_bytes());
        Some(identity)
    }

    pub(crate) fn name(&self) -> &str {
        // Both arrays are populated exclusively from validated UTF-8 `str`
        // slices and read back at their original lengths.
        std::str::from_utf8(&self.name[..usize::from(self.name_len)]).expect("plugin identity must remain UTF-8")
    }

    pub(crate) fn alias(&self) -> Option<&str> {
        self.has_alias.then(|| {
            std::str::from_utf8(&self.alias[..usize::from(self.alias_len)])
                .expect("plugin alias identity must remain UTF-8")
        })
    }
}

#[cfg(test)]
mod plugin_entry_identity_tests {
    use super::CurrentPluginEntryIdentity;

    #[test]
    fn preserves_bounded_name_and_alias() {
        let identity = CurrentPluginEntryIdentity::new("AiProxy", Some("edge.ai-1")).unwrap();
        assert_eq!(identity.name(), "AiProxy");
        assert_eq!(identity.alias(), Some("edge.ai-1"));
        assert_eq!(CurrentPluginEntryIdentity::new("AiProxy", None).unwrap().alias(), None);
    }

    #[test]
    fn rejects_empty_or_overlong_identity_parts() {
        assert!(CurrentPluginEntryIdentity::new("", None).is_none());
        assert!(CurrentPluginEntryIdentity::new(&"x".repeat(37), None).is_none());
        assert!(CurrentPluginEntryIdentity::new("AiProxy", Some(&"x".repeat(33))).is_none());
    }
}

/// Operator-defined log labels, resolved once per resource and shared by
/// reference across `Rsc` clones (so the per-request path never re-allocates).
///
/// Serializes as a flat JSON object, e.g. `{"env":"prod","tenant":"internal"}`.
/// Backed by a `BTreeMap` so the output ordering is deterministic across reloads
/// and route-table rebuilds. The resolution logic and guardrails live in
/// `edgion_resources::resources::common::resolve_log_labels` (the single source
/// of truth, applied at resource-parse time); this newtype only wraps the
/// resolved map for the data-plane log path.
#[derive(Debug, Clone)]
pub struct LogLabels(Arc<BTreeMap<String, String>>);

impl LogLabels {
    /// Wrap a map resolved at parse time (e.g. a `pre_parse` cache field) into a
    /// `LogLabels`. Cheap: shares the `Arc`, performs no scanning.
    pub fn from_resolved(map: Arc<BTreeMap<String, String>>) -> Self {
        LogLabels(map)
    }
}

impl Serialize for LogLabels {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Delegate to the inner map; serde's `rc` feature is not enabled, so the
        // `Arc` wrapper must be unwrapped manually here.
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LogLabels {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<String, String>::deserialize(deserializer)?;
        Ok(LogLabels(Arc::new(map)))
    }
}

/// Resolve operator-defined log labels from a resource's `metadata`.
///
/// Thin gateway-side wrapper over
/// [`edgion_resources::resources::common::resolve_log_labels`] (the single
/// implementation of the prefix scan + guardrails). Called from TLSRoute
/// build only (`routes::tls::route_unit`) — the `edgion.io/log-label.<suffix>`
/// carrier is narrowed to TLS resources, whose handshake layer has no plugin
/// chain and no `logging()` stage to hook an `AccessLogExtern` declaration
/// onto. HTTPRoute/GRPCRoute build no longer calls this: their log projection
/// goes through `AccessLogExtern`'s `from: routeLabel` / `from:
/// routeAnnotation` instead. EdgionTls caches its resolved map in
/// `pre_parse`, so it wraps the cached `Arc` via [`LogLabels::from_resolved`]
/// rather than calling this function.
pub fn extract_log_labels(meta: &kube::api::ObjectMeta) -> Option<LogLabels> {
    edgion_resources::resources::common::resolve_log_labels(meta).map(LogLabels)
}

/// A route's raw `metadata.labels` or `metadata.annotations`, resolved once at
/// route build and `Arc`-shared onto every match unit.
///
/// Distinct from [`LogLabels`], which is the *projected* log-label set. This is
/// the raw metadata, needed to answer `AccessLogExtern`'s `from: routeLabel` /
/// `from: routeAnnotation` lookups without touching `ObjectMeta` per request.
pub type RouteMeta = Option<Arc<BTreeMap<String, String>>>;

/// Clone a route's labels and annotations into `Arc`s at build time.
/// Returns `(labels, annotations)`; `None` when the map is absent or empty, so
/// the common case stores nothing.
pub fn extract_route_meta(meta: &kube::api::ObjectMeta) -> (RouteMeta, RouteMeta) {
    let take = |m: Option<&BTreeMap<String, String>>| -> RouteMeta {
        match m {
            Some(map) if !map.is_empty() => Some(Arc::new(map.clone())),
            _ => None,
        }
    };
    (take(meta.labels.as_ref()), take(meta.annotations.as_ref()))
}

/// Unified resource identifier for access log.
/// Provides consistent `kind`/`ns`/`name` across all resource types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rsc {
    pub kind: String,
    pub ns: String,
    pub name: String,

    /// Operator-defined log labels for this resource (see [`extract_log_labels`]).
    /// Omitted from the serialized log entry when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<LogLabels>,
}

impl Rsc {
    /// Build a resource identifier with no labels (the common case for backend /
    /// internally-derived resources that carry no operator log labels).
    pub fn new(kind: impl Into<String>, ns: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            ns: ns.into(),
            name: name.into(),
            labels: None,
        }
    }

    /// Attach resolved operator log labels, consuming and returning `self`.
    #[cfg(test)]
    pub fn with_labels(mut self, labels: Option<LogLabels>) -> Self {
        self.labels = labels;
        self
    }
}

#[derive(Clone, Serialize)]
pub struct MatchInfo {
    /// Resource identifier (kind="HTTPRoute")
    pub rsc: Rsc,

    /// Rule id in HTTPROUTE
    pub rule_id: usize,
    /// Match id at rule id,
    pub match_id: usize,

    /// sync_version from gRPC sync (0 = not set)
    #[serde(skip_serializing_if = "is_zero")]
    pub sv: u64,

    /// match item
    #[serde(skip)]
    pub m: HTTPRouteMatch,

    /// Raw route labels for `AccessLogExtern` `from: routeLabel`. In-process
    /// only — never part of the serialized entry; only declared keys reach `ext`.
    #[serde(skip)]
    pub labels_meta: RouteMeta,

    /// Raw route annotations for `AccessLogExtern` `from: routeAnnotation`.
    #[serde(skip)]
    pub annotations_meta: RouteMeta,
}

pub fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl MatchInfo {
    /// HTTPRoute no longer carries operator log labels (see
    /// [`extract_log_labels`]'s doc) — `rsc.labels` is always `None` here.
    /// `AccessLogExtern`'s `from: routeLabel` / `from: routeAnnotation` reads
    /// `labels_meta` / `annotations_meta` instead (attached via
    /// [`MatchInfo::with_route_meta`]).
    pub fn new(ns: String, name: String, rule_id: usize, match_id: usize, m: HTTPRouteMatch, sv: u64) -> Self {
        Self {
            rsc: Rsc {
                kind: "HTTPRoute".to_string(),
                ns,
                name,
                labels: None,
            },
            m,
            rule_id,
            match_id,
            sv,
            labels_meta: None,
            annotations_meta: None,
        }
    }

    /// Attach the route's raw metadata maps, resolved once at route build.
    pub fn with_route_meta(mut self, labels: RouteMeta, annotations: RouteMeta) -> Self {
        self.labels_meta = labels;
        self.annotations_meta = annotations;
        self
    }
}

impl fmt::Display for MatchInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{} (rule:{}, match:{})",
            self.rsc.ns, self.rsc.name, self.rule_id, self.match_id
        )
    }
}

/// Client certificate information extracted from TLS connection
#[derive(Clone, Serialize)]
pub struct ClientCertInfo {
    /// Certificate subject DN (Distinguished Name)
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    pub subject: String,
    /// Subject Alternative Names (SANs) from certificate
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "edgion_resources::sensitive::masked_vec::serialize"
    )]
    pub sans: Vec<String>,
    /// Common Name (CN) extracted from subject
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "edgion_resources::sensitive::masked_opt::serialize"
    )]
    pub cn: Option<String>,
    /// Certificate fingerprint (SHA256 hex) — not PII
    pub fingerprint: String,
}

impl fmt::Debug for ClientCertInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClientCertInfo([redacted])")
    }
}

/// TLS connection identifier stored in SslDigestExtension
#[derive(Debug, Clone)]
pub struct TlsConnId(pub u64);

#[derive(Debug, Clone, Serialize)]
pub struct MatchedInfo {
    pub rsc: Rsc,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// sync_version from gRPC sync (0 = not set)
    #[serde(skip_serializing_if = "is_zero")]
    pub sv: u64,
}

/// TLS handshake metadata stored in SslDigestExtension.
///
/// Created once per TLS connection in handshake_complete_callback and shared by
/// requests on that connection through Pingora's digest extension.
///
/// For the HTTP path, consumed in pg_request_filter.rs.
/// For the TLS proxy path, consumed in EdgionTls::process_new.
#[derive(Debug, Clone, Serialize)]
pub struct TlsConnMeta {
    pub ts: i64,
    #[serde(skip_serializing)]
    pub start_at: Instant,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handshake_complete_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Connection id for tls.log <-> access.log correlation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_id: Option<String>,
    /// SNI from handshake (if present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<LogBuffer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<MatchedInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err_log: Option<String>,
    /// mTLS client cert info (only when annotation gate is enabled).
    #[serde(skip_serializing)]
    pub client_cert_info: Option<ClientCertInfo>,
    /// Whether mTLS is configured for this connection.
    #[serde(skip_serializing_if = "is_false")]
    pub is_mtls: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// A hostname that has been normalized (port-stripped, FQDN-dot-removed, lowercased).
///
/// Construct via `NormalizedHostname::new()` which applies the normalization once.
/// All downstream consumers can rely on the invariant — no further lowercasing needed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NormalizedHostname(String);

impl NormalizedHostname {
    /// Normalize and wrap a raw hostname.
    /// Applies: trim, strip port (IPv4/IPv6), remove trailing FQDN dot, lowercase.
    pub fn new(raw: &str) -> Self {
        let h = raw.trim();

        let host = if h.starts_with('[') {
            match h.find(']') {
                Some(end) => &h[1..end],
                None => h,
            }
        } else {
            match h.rfind(':') {
                // A registered name or IPv4 authority has at most one colon.
                // Bare IPv6 has multiple colons and no syntactically distinct
                // port suffix, so it must remain intact.
                Some(pos) if h.matches(':').count() == 1 && h[pos + 1..].bytes().all(|b| b.is_ascii_digit()) => {
                    &h[..pos]
                }
                _ => h,
            }
        };

        let host = host.strip_suffix('.').unwrap_or(host);
        Self(host.to_ascii_lowercase())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::ops::Deref for NormalizedHostname {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for NormalizedHostname {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NormalizedHostname {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod normalized_hostname_tests {
    use super::NormalizedHostname;

    #[test]
    fn client_certificate_debug_omits_identity() {
        let info = super::ClientCertInfo {
            subject: "private-subject".into(),
            sans: vec!["private-san".into()],
            cn: Some("private-cn".into()),
            fingerprint: "public-fingerprint".into(),
        };
        for debug in [format!("{info:?}"), format!("{info:#?}"), format!("{:?}", Some(&info))] {
            assert!(debug.contains("[redacted]"));
            assert!(!debug.contains("private"));
        }
        assert_eq!(info.subject, "private-subject");
    }

    #[test]
    fn normalization_strips_only_unambiguous_ports() {
        assert_eq!(NormalizedHostname::new("Example.COM:8443").as_str(), "example.com");
        assert_eq!(NormalizedHostname::new("127.0.0.1:8080").as_str(), "127.0.0.1");
        assert_eq!(NormalizedHostname::new("[2001:db8::1]:443").as_str(), "2001:db8::1");
        assert_eq!(NormalizedHostname::new("2001:db8::1").as_str(), "2001:db8::1");
    }
}

/// TLS-only request facts allocated when handshake metadata is present.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct RequestTlsInfo {
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "edgion_resources::sensitive::masked_opt::serialize"
    )]
    sni: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_cert_info: Option<ClientCertInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tls_id: Option<String>,
    #[serde(skip)]
    matched_listener: Option<String>,
    #[serde(skip)]
    matched_gateway_key: Option<String>,
}

/// gRPC-only request facts allocated on the first gRPC-specific write.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct RequestGrpcInfo {
    #[serde(rename = "grpc_service", skip_serializing_if = "Option::is_none")]
    service: Option<String>,
    #[serde(rename = "grpc_method", skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(rename = "grpc_status", skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip)]
    client_deadline: Option<Instant>,
}

/// Request information extracted from the incoming request.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RequestInfo {
    /// Physical downstream transport peer IP before PROXY/XFF rewriting.
    #[serde(
        rename = "direct_peer_ip",
        serialize_with = "edgion_resources::sensitive::masked::serialize"
    )]
    pub direct_peer_ip: String,
    /// Physical downstream transport peer port, or zero when unknown.
    pub direct_peer_port: u16,
    /// PROXY-effective downstream peer before HTTP RealIp/XFF processing.
    #[serde(skip)]
    pub downstream_peer_ip: String,
    /// Layer-resolved client IP, possibly updated by trusted RealIp processing.
    #[serde(
        rename = "remote_ip",
        serialize_with = "edgion_resources::sensitive::masked::serialize"
    )]
    pub remote_ip: String,
    /// Canonical W3C context and managed correlation identifiers.
    pub trace_id: crate::request_trace_id::RequestTraceId,
    /// Normalized hostname from the Host header (lowercased, port-stripped).
    /// Construct via `NormalizedHostname::new()` — never apply `to_lowercase()` again.
    #[serde(rename = "host")]
    pub hostname: NormalizedHostname,
    /// Snapshot of req_header().uri.path() at request entry. Frozen for the request lifetime.
    /// Source for `request_path()` / `request_uri()` views.
    /// Not serialized into access_log directly; surfaced via the optional
    /// `request_uri` field on `AccessLogEntry` when `include_raw_uri` is on.
    #[serde(skip)]
    pub entry_raw_path: String,

    /// Snapshot of req_header().uri.query() at request entry. Frozen for the request lifetime.
    /// Source for `request_uri()` view.
    /// Not serialized into access_log directly; surfaced via the optional
    /// `request_uri` field on `AccessLogEntry` when `include_raw_uri` is on.
    #[serde(skip)]
    pub entry_raw_query: Option<String>,

    /// Cache of normalized current path. Populated at entry from entry_raw_path,
    /// refreshed by session_adapter::set_upstream_uri after rewrite.
    /// Source for `path()` / `uri()` views.
    /// Serializes as JSON field "path" for backward-compatible access_log output.
    #[serde(rename = "path", serialize_with = "edgion_resources::sensitive::masked::serialize")]
    pub normalized_current_path: String,
    /// Final downstream/access-log HTTP status. Zero represents Nginx `000`.
    status: u16,
    /// Original X-Forwarded-For header value (before appending client IP)
    #[serde(
        rename = "x-forwarded-for",
        skip_serializing_if = "Option::is_none",
        serialize_with = "edgion_resources::sensitive::masked_opt::serialize"
    )]
    pub x_forwarded_for: Option<String>,
    /// Auto-discovered protocol (e.g., "grpc", "grpc-web", "websocket")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discover_protocol: Option<String>,
    /// Whether the request is a gRPC or gRPC-Web request
    #[serde(skip)]
    pub is_grpc_request: bool,
    /// Whether the client declared a request body, snapshotted in
    /// `build_request_metadata` **before any plugin runs**.
    ///
    /// `None` means the snapshot never ran, which is a bug — the body-gate
    /// predicate treats it as a refusal (fail-close) rather than guessing.
    /// This is deliberately `Option<bool>` and not `bool`: `RequestInfo`
    /// derives `Default`, and a defaulted `false` would read as "no body,
    /// run the plugin without one".
    #[serde(skip)]
    pub body_declared: Option<bool>,
    /// Whether the request is currently within the request-processing stage,
    /// where the client body (if any) has not yet been proxied upstream.
    ///
    /// Set `true` in `snapshot_body_declared` at the same entry point as
    /// `body_declared`. Stays `true` only through `request_filter`'s global,
    /// Gateway, and route request-stage plugins, then flips to `false` before
    /// backend selection and Pingora's retry loop. This is the request-scoped
    /// body-policy freeze: backendRef plugins may read an already-cached
    /// snapshot, but cannot start capture, mutate it, install observers or
    /// streamed handlers, or enable WAF streaming. The first backend request
    /// dispatcher separately permits a read-only mirror tee before body delivery;
    /// its adapter-local permission does not reopen this stage.
    ///
    /// Defaults to `false` (the safe/refuse direction): a `RequestInfo` built
    /// without going through the real request-entry snapshot (a bare
    /// `RequestInfo::default()` in a test, or a future code path that never
    /// reaches `snapshot_body_declared`) reads as "past the request stage"
    /// rather than silently defaulting to "still the request stage" and
    /// permitting a body read it should not.
    #[serde(skip)]
    pub in_request_body_stage: bool,
    /// Whether the DOWNSTREAM request method was `HEAD`, snapshotted in
    /// `snapshot_body_declared` at the same entry point as `body_declared`,
    /// **before any plugin runs**.
    ///
    /// `read_response_body` needs this fact (RFC 9110 9.3.2: a HEAD response
    /// carries `Content-Length` but no body) to decide the bodyless
    /// short-circuit, and ANDs it with the live request method — the silent
    /// `PassThrough` disposition requires both to agree. Neither is sufficient
    /// alone, because `set_upstream_method` (reachable from `proxy_rewrite` and
    /// from DSL's `req.set_method`, which is not stage-gated) rewrites the live
    /// header in place and each direction breaks one of them:
    ///
    /// - Live header alone: a rewrite TO `HEAD` makes `read_response_body`
    ///   report "no body" for a response whose body already arrived.
    /// - This snapshot alone: a downstream `HEAD` rewritten to a body-carrying
    ///   method makes pingora write the real body downstream (its
    ///   `init_body_writer` keys on the same live header this rewrite mutated),
    ///   while the snapshot still says HEAD.
    ///
    /// Both directions land a body-inspecting plugin on `Ok(Bytes::new())`,
    /// whose contract is "the response truthfully declared no body" — silently
    /// downgrading "inspect the body" into "inspect nothing and believe it was
    /// empty". ANDing lets the mutable fact only NARROW `PassThrough`, never
    /// widen it; `request_admission` applies the same discipline in the mirror
    /// direction (a client-controlled fact may only widen a refusal).
    ///
    /// Defaults to `false` (the safe direction): a `RequestInfo` that never
    /// reached the snapshot reads as "not HEAD", which routes to `Buffer` — an
    /// attempted read that can fail loudly — rather than to the silent
    /// `PassThrough`. A genuine HEAD response has no body to buffer either way.
    #[serde(skip)]
    pub method_is_head: bool,
    /// Listener port that received this request (set at context creation, always available)
    #[serde(rename = "listener-port")]
    pub listener_port: u16,
    /// TLS-only request facts. Absent on cleartext requests.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    tls: Option<Box<RequestTlsInfo>>,
    /// gRPC-only request facts. Absent until a gRPC-specific fact is written.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    grpc: Option<Box<RequestGrpcInfo>>,

    /// Lazily allocated GeoIP facts derived from the downstream identities.
    /// Ordinary requests retain only the empty `OnceLock`; the first GeoIP
    /// consumer performs one allocation for both independently cached views.
    #[serde(skip)]
    pub(crate) geo: std::sync::OnceLock<Box<RequestGeoInfo>>,
}

/// GeoIP facts derived from the two downstream client identities.
#[derive(Debug, Clone, Default)]
pub(crate) struct RequestGeoInfo {
    /// Geo for the physical direct peer (`RequestInfo::direct_peer_ip`).
    pub(crate) direct_peer: std::sync::OnceLock<crate::geoip::GeoLookup>,
    /// Geo for the resolved client (`RequestInfo::remote_ip`).
    pub(crate) remote: std::sync::OnceLock<crate::geoip::GeoLookup>,
}

impl RequestInfo {
    #[inline]
    pub(crate) fn install_tls_info(
        &mut self,
        tls_id: Option<String>,
        sni: Option<String>,
        client_cert_info: Option<ClientCertInfo>,
        matched_listener: Option<String>,
        matched_gateway_key: Option<String>,
    ) {
        self.tls = Some(Box::new(RequestTlsInfo {
            sni,
            client_cert_info,
            tls_id,
            matched_listener,
            matched_gateway_key,
        }));
    }

    #[inline]
    pub(crate) fn set_tls_id(&mut self, tls_id: String) {
        self.tls.get_or_insert_with(Default::default).tls_id = Some(tls_id);
    }

    #[inline]
    pub(crate) fn client_cert_info(&self) -> Option<&ClientCertInfo> {
        self.tls.as_deref().and_then(|info| info.client_cert_info.as_ref())
    }

    #[inline]
    pub(crate) fn gateway_tls_match(&self) -> Option<(&str, &str)> {
        let info = self.tls.as_deref()?;
        Some((info.matched_listener.as_deref()?, info.matched_gateway_key.as_deref()?))
    }

    #[inline]
    fn grpc_mut(&mut self) -> &mut RequestGrpcInfo {
        self.grpc.get_or_insert_with(Default::default)
    }

    #[inline]
    pub(crate) fn set_grpc_method(&mut self, service: String, method: String) {
        let info = self.grpc_mut();
        info.service = Some(service);
        info.method = Some(method);
    }

    #[inline]
    pub(crate) fn grpc_service(&self) -> Option<&str> {
        self.grpc.as_deref().and_then(|info| info.service.as_deref())
    }

    #[inline]
    pub(crate) fn grpc_method(&self) -> Option<&str> {
        self.grpc.as_deref().and_then(|info| info.method.as_deref())
    }

    #[inline]
    pub(crate) fn set_grpc_status(&mut self, status: u16) {
        self.grpc_mut().status = Some(status);
    }

    #[inline]
    pub fn grpc_status(&self) -> Option<u16> {
        self.grpc.as_deref().and_then(|info| info.status)
    }

    #[inline]
    pub(crate) fn set_grpc_client_deadline(&mut self, deadline: Instant) {
        self.grpc_mut().client_deadline = Some(deadline);
    }

    #[inline]
    pub(crate) fn grpc_client_deadline(&self) -> Option<Instant> {
        self.grpc.as_deref().and_then(|info| info.client_deadline)
    }

    #[cfg(test)]
    pub(crate) fn has_tls_info(&self) -> bool {
        self.tls.is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_grpc_info(&self) -> bool {
        self.grpc.is_some()
    }

    #[inline]
    pub fn status(&self) -> u16 {
        self.status
    }

    #[inline]
    pub fn status_option(&self) -> Option<u16> {
        (self.status != 0).then_some(self.status)
    }

    #[inline]
    fn finalize_status(&mut self, status: u16) {
        self.status = status;
    }

    #[inline]
    fn geo(&self) -> &RequestGeoInfo {
        self.geo.get_or_init(|| Box::new(RequestGeoInfo::default())).as_ref()
    }

    /// Return only already-resolved GeoIP facts. Access-log construction must
    /// not turn logging into a GeoIP lookup or allocate the optional cache.
    pub(crate) fn resolved_geo(&self) -> (Option<&crate::geoip::GeoLookup>, Option<&crate::geoip::GeoLookup>) {
        self.geo
            .get()
            .map(|geo| (geo.remote.get(), geo.direct_peer.get()))
            .unwrap_or((None, None))
    }

    /// Invalidate facts derived from `remote_ip` after RealIp rewrites it.
    pub(crate) fn invalidate_remote_geo(&mut self) {
        if let Some(geo) = self.geo.get_mut() {
            geo.remote.take();
        }
    }
}

#[cfg(test)]
mod request_scenario_state_tests {
    use super::*;

    #[test]
    fn ordinary_request_keeps_tls_and_grpc_state_absent() {
        let info = RequestInfo::default();

        assert!(!info.has_tls_info());
        assert!(!info.has_grpc_info());
        assert_eq!(std::mem::size_of::<RequestInfo>(), 288);
    }

    #[test]
    fn tls_facts_share_one_lazy_allocation_and_keep_flat_json() {
        let cert = ClientCertInfo {
            subject: "CN=client".to_string(),
            sans: vec!["client.example.com".to_string()],
            cn: Some("client".to_string()),
            fingerprint: "sha256".to_string(),
        };
        let tls_id = "tls-1".to_string();
        let sni = "api.example.com".to_string();
        let listener = "https".to_string();
        let gateway = "default/gateway".to_string();
        let (info, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            let mut info = RequestInfo::default();
            info.install_tls_info(Some(tls_id), Some(sni), Some(cert), Some(listener), Some(gateway));
            assert!(info.client_cert_info().is_some());
            assert_eq!(info.gateway_tls_match(), Some(("https", "default/gateway")));
            info
        });

        assert_eq!(allocations.calls, 1);
        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("sni").is_some());
        assert_eq!(json["tls_id"], "tls-1");
        assert!(json.get("tls").is_none());
    }

    #[test]
    fn grpc_facts_share_one_lazy_allocation_and_keep_flat_json() {
        let service = "catalog.v1.Catalog".to_string();
        let method = "GetItem".to_string();
        let deadline = Instant::now() + Duration::from_secs(3);
        let (info, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            let mut info = RequestInfo::default();
            info.set_grpc_method(service, method);
            info.set_grpc_status(8);
            info.set_grpc_client_deadline(deadline);
            assert_eq!(info.grpc_service(), Some("catalog.v1.Catalog"));
            assert_eq!(info.grpc_method(), Some("GetItem"));
            assert_eq!(info.grpc_client_deadline(), Some(deadline));
            info
        });

        assert_eq!(allocations.calls, 1);
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["grpc_service"], "catalog.v1.Catalog");
        assert_eq!(json["grpc_method"], "GetItem");
        assert_eq!(json["grpc_status"], 8);
        assert!(json.get("grpc").is_none());
        assert!(json.get("client_deadline").is_none());
    }
}

/// Backend TLS connection information
#[derive(Debug, Clone, Serialize)]
pub struct BackendTlsInfo {
    /// SNI (Server Name Indication) sent to backend
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    /// TLS handshake success
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handshake_ok: Option<bool>,
    /// TLS protocol version (e.g., "TLSv1.3")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Cipher suite used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cipher: Option<String>,
}

/// Upstream connection information for a single connection attempt
#[derive(Debug, Serialize)]
pub struct UpstreamInfo {
    /// Upstream IP address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Upstream port
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// HTTP status code for this upstream
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Connect time in milliseconds (from start_time to connection established)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ct: Option<u64>,
    /// Header time in milliseconds (from start_time to receiving response header)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ht: Option<u64>,
    /// Body time in milliseconds (from start_time to receiving first body chunk)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bt: Option<u64>,
    /// Elapsed time in milliseconds (total time for this upstream attempt)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub et: Option<u64>,
    /// Write pending time in milliseconds (HTTP/1.x only, upstream backpressure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wpt: Option<u64>,
    /// Upstream response body size in bytes (payload only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body_size: Option<usize>,
    /// Start time - when this upstream attempt started (for calculating ct, ht, and bt)
    #[serde(skip)]
    pub start_time: Instant,
    /// Error messages collected during upstream attempts
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub err: Vec<String>,
    /// Canonical backend address used by the LB selection algorithm, before
    /// any target-port rewrite. Runtime-state accounting must use this key
    /// so that selection reads and request-completion writes are consistent.
    #[serde(skip)]
    pub lb_backend_addr: Option<SocketAddr>,
    /// Live `LbBackend` captured from the joint Service selector generation.
    /// Set by `build_peer_from_lb_selection`; read by `connected_to_upstream`
    /// to call `inc_conn()` and by other attempt hooks for endpoint metadata.
    /// Sticky selection restores the same live Arc; bypass and non-LB paths
    /// leave this field as `None`.
    #[serde(skip)]
    pub lb_backend: Option<Arc<LbBackend>>,
    /// Outlier policy identity captured with this upstream attempt. Completion
    /// must present the same token so a policy rotation cannot accept a late
    /// result from the previous policy generation.
    #[serde(skip)]
    pub lb_outlier_completion: Option<crate::backends::OutlierCompletionToken>,
    /// Service key (`"namespace/name"`) that owns this backend, needed by
    /// service-scoped LB runtime state.
    #[serde(skip)]
    pub service_key: Option<String>,
    /// LB policy used for this upstream selection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lb_policy: Option<ParsedLBPolicy>,
    /// Backend TLS connection information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<BackendTlsInfo>,
    /// Attempt-owned LB outstanding-connection guard. `Some` only when this
    /// attempt reached `connected_to_upstream` AND its policy scores
    /// (LeastConn / Ewma). Consumed by `finalize_attempt` from
    /// `pg_upstream_peer::begin_upstream_attempt` / `begin_ai_peer_attempt` on
    /// retry, and from `pg_logging::lb_runtime_cleanup` at request end; an
    /// unresolved guard balances itself on drop. The latency sample is NOT
    /// recorded here — it lands at response headers in
    /// `pg_upstream_response_filter`.
    /// The guard owns the live backend Arc required for cancellation-safe Drop;
    /// `lb_backend` remains the selected-endpoint metadata used by other
    /// attempt hooks.
    #[serde(skip)]
    pub lb_committed: Option<crate::lb::LbCommitGuard>,
}

impl UpstreamInfo {
    #[inline]
    pub fn set_response_body_size(&mut self, size: usize) {
        if self.response_body_size.is_some_and(|previous| previous != size) {
            self.err.push("response_body_size already set".to_string());
        }
        self.response_body_size = Some(size);
    }
}

#[cfg(test)]
mod upstream_info_tests {
    use super::*;

    fn upstream_info() -> UpstreamInfo {
        UpstreamInfo {
            ip: None,
            port: None,
            status: None,
            ct: None,
            ht: None,
            bt: None,
            et: None,
            wpt: None,
            response_body_size: None,
            start_time: Instant::now(),
            err: Vec::new(),
            lb_backend_addr: None,
            lb_backend: None,
            lb_outlier_completion: None,
            service_key: None,
            lb_policy: None,
            tls: None,
            lb_committed: None,
        }
    }

    #[test]
    fn repeated_response_body_size_is_idempotent_when_unchanged() {
        let mut upstream = upstream_info();

        upstream.set_response_body_size(17);
        upstream.set_response_body_size(17);

        assert_eq!(upstream.response_body_size, Some(17));
        assert!(upstream.err.is_empty());
    }

    #[test]
    fn changed_response_body_size_keeps_double_set_diagnostic() {
        let mut upstream = upstream_info();

        upstream.set_response_body_size(17);
        upstream.set_response_body_size(23);

        assert_eq!(upstream.response_body_size, Some(23));
        assert_eq!(upstream.err, ["response_body_size already set"]);
    }
}

/// Bypass-path tag set when a plugin staged a non-default backend selection.
/// `None` on `BackendContext.source` means the default K8s resource-selected path
/// (the common case — emitting nothing in access log keeps the JSON quiet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendSource {
    /// DirectEndpoint bypass (plugin-staged exact IP).
    DirectEndpoint,
    /// ExternalJump bypass (plugin-staged external domain, DNS-resolved at runtime).
    ExternalJump,
    /// Typed EdgionBackend selected by an AiProxy attempt plan.
    AiBackend,
}

/// Backend context containing service info and upstream attempt history.
///
/// DirectEndpoint retains the validated Service identity but bypasses its load
/// balancer; ExternalJump has no K8s resource identity. `source` distinguishes
/// those paths from ordinary resource selection. `upstreams` is populated as
/// attempts happen.
#[derive(Debug, Serialize)]
pub struct BackendContext {
    /// Bypass-path tag — `None` on the default K8s resource path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSource>,
    /// Resource identifier (kind="Service") — `None` for ExternalJump.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rsc: Option<Rsc>,
    /// Service identity key `"namespace/name"` — `None` for ExternalJump.
    #[serde(skip)]
    pub service_key: Option<String>,
    /// Routing path (ClusterIP / Endpoint / EPS / ExternalName).
    /// `None` for ExternalJump (DNS-resolved external domain — none of the K8s
    /// service-routing modes apply). `Some` for the default path and DirectEndpoint.
    #[serde(skip)]
    pub service_kind: Option<ServiceBackendKind>,
    /// List of upstream connection attempts (for retry tracking and access log)
    pub upstreams: Vec<UpstreamInfo>,
    /// Current upstream index (for status updates)
    #[serde(skip)]
    pub current_upstream_id: Option<usize>,
    /// Position of the selected backend in `route_unit.rule.backend_refs` (HTTP)
    /// or the gRPC equivalent. `None` on the ExternalJump path (DNS-resolved
    /// external domain — no K8s backend ref) and when the route unit or its
    /// backend_refs list is absent during context initialization. `Some(idx)`
    /// on the normal HTTP/gRPC path, DirectEndpoint, and InternalJump. The
    /// hot metric path falls back to the dynamic lookup when this is `None`.
    #[serde(skip)]
    pub backend_ref_idx: Option<usize>,
    /// Captured Service `EdgionBackendTrafficPolicy` generation. Set once at
    /// first `begin_upstream_attempt` and reused for retries, including `None`
    /// when no policy matched. The request's attempt count gates capture. AI keeps its
    /// own capture on `AiHttpState`.
    #[serde(skip)]
    pub effective_policy:
        Option<std::sync::Arc<crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy>>,
}

/// Session persistence state for the current request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPersistenceState {
    /// No session persistence configured or token matched healthy backend (no action needed)
    Inactive,
    /// First request or stale token — inject new token in response
    InjectNew,
    /// Fallback happened — inject updated token in response
    InjectUpdate,
}

/// Session-persistence-only state allocated when the matched route enables
/// session persistence.
#[derive(Debug)]
pub(crate) struct SessionPersistenceContext {
    /// Response action settled by token extraction and fallback handling.
    pub(crate) state: SessionPersistenceState,
    /// Sticky endpoint for the next attempt. Consumed on first use.
    pub(crate) target_addr: Option<std::net::SocketAddr>,
    /// Sticky endpoint retained across retries for strict mode.
    pub(crate) sticky_addr: Option<std::net::SocketAddr>,
    /// Optional typed scope used by the canonical affinity token/map.
    pub(crate) token_scope: Option<String>,
    /// Whether an earlier attempt consumed the sticky endpoint.
    pub(crate) sticky_consumed: bool,
}

impl Default for SessionPersistenceContext {
    fn default() -> Self {
        Self {
            state: SessionPersistenceState::Inactive,
            target_addr: None,
            sticky_addr: None,
            token_scope: None,
            sticky_consumed: false,
        }
    }
}

/// Stored internal jump target info (lightweight, survives retries via Clone)
///
/// Unlike DirectEndpointPreset which specifies an exact endpoint address,
/// InternalJumpPreset specifies a BackendRef by name. The actual endpoint
/// selection happens via normal LB in get_peer().
///
/// This preset is consumed by select_http_backend() to find the matching
/// BackendRef instead of using weighted round-robin selection.
#[derive(Debug, Clone)]
pub struct InternalJumpPreset {
    /// Name of the target backend_ref (must match a backend_ref in the route)
    pub backend_ref_name: String,
    /// Optional namespace of the target backend_ref
    /// If None, matches by name only
    pub backend_ref_namespace: Option<String>,
}

/// Stored external jump target info (lightweight, survives retries via Clone)
///
/// Unlike DirectEndpointPreset (exact IP) or InternalJumpPreset (BackendRef name),
/// ExternalJumpPreset stores a domain name that requires DNS resolution
/// before creating the HttpPeer.
///
/// DNS resolution happens in upstream_peer_http() phase (async context).
/// On retry, DNS resolution is redone — the domain may resolve to a different
/// IP, providing natural DNS-level failover.
#[derive(Debug, Clone)]
pub struct ExternalJumpPreset {
    /// Target domain name (e.g., "api-us.example.com")
    pub domain: String,
    /// Target port
    pub port: u16,
    /// Whether to use TLS
    pub use_tls: bool,
    /// TLS SNI (Server Name Indication)
    pub sni: String,
    /// True when this jump represents gateway-to-gateway forwarding (e.g.,
    /// `region_route` cross-region traffic). The upstream gateway will run its
    /// own path normalization, so the path should be forwarded raw to avoid
    /// double-normalization conflicts.
    ///
    /// Set to `true` only by plugins that route traffic to another Edgion-like
    /// gateway. Plugins that target a regular external server (e.g.,
    /// `dynamic_external_upstream`) should set `false` so the global
    /// `forward_normalized_path` / per-route annotation policy applies.
    ///
    /// Default `false`: a future plugin that forgets to set this explicitly
    /// will not silently degrade to raw-forwarding behavior.
    pub preserve_raw_path: bool,
    /// Optional per-read upstream timeout supplied by the jump owner.
    ///
    /// When present, this replaces the normal route/global backendRequest
    /// bound before request and gRPC deadline clamping. `None` preserves the
    /// legacy external-jump timeout behavior.
    pub upstream_read_timeout_override: Option<Duration>,
}

/// Stored direct endpoint info (lightweight, survives retries via Clone)
///
/// Unlike Box<HttpPeer> which would be consumed on first use,
/// DirectEndpointPreset persists across retries so upstream_peer can
/// rebuild the same HttpPeer on each attempt.
#[derive(Debug, Clone)]
pub struct DirectEndpointPreset {
    /// Target socket address (ip:port)
    pub addr: std::net::SocketAddr,
    /// Policy captured using the same proven Service section as the address.
    pub tls_policy: crate::backends::BackendTlsPolicyLookup,
    /// Index of the matched backend_ref in route_unit.rule.backend_refs
    /// Used to pre-select the correct backend_ref for logging and metrics
    pub backend_ref_idx: usize,
}

/// Mutually-exclusive upstream-jump slot — exactly one variant lives per request.
///
/// - `Direct`: bypass LB, peer built from a static IP. Its validated backend
///   slot resolves through the request-pinned `HttpRouteRuleUnit` generation;
///   no backend schema is copied into request state.
/// - `Internal`: LB-input modifier. Dispatch falls through to `select_http_backend`,
///   which uses the carried preset to look up a named `BackendRef` before LB.
/// - `External`: bypass LB, peer built from a domain (DNS-resolved at runtime).
#[derive(Debug, Clone)]
pub enum UpstreamJumpTarget {
    Direct(DirectEndpointPreset),
    Internal(InternalJumpPreset),
    External(ExternalJumpPreset),
}

/// Outcome of `EdgionHttpContext::try_set_jump` / `PluginSession::set_upstream_jump`.
///
/// First-write-wins: once the slot is set, subsequent calls return `Rejected` and
/// the context is left untouched. Plugin call sites switch on this to log the
/// rejection on their own `PluginLog` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetJumpOutcome {
    Set,
    Rejected,
}

/// Snapshot of mirror config for request runtime.
/// Note: max_concurrent is managed by the Semaphore in RequestMirrorPlugin and
/// is NOT included here — the spawned task does not need it.
#[derive(Debug, Clone)]
pub struct MirrorConfig {
    pub connect_timeout: Duration,
    pub write_timeout: Duration,
    pub max_buffered_chunks: usize,
    pub mirror_log: bool,
}

/// Why a streaming mirror was cancelled before its body completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirrorAbandonReason {
    BodyAbandoned = 1,
    ChannelFull = 2,
    BufferLimit = 3,
}

/// Cancellation guard for an in-flight streaming mirror request.
///
/// Dropping an armed guard fails closed as `BodyAbandoned`. A real downstream
/// EOF must explicitly call [`Self::complete`] to disarm it. This makes future
/// early-return paths cancel the mirror instead of turning a delivered prefix
/// into a clean HTTP EOF.
pub struct MirrorCancellation {
    cancel_tx: Option<oneshot::Sender<()>>,
    reason: Arc<AtomicU8>,
}

impl MirrorCancellation {
    pub(crate) fn new() -> (Self, oneshot::Receiver<()>) {
        let (cancel_tx, cancel_rx) = oneshot::channel();
        (
            Self {
                cancel_tx: Some(cancel_tx),
                reason: Arc::new(AtomicU8::new(0)),
            },
            cancel_rx,
        )
    }

    pub(crate) fn reason_handle(&self) -> Arc<AtomicU8> {
        self.reason.clone()
    }

    pub(crate) fn cancel(mut self, reason: MirrorAbandonReason) {
        self.reason.store(reason as u8, Ordering::Release);
        if let Some(cancel_tx) = self.cancel_tx.take() {
            let _ = cancel_tx.send(());
        }
    }

    pub(crate) fn complete(mut self) {
        drop(self.cancel_tx.take());
    }

    pub(crate) fn load_reason(reason: &AtomicU8) -> Option<MirrorAbandonReason> {
        match reason.load(Ordering::Acquire) {
            1 => Some(MirrorAbandonReason::BodyAbandoned),
            2 => Some(MirrorAbandonReason::ChannelFull),
            3 => Some(MirrorAbandonReason::BufferLimit),
            _ => None,
        }
    }
}

impl Drop for MirrorCancellation {
    fn drop(&mut self) {
        if let Some(cancel_tx) = self.cancel_tx.take() {
            self.reason
                .store(MirrorAbandonReason::BodyAbandoned as u8, Ordering::Release);
            let _ = cancel_tx.send(());
        }
    }
}

/// Mirror stream state kept in request context.
pub enum MirrorState {
    /// Mirror request task is active and waiting for body chunks from request_body_filter.
    Streaming {
        /// Cancels the in-flight mirror request without ending its streamed
        /// body as a clean HTTP EOF. Keep this field first: Rust drops enum
        /// fields in declaration order, so implicit state destruction must
        /// signal cancellation before `body_tx` can yield a normal EOF.
        cancellation: MirrorCancellation,
        body_tx: mpsc::Sender<Result<Bytes, std::io::Error>>,
        /// Counts outstanding owned chunks until the transport releases them.
        body_budget: crate::plugins::http::request_mirror::MirrorBodyBudget,
        writer_handle: JoinHandle<()>,
        /// Maximum milliseconds to wait for channel space before abandoning the mirror.
        /// 0 = immediate abandon (zero impact on main request latency, default).
        /// > 0 = brief back-pressure window (adds at most this many ms to body processing).
        channel_full_timeout_ms: u64,
    },
    /// Defensive terminal input retained for adapter and cleanup compatibility.
    /// Runtime terminal paths retire mirror ownership from the request context.
    Abandoned,
}

/// A response-header mutation queued by a plugin in a stage where the real
/// `ResponseHeader` is not yet available (typically `request_filter`).
///
/// Applied non-destructively onto the forwarded header at the upstream-response
/// stage (`upstream_response_filter` via `apply_pending_response_header_ops`),
/// and drained destructively (`drain_pending_response_header_ops`) when building
/// a response-phase local reply (`build_response_local_reply`) and at
/// request-stage termination points that build their own response (`ErrResponse`,
/// gRPC errors).
#[derive(Debug, Clone)]
pub enum PendingHeaderOp {
    Set { name: String, value: String },
    Append { name: String, value: String },
    AddIfAbsent { name: String, value: String },
    OverwriteIfExists { name: String, value: String },
    Remove { name: String },
}

impl PendingHeaderOp {
    /// Apply this queued operation to a real ResponseHeader.
    ///
    /// `insert_header` / `append_header` errors (invalid header value bytes,
    /// e.g. control chars) are intentionally swallowed: dropping a malformed
    /// queued op silently is preferable to failing the entire response, and
    /// matches the behavior of the pre-tagged-queue implementation.
    pub fn apply(self, response_header: &mut ResponseHeader) {
        match self {
            PendingHeaderOp::Set { name, value } => {
                let _ = response_header.insert_header(name, value);
            }
            PendingHeaderOp::Append { name, value } => {
                let _ = response_header.append_header(name, value);
            }
            PendingHeaderOp::AddIfAbsent { name, value } => {
                if response_header.headers.get(&name).is_none() {
                    let _ = response_header.insert_header(name, value);
                }
            }
            PendingHeaderOp::OverwriteIfExists { name, value } => {
                if response_header.headers.get(&name).is_some() {
                    let _ = response_header.insert_header(name, value);
                }
            }
            PendingHeaderOp::Remove { name } => {
                response_header.remove_header(&name);
            }
        }
    }
}

/// One immutable credential header selected by request-stage plugin order.
///
/// The value is held in a prevalidated `HeaderValue` and is deliberately not
/// exposed through `Debug`, access-log state, or `ctx_map`.
#[derive(Clone)]
pub(crate) struct DeferredCredentialMutation {
    name: http::HeaderName,
    value: Arc<http::HeaderValue>,
    valid_until: Option<std::time::Instant>,
}

/// Request-scoped winning credential mutations, retained across upstream
/// retries and applied only to each attempt-local upstream request header.
#[derive(Clone, Default)]
pub(crate) struct DeferredCredentialMutations {
    entries: Vec<DeferredCredentialMutation>,
}

/// Frequently accessed request state shared by native HTTP plugins.
///
/// Plugin chains are a normal gateway workload, so their execution cursor,
/// logs, and request-pinned runtime generations remain inline. Empty vectors
/// and nullable `Arc`s do not allocate, while the inline layout avoids an
/// extra branch and heap indirection at every plugin callback.
struct PluginExecutionState {
    stage_logs: Vec<StageLogs>,
    pending_edgion_plugins_logs: Vec<EdgionPluginsLog>,
    terminated: bool,
    current_entry: Option<CurrentPluginEntryIdentity>,
    resolved_global_plugins: Option<Arc<[Arc<PluginRuntime>]>>,
    resolved_gateway_plugins: Option<Arc<[Arc<PluginRuntime>]>>,
    resolved_route_ext_refs: Option<Arc<[Option<Arc<PluginRuntime>>]>>,
}

impl Default for PluginExecutionState {
    fn default() -> Self {
        Self {
            stage_logs: Vec::new(),
            pending_edgion_plugins_logs: Vec::new(),
            terminated: false,
            current_entry: None,
            resolved_global_plugins: None,
            resolved_gateway_plugins: None,
            resolved_route_ext_refs: None,
        }
    }
}

/// Sparse plugin side effects and feature-specific caches.
///
/// These fields have low cross-plugin co-occurrence. Keeping them behind one
/// lazy pointer avoids charging every request for empty map/vector headers,
/// while a request using several advanced plugins still pays only one extra
/// state allocation.
#[derive(Default)]
struct PluginAuxState {
    ctx_map: HashMap<String, String>,
    pending_response_header_ops: Vec<PendingHeaderOp>,
    deferred_credential_mutations: DeferredCredentialMutations,
    current_dye_plan: Option<Arc<crate::plugins::runtime::CompiledDyePlan>>,
    dye_handled_request_names: Vec<String>,
    mirror_state: Option<MirrorState>,
    json_schema_validation_claimed: bool,
    resolved_backend_ext_refs: HashMap<String, Option<Arc<PluginRuntime>>>,
}

const REQUEST_BODY_SOURCE_HANDLER: u8 = 1 << 0;
const REQUEST_BODY_SOURCE_WAF: u8 = 1 << 1;
const REQUEST_BODY_SOURCE_OBSERVER: u8 = 1 << 2;
const REQUEST_BODY_SOURCE_MIRROR: u8 = 1 << 3;

pub(crate) type RequestBodyStreamHandlers =
    Vec<sync_wrapper::SyncWrapper<Box<dyn crate::plugins::runtime::traits::RequestBodyStreamHandler>>>;

/// Optional drivers attached to Pingora's per-chunk request-body callback.
///
/// Handler and WAF source bits are monotonic for the request lifetime and remain
/// set while handlers are temporarily moved across an await point, so
/// cancellation can never make a partially streamed request replayable again.
/// The mirror bit is retired after any terminal mirror outcome; observer state
/// remains installed for the rest of the body lifecycle.
#[derive(Default)]
struct RequestBodyPluginState {
    handlers: RequestBodyStreamHandlers,
    observers: Option<Box<crate::request_body::observer::RequestBodyObservers>>,
    active_sources: u8,
}

impl RequestBodyPluginState {
    #[inline]
    fn is_non_replayable(&self) -> bool {
        self.active_sources & (REQUEST_BODY_SOURCE_HANDLER | REQUEST_BODY_SOURCE_WAF) != 0
    }
}

/// Correlated state for ordinary response processors, ExtProc owners, and
/// trailer companions.
///
/// Terminal arbitration and response-write facts stay inline because AI and
/// ordinary streaming responses use them without installing this state.
struct ResponseProcessingState {
    stream_processors: Vec<Box<dyn StreamingResponseProcessor>>,
    trailer_companions: Vec<Box<dyn UpstreamResponseTrailer>>,
    processor_driver: Option<std::sync::Arc<crate::response_processor_driver::ResponseProcessorDriver>>,
    trailer_lifecycle: UpstreamTrailerLifecycle,
    attached_ext_proc: AttachedExtProcMap,
    framing_reservations: usize,
}

impl Default for ResponseProcessingState {
    fn default() -> Self {
        Self {
            stream_processors: Vec::new(),
            trailer_companions: Vec::new(),
            processor_driver: None,
            trailer_lifecycle: UpstreamTrailerLifecycle::Open,
            attached_ext_proc: HashMap::new(),
            framing_reservations: 0,
        }
    }
}

impl fmt::Debug for DeferredCredentialMutations {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeferredCredentialMutations")
            .field(
                "targets",
                &self.entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),
            )
            .field("count", &self.entries.len())
            .finish()
    }
}

impl DeferredCredentialMutations {
    pub(crate) fn upsert(
        &mut self,
        name: http::HeaderName,
        value: Arc<http::HeaderValue>,
        valid_until: Option<std::time::Instant>,
    ) {
        if let Some(existing) = self.entries.iter_mut().find(|entry| entry.name == name) {
            existing.value = value;
            existing.valid_until = valid_until;
            return;
        }
        self.entries.push(DeferredCredentialMutation {
            name,
            value,
            valid_until,
        });
    }

    pub(crate) fn remove(&mut self, name: &str) {
        self.entries
            .retain(|entry| !entry.name.as_str().eq_ignore_ascii_case(name));
    }

    pub(crate) fn contains_target(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.name.as_str().eq_ignore_ascii_case(name))
    }

    pub(crate) fn all_unexpired(&self, now: std::time::Instant) -> bool {
        self.entries
            .iter()
            .all(|entry| entry.valid_until.is_none_or(|deadline| now < deadline))
    }

    pub(crate) fn apply(&self, request: &mut pingora_http::RequestHeader) -> Result<(), ()> {
        for entry in &self.entries {
            request
                .insert_header(entry.name.clone(), entry.value.as_ref().clone())
                .map_err(|_| ())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod deferred_credential_mutation_tests {
    use super::DeferredCredentialMutations;
    use http::{HeaderName, HeaderValue};
    use std::sync::Arc;

    fn value(raw: &'static str) -> Arc<HeaderValue> {
        Arc::new(HeaderValue::from_static(raw))
    }

    #[test]
    fn later_value_replaces_same_target_and_removal_is_case_insensitive() {
        let mut mutations = DeferredCredentialMutations::default();
        mutations.upsert(HeaderName::from_static("authorization"), value("Bearer first"), None);
        mutations.upsert(HeaderName::from_static("authorization"), value("Bearer second"), None);

        let mut request = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
        mutations.apply(&mut request).unwrap();
        assert_eq!(request.headers.get("authorization").unwrap(), "Bearer second");

        mutations.remove("Authorization");
        let mut request = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
        mutations.apply(&mut request).unwrap();
        assert!(request.headers.get("authorization").is_none());
    }

    #[test]
    fn applying_to_each_attempt_is_non_destructive_and_debug_is_redacted() {
        let secret = "private-upstream-token";
        let mut mutations = DeferredCredentialMutations::default();
        let mut value = HeaderValue::from_str(secret).unwrap();
        value.set_sensitive(true);
        mutations.upsert(HeaderName::from_static("x-provider-token"), Arc::new(value), None);

        for _ in 0..2 {
            let mut request = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
            mutations.apply(&mut request).unwrap();
            let value = request.headers.get("x-provider-token").unwrap();
            assert_eq!(value, secret);
            assert!(value.is_sensitive());
            assert_eq!(format!("{value:?}"), "Sensitive");
            let request_debug = format!("{request:?}");
            assert!(request_debug.contains("Sensitive"));
            assert!(!request_debug.contains(secret));
        }

        let debug = format!("{mutations:?}");
        assert!(debug.contains("x-provider-token"));
        assert!(!debug.contains(secret));
    }

    #[test]
    fn preflight_rejects_any_expired_deferred_credential() {
        let now = std::time::Instant::now();
        let mut mutations = DeferredCredentialMutations::default();
        mutations.upsert(
            HeaderName::from_static("authorization"),
            value("Bearer current"),
            Some(now + std::time::Duration::from_secs(30)),
        );
        assert!(mutations.all_unexpired(now));

        mutations.upsert(
            HeaderName::from_static("x-provider-token"),
            value("expired"),
            Some(now - std::time::Duration::from_secs(1)),
        );
        assert!(!mutations.all_unexpired(now));
    }
}

/// Which upper bound won the read/write-timeout fold in
/// `configure_peer_timeouts` for the current upstream attempt.
/// Consumed by `error_while_proxy` to decide whether a timeout is a
/// per-try timeout (retryable, GEP-1731) or a deadline expiry (final).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerTimeoutSource {
    /// Route `timeouts.backendRequest`, or the global default fallback.
    BackendRequest,
    /// Route `timeouts.request` remaining budget.
    RequestDeadline,
    /// Client-supplied `grpc-timeout` remaining budget.
    GrpcDeadline,
}

/// Owner of the current attempt's connection-establishment bound, independent
/// of the backend request I/O budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectTimeoutSource {
    Configured,
    RequestDeadline,
    GrpcDeadline,
}

/// A response-phase plugin denial that the response stage could not deliver on
/// its own, carried to the abort path.
///
/// A denial the response stage cannot express is NOT permission to forward the
/// upstream response. The runners answer such a plugin result by calling
/// `set_terminate`, which only marks the common plugin request state — and
/// every reader of that marker is request-stage, so on its own it is a silent ALLOW.
/// Setting this instead makes the response-stage entry point abort the exchange,
/// and the abort is raised as `ErrorType::HTTPStatus`, the same mechanism the
/// retry-budget denial in `upstream_response_filter` already uses: the upstream
/// response is never forwarded, the upstream connection is torn down rather than
/// reused, and `fail_to_proxy` delivers the denial downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseStageDenial {
    /// `ErrResponse` on an exchange that is no longer a replaceable HTTP
    /// response — a real `101`, an established CONNECT tunnel, or a gRPC route
    /// (see `apply_response_local_reply` / `build_response_local_reply`).
    /// `fail_to_proxy`'s ordinary status handling writes it.
    Http(u16),

    /// `ErrGrpcError`, which names a gRPC status directly. The sync response
    /// runner cannot await, so it cannot write the trailers itself, and routing
    /// the code through an HTTP status would flatten it through a lossy mapping.
    /// `fail_to_proxy` writes these trailers verbatim.
    Grpc { status: u16, message: Option<String> },
}

/// Exceptional response replacement or denial state.
///
/// Kept behind one lazy box in [`EdgionHttpContext`] so ordinary requests pay
/// only for a nullable pointer and terminal outcomes remain mutually exclusive.
#[derive(Debug)]
enum ResponseOverride {
    /// A response-stage plugin built a complete replacement response that has
    /// not yet been written downstream.
    PendingLocalReply {
        header: Box<ResponseHeader>,
        body: bytes::Bytes,
    },

    /// A local reply header was written successfully. The body write may still
    /// fail, but generic proxy error handling must not emit a second response.
    LocalReplyWritten,

    /// The response-stage denial could not be represented as a local reply.
    Denied(ResponseStageDenial),
}

/// Classification of the upstream response as a streaming protocol. Set once
/// per upstream attempt in `pg_upstream_response_filter` from the normalized
/// Content-Type; reassigned on retry (a later attempt may answer with a
/// different media type). Explicit media types only — absence of
/// Content-Length or chunked framing never implies streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseStreamKind {
    /// Not a recognized streaming media type (the default).
    #[default]
    None,
    /// `text/event-stream` (Server-Sent Events).
    Sse,
}

/// Request-stage declaration and response-stage activation for the one
/// response-head Hold claimant.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ResponseHeadHoldClaim {
    pub(crate) limits: pingora_proxy::ResponseHeadHoldLimits,
    pub(crate) active: bool,
}

/// AI-only state allocated on first effective AI plugin action.
///
/// Keep this boxed from [`EdgionHttpContext`]: several payloads have large
/// inline representations even while wrapped in `Option`, especially
/// `AiAttemptPlan`. The box keeps ordinary HTTP requests independent of that
/// storage cost while retaining one allocation and one ownership boundary for
/// the complete AI exchange.
#[derive(Default)]
pub(crate) struct AiHttpState {
    /// Bounded request metadata and shared atomic usage counters. Contains no
    /// prompt or credential.
    pub(crate) request: Option<AiRequestContext>,
    /// Immutable canonical request and candidate snapshot for forwarding. The
    /// canonical body is never rendered by Debug or access logging.
    pub(crate) attempt_plan: Option<AiAttemptPlan>,
    /// Request-body generation observed by `AiGuard` when its decision settled.
    pub(crate) guard_body_generation: Option<u64>,
    /// Guard entries that already decided; prevents retry re-entry.
    pub(crate) guard_decided: Vec<String>,
    /// Active-attempt and pending Redis settlement state.
    pub(crate) attempt_runtime: crate::ai::attempt_runtime::AiAttemptRuntimeState,
    /// Request-captured typed destination policy for the selected AI backend.
    pub(crate) effective_policy: Option<Arc<crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy>>,
    /// Pristine post-rule request used to rebuild later AI attempts.
    pub(crate) request_baseline: Option<pingora_http::RequestHeader>,
    /// Replaceable provider-aware response processor for the active attempt.
    pub(crate) stream_processor: Option<Box<dyn StreamingResponseProcessor>>,
    /// Client-facing stream dialect selected at canonical AI ingress.
    pub(crate) client_stream_dialect: Option<AiClientStreamDialect>,
    /// Final method/status/media eligibility captured after the response onion.
    pub(crate) final_response_eligibility: Option<FinalResponseEligibility>,
    /// Immutable GenAI usage summary committed once during final logging.
    pub(crate) summary: Option<GenAiSummary>,
    /// Whether a semantic response guard claimed the canonical AI source.
    pub(crate) semantic_guard_claimed: bool,
    /// The single request-scoped semantic Guardrail owner and its compiled
    /// request-entry identity. Multiple matched entries are rejected.
    pub(crate) attached_guardrail: Option<(
        String,
        std::sync::Arc<crate::plugins::http::guardrail::AttachedGuardrail>,
    )>,
    /// Installed before cache lookup and activated only after the final
    /// response onion installs Guardrail's semantic processor.
    pub(crate) response_head_hold_claim: Option<ResponseHeadHoldClaim>,
    /// Same-callback handoff from the canonical source to the semantic guard.
    pub(crate) canonical_record_batch: Option<crate::ai::canonical_sse::CanonicalRecordBatch>,
    /// Canonical semantic guard, run after the AI source processor.
    pub(crate) semantic_processor: Option<Box<dyn StreamingResponseProcessor>>,
}

pub struct EdgionHttpContext {
    // --- Request lifecycle and identity ---
    /// Request start time for latency calculation
    pub start_time: Instant,

    /// Request-stable Gateway/listener generation selected during route match.
    /// Route misses retain only the empty pointer-sized option.
    pub gateway_info: Option<Arc<GatewayInfo>>,

    /// Request information (hostname, path, W3C and correlation IDs)
    pub request_info: RequestInfo,
    pub(crate) global_request_ids_initialized: bool,
    pub(crate) gateway_request_ids_initialized: bool,

    /// Ordered, bounded information and error diagnostics for the request.
    pub diagnostics: RequestDiagnostics,

    // --- Route and backend selection ---
    /// Matched HTTP route unit containing full route information
    pub route_unit: Option<Arc<HttpRouteRuleUnit>>,

    /// Selected HTTP backend from load balancing (schema + compiled plugin runtime)
    pub selected_backend: Option<SelectedHttpBackend>,

    /// Matched gRPC route unit (for gRPC routes)
    pub grpc_route_unit: Option<Arc<GrpcRouteRuleUnit>>,

    /// Request-stable gRPC backend generation/slot and captured TLS policy.
    pub selected_grpc_backend: Option<SelectedGrpcBackend>,

    /// Whether this request is handled by GRPCRoute (not just gRPC protocol)
    /// Used to determine backend peer selection and plugin execution
    pub is_grpc_route_matched: bool,

    /// Backend context containing service info and upstream attempts
    pub backend_context: Option<BackendContext>,

    // --- Plugin execution and access-log staging ---
    /// Hot plugin-chain state. It remains allocation-free while empty and is
    /// inline because plugin execution is a common gateway workload.
    plugin_execution: PluginExecutionState,

    /// Sparse plugin effects and feature-specific caches. Ordinary plugin
    /// chains do not allocate this box.
    plugin_aux: Option<Box<PluginAuxState>>,

    /// The sole native WAF state for this request: selected owner, exactly-once
    /// phase claims, transaction, streaming state, timing, and memory charge.
    /// Ordinary requests retain only this pointer-sized empty option.
    pub(crate) waf: Option<Box<WafRequestContext>>,

    /// Whether request-wide WAF winner selection has completed. Kept separate
    /// from the nullable WAF box so a completed no-winner result remains final
    /// without allocating a sentinel box.
    waf_selection_complete: bool,

    // --- Upstream attempts and retry bookkeeping ---
    /// Number of connection attempts to backends
    pub try_cnt: u32,

    /// Signal set by upstream_response_filter when upstream status code matches retry.codes.
    /// Consumed by error_while_proxy to trigger retry via e.set_retry(true).
    /// Reset to false after each retry attempt.
    pub retry_on_status: bool,

    /// Source of the read/write timeout applied to the current upstream
    /// attempt's peer. Overwritten by `configure_peer_timeouts` on every
    /// attempt; `None` when no read/write bound was applied.
    pub peer_timeout_source: Option<PeerTimeoutSource>,

    /// Replaced at every peer configuration, including when all bounds are disabled.
    pub(crate) connect_timeout_source: Option<ConnectTimeoutSource>,

    /// Circuit-breaker admission permit for the active upstream attempt,
    /// acquired at `upstream_peer` (pre-connect) when the target backend carries
    /// a `circuitBreaker` concurrency cap. It is released before the next attempt
    /// or when the request context drops, covering every terminal path (success,
    /// upstream error, client disconnect, cancellation). `None` when no cap
    /// applies or admission was not attempted (bypass paths, no policy).
    pub cb_permit: Option<crate::backends::circuit_breaker::CbPermit>,

    /// Time when first upstream connection was initiated
    /// Set only once on first connection attempt
    pub upstream_start_time: Option<Instant>,

    /// Hash key used for consistent hashing (for test metrics)
    pub hash_key: Option<String>,

    // --- Request-scoped URI and routing state ---
    /// Composed upstream authority for this request, when the target Service
    /// carries an `upstreamAuthority` policy. Written in `upstream_peer` (which
    /// also sets it as the peer SNI) and read in `upstream_request_filter`, which
    /// is the only place the upstream `Host` header can be written.
    pub upstream_authority: Option<String>,

    /// Compact path/query caches. The two cells preserve immutable query access
    /// required by route matching without a shared outer pointer check. Path
    /// values are frozen from the normalized route-match view before plugins;
    /// query values always use the frozen entry snapshot.
    parsed_request_caches: ParsedRequestCaches,

    /// Unified upstream-jump slot. The 72-byte target is boxed because only
    /// explicit jump plugins activate it; ordinary routing retains one nullable
    /// pointer. First-write-wins and the target survives upstream retries.
    pub jump: Option<Box<UpstreamJumpTarget>>,

    /// True when `req_header.uri` is already in its upstream-forwarded final form;
    /// further writes are forbidden.
    ///
    /// Set in exactly two locations (matching the §9 Rule 2 lint allowlist):
    ///   1. `session_adapter::set_upstream_uri` — set after rewrite-class plugin writes
    ///   2. `pg_upstream_peer::upstream_peer_http` single-point decision block —
    ///      set after the default-normalized vs raw decision is finalized
    ///
    /// Set at most once per request lifecycle. Covers:
    ///   - rewrite plugin "final form" semantics
    ///   - single-point decision idempotency (retries don't re-decide)
    ///   - mirror / fan-out consumers see consistent `req_header.uri`
    pub uri_finalized: bool,

    // --- Optional route state ---
    /// Lazily allocated state for routes that enable session persistence.
    /// Ordinary routes retain only this nullable pointer.
    pub(crate) session_persistence: Option<Box<SessionPersistenceContext>>,

    /// Per-Gateway operator override for loopback upstream addresses.
    /// Link-local / metadata ranges are never allowed by this switch.
    pub allow_loopback_upstream: bool,

    // --- Request-body policy, buffering, and streaming ---
    /// Resolved GatewayClass request-body policy.
    /// Parsed when the listener is built; the request hot path only copies and
    /// reads these numeric values.
    pub request_body_config: crate::request_body::ResolvedRequestBodyConfig,

    /// Validated upstream header carrying the final RemoteIp. Captured from the
    /// immutable EGC runtime snapshot when this request context is created, so
    /// an in-flight request is unaffected by a later config update.
    pub remote_ip_header: http::HeaderName,

    /// Full request-body cache and mutation framing state. Its read-only view
    /// stays available through the final upstream-response header plugin chain
    /// and is then released before streaming the response.
    pub request_body: RequestBodyState,

    /// Lazily allocated request-body callback drivers. The outer state is
    /// small; the fixed-capacity observer registry remains independently boxed
    /// inside it because that registry is more than two KiB on ARM64.
    request_body_plugins: Option<Box<RequestBodyPluginState>>,

    // --- Lazily allocated AI exchange state ---
    /// Lazily allocated AI-only request, attempt, response, and accounting
    /// state. Ordinary HTTP requests retain only this nullable pointer.
    pub(crate) ai_state: Option<Box<AiHttpState>>,

    // --- Response-stream processors and terminal arbitration ---
    /// First-wins request-local terminal and close ownership.
    pub(crate) response_terminal_state: ResponseTerminalState,

    /// Normalized, accepted response status captured immediately before the
    /// response plugin chain. Never serialized.
    pub(crate) pre_plugin_response_status: Option<u16>,

    /// Replaceable downstream status candidate selected before any writer or
    /// terminal outcome owns the request.
    candidate_downstream_status: Option<u16>,

    /// Downstream status owned by a writer claim or terminal selection. Private:
    /// writers must use the guarded lifecycle methods below.
    final_downstream_status: Option<u16>,

    /// Whether a final response writer has claimed ownership. This is not
    /// successful-write evidence; Pingora `response_written()` owns that fact.
    response_write_started: bool,

    /// Lazily allocated ordinary response processors, ExtProc owners, framing
    /// reservations, and trailer companions. AI processors and terminal
    /// arbitration remain in their existing hot state.
    response_processing: Option<Box<ResponseProcessingState>>,

    /// EOS fact from the final upstream response Header task. This is latched
    /// before response plugins run so protocol owners can decide whether a
    /// header verdict is the final outbound message without guessing from
    /// body-mode configuration.
    pub(crate) upstream_response_headers_end_stream: Option<bool>,

    /// Becomes true when the successful final-response filter freezes writable
    /// processors. A body/trailer callback before this boundary fails closed;
    /// late installation is rejected to keep composition deterministic.
    pub response_body_processing_started: bool,

    // --- Response replacement, denial, and final classification ---
    /// Lazily allocated, mutually exclusive response replacement/denial state.
    /// Also records request-body handlers that directly wrote a local reply.
    response_override: Option<Box<ResponseOverride>>,

    /// See [`ResponseStreamKind`]. Consumed by the compression exemption and by
    /// streaming processors/plugins that must know how to interpret chunks.
    pub response_stream_kind: ResponseStreamKind,
}

/// Parse a `RequestInfo` address field as an `IpAddr`, tolerating either a
/// bare IP or a `SocketAddr` (`ip:port`). Mirrors
/// `real_ip_extractor::extract_ip_from_socket_addr`'s try-`SocketAddr`-then-
/// `IpAddr` fallback, but returns `None` on failure instead of defaulting to
/// `0.0.0.0` — an unparseable geo input must become `GeoLookup::NotFound`,
/// never a lookup against a bogus address.
fn parse_ip_str(addr: &str) -> Option<std::net::IpAddr> {
    addr.parse::<std::net::SocketAddr>()
        .map(|sa| sa.ip())
        .or_else(|_| addr.parse::<std::net::IpAddr>())
        .ok()
}

/// Return one canonical textual form for a downstream IP identity.
///
/// IPv4-mapped IPv6 addresses are collapsed to native IPv4 so policy keys,
/// logs, templates, and GeoIP all observe the same host spelling.
pub(crate) fn canonicalize_ip(ip: std::net::IpAddr) -> String {
    match ip {
        std::net::IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(std::net::IpAddr::V4)
            .unwrap_or(std::net::IpAddr::V6(v6))
            .to_string(),
        std::net::IpAddr::V4(v4) => v4.to_string(),
    }
}

pub(crate) fn canonicalize_ip_str(value: &str) -> Option<String> {
    value.parse::<std::net::IpAddr>().ok().map(canonicalize_ip)
}

impl Default for EdgionHttpContext {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgionHttpContext {
    #[inline]
    fn plugin_aux(&self) -> Option<&PluginAuxState> {
        self.plugin_aux.as_deref()
    }

    #[inline]
    fn plugin_aux_mut(&mut self) -> Option<&mut PluginAuxState> {
        self.plugin_aux.as_deref_mut()
    }

    #[inline]
    fn ensure_plugin_aux(&mut self) -> &mut PluginAuxState {
        self.plugin_aux
            .get_or_insert_with(|| Box::new(PluginAuxState::default()))
            .as_mut()
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn has_plugin_aux(&self) -> bool {
        self.plugin_aux.is_some()
    }

    #[inline]
    pub(crate) fn stage_logs(&self) -> &[StageLogs] {
        self.plugin_execution.stage_logs.as_slice()
    }

    #[inline]
    pub(crate) fn stage_logs_mut(&mut self) -> &mut Vec<StageLogs> {
        &mut self.plugin_execution.stage_logs
    }

    pub(crate) fn stage_logs_and_attached_ext_proc(&mut self) -> Option<(&mut Vec<StageLogs>, &AttachedExtProcMap)> {
        let Self {
            plugin_execution,
            response_processing,
            ..
        } = self;
        Some((
            &mut plugin_execution.stage_logs,
            &response_processing.as_deref()?.attached_ext_proc,
        ))
    }

    #[inline]
    pub(crate) fn push_stage_log(&mut self, log: StageLogs) {
        self.plugin_execution.stage_logs.push(log);
    }

    pub(crate) fn finish_plugin_stage(&mut self, stage: &'static str, filters: Vec<crate::plugins::PluginLog>) {
        let state = &mut self.plugin_execution;
        if filters.is_empty() && state.pending_edgion_plugins_logs.is_empty() {
            return;
        }
        state.stage_logs.push(StageLogs {
            stage,
            filters,
            edgion_plugins: std::mem::take(&mut state.pending_edgion_plugins_logs),
        });
    }

    #[inline]
    pub(crate) fn push_pending_edgion_plugins_log(&mut self, log: EdgionPluginsLog) {
        self.plugin_execution.pending_edgion_plugins_logs.push(log);
    }

    #[inline]
    pub(crate) fn start_pending_edgion_plugins_log(&mut self, name: String) -> usize {
        let state = &mut self.plugin_execution;
        let idx = state.pending_edgion_plugins_logs.len();
        state
            .pending_edgion_plugins_logs
            .push(EdgionPluginsLog { name, logs: Vec::new() });
        idx
    }

    #[inline]
    pub(crate) fn pending_edgion_plugins_log_mut(&mut self, index: usize) -> Option<&mut EdgionPluginsLog> {
        self.plugin_execution.pending_edgion_plugins_logs.get_mut(index)
    }

    #[inline]
    pub(crate) fn plugin_terminated(&self) -> bool {
        self.plugin_execution.terminated
    }

    #[inline]
    pub(crate) fn terminate_plugin_execution(&mut self) {
        self.plugin_execution.terminated = true;
    }

    #[inline]
    pub(crate) fn replace_plugin_entry_identity(
        &mut self,
        identity: Option<CurrentPluginEntryIdentity>,
    ) -> Option<CurrentPluginEntryIdentity> {
        std::mem::replace(&mut self.plugin_execution.current_entry, identity)
    }

    #[inline]
    pub(crate) fn current_plugin_entry_identity(&self) -> Option<CurrentPluginEntryIdentity> {
        self.plugin_execution.current_entry
    }

    #[inline]
    pub(crate) fn queue_pending_response_header_op(&mut self, op: PendingHeaderOp) {
        self.ensure_plugin_aux().pending_response_header_ops.push(op);
    }

    #[inline]
    pub(crate) fn has_pending_response_header_ops(&self) -> bool {
        self.plugin_aux()
            .is_some_and(|state| !state.pending_response_header_ops.is_empty())
    }

    #[inline]
    pub(crate) fn pending_response_header_ops(&self) -> &[PendingHeaderOp] {
        self.plugin_aux()
            .map_or(&[], |state| state.pending_response_header_ops.as_slice())
    }

    #[inline]
    pub(crate) fn deferred_credential_mutations(&self) -> Option<&DeferredCredentialMutations> {
        self.plugin_aux().map(|state| &state.deferred_credential_mutations)
    }

    #[inline]
    pub(crate) fn deferred_credentials_all_unexpired(&self, now: std::time::Instant) -> bool {
        self.deferred_credential_mutations()
            .is_none_or(|mutations| mutations.all_unexpired(now))
    }

    #[inline]
    pub(crate) fn apply_deferred_credentials(&self, request: &mut pingora_http::RequestHeader) -> Result<(), ()> {
        self.deferred_credential_mutations()
            .map_or(Ok(()), |mutations| mutations.apply(request))
    }

    #[inline]
    pub(crate) fn remove_deferred_credential_mutation(&mut self, name: &str) {
        if let Some(state) = self.plugin_aux_mut() {
            state.deferred_credential_mutations.remove(name);
        }
    }

    #[inline]
    pub(crate) fn upsert_deferred_credential_mutation(
        &mut self,
        name: http::HeaderName,
        value: Arc<http::HeaderValue>,
        valid_until: Option<std::time::Instant>,
    ) {
        self.ensure_plugin_aux()
            .deferred_credential_mutations
            .upsert(name, value, valid_until);
    }

    #[inline]
    pub(crate) fn install_current_dye_plan(&mut self, plan: Option<Arc<crate::plugins::runtime::CompiledDyePlan>>) {
        if plan.is_none() && self.plugin_aux.is_none() {
            return;
        }
        self.ensure_plugin_aux().current_dye_plan = plan;
    }

    #[inline]
    pub(crate) fn current_dye_plan(&self) -> Option<Arc<crate::plugins::runtime::CompiledDyePlan>> {
        self.plugin_aux()
            .and_then(|state| state.current_dye_plan.as_ref().map(Arc::clone))
    }

    #[inline]
    pub(crate) fn take_dye_handled_request_names(&mut self) -> Vec<String> {
        self.plugin_aux_mut()
            .map_or_else(Vec::new, |state| std::mem::take(&mut state.dye_handled_request_names))
    }

    #[inline]
    pub(crate) fn restore_dye_handled_request_names(&mut self, names: Vec<String>) {
        if names.is_empty() && self.plugin_aux.is_none() {
            return;
        }
        self.ensure_plugin_aux().dye_handled_request_names = names;
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn dye_handled_request_names(&self) -> &[String] {
        self.plugin_aux()
            .map_or(&[], |state| state.dye_handled_request_names.as_slice())
    }

    #[inline]
    pub(crate) fn has_mirror_state(&self) -> bool {
        self.plugin_aux().is_some_and(|state| state.mirror_state.is_some())
    }

    #[inline]
    pub(crate) fn take_mirror_state(&mut self) -> Option<MirrorState> {
        self.plugin_aux_mut()?.mirror_state.take()
    }

    #[inline]
    pub(crate) fn set_mirror_state(&mut self, state: MirrorState) {
        if matches!(&state, MirrorState::Streaming { .. }) {
            self.activate_request_body_source(REQUEST_BODY_SOURCE_MIRROR);
        }
        self.ensure_plugin_aux().mirror_state = Some(state);
    }

    /// Remove terminal mirror ownership without disturbing co-resident request
    /// body consumers. The outer optional state is released only when mirror
    /// was its sole source, restoring the no-body-plugin fast path.
    #[inline]
    pub(crate) fn retire_request_mirror(&mut self) {
        if let Some(aux) = self.plugin_aux_mut() {
            aux.mirror_state = None;
        }

        let remove_request_body_state = self.request_body_plugins_mut().is_some_and(|state| {
            state.active_sources &= !REQUEST_BODY_SOURCE_MIRROR;
            state.active_sources
                & (REQUEST_BODY_SOURCE_HANDLER | REQUEST_BODY_SOURCE_WAF | REQUEST_BODY_SOURCE_OBSERVER)
                == 0
        });
        if remove_request_body_state {
            self.request_body_plugins = None;
        }
    }

    #[inline]
    pub(crate) fn install_resolved_global_plugins(&mut self, plugins: Vec<Arc<PluginRuntime>>) {
        if !plugins.is_empty() {
            self.plugin_execution.resolved_global_plugins = Some(Arc::from(plugins));
        }
    }

    #[inline]
    pub(crate) fn resolved_global_plugins(&self) -> Option<Arc<[Arc<PluginRuntime>]>> {
        self.plugin_execution.resolved_global_plugins.as_ref().map(Arc::clone)
    }

    #[inline]
    pub(crate) fn install_resolved_gateway_plugins(&mut self, plugins: Vec<Arc<PluginRuntime>>) {
        if !plugins.is_empty() {
            self.plugin_execution.resolved_gateway_plugins = Some(Arc::from(plugins));
        }
    }

    #[inline]
    pub(crate) fn resolved_gateway_plugins(&self) -> Option<Arc<[Arc<PluginRuntime>]>> {
        self.plugin_execution.resolved_gateway_plugins.as_ref().map(Arc::clone)
    }

    #[inline]
    pub(crate) fn install_resolved_route_ext_refs(&mut self, plugins: Vec<Option<Arc<PluginRuntime>>>) {
        if !plugins.is_empty() {
            self.plugin_execution.resolved_route_ext_refs = Some(Arc::from(plugins));
        }
    }

    #[inline]
    pub(crate) fn resolved_route_ext_refs(&self) -> Option<Arc<[Option<Arc<PluginRuntime>>]>> {
        self.plugin_execution.resolved_route_ext_refs.as_ref().map(Arc::clone)
    }

    #[inline]
    pub(crate) fn resolved_route_ext_ref(&self, index: usize) -> Option<Arc<PluginRuntime>> {
        self.plugin_execution
            .resolved_route_ext_refs
            .as_ref()?
            .get(index)?
            .as_ref()
            .map(Arc::clone)
    }

    pub(crate) fn pinned_backend_ext_ref(
        &mut self,
        key: &str,
        resolve: impl FnOnce() -> Option<Arc<PluginRuntime>>,
    ) -> Option<Arc<PluginRuntime>> {
        let cache = &mut self.ensure_plugin_aux().resolved_backend_ext_refs;
        if let Some(runtime) = cache.get(key) {
            return runtime.as_ref().map(Arc::clone);
        }
        let runtime = resolve();
        cache.insert(key.to_string(), runtime.as_ref().map(Arc::clone));
        runtime
    }

    #[inline]
    fn request_body_plugins_mut(&mut self) -> Option<&mut RequestBodyPluginState> {
        self.request_body_plugins.as_deref_mut()
    }

    #[inline]
    fn ensure_request_body_plugins(&mut self) -> &mut RequestBodyPluginState {
        self.request_body_plugins
            .get_or_insert_with(|| Box::new(RequestBodyPluginState::default()))
            .as_mut()
    }

    #[inline]
    fn activate_request_body_source(&mut self, source: u8) {
        self.ensure_request_body_plugins().active_sources |= source;
    }

    #[inline]
    pub(crate) fn has_request_body_plugins(&self) -> bool {
        self.request_body_plugins.is_some()
    }

    /// Whether request bytes have entered an unreplayable streaming consumer.
    /// This remains true even while the handler vector is moved across await.
    #[inline]
    pub(crate) fn request_body_stream_active(&self) -> bool {
        self.request_body_plugins
            .as_deref()
            .is_some_and(RequestBodyPluginState::is_non_replayable)
    }

    #[inline]
    pub(crate) fn activate_waf_request_body_streaming(&mut self) {
        self.activate_request_body_source(REQUEST_BODY_SOURCE_WAF);
    }

    #[inline]
    pub(crate) fn push_request_body_stream_handler(
        &mut self,
        handler: Box<dyn crate::plugins::runtime::traits::RequestBodyStreamHandler>,
    ) {
        let state = self.ensure_request_body_plugins();
        state.handlers.push(sync_wrapper::SyncWrapper::new(handler));
        state.active_sources |= REQUEST_BODY_SOURCE_HANDLER;
    }

    #[inline]
    pub(crate) fn has_request_body_stream_handlers(&self) -> bool {
        self.request_body_plugins
            .as_deref()
            .is_some_and(|state| !state.handlers.is_empty())
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn request_body_stream_handler_count(&self) -> usize {
        self.request_body_plugins
            .as_deref()
            .map_or(0, |state| state.handlers.len())
    }

    #[inline]
    pub(crate) fn take_request_body_stream_handlers(&mut self) -> RequestBodyStreamHandlers {
        self.request_body_plugins_mut()
            .map_or_else(Vec::new, |state| std::mem::take(&mut state.handlers))
    }

    #[inline]
    pub(crate) fn restore_request_body_stream_handlers(&mut self, handlers: RequestBodyStreamHandlers) {
        if let Some(state) = self.request_body_plugins_mut() {
            state.handlers = handlers;
        }
    }

    #[inline]
    pub(crate) fn request_body_observers_are_full(&self) -> bool {
        self.request_body_plugins
            .as_deref()
            .and_then(|state| state.observers.as_deref())
            .is_some_and(crate::request_body::observer::RequestBodyObservers::is_full)
    }

    #[inline]
    pub(crate) fn request_body_observers_mut(
        &mut self,
    ) -> Option<&mut crate::request_body::observer::RequestBodyObservers> {
        self.request_body_plugins_mut()?.observers.as_deref_mut()
    }

    #[inline]
    pub(crate) fn install_request_body_observers(
        &mut self,
        observers: Box<crate::request_body::observer::RequestBodyObservers>,
    ) {
        let state = self.ensure_request_body_plugins();
        state.observers = Some(observers);
        state.active_sources |= REQUEST_BODY_SOURCE_OBSERVER;
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn has_request_body_observers(&self) -> bool {
        self.request_body_plugins
            .as_deref()
            .is_some_and(|state| state.observers.is_some())
    }

    #[inline]
    fn response_processing(&self) -> Option<&ResponseProcessingState> {
        self.response_processing.as_deref()
    }

    #[inline]
    fn response_processing_mut(&mut self) -> Option<&mut ResponseProcessingState> {
        self.response_processing.as_deref_mut()
    }

    #[inline]
    fn ensure_response_processing(&mut self) -> &mut ResponseProcessingState {
        self.response_processing
            .get_or_insert_with(|| Box::new(ResponseProcessingState::default()))
            .as_mut()
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn has_response_processing(&self) -> bool {
        self.response_processing.is_some()
    }

    #[inline]
    pub(crate) fn push_response_stream_processor(&mut self, processor: Box<dyn StreamingResponseProcessor>) -> bool {
        if self.response_body_processing_started {
            return false;
        }
        self.ensure_response_processing().stream_processors.push(processor);
        true
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn has_response_stream_processors(&self) -> bool {
        self.response_processing().is_some_and(|state| {
            !state.stream_processors.is_empty()
                || state
                    .processor_driver
                    .as_ref()
                    .is_some_and(|driver| driver.has_body_processors())
        })
    }

    #[inline]
    #[cfg(test)]
    pub(crate) fn response_stream_processor_count(&self) -> usize {
        self.response_processing().map_or(0, |state| {
            state
                .processor_driver
                .as_ref()
                .map_or(state.stream_processors.len(), |driver| driver.ordinary_count())
        })
    }

    #[inline]
    pub(crate) fn response_stream_processors_mut(&mut self) -> Option<&mut [Box<dyn StreamingResponseProcessor>]> {
        Some(self.response_processing_mut()?.stream_processors.as_mut_slice())
    }

    #[inline]
    pub(crate) fn response_stream_processors_change_body_length(&self) -> bool {
        self.response_processing().is_some_and(|state| {
            state.processor_driver.as_ref().map_or_else(
                || {
                    state
                        .stream_processors
                        .iter()
                        .any(|processor| processor.changes_body_length())
                },
                |driver| driver.body_changes_length(),
            )
        })
    }

    /// Freeze the final response's writable processors into one request-local
    /// owner. The empty fast path only flips the inline latch.
    pub(crate) fn freeze_response_processor_driver(&mut self) {
        if self.response_body_processing_started {
            return;
        }
        self.response_body_processing_started = true;

        let (ai, semantic) = match self.ai_state_mut() {
            Some(state) => (state.stream_processor.take(), state.semantic_processor.take()),
            None => (None, None),
        };
        let (ordinary, trailers) = match self.response_processing_mut() {
            Some(state) => (
                std::mem::take(&mut state.stream_processors),
                std::mem::take(&mut state.trailer_companions),
            ),
            None => (Vec::new(), Vec::new()),
        };
        if let Some(driver) =
            crate::response_processor_driver::ResponseProcessorDriver::new(ai, semantic, ordinary, trailers)
        {
            self.ensure_response_processing().processor_driver = Some(driver);
        }
    }

    #[inline]
    pub(crate) fn response_processors_frozen(&self) -> bool {
        self.response_body_processing_started
    }

    #[inline]
    pub(crate) fn response_processor_driver(
        &self,
    ) -> Option<std::sync::Arc<crate::response_processor_driver::ResponseProcessorDriver>> {
        self.response_processing()?.processor_driver.clone()
    }

    #[inline]
    pub(crate) fn reserve_response_framing(&mut self) {
        let state = self.ensure_response_processing();
        state.framing_reservations = state.framing_reservations.saturating_add(1);
    }

    #[inline]
    pub(crate) fn has_response_framing_reservation(&self) -> bool {
        self.response_processing()
            .is_some_and(|state| state.framing_reservations > 0)
    }

    pub(crate) fn install_attached_ext_proc(
        &mut self,
        identity: String,
        owner: Arc<crate::plugins::http::ext_proc::AttachedExtProc>,
        reserve_framing: bool,
    ) -> bool {
        if self
            .response_processing()
            .is_some_and(|state| state.attached_ext_proc.contains_key(&identity))
        {
            return false;
        }
        let state = self.ensure_response_processing();
        state.attached_ext_proc.insert(identity, owner);
        if reserve_framing {
            state.framing_reservations = state.framing_reservations.saturating_add(1);
        }
        true
    }

    #[inline]
    pub(crate) fn attached_ext_proc(
        &self,
        identity: &str,
    ) -> Option<Arc<crate::plugins::http::ext_proc::AttachedExtProc>> {
        self.response_processing()?.attached_ext_proc.get(identity).cloned()
    }

    #[inline]
    pub(crate) fn install_upstream_response_trailer(&mut self, processor: Box<dyn UpstreamResponseTrailer>) -> bool {
        if self.response_body_processing_started
            || self
                .response_processing()
                .is_some_and(|state| !state.trailer_lifecycle.is_open())
        {
            return false;
        }
        self.ensure_response_processing().trailer_companions.push(processor);
        true
    }

    #[inline]
    pub(crate) fn upstream_response_trailer_count(&self) -> usize {
        self.response_processing().map_or(0, |state| {
            state
                .processor_driver
                .as_ref()
                .map_or(state.trailer_companions.len(), |driver| driver.trailer_count())
        })
    }

    #[inline]
    pub(crate) fn claim_upstream_response_trailers(&mut self, present: bool) -> bool {
        let Some(state) = self.response_processing_mut() else {
            return false;
        };
        if present {
            state.trailer_lifecycle.claim_real_trailers()
        } else {
            state.trailer_lifecycle.claim_without_trailers()
        }
    }

    #[inline]
    pub(crate) fn mark_upstream_before_trailers(&mut self) -> bool {
        self.response_processing_mut()
            .is_some_and(|state| state.trailer_lifecycle.mark_before_trailers())
    }

    #[inline]
    pub(crate) fn claim_upstream_trailer_cleanup(&mut self) -> bool {
        self.response_processing_mut()
            .is_some_and(|state| state.trailer_lifecycle.claim_cleanup())
    }

    #[inline]
    pub(crate) fn upstream_trailer_lifecycle_is_open(&self) -> bool {
        self.response_processing()
            .is_none_or(|state| state.trailer_lifecycle.is_open())
    }

    #[inline]
    pub(crate) fn upstream_response_trailers_mut(&mut self) -> Option<&mut [Box<dyn UpstreamResponseTrailer>]> {
        Some(self.response_processing_mut()?.trailer_companions.as_mut_slice())
    }

    pub async fn replace_request_body_slice(
        &mut self,
        range: std::ops::Range<usize>,
        replacement: Bytes,
    ) -> crate::plugins::runtime::traits::session::BodyReadResult {
        self.request_body.replace_slice(range, replacement).await
    }

    pub fn overwrite_request_body(
        &mut self,
        body: Bytes,
    ) -> Result<(), crate::plugins::runtime::traits::session::BodyReadError> {
        self.request_body.overwrite(body)
    }

    /// Claim the one JSON Schema validation slot for this request.
    ///
    /// Returns `true` only to the first caller.
    pub(crate) fn claim_json_schema_validation(&mut self) -> bool {
        if self
            .plugin_aux()
            .is_some_and(|state| state.json_schema_validation_claimed)
        {
            false
        } else {
            self.ensure_plugin_aux().json_schema_validation_claimed = true;
            true
        }
    }

    #[inline]
    pub(crate) fn session_persistence(&self) -> Option<&SessionPersistenceContext> {
        self.session_persistence.as_deref()
    }

    #[inline]
    pub(crate) fn session_persistence_mut(&mut self) -> Option<&mut SessionPersistenceContext> {
        self.session_persistence.as_deref_mut()
    }

    #[inline]
    pub(crate) fn ensure_session_persistence(&mut self) -> &mut SessionPersistenceContext {
        self.session_persistence
            .get_or_insert_with(|| Box::new(SessionPersistenceContext::default()))
            .as_mut()
    }

    pub(crate) fn try_set_ai_request_context(&mut self, context: AiRequestContext) -> bool {
        let state = self.ensure_ai_state();
        if state.request.is_some() {
            false
        } else {
            state.request = Some(context);
            state.client_stream_dialect = Some(AiClientStreamDialect::OpenAiCompatibleSse);
            true
        }
    }

    /// Replace attempt-local provider/usage state while retaining the one
    /// request-wide quota handle across AI failover attempts.
    pub(crate) fn replace_ai_attempt_context(&mut self, mut context: AiRequestContext) {
        let state = self.ensure_ai_state();
        state.client_stream_dialect = Some(AiClientStreamDialect::OpenAiCompatibleSse);
        if let Some(mut previous) = state.request.take() {
            if context.quota.is_none() {
                context.quota = previous.quota.take();
            }
            if context.quota_status.is_none() {
                context.quota_status = previous.quota_status;
            }
        }
        state.request = Some(context);
    }

    pub(crate) fn try_set_ai_attempt_plan(&mut self, plan: AiAttemptPlan) -> bool {
        let state = self.ensure_ai_state();
        if state.attempt_plan.is_some() {
            false
        } else {
            state.attempt_plan = Some(plan);
            true
        }
    }

    #[inline]
    pub(crate) fn ai_state(&self) -> Option<&AiHttpState> {
        self.ai_state.as_deref()
    }

    #[inline]
    pub(crate) fn ai_state_mut(&mut self) -> Option<&mut AiHttpState> {
        self.ai_state.as_deref_mut()
    }

    #[inline]
    pub(crate) fn ensure_ai_state(&mut self) -> &mut AiHttpState {
        self.ai_state
            .get_or_insert_with(|| Box::new(AiHttpState::default()))
            .as_mut()
    }

    #[inline]
    pub(crate) fn has_ai_attempt_plan(&self) -> bool {
        self.ai_state().is_some_and(|state| state.attempt_plan.is_some())
    }

    #[inline]
    pub(crate) fn take_ai_attempt_plan(&mut self) -> Option<AiAttemptPlan> {
        self.ai_state_mut()?.attempt_plan.take()
    }

    #[inline]
    pub(crate) fn restore_ai_attempt_plan(&mut self, plan: AiAttemptPlan) {
        self.ensure_ai_state().attempt_plan = Some(plan);
    }

    pub(crate) fn select_waf(&mut self, selection: Option<WafSelection>) -> bool {
        if self.waf_selection_complete {
            return false;
        }
        self.waf_selection_complete = true;
        // This is resolved once before any request-stage plugin runs. Keeping
        // the no-winner state as `None` avoids an allocation for ordinary
        // requests; a selected winner allocates the sole WAF state box.
        let Some(selection) = selection else {
            return true;
        };
        debug_assert!(self.waf.is_none());
        self.waf = Some(Box::new(WafRequestContext::selected(selection)));
        true
    }

    pub(crate) fn claim_waf_phase(&mut self, owner: &Arc<()>, phase: WafPhase) -> WafPhaseClaim {
        // No selected box means no WAF callback should run. Report `Pending`
        // so an ordering/wiring bug fails closed instead of silently skipping.
        self.waf
            .as_deref_mut()
            .map_or(WafPhaseClaim::Pending, |waf| waf.claim_phase(owner, phase))
    }

    pub(crate) fn record_waf_phase_outcome(
        &mut self,
        owner: &Arc<()>,
        phase: WafLogPhase,
        execution_us: u64,
        outcome: &edgion_waf::seclang::PhaseOutcome,
    ) {
        if let Some(waf) = self.waf.as_deref_mut() {
            waf.record_phase_outcome(owner, phase, execution_us, outcome);
        }
    }

    pub(crate) fn record_waf_terminal_error(
        &mut self,
        owner: &Arc<()>,
        phase: WafLogPhase,
        code: WafErrorCode,
        status: u16,
    ) {
        if let Some(waf) = self.waf.as_deref_mut() {
            waf.record_terminal_error(owner, phase, code, status);
        }
    }

    pub(crate) fn record_selected_waf_phase_outcome(
        &mut self,
        phase: WafLogPhase,
        execution_us: u64,
        outcome: &edgion_waf::seclang::PhaseOutcome,
    ) {
        if let Some(waf) = self.waf.as_deref_mut() {
            waf.access_log.record_phase(phase, execution_us, outcome);
        }
    }

    pub(crate) fn record_selected_waf_terminal_error(&mut self, phase: WafLogPhase, code: WafErrorCode, status: u16) {
        if let Some(waf) = self.waf.as_deref_mut() {
            waf.access_log.record_error(phase, code, status);
        }
    }

    pub(crate) fn finalize_waf_for_logging(&mut self) {
        if let Some(waf) = self.waf.as_deref_mut() {
            waf.access_log.finalize();
        }
    }

    pub(crate) fn waf_access_log(&self) -> Option<&WafAccessLog> {
        self.waf.as_deref().map(|waf| &waf.access_log)
    }

    /// Drain queued response-header ops onto the given real `ResponseHeader`,
    /// preserving each op's original set/append/conditional/remove semantics. Called
    /// from drain points: upstream-response stage and request-stage termination
    /// paths that build their own response (ErrResponse, gRPC errors).
    pub fn drain_pending_response_header_ops(&mut self, response_header: &mut ResponseHeader) {
        let Some(state) = self.plugin_aux_mut() else {
            return;
        };
        let ops = std::mem::take(&mut state.pending_response_header_ops);
        for op in ops {
            op.apply(response_header);
        }
    }

    /// Apply queued response-header ops onto `response_header` WITHOUT consuming
    /// the queue (each op is cloned). This is the single application point on the
    /// normal forward path, called at the upstream-response stage
    /// (`upstream_response_filter`). The queue is deliberately left intact so that
    /// if a response-phase plugin subsequently replaces the response with a local
    /// reply, `build_response_local_reply` can re-apply the same ops onto the
    /// fresh reply header via `drain_pending_response_header_ops` (filter-04c).
    ///
    /// Placed before the response-stage sync plugin runners so those plugins still
    /// observe the merged headers (the semantic the former in-runner pre-drain
    /// provided).
    pub fn apply_pending_response_header_ops(&self, response_header: &mut ResponseHeader) {
        for op in self.pending_response_header_ops() {
            op.clone().apply(response_header);
        }
    }

    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            gateway_info: None,
            request_info: RequestInfo::default(),
            global_request_ids_initialized: false,
            gateway_request_ids_initialized: false,
            diagnostics: RequestDiagnostics::default(),
            route_unit: None,
            selected_backend: None,
            grpc_route_unit: None,
            selected_grpc_backend: None,
            is_grpc_route_matched: false,
            backend_context: None,
            plugin_execution: PluginExecutionState::default(),
            plugin_aux: None,
            waf: None,
            waf_selection_complete: false,
            try_cnt: 0,
            retry_on_status: false,
            peer_timeout_source: None,
            connect_timeout_source: None,
            upstream_start_time: None,
            hash_key: None,
            upstream_authority: None,
            parsed_request_caches: ParsedRequestCaches::default(),
            jump: None,
            uri_finalized: false,
            session_persistence: None,
            allow_loopback_upstream: false,
            request_body_config: crate::request_body::ResolvedRequestBodyConfig::default(),
            remote_ip_header: http::HeaderName::from_static("x-real-ip"),
            request_body: RequestBodyState::default(),
            request_body_plugins: None,
            ai_state: None,
            response_terminal_state: ResponseTerminalState::Open,
            pre_plugin_response_status: None,
            candidate_downstream_status: None,
            final_downstream_status: None,
            response_write_started: false,
            response_processing: None,
            upstream_response_headers_end_stream: None,
            response_body_processing_started: false,
            response_override: None,
            response_stream_kind: ResponseStreamKind::None,
            cb_permit: None,
        }
    }

    /// Clear response control left by a discarded upstream attempt.
    #[inline]
    pub(crate) fn reset_response_override(&mut self) {
        self.response_override = None;
    }

    /// Stage a complete response-phase local reply for `response_filter`.
    #[inline]
    pub(crate) fn stage_response_local_reply(&mut self, header: Box<ResponseHeader>, body: bytes::Bytes) {
        self.response_override = Some(Box::new(ResponseOverride::PendingLocalReply { header, body }));
    }

    /// Take a pending response-phase local reply without disturbing another
    /// terminal state.
    pub(crate) fn take_pending_response_local_reply(&mut self) -> Option<(Box<ResponseHeader>, bytes::Bytes)> {
        if !matches!(
            self.response_override.as_deref(),
            Some(ResponseOverride::PendingLocalReply { .. })
        ) {
            return None;
        }
        match *self.response_override.take()? {
            ResponseOverride::PendingLocalReply { header, body } => Some((header, body)),
            ResponseOverride::LocalReplyWritten | ResponseOverride::Denied(_) => None,
        }
    }

    /// Record that a local reply header was successfully written downstream.
    #[inline]
    pub(crate) fn mark_local_reply_written(&mut self) {
        self.response_override = Some(Box::new(ResponseOverride::LocalReplyWritten));
    }

    #[inline]
    pub(crate) fn local_reply_written(&self) -> bool {
        matches!(
            self.response_override.as_deref(),
            Some(ResponseOverride::LocalReplyWritten)
        )
    }

    #[inline]
    pub(crate) fn set_response_denial(&mut self, denial: ResponseStageDenial) {
        self.response_override = Some(Box::new(ResponseOverride::Denied(denial)));
    }

    #[inline]
    pub(crate) fn response_denial(&self) -> Option<&ResponseStageDenial> {
        match self.response_override.as_deref() {
            Some(ResponseOverride::Denied(denial)) => Some(denial),
            Some(ResponseOverride::PendingLocalReply { .. }) | Some(ResponseOverride::LocalReplyWritten) | None => None,
        }
    }

    pub(crate) fn take_response_denial(&mut self) -> Option<ResponseStageDenial> {
        if !matches!(self.response_override.as_deref(), Some(ResponseOverride::Denied(_))) {
            return None;
        }
        match *self.response_override.take()? {
            ResponseOverride::Denied(denial) => Some(denial),
            ResponseOverride::PendingLocalReply { .. } | ResponseOverride::LocalReplyWritten => None,
        }
    }

    #[inline]
    pub(crate) fn response_stage_settled(&self) -> bool {
        matches!(
            self.response_override.as_deref(),
            Some(ResponseOverride::PendingLocalReply { .. } | ResponseOverride::Denied(_))
        )
    }

    /// Record an intrinsically classified error diagnostic.
    pub fn record_error(&mut self, code: EdgionErrorCode) -> bool {
        self.diagnostics.record_error(code)
    }

    /// Record an intrinsically classified informational diagnostic.
    pub fn record_info(&mut self, code: EdgionInfoCode) -> bool {
        self.diagnostics.record_info(code)
    }

    /// Select a candidate final response while response processing may still
    /// replace it. Rejected after a writer claims ownership.
    pub(crate) fn select_final_downstream_status(&mut self, status: u16) -> bool {
        if self.response_write_started
            || self.final_downstream_status.is_some()
            || !edgion_resources::http_status::is_valid_final_http_status(status)
        {
            return false;
        }
        self.candidate_downstream_status = Some(status);
        true
    }

    /// Select the response and claim the sole final-header write in one state
    /// transition. The selected status survives a later write failure.
    pub(crate) fn claim_final_response(&mut self, status: u16) -> bool {
        if self.response_write_started
            || self.final_downstream_status.is_some()
            || !edgion_resources::http_status::is_valid_final_http_status(status)
        {
            return false;
        }
        self.final_downstream_status = Some(status);
        self.candidate_downstream_status = None;
        self.response_write_started = true;
        true
    }

    /// Record a request-ending status only when no response already owns the
    /// outcome. Used for narrow terminal outcomes such as downstream 499.
    pub(crate) fn select_terminal_downstream_status(&mut self, status: u16) -> bool {
        if self.response_write_started
            || self.final_downstream_status.is_some()
            || !edgion_resources::http_status::is_valid_final_http_status(status)
        {
            return false;
        }
        self.final_downstream_status = Some(status);
        self.candidate_downstream_status = None;
        true
    }

    #[inline]
    pub(crate) fn response_write_started(&self) -> bool {
        self.response_write_started
    }

    #[inline]
    pub(crate) fn final_downstream_status(&self) -> Option<u16> {
        self.final_downstream_status.or(self.candidate_downstream_status)
    }

    /// Finalize the serialized access-log status exactly once at terminal
    /// logging. Interim 1xx values are ignored except terminal 101.
    pub(crate) fn finalize_downstream_status(&mut self, session_status: Option<u16>) {
        let session_status = session_status
            .filter(|status| *status == 101 || edgion_resources::http_status::is_valid_final_http_status(*status));
        let selected_status = self
            .final_downstream_status
            .or(self.candidate_downstream_status)
            .filter(|status| edgion_resources::http_status::is_valid_final_http_status(*status));
        self.request_info
            .finalize_status(session_status.or(selected_status).unwrap_or(0));
    }

    /// Get (and lazily build) the per-request decoded query parameter map.
    ///
    /// Callers should pass `self.request_info.entry_raw_query.as_deref()` —
    /// the entry-time frozen query — so the cache is order-independent: any
    /// reader (route matcher or plugin) sees the same client-sent query
    /// regardless of whether an earlier plugin called `set_upstream_uri` and
    /// rewrote the live URI.
    ///
    /// `raw_query` is consulted only on first access. Subsequent calls return
    /// the already-cached map regardless of the argument.
    ///
    /// Decoding follows `edgion_resources::uri_utils::parse_query_string` —
    /// `%XX` percent-decoded and `+` translated to space, matching what the
    /// route matcher sees in `queryParams`.
    pub fn cached_query(&self, raw_query: Option<&str>) -> &HashMap<String, String> {
        self.parsed_request_caches
            .query
            .get_or_init(|| {
                raw_query
                    .map(uri_utils::parse_query_string)
                    .filter(|params| !params.is_empty())
                    .map(Box::new)
            })
            .as_deref()
            .unwrap_or(&EMPTY_PARSED_REQUEST_PARAMS)
    }

    /// Return the path-parameter cache when it has already been initialized.
    #[inline]
    pub(crate) fn cached_path_params(&self) -> Option<&HashMap<String, String>> {
        self.parsed_request_caches
            .path
            .get()
            .map(|params| params.as_deref().unwrap_or(&EMPTY_PARSED_REQUEST_PARAMS))
    }

    /// Install the init-once path-parameter view and return the winning map.
    ///
    /// The production route-match hook calls this before plugins. The session
    /// adapter uses it lazily only for synthetic/test contexts that bypass that
    /// hook.
    #[inline]
    pub(crate) fn cache_path_params(&self, params: HashMap<String, String>) -> &HashMap<String, String> {
        self.parsed_request_caches
            .path
            .get_or_init(|| (!params.is_empty()).then(|| Box::new(params)))
            .as_deref()
            .unwrap_or(&EMPTY_PARSED_REQUEST_PARAMS)
    }

    /// Freeze named path captures from the route-match view before any rewrite
    /// plugin can change `normalized_current_path`.
    pub(crate) fn capture_matched_path_params(&self, matched: &HTTPRouteMatch) {
        let params = matched
            .path
            .as_ref()
            .filter(|path| !matches!(path.match_type.as_deref(), Some("RegularExpression")))
            .and_then(|path| path.value.as_deref())
            .map(|pattern| parse_path_params(pattern, self.path()))
            .unwrap_or_default();
        self.cache_path_params(params);
    }

    /// Geo for the resolved client (`request_info.remote_ip`). Resolves
    /// on first call, then caches. Never queries eagerly — the `real_ip`
    /// plugin can rewrite `remote_ip` after the listener-level extractor
    /// runs, so filling this at request entry would cache the wrong address.
    pub fn remote_geo(&self) -> &crate::geoip::GeoLookup {
        let geo = self.request_info.geo();
        geo.remote.get_or_init(|| {
            self.resolve_geo(
                &self.request_info.remote_ip,
                &geo.direct_peer,
                &self.request_info.direct_peer_ip,
            )
        })
    }

    /// Geo for the physical direct peer (`request_info.direct_peer_ip`). Resolves
    /// on first call, then caches.
    pub fn direct_peer_geo(&self) -> &crate::geoip::GeoLookup {
        let geo = self.request_info.geo();
        geo.direct_peer.get_or_init(|| {
            self.resolve_geo(
                &self.request_info.direct_peer_ip,
                &geo.remote,
                &self.request_info.remote_ip,
            )
        })
    }

    /// Resolve `addr`, reusing `other_slot` when the two addresses normalize
    /// equal. Most requests have no `X-Forwarded-For`, so `direct_peer_ip` and
    /// `remote_ip` are usually the same host — and comparing the raw
    /// strings would miss that, because `::ffff:203.0.113.5` and
    /// `203.0.113.5` are the same host spelled two ways. Mirrors
    /// `real_ip_extractor`'s address handling: try `SocketAddr`
    /// first (a peer address may carry a port), then fall back to a bare
    /// `IpAddr`. An address that fails to parse yields `NotFound`, not a
    /// panic.
    fn resolve_geo(
        &self,
        addr: &str,
        other_slot: &std::sync::OnceLock<crate::geoip::GeoLookup>,
        other_addr: &str,
    ) -> crate::geoip::GeoLookup {
        use crate::geoip::info::normalize_addr;
        use crate::geoip::{GeoIpProvider, GeoLookup};

        let Some(ip) = parse_ip_str(addr) else {
            return GeoLookup::NotFound;
        };
        let ip = normalize_addr(ip);

        if let Some(cached) = other_slot.get() {
            if parse_ip_str(other_addr).map(normalize_addr) == Some(ip) {
                return cached.clone();
            }
        }
        GeoIpProvider::global().lookup(ip)
    }

    /// Resolve the route-level `request_timeout`, preferring HTTP `route_unit`
    /// over `grpc_route_unit`. Returns `None` when neither side resolves to a
    /// concrete duration (either side may have `parsed_timeouts = None`, or have
    /// `request_timeout` set to `Unset`/`Disabled`, both of which yield `None`
    /// from `as_duration()`).
    ///
    /// The fallback is applied at the resolved `Duration` level — not at the
    /// `parsed_timeouts` level — so that an HTTP route with `parsed_timeouts`
    /// present but `request_timeout` not configured correctly falls through to
    /// the gRPC side.
    #[inline]
    pub fn request_timeout(&self) -> Option<Duration> {
        self.route_unit
            .as_ref()
            .and_then(|u| u.rule.parsed_timeouts.as_ref())
            .and_then(|t| t.request_timeout.as_duration())
            .or_else(|| {
                self.grpc_route_unit
                    .as_ref()
                    .and_then(|u| u.rule.parsed_timeouts.as_ref())
                    .and_then(|t| t.request_timeout.as_duration())
            })
    }

    /// True when a route-level `request_timeout` is set and `start_time`
    /// has already exceeded it. Returns `false` when no timeout is configured.
    #[inline]
    pub fn request_timeout_exceeded(&self) -> bool {
        self.request_timeout()
            .is_some_and(|t| Instant::now().saturating_duration_since(self.start_time) >= t)
    }

    /// Remaining wall-clock time before the client-supplied `grpc-timeout`
    /// deadline expires. Returns `None` when no `grpc-timeout` header was
    /// parsed, and `Some(Duration::ZERO)` when the deadline has already
    /// passed.
    #[inline]
    pub fn grpc_remaining(&self) -> Option<Duration> {
        self.request_info
            .grpc_client_deadline()
            .map(|d| d.saturating_duration_since(Instant::now()))
    }

    /// True when a `grpc-timeout` deadline was parsed for this request and
    /// `Instant::now()` is already past it.
    #[inline]
    pub fn grpc_deadline_exceeded(&self) -> bool {
        self.grpc_remaining().is_some_and(|r| r.is_zero())
    }

    /// Resolve the retry `attempts` cap with field-level fallback between
    /// HTTP and gRPC route units.
    #[inline]
    pub fn retry_attempts(&self) -> Option<u32> {
        self.route_unit
            .as_ref()
            .and_then(|u| u.rule.parsed_retry.as_ref())
            .and_then(|r| r.attempts)
            .or_else(|| {
                self.grpc_route_unit
                    .as_ref()
                    .and_then(|u| u.rule.parsed_retry.as_ref())
                    .and_then(|r| r.attempts)
            })
    }

    /// Resolve the retry `backoff` base duration with field-level fallback
    /// between HTTP and gRPC route units.
    #[inline]
    pub fn retry_backoff(&self) -> Option<Duration> {
        self.route_unit
            .as_ref()
            .and_then(|u| u.rule.parsed_retry.as_ref())
            .and_then(|r| r.backoff)
            .or_else(|| {
                self.grpc_route_unit
                    .as_ref()
                    .and_then(|u| u.rule.parsed_retry.as_ref())
                    .and_then(|r| r.backoff)
            })
    }

    /// Resolve the effective `sessionPersistence` config with field-level
    /// fallback between HTTP and gRPC route units (mirrors `retry_attempts`).
    /// Only one route unit is set per request; the order pins the tie-break
    /// for consistency with the retry helpers.
    #[inline]
    pub fn effective_session_persistence(&self) -> Option<&SessionPersistence> {
        pick_effective_sp(
            self.route_unit
                .as_ref()
                .and_then(|u| u.rule.session_persistence.as_ref()),
            self.grpc_route_unit
                .as_ref()
                .and_then(|u| u.rule.session_persistence.as_ref()),
        )
    }

    /// True when the effective `sessionPersistence` declares `strict: true`.
    #[inline]
    pub fn effective_sp_strict(&self) -> bool {
        self.effective_session_persistence()
            .and_then(|sp| sp.strict)
            .unwrap_or(false)
    }

    /// True when the matched route opted into retrying non-idempotent methods
    /// on the reused-connection retry path via the
    /// `edgion.io/retry-non-idempotent: "true"` annotation. Defaults to `false`
    /// (the safe default: a reused keepalive reset is not retried for
    /// non-idempotent methods, to avoid a silent duplicate mutation).
    #[inline]
    pub fn retry_allow_non_idempotent(&self) -> bool {
        self.route_unit
            .as_ref()
            .and_then(|u| u.rule.parsed_allow_non_idempotent_retry)
            .or_else(|| {
                self.grpc_route_unit
                    .as_ref()
                    .and_then(|u| u.rule.parsed_allow_non_idempotent_retry)
            })
            .unwrap_or(false)
    }

    /// True when the matched rule (HTTP or gRPC route unit) carries a retry
    /// policy — i.e. `rules[].retry` was present with at least one recognized field.
    #[inline]
    pub fn retry_configured(&self) -> bool {
        self.route_unit.as_ref().is_some_and(|u| u.rule.parsed_retry.is_some())
            || self
                .grpc_route_unit
                .as_ref()
                .is_some_and(|u| u.rule.parsed_retry.is_some())
    }

    /// Initialize backend context for the resource-selected path (HTTP/gRPC backend ref).
    /// `source` stays `None` — this is the default path, no bypass tag needed.
    /// Bypass paths (DirectEndpoint / ExternalJump) construct `BackendContext` directly
    /// at the call site (in `try_direct_endpoint_peer` / `try_external_jump_peer`)
    /// and set their own `Some(source)` tag.
    pub fn init_backend_context(
        &mut self,
        rsc: Rsc,
        service_key: String,
        service_kind: ServiceBackendKind,
        backend_ref_idx: Option<usize>,
    ) {
        self.backend_context = Some(BackendContext {
            source: None,
            rsc: Some(rsc),
            service_key: Some(service_key),
            service_kind: Some(service_kind),
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx,
            effective_policy: None,
        });
    }

    /// Push a new upstream connection attempt with address info
    pub fn push_upstream(&mut self, ip: Option<String>, port: Option<u16>) {
        if let Some(ref mut bc) = self.backend_context {
            bc.upstreams.push(UpstreamInfo {
                ip,
                port,
                status: None,
                ct: None,
                ht: None,
                bt: None,
                et: None,
                wpt: None,
                response_body_size: None,
                start_time: std::time::Instant::now(),
                err: Vec::new(),
                lb_backend_addr: None,
                lb_backend: None,
                lb_outlier_completion: None,
                service_key: None,
                lb_policy: None,
                tls: None,
                lb_committed: None,
            });
            bc.current_upstream_id = Some(bc.upstreams.len() - 1);
        }
    }

    /// Get mutable reference to current upstream
    pub fn get_current_upstream_mut(&mut self) -> Option<&mut UpstreamInfo> {
        self.backend_context
            .as_mut()
            .and_then(|bc| bc.current_upstream_id.and_then(|id| bc.upstreams.get_mut(id)))
    }

    /// Get reference to current upstream
    pub fn get_current_upstream(&self) -> Option<&UpstreamInfo> {
        self.backend_context
            .as_ref()
            .and_then(|bc| bc.current_upstream_id.and_then(|id| bc.upstreams.get(id)))
    }

    // ========== Context variable methods ==========

    /// Get a context variable by key
    pub fn get_ctx_var(&self, key: &str) -> Option<&str> {
        self.plugin_aux()?.ctx_map.get(key).map(String::as_str)
    }

    /// Set a context variable (for plugins like KeySet)
    pub fn set_ctx_var(&mut self, key: String, value: String) {
        self.ensure_plugin_aux().ctx_map.insert(key, value);
    }

    /// Remove a context variable
    pub fn remove_ctx_var(&mut self, key: &str) -> Option<String> {
        self.plugin_aux_mut()?.ctx_map.remove(key)
    }

    // ===== URI/path canonical accessors =====
    // See docs/superpowers/specs/2026-04-26-uri-normalization-design.md §3.2

    /// Current normalized path. Reflects post-rewrite state (cache refreshed by
    /// session_adapter::set_upstream_uri after each rewrite). Default view for
    /// routing matchers, ACL plugins, access_log.
    pub fn path(&self) -> &str {
        &self.request_info.normalized_current_path
    }

    /// Entry-time raw path. Frozen for the request lifetime.
    /// Use for audit, security investigation, special raw-form deny rules.
    pub fn request_path(&self) -> &str {
        &self.request_info.entry_raw_path
    }

    /// Current query string (passthrough; never normalized).
    pub fn query<'a>(&self, req_header: &'a pingora_http::RequestHeader) -> Option<&'a str> {
        req_header.uri.query()
    }

    /// Current normalized full URI (current path + current query).
    pub fn uri(&self, req_header: &pingora_http::RequestHeader) -> String {
        compose_uri(self.path(), self.query(req_header))
    }

    /// Entry-time raw full URI (entry raw path + entry raw query). Frozen.
    pub fn request_uri(&self) -> String {
        compose_uri(
            &self.request_info.entry_raw_path,
            self.request_info.entry_raw_query.as_deref(),
        )
    }

    /// Refresh `normalized_current_path` from current `req_header.uri.path()`.
    /// Called by session_adapter::set_upstream_uri after rewrite, or by entry filter.
    /// Business code should NOT call this directly.
    pub fn refresh_normalized_path(
        &mut self,
        req_header: &pingora_http::RequestHeader,
        cfg: &edgion_resources::path_normalization::PathNormalizationConfig,
    ) {
        let raw = req_header.uri.path();
        self.request_info.normalized_current_path = match edgion_resources::path_normalization::normalize_path(raw, cfg)
        {
            Some(s) => s,
            None => raw.to_string(),
        };
    }

    /// First-write-wins setter for the unified upstream-jump slot. Returns
    /// `Set` on the first successful write, `Rejected` on every subsequent
    /// call. The target is stored without modifying backend selection state.
    pub fn try_set_jump(&mut self, target: UpstreamJumpTarget) -> SetJumpOutcome {
        if self.jump.is_some() {
            return SetJumpOutcome::Rejected;
        }
        self.jump = Some(Box::new(target));
        SetJumpOutcome::Set
    }
}

/// Pure precedence step for `effective_session_persistence`: the HTTP rule's
/// config wins over the gRPC rule's. Split out so the precedence is
/// unit-testable without constructing route units.
#[inline]
pub(crate) fn pick_effective_sp<'a>(
    http_sp: Option<&'a SessionPersistence>,
    grpc_sp: Option<&'a SessionPersistence>,
) -> Option<&'a SessionPersistence> {
    http_sp.or(grpc_sp)
}

fn compose_uri(path: &str, query: Option<&str>) -> String {
    match query {
        Some(q) if !q.is_empty() => format!("{}?{}", path, q),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod log_label_tests {
    use super::*;
    use edgion_resources::constants::annotations::edgion::LOG_LABEL_PREFIX;
    use kube::api::ObjectMeta;

    fn json_of(labels: &Option<LogLabels>) -> String {
        let rsc = Rsc::new("HTTPRoute", "ns", "name").with_labels(labels.clone());
        serde_json::to_string(&rsc).unwrap()
    }

    // Resolution semantics (prefix strip, precedence, guardrails, cap) are
    // covered by `edgion_resources::resources::common::log_labels`. These tests
    // pin the gateway-side serialization shape and the delegating wrapper.

    #[test]
    fn extract_serializes_sorted_object_with_prefix_stripped() {
        let meta = ObjectMeta {
            annotations: Some(
                [(format!("{LOG_LABEL_PREFIX}tenant"), "internal".to_string())]
                    .into_iter()
                    .collect(),
            ),
            labels: Some(
                [(format!("{LOG_LABEL_PREFIX}env"), "prod".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };
        let out = extract_log_labels(&meta).expect("labels");
        assert_eq!(
            json_of(&Some(out)),
            r#"{"kind":"HTTPRoute","ns":"ns","name":"name","labels":{"env":"prod","tenant":"internal"}}"#
        );
    }

    #[test]
    fn extract_returns_none_without_prefixed_entries() {
        let meta = ObjectMeta {
            annotations: Some([("unrelated".to_string(), "x".to_string())].into_iter().collect()),
            ..Default::default()
        };
        assert!(extract_log_labels(&meta).is_none());
    }

    #[test]
    fn from_resolved_wraps_cached_arc_without_rescanning() {
        let map = Arc::new([("env".to_string(), "prod".to_string())].into_iter().collect());
        let out = LogLabels::from_resolved(Arc::clone(&map));
        assert_eq!(
            json_of(&Some(out)),
            r#"{"kind":"HTTPRoute","ns":"ns","name":"name","labels":{"env":"prod"}}"#
        );
    }

    #[test]
    fn labels_field_omitted_when_absent() {
        assert_eq!(json_of(&None), r#"{"kind":"HTTPRoute","ns":"ns","name":"name"}"#);
    }
}

#[cfg(test)]
mod route_meta_tests {
    use super::*;

    #[test]
    fn route_meta_is_resolved_once_and_shared() {
        use std::collections::BTreeMap;

        let meta = kube::api::ObjectMeta {
            labels: Some(BTreeMap::from([("app".to_string(), "web".to_string())])),
            annotations: Some(BTreeMap::from([("owner".to_string(), "platform".to_string())])),
            ..Default::default()
        };

        let (labels, annotations) = extract_route_meta(&meta);
        assert_eq!(labels.as_ref().unwrap().get("app").unwrap(), "web");
        assert_eq!(annotations.as_ref().unwrap().get("owner").unwrap(), "platform");
    }

    #[test]
    fn route_meta_is_none_when_absent() {
        let (labels, annotations) = extract_route_meta(&kube::api::ObjectMeta::default());
        assert!(labels.is_none());
        assert!(annotations.is_none());
    }

    #[test]
    fn route_meta_is_not_serialized_into_the_log_entry() {
        // The maps exist to answer `from: routeLabel` / `from: routeAnnotation`
        // lookups. They must never leak into `match_info` wholesale — only
        // declared keys reach `ext`. Populate both maps via `with_route_meta`
        // with distinctive values so the assertion actually exercises
        // `#[serde(skip)]` on both `labels_meta` and `annotations_meta`
        // (a prior version of this test left both maps `None` via `MatchInfo::new`
        // alone, which caught removing `#[serde(skip)]` from `annotations_meta` —
        // that field's name contains "annotations" — but not from `labels_meta`,
        // since `"labels_meta":null` doesn't contain "annotations").
        // `HTTPRouteMatch` has no `Default` impl, so build it explicitly with
        // every field empty/None (all optional matchers).
        use std::collections::BTreeMap;

        let match_item = HTTPRouteMatch {
            path: None,
            headers: None,
            query_params: None,
            method: None,
        };
        let labels = Arc::new(BTreeMap::from([(
            "canary-label-marker".to_string(),
            "canary-label-value".to_string(),
        )]));
        let annotations = Arc::new(BTreeMap::from([(
            "canary-annotation-marker".to_string(),
            "canary-annotation-value".to_string(),
        )]));
        let info = MatchInfo::new("default".to_string(), "r".to_string(), 0, 0, match_item, 0)
            .with_route_meta(Some(labels), Some(annotations));
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("labels_meta"), "got: {json}");
        assert!(!json.contains("annotations_meta"), "got: {json}");
        assert!(!json.contains("canary-label-marker"), "got: {json}");
        assert!(!json.contains("canary-label-value"), "got: {json}");
        assert!(!json.contains("canary-annotation-marker"), "got: {json}");
        assert!(!json.contains("canary-annotation-value"), "got: {json}");
    }
}

#[cfg(test)]
mod sensitive_tests {
    use super::*;
    use edgion_resources::sensitive::SENSITIVE_MODE;
    use parking_lot::Mutex;
    use std::sync::atomic::Ordering;

    static MODE_LOCK: Mutex<()> = Mutex::new(());

    struct ModeGuard<'a> {
        prev: u8,
        // Held purely for RAII: serializes tests that mutate SENSITIVE_MODE.
        _lock: parking_lot::MutexGuard<'a, ()>,
    }
    impl Drop for ModeGuard<'_> {
        fn drop(&mut self) {
            SENSITIVE_MODE.store(self.prev, Ordering::SeqCst);
        }
    }
    fn set_mode(mode: u8) -> ModeGuard<'static> {
        let guard = MODE_LOCK.lock();
        let prev = SENSITIVE_MODE.swap(mode, Ordering::SeqCst);
        ModeGuard { prev, _lock: guard }
    }

    #[test]
    fn retry_configured_false_without_route_units() {
        let ctx = EdgionHttpContext::new();
        assert!(!ctx.retry_configured());
        assert!(ctx.peer_timeout_source.is_none());
    }

    #[test]
    fn test_client_cert_info_wrap_mode() {
        let _g = set_mode(2);
        let cert = ClientCertInfo {
            subject: "CN=alice@example.com".to_string(),
            sans: vec!["alice@example.com".to_string()],
            cn: Some("alice@example.com".to_string()),
            fingerprint: "abc123".to_string(),
        };
        let json = serde_json::to_string(&cert).unwrap();
        assert!(
            json.contains("<E>CN=alice@example.com</E>"),
            "subject not wrapped: {}",
            json
        );
        assert!(json.contains("<E>alice@example.com</E>"), "sans not wrapped: {}", json);
        assert!(json.contains("abc123"), "fingerprint should not be wrapped: {}", json);
    }

    #[test]
    fn test_client_cert_info_off_mode() {
        let _g = set_mode(0);
        let cert = ClientCertInfo {
            subject: "CN=alice@example.com".to_string(),
            sans: vec![],
            cn: None,
            fingerprint: "abc123".to_string(),
        };
        let json = serde_json::to_string(&cert).unwrap();
        assert!(
            json.contains("CN=alice@example.com"),
            "subject should be plain in off mode: {}",
            json
        );
    }

    #[test]
    fn test_request_info_wrap_mode() {
        let _g = set_mode(2);
        let mut ri = RequestInfo {
            direct_peer_ip: "127.0.0.1".to_string(),
            remote_ip: "10.0.0.1".to_string(),
            entry_raw_path: "/api/users/alice".to_string(),
            normalized_current_path: "/api/users/alice".to_string(),
            x_forwarded_for: Some("203.0.113.5, 10.0.0.1".to_string()),
            ..Default::default()
        };
        ri.install_tls_info(None, Some("example.com".to_string()), None, None, None);
        let json = serde_json::to_string(&ri).unwrap();
        assert!(json.contains("<E>127.0.0.1</E>"), "direct_peer_ip: {}", json);
        assert!(json.contains("<E>10.0.0.1</E>"), "remote_ip: {}", json);
        assert!(json.contains("<E>/api/users/alice</E>"), "path: {}", json);
        assert!(json.contains("<E>203.0.113.5, 10.0.0.1</E>"), "xff: {}", json);
        assert!(json.contains("<E>example.com</E>"), "sni: {}", json);
    }

    #[test]
    fn test_request_info_replace_mode() {
        let _g = set_mode(1);
        let ri = RequestInfo {
            direct_peer_ip: "127.0.0.1".to_string(),
            remote_ip: "10.0.0.1".to_string(),
            entry_raw_path: "/api/users/alice".to_string(),
            normalized_current_path: "/api/users/alice".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&ri).unwrap();
        assert!(!json.contains("127.0.0.1"), "direct_peer_ip leaked: {}", json);
        assert!(!json.contains("10.0.0.1"), "remote_ip leaked: {}", json);
        assert!(!json.contains("/api/users/alice"), "path leaked: {}", json);
        assert!(json.contains("***"), "replace_with not found: {}", json);
    }

    #[test]
    fn downstream_ip_canonicalization_collapses_ipv4_mapped_ipv6() {
        assert_eq!(
            canonicalize_ip_str("::ffff:203.0.113.7").as_deref(),
            Some("203.0.113.7")
        );
        assert_eq!(canonicalize_ip_str("2001:db8::1").as_deref(), Some("2001:db8::1"));
        assert!(canonicalize_ip_str("not-an-ip").is_none());
    }

    #[test]
    fn body_declared_defaults_to_none_not_false() {
        // `RequestInfo` derives `Default`. A plain `bool` would default `false`,
        // which the predicate would read as "no body — run the plugin". `None`
        // means "the snapshot never ran" and fails closed instead.
        let info = RequestInfo::default();
        assert_eq!(info.body_declared, None);
    }
}

#[cfg(test)]
mod request_body_cap_tests {
    use super::*;
    use crate::plugins::runtime::traits::session::BodyReadError;
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;

    async fn context_with_body(body: &'static [u8], cap: u64) -> EdgionHttpContext {
        let (mut storage, view) = BodyStorage::new(128 * 1024, cap);
        storage.write(&Bytes::from_static(body)).await.unwrap();
        storage.finish().await.unwrap();
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = cap;
        ctx.request_body.install(view);
        ctx
    }

    #[tokio::test]
    async fn overwrite_enforces_physical_cap_atomically() {
        let mut exact = context_with_body(b"old", 4).await;
        exact.overwrite_request_body(Bytes::from_static(b"1234")).unwrap();
        assert_eq!(
            exact.request_body.read_all().await.unwrap().unwrap(),
            Bytes::from_static(b"1234")
        );

        let mut over = context_with_body(b"old", 4).await;
        assert_eq!(
            over.overwrite_request_body(Bytes::from_static(b"12345")),
            Err(BodyReadError::BufferLimitExceeded { limit: 4 })
        );
        assert_eq!(
            over.request_body.read_all().await.unwrap().unwrap(),
            Bytes::from_static(b"old"),
            "a rejected mutation must leave the previous version visible"
        );
    }

    #[tokio::test]
    async fn slice_replacement_enforces_physical_cap_atomically() {
        let mut ctx = context_with_body(b"abcd", 5).await;
        assert_eq!(
            ctx.replace_request_body_slice(1..3, Bytes::from_static(b"WXYZ")).await,
            Err(BodyReadError::BufferLimitExceeded { limit: 5 })
        );
        assert_eq!(
            ctx.request_body.read_all().await.unwrap().unwrap(),
            Bytes::from_static(b"abcd")
        );
    }
}

#[cfg(test)]
mod path_view_tests {
    use super::*;
    use pingora_http::RequestHeader;

    fn make_ctx(entry_path: &str, entry_query: Option<&str>) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.entry_raw_path = entry_path.to_string();
        ctx.request_info.entry_raw_query = entry_query.map(str::to_string);
        let path_cfg = edgion_resources::path_normalization::PathNormalizationConfig::default();
        ctx.request_info.normalized_current_path =
            match edgion_resources::path_normalization::normalize_path(entry_path, &path_cfg) {
                Some(s) => s,
                None => entry_path.to_string(),
            };
        ctx
    }

    #[test]
    fn path_returns_normalized_current() {
        let ctx = make_ctx("/%61dmin", None);
        assert_eq!(ctx.path(), "/admin");
    }

    #[test]
    fn request_path_returns_entry_raw() {
        let ctx = make_ctx("/%61dmin", None);
        assert_eq!(ctx.request_path(), "/%61dmin");
    }

    #[test]
    fn request_uri_composes_entry() {
        let ctx = make_ctx("/%61dmin", Some("x=1"));
        assert_eq!(ctx.request_uri(), "/%61dmin?x=1");
    }

    #[test]
    fn refresh_after_rewrite() {
        let mut ctx = make_ctx("/api/old", None);
        let req = RequestHeader::build("GET", b"/api/new", None).unwrap();
        let path_cfg = edgion_resources::path_normalization::PathNormalizationConfig::default();
        ctx.refresh_normalized_path(&req, &path_cfg);
        assert_eq!(ctx.path(), "/api/new");
        assert_eq!(ctx.request_path(), "/api/old");
    }
}

#[cfg(test)]
mod jump_tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn jump_target_uses_one_pointer_in_the_base_context() {
        assert_eq!(std::mem::size_of::<UpstreamJumpTarget>(), 72);
        assert_eq!(std::mem::size_of::<Option<Box<UpstreamJumpTarget>>>(), 8);
    }

    fn fresh_ctx() -> EdgionHttpContext {
        EdgionHttpContext::new()
    }

    fn sample_internal() -> InternalJumpPreset {
        InternalJumpPreset {
            backend_ref_name: "svc-a".to_string(),
            backend_ref_namespace: None,
        }
    }

    fn sample_external() -> ExternalJumpPreset {
        ExternalJumpPreset {
            domain: "api.example.com".to_string(),
            port: 443,
            use_tls: true,
            sni: "api.example.com".to_string(),
            preserve_raw_path: false,
            upstream_read_timeout_override: None,
        }
    }

    fn sample_direct() -> DirectEndpointPreset {
        DirectEndpointPreset {
            addr: "10.0.0.1:8080".parse().unwrap(),
            tls_policy: crate::backends::BackendTlsPolicyLookup::Absent,
            backend_ref_idx: 0,
        }
    }

    #[test]
    fn first_internal_write_returns_set() {
        let mut ctx = fresh_ctx();
        let outcome = ctx.try_set_jump(UpstreamJumpTarget::Internal(sample_internal()));
        assert_eq!(outcome, SetJumpOutcome::Set);
        assert!(matches!(ctx.jump.as_deref(), Some(UpstreamJumpTarget::Internal(_))));
        assert!(
            ctx.selected_backend.is_none(),
            "Internal must NOT co-write selected_backend"
        );
    }

    #[test]
    fn first_external_write_returns_set() {
        let mut ctx = fresh_ctx();
        let outcome = ctx.try_set_jump(UpstreamJumpTarget::External(sample_external()));
        assert_eq!(outcome, SetJumpOutcome::Set);
        assert!(matches!(ctx.jump.as_deref(), Some(UpstreamJumpTarget::External(_))));
        assert!(
            ctx.selected_backend.is_none(),
            "External must NOT co-write selected_backend"
        );
    }

    #[test]
    fn first_direct_write_keeps_backend_selection_empty() {
        let ((ctx, outcome), allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            let mut ctx = fresh_ctx();
            let outcome = ctx.try_set_jump(UpstreamJumpTarget::Direct(sample_direct()));
            (ctx, outcome)
        });
        assert_eq!(outcome, SetJumpOutcome::Set);
        assert!(matches!(ctx.jump.as_deref(), Some(UpstreamJumpTarget::Direct(_))));
        assert!(ctx.selected_backend.is_none());
        assert_eq!(allocations.calls, 1);
        assert_eq!(allocations.requested_bytes, std::mem::size_of::<UpstreamJumpTarget>());
    }

    #[test]
    fn second_write_returns_rejected_and_does_not_mutate_ctx() {
        let mut ctx = fresh_ctx();
        let first = ctx.try_set_jump(UpstreamJumpTarget::External(sample_external()));
        assert_eq!(first, SetJumpOutcome::Set);
        let selected_before_name = ctx.selected_backend.as_ref().map(|b| b.name.clone());

        let second = ctx.try_set_jump(UpstreamJumpTarget::Direct(sample_direct()));
        assert_eq!(second, SetJumpOutcome::Rejected);

        assert!(matches!(ctx.jump.as_deref(), Some(UpstreamJumpTarget::External(_))));
        assert_eq!(
            ctx.selected_backend.as_ref().map(|b| b.name.as_str()),
            selected_before_name.as_deref(),
            "selected_backend must not change on rejected second write"
        );
    }

    #[test]
    fn second_direct_write_after_internal_does_not_leak_backend() {
        let mut ctx = fresh_ctx();
        ctx.try_set_jump(UpstreamJumpTarget::Internal(sample_internal()));
        assert!(ctx.selected_backend.is_none());

        let outcome = ctx.try_set_jump(UpstreamJumpTarget::Direct(sample_direct()));
        assert_eq!(outcome, SetJumpOutcome::Rejected);
        assert!(
            ctx.selected_backend.is_none(),
            "Rejected Direct must NOT touch selected_backend"
        );
    }
}

#[cfg(test)]
mod cached_query_tests {
    use super::*;
    use edgion_resources::resources::http_route::HTTPPathMatch;

    fn matched_path(match_type: &str, value: &str) -> HTTPRouteMatch {
        HTTPRouteMatch {
            path: Some(HTTPPathMatch {
                match_type: Some(match_type.to_string()),
                value: Some(value.to_string()),
            }),
            headers: None,
            query_params: None,
            method: None,
        }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn parsed_request_caches_have_compact_inline_layout() {
        assert_eq!(std::mem::size_of::<ParsedRequestCaches>(), 32);
    }

    #[test]
    fn empty_parsed_views_initialize_without_allocating() {
        let ctx = EdgionHttpContext::new();
        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            assert!(ctx.cached_query(None).is_empty());
            assert!(ctx.cache_path_params(HashMap::new()).is_empty());
        });

        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }

    #[test]
    fn matched_path_captures_are_frozen_before_rewrite() {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.normalized_current_path = "/users/123".to_string();
        ctx.capture_matched_path_params(&matched_path("Exact", "/users/:id"));
        ctx.request_info.normalized_current_path = "/backend/999".to_string();
        ctx.capture_matched_path_params(&matched_path("Exact", "/backend/:id"));

        assert_eq!(
            ctx.cached_path_params().unwrap().get("id").map(String::as_str),
            Some("123")
        );
    }

    #[test]
    fn regular_expression_paths_do_not_create_named_captures() {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.normalized_current_path = "/users/123".to_string();
        ctx.capture_matched_path_params(&matched_path("RegularExpression", "/users/:id"));

        assert!(ctx.cached_path_params().unwrap().is_empty());
    }

    #[test]
    fn cached_query_initializes_lazily_and_caches() {
        let ctx = EdgionHttpContext::new();
        // First access triggers parse. The space in `hello world` is percent-
        // encoded as `%20`, so the decoded value exercises url-decode.
        let first = ctx.cached_query(Some("name=hello%20world&n=1")) as *const _;
        // Second access with a *different* raw_query must still return the
        // already-cached HashMap — init-once semantics, not "re-parse".
        let second = ctx.cached_query(Some("name=changed&n=2")) as *const _;
        assert_eq!(first, second, "cached_query must be init-once per request");

        // Verify decoded values (route-matcher-compatible).
        let map = ctx.cached_query(Some("ignored=value"));
        assert_eq!(map.get("name"), Some(&"hello world".to_string()));
        assert_eq!(map.get("n"), Some(&"1".to_string()));
    }

    #[test]
    fn cached_query_handles_none_raw() {
        let ctx = EdgionHttpContext::new();
        let map = ctx.cached_query(None);
        assert!(map.is_empty(), "None raw_query yields empty cache");
        // Subsequent call with Some(...) must NOT replace the cached empty map.
        let map2 = ctx.cached_query(Some("a=1"));
        assert!(map2.is_empty(), "OnceLock already initialized to empty");
    }
}

#[cfg(test)]
mod geo_context_tests {
    use super::*;
    use crate::geoip::{GeoIpProvider, GeoLookup, GEO_TEST_LOCK};

    #[test]
    fn geo_slots_start_unresolved_and_resolve_once() {
        // Serialize against the process-wide GeoIpProvider singleton: other
        // modules' tests in this same test binary load the fixture database
        // into it, so without the lock + clear this test would flake
        // depending on run order.
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.direct_peer_ip = "203.0.113.7".to_string();
        ctx.request_info.remote_ip = "203.0.113.7".to_string();
        assert!(ctx.request_info.geo.get().is_none());

        // No database loaded in this test process.
        assert_eq!(*ctx.remote_geo(), GeoLookup::Unavailable);
        assert!(ctx.request_info.geo.get().is_some());
        // Second call returns the cached value, not a second lookup.
        assert_eq!(*ctx.remote_geo(), GeoLookup::Unavailable);
    }

    #[test]
    fn equal_addresses_share_one_lookup() {
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.direct_peer_ip = "::ffff:203.0.113.7".to_string();
        ctx.request_info.remote_ip = "203.0.113.7".to_string();
        // The mapped and plain forms normalize equal, so the second slot must
        // reuse the first result rather than resolving independently.
        let a = ctx.direct_peer_geo().clone();
        let b = ctx.remote_geo().clone();
        assert_eq!(a, b);
    }

    #[test]
    fn unparseable_address_reports_not_found_not_panic() {
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.remote_ip = "not-an-ip".to_string();
        assert_eq!(*ctx.remote_geo(), GeoLookup::NotFound);
    }
}

#[cfg(test)]
mod response_override_tests {
    use super::*;

    #[test]
    fn lazy_holder_is_pointer_sized_and_defaults_inactive() {
        let former_inline_fields = std::mem::size_of::<Option<bytes::Bytes>>()
            + std::mem::size_of::<Option<Box<ResponseHeader>>>()
            + std::mem::size_of::<bool>()
            + std::mem::size_of::<Option<ResponseStageDenial>>();
        assert_eq!(
            std::mem::size_of::<Option<Box<ResponseOverride>>>(),
            std::mem::size_of::<usize>()
        );
        assert!(std::mem::size_of::<Option<Box<ResponseOverride>>>() < former_inline_fields);
        let ctx = EdgionHttpContext::default();
        assert!(!ctx.response_stage_settled());
        assert!(!ctx.local_reply_written());
        assert_eq!(ctx.response_denial(), None);
    }

    #[test]
    fn terminal_states_are_mutually_exclusive() {
        let mut ctx = EdgionHttpContext::default();
        let header = ResponseHeader::build(403, None).unwrap();
        ctx.stage_response_local_reply(Box::new(header), bytes::Bytes::from_static(b"denied"));
        assert!(ctx.response_stage_settled());
        assert_eq!(ctx.response_denial(), None);

        ctx.set_response_denial(ResponseStageDenial::Http(502));
        assert!(ctx.take_pending_response_local_reply().is_none());
        assert_eq!(ctx.response_denial(), Some(&ResponseStageDenial::Http(502)));

        ctx.mark_local_reply_written();
        assert!(ctx.local_reply_written());
        assert!(!ctx.response_stage_settled());
        assert_eq!(ctx.response_denial(), None);
    }

    #[test]
    fn pending_reply_round_trips_and_clears_state() {
        let mut ctx = EdgionHttpContext::default();
        let header = ResponseHeader::build(403, None).unwrap();
        ctx.stage_response_local_reply(Box::new(header), bytes::Bytes::from_static(b"denied"));

        let (header, body) = ctx.take_pending_response_local_reply().unwrap();
        assert_eq!(header.status.as_u16(), 403);
        assert_eq!(body, bytes::Bytes::from_static(b"denied"));
        assert!(!ctx.response_stage_settled());
    }

    #[test]
    fn denial_round_trips_and_clears_state() {
        let mut ctx = EdgionHttpContext::default();
        let denial = ResponseStageDenial::Grpc {
            status: 8,
            message: Some("quota exceeded".to_string()),
        };
        ctx.set_response_denial(denial.clone());

        assert_eq!(ctx.take_response_denial(), Some(denial));
        assert_eq!(ctx.response_denial(), None);
        assert!(!ctx.response_stage_settled());
    }
}

#[cfg(test)]
mod final_downstream_status_tests {
    use super::*;

    #[test]
    fn finalization_prefers_written_final_header_then_selected_status_then_zero() {
        let mut written = EdgionHttpContext::default();
        assert!(written.select_final_downstream_status(502));
        written.finalize_downstream_status(Some(200));
        assert_eq!(written.request_info.status(), 200);

        let mut selected = EdgionHttpContext::default();
        assert!(selected.claim_final_response(503));
        selected.finalize_downstream_status(None);
        assert_eq!(selected.request_info.status(), 503);

        let mut unknown = EdgionHttpContext::default();
        unknown.finalize_downstream_status(None);
        assert_eq!(unknown.request_info.status(), 0);
        assert_eq!(unknown.request_info.status_option(), None);
    }

    #[test]
    fn interim_headers_do_not_replace_selected_status_but_101_is_terminal() {
        let mut interim = EdgionHttpContext::default();
        assert!(interim.select_final_downstream_status(502));
        interim.finalize_downstream_status(Some(103));
        assert_eq!(interim.request_info.status(), 502);

        let mut switching = EdgionHttpContext::default();
        assert!(switching.select_final_downstream_status(502));
        switching.finalize_downstream_status(Some(101));
        assert_eq!(switching.request_info.status(), 101);
    }

    #[test]
    fn candidates_and_claims_reject_informational_or_invalid_statuses() {
        for status in [100, 101, 102, 103, 199, 600, 700] {
            let mut ctx = EdgionHttpContext::default();
            assert!(!ctx.select_final_downstream_status(status), "candidate {status}");
            assert!(!ctx.claim_final_response(status), "claim {status}");
            assert!(!ctx.select_terminal_downstream_status(status), "terminal {status}");
            assert!(!ctx.response_write_started());
            assert_eq!(ctx.final_downstream_status(), None);
        }
    }

    #[test]
    fn terminal_finalization_filters_an_invalid_internal_candidate() {
        let mut ctx = EdgionHttpContext {
            final_downstream_status: Some(103),
            ..Default::default()
        };
        ctx.finalize_downstream_status(None);
        assert_eq!(ctx.request_info.status_option(), None);
    }

    #[test]
    fn write_claim_rejects_late_replacement_and_terminal_499() {
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.claim_final_response(200));
        assert!(!ctx.select_final_downstream_status(502));
        assert!(!ctx.select_terminal_downstream_status(499));
        assert!(!ctx.claim_final_response(503));
        ctx.finalize_downstream_status(None);
        assert_eq!(ctx.request_info.status(), 200);
    }

    #[test]
    fn claimed_error_status_survives_delivery_failure() {
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.select_final_downstream_status(200));
        assert!(ctx.claim_final_response(502));
        assert!(!ctx.select_terminal_downstream_status(499));
        assert!(!ctx.select_final_downstream_status(504));

        ctx.finalize_downstream_status(None);
        assert_eq!(ctx.request_info.status(), 502);
    }

    #[test]
    fn selected_terminal_status_blocks_later_candidate_or_writer() {
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.select_terminal_downstream_status(499));
        assert!(!ctx.select_final_downstream_status(502));
        assert!(!ctx.claim_final_response(502));
        assert_eq!(ctx.final_downstream_status(), Some(499));
    }

    #[test]
    fn grpc_error_keeps_http_and_grpc_status_independent() {
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.select_final_downstream_status(504));
        assert!(ctx.claim_final_response(200));
        ctx.request_info.set_grpc_status(4);

        ctx.finalize_downstream_status(None);

        assert_eq!(ctx.request_info.status(), 200);
        assert_eq!(ctx.request_info.grpc_status(), Some(4));
    }
}

#[cfg(test)]
mod json_schema_validation_claim_tests {
    use super::*;

    #[test]
    fn request_context_allows_exactly_one_json_schema_validator() {
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.claim_json_schema_validation());
        assert!(!ctx.claim_json_schema_validation());
    }
}

#[cfg(test)]
mod plugin_request_state_tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn plugin_state_layout_keeps_hot_and_sparse_costs_separate() {
        assert_eq!(std::mem::size_of::<PluginExecutionState>(), 168);
        assert_eq!(std::mem::size_of::<PluginAuxState>(), 248);
        assert_eq!(std::mem::size_of::<Option<Box<PluginAuxState>>>(), 8);
    }

    #[test]
    fn ordinary_context_keeps_plugin_aux_unallocated_on_reads() {
        let mut ctx = EdgionHttpContext::new();
        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            assert!(!ctx.has_plugin_aux());
            assert!(ctx.stage_logs().is_empty());
            assert!(!ctx.plugin_terminated());
            assert!(ctx.get_ctx_var("missing").is_none());
            assert!(!ctx.has_pending_response_header_ops());
            assert!(ctx.pending_response_header_ops().is_empty());
            assert!(ctx.current_dye_plan().is_none());
            assert!(!ctx.has_mirror_state());
            assert!(ctx.resolved_global_plugins().is_none());
            assert!(ctx.resolved_gateway_plugins().is_none());
            assert!(ctx.resolved_route_ext_refs().is_none());
            assert!(ctx.take_mirror_state().is_none());
        });

        assert!(!ctx.has_plugin_aux());
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }

    #[test]
    fn hot_plugin_execution_state_remains_allocation_free() {
        let mut ctx = EdgionHttpContext::new();
        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.terminate_plugin_execution();
            assert!(ctx.plugin_terminated());
        });
        assert!(!ctx.has_plugin_aux());
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }

    #[test]
    fn suppressed_only_stage_does_not_create_an_empty_stage_log() {
        let mut ctx = EdgionHttpContext::new();
        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.finish_plugin_stage("request_filters", Vec::new());
        });
        assert!(ctx.stage_logs().is_empty());
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }

    #[test]
    fn first_sparse_plugin_effect_allocates_one_aux_box_and_reuses_it() {
        let mut ctx = EdgionHttpContext::new();
        let (_, first) = crate::test_allocation_counter::count_allocation_stats(|| {
            assert!(ctx.claim_json_schema_validation());
        });
        assert!(ctx.has_plugin_aux());
        assert_eq!(first.calls, 1);
        assert_eq!(first.requested_bytes, std::mem::size_of::<PluginAuxState>());

        let (_, later) = crate::test_allocation_counter::count_allocation_stats(|| {
            assert!(!ctx.claim_json_schema_validation());
        });
        assert_eq!(later.calls, 0);
        assert_eq!(later.requested_bytes, 0);
    }

    #[test]
    fn runtime_snapshots_do_not_instantiate_plugin_aux() {
        let mut ctx = EdgionHttpContext::new();
        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.install_resolved_global_plugins(Vec::new());
            ctx.install_resolved_gateway_plugins(Vec::new());
            ctx.install_resolved_route_ext_refs(Vec::new());
        });

        assert!(!ctx.has_plugin_aux());
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);

        ctx.install_resolved_route_ext_refs(vec![None]);
        assert!(!ctx.has_plugin_aux());
    }

    #[test]
    fn dangling_route_and_backend_refs_remain_request_pinned() {
        let mut ctx = EdgionHttpContext::new();
        ctx.install_resolved_route_ext_refs(vec![None]);
        assert!(!ctx.has_plugin_aux());
        assert_eq!(ctx.resolved_route_ext_refs().unwrap().len(), 1);
        assert!(ctx.resolved_route_ext_ref(0).is_none());

        let mut resolve_calls = 0;
        assert!(ctx
            .pinned_backend_ext_ref("default/missing", || {
                resolve_calls += 1;
                None
            })
            .is_none());
        assert!(ctx.has_plugin_aux());
        assert!(ctx
            .pinned_backend_ext_ref("default/missing", || {
                resolve_calls += 1;
                None
            })
            .is_none());
        assert_eq!(resolve_calls, 1);
    }
}

#[cfg(test)]
mod request_body_plugin_state_tests {
    use super::*;
    use pingora_proxy::RequestBodyEvent;

    struct NoopStreamHandler;

    #[async_trait::async_trait]
    impl crate::plugins::runtime::traits::RequestBodyStreamHandler for NoopStreamHandler {
        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _event: RequestBodyEvent,
        ) -> crate::plugins::runtime::traits::ChunkVerdict {
            crate::plugins::runtime::traits::ChunkVerdict::Continue
        }

        fn take_log(&mut self) -> Option<crate::plugins::runtime::log::PluginLog> {
            None
        }

        fn dye_plan(&self) -> Option<Arc<crate::plugins::runtime::dye_plan::CompiledDyePlan>> {
            None
        }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn request_body_plugin_state_has_one_small_outer_box() {
        assert_eq!(std::mem::size_of::<RequestBodyPluginState>(), 40);
        assert_eq!(std::mem::size_of::<Option<Box<RequestBodyPluginState>>>(), 8);
    }

    #[test]
    fn first_waf_stream_activation_allocates_one_state_and_is_monotonic() {
        let mut ctx = EdgionHttpContext::new();
        let (_, first) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.activate_waf_request_body_streaming();
        });
        assert!(ctx.has_request_body_plugins());
        assert!(ctx.request_body_stream_active());
        assert_eq!(first.calls, 1);
        assert_eq!(first.requested_bytes, std::mem::size_of::<RequestBodyPluginState>());

        let (_, later) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.activate_waf_request_body_streaming();
        });
        assert_eq!(later.calls, 0);
        assert!(ctx.request_body_stream_active());
    }

    #[test]
    fn observer_source_does_not_disable_request_replay() {
        let mut ctx = EdgionHttpContext::new();
        let observers = Box::new(crate::request_body::observer::RequestBodyObservers::default());
        let (_, allocation) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.install_request_body_observers(observers);
        });

        assert!(ctx.has_request_body_plugins());
        assert!(ctx.has_request_body_observers());
        assert!(!ctx.request_body_stream_active());
        assert_eq!(allocation.calls, 1);
        assert_eq!(
            allocation.requested_bytes,
            std::mem::size_of::<RequestBodyPluginState>()
        );
    }

    #[test]
    fn retiring_mirror_only_state_restores_outer_fast_path() {
        let mut ctx = EdgionHttpContext::new();
        ctx.activate_request_body_source(REQUEST_BODY_SOURCE_MIRROR);
        ctx.ensure_plugin_aux().mirror_state = Some(MirrorState::Abandoned);

        ctx.retire_request_mirror();

        assert!(!ctx.has_mirror_state());
        assert!(!ctx.has_request_body_plugins());
    }

    #[test]
    fn retiring_mirror_preserves_handler_and_waf_replay_latches() {
        let mut handler_ctx = EdgionHttpContext::new();
        handler_ctx.push_request_body_stream_handler(Box::new(NoopStreamHandler));
        handler_ctx.activate_request_body_source(REQUEST_BODY_SOURCE_MIRROR);
        handler_ctx.ensure_plugin_aux().mirror_state = Some(MirrorState::Abandoned);

        handler_ctx.retire_request_mirror();

        assert!(!handler_ctx.has_mirror_state());
        assert!(handler_ctx.has_request_body_plugins());
        assert_eq!(handler_ctx.request_body_stream_handler_count(), 1);
        assert!(handler_ctx.request_body_stream_active());

        let mut waf_ctx = EdgionHttpContext::new();
        waf_ctx.activate_waf_request_body_streaming();
        waf_ctx.activate_request_body_source(REQUEST_BODY_SOURCE_MIRROR);
        waf_ctx.ensure_plugin_aux().mirror_state = Some(MirrorState::Abandoned);

        waf_ctx.retire_request_mirror();

        assert!(!waf_ctx.has_mirror_state());
        assert!(waf_ctx.has_request_body_plugins());
        assert!(waf_ctx.request_body_stream_active());
    }

    #[test]
    fn retiring_mirror_preserves_observer_state_without_disabling_replay() {
        let mut ctx = EdgionHttpContext::new();
        ctx.install_request_body_observers(Box::new(crate::request_body::observer::RequestBodyObservers::default()));
        ctx.activate_request_body_source(REQUEST_BODY_SOURCE_MIRROR);
        ctx.ensure_plugin_aux().mirror_state = Some(MirrorState::Abandoned);

        ctx.retire_request_mirror();

        assert!(!ctx.has_mirror_state());
        assert!(ctx.has_request_body_plugins());
        assert!(ctx.has_request_body_observers());
        assert!(!ctx.request_body_stream_active());
    }
}

#[cfg(test)]
mod response_processing_state_tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn response_processing_state_has_one_scenario_box() {
        assert_eq!(std::mem::size_of::<ResponseProcessingState>(), 120);
        assert_eq!(std::mem::size_of::<Option<Box<ResponseProcessingState>>>(), 8);
    }

    #[test]
    fn freezing_an_empty_processor_set_does_not_allocate() {
        let mut ctx = EdgionHttpContext::new();
        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.freeze_response_processor_driver();
        });

        assert!(ctx.response_processors_frozen());
        assert!(ctx.response_processor_driver().is_none());
        assert!(!ctx.has_response_processing());
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }

    #[test]
    fn first_framing_reservation_allocates_once_and_reuses_state() {
        let mut ctx = EdgionHttpContext::new();
        let (_, first) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.reserve_response_framing();
        });
        assert!(ctx.has_response_processing());
        assert!(ctx.has_response_framing_reservation());
        assert_eq!(first.calls, 1);
        assert_eq!(first.requested_bytes, std::mem::size_of::<ResponseProcessingState>());

        let (_, later) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.reserve_response_framing();
        });
        assert_eq!(later.calls, 0);
        assert!(ctx.has_response_framing_reservation());
    }

    #[test]
    fn absent_response_processing_terminal_hooks_do_not_allocate() {
        let mut ctx = EdgionHttpContext::new();
        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            assert!(!ctx.claim_upstream_response_trailers(false));
            assert!(!ctx.mark_upstream_before_trailers());
            assert!(!ctx.claim_upstream_trailer_cleanup());
        });

        assert!(!ctx.has_response_processing());
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }
}

#[cfg(test)]
mod session_persistence_context_tests {
    use super::*;

    #[test]
    fn ordinary_context_keeps_session_persistence_unallocated() {
        let ctx = EdgionHttpContext::new();
        assert!(ctx.session_persistence().is_none());
    }

    #[test]
    fn first_session_persistence_use_allocates_default_state() {
        let mut ctx = EdgionHttpContext::new();
        let persistence = ctx.ensure_session_persistence();

        assert_eq!(persistence.state, SessionPersistenceState::Inactive);
        assert!(persistence.target_addr.is_none());
        assert!(persistence.sticky_addr.is_none());
        assert!(persistence.token_scope.is_none());
        assert!(!persistence.sticky_consumed);
    }
}

#[cfg(test)]
mod ai_context_claim_tests {
    use std::sync::Arc;

    use super::*;
    use crate::ai::{AiAttemptPlan, AiPlanError, AiProviderLabel, AiRequestContext, AiUsageShared};

    fn context(model: &str) -> AiRequestContext {
        AiRequestContext {
            provider: AiProviderLabel::OpenAi,
            model: model.to_string(),
            pricing: None,
            requested_streaming: false,
            usage: Arc::new(AiUsageShared::default()),
            quota: None,
            quota_status: None,
        }
    }

    #[test]
    fn first_ai_context_writer_wins() {
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.ai_state().is_none());
        assert!(ctx.try_set_ai_request_context(context("model-a")));
        assert!(!ctx.try_set_ai_request_context(context("model-b")));
        assert_eq!(
            ctx.ai_state()
                .and_then(|state| state.request.as_ref())
                .map(|ai| ai.model.as_str()),
            Some("model-a")
        );
    }

    #[test]
    fn first_ai_attempt_plan_writer_wins() {
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.try_set_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend)));
        assert!(!ctx.try_set_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidRouteConfiguration,)));
        assert!(matches!(
            ctx.ai_state().and_then(|state| state.attempt_plan.as_ref()),
            Some(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend))
        ));
    }

    #[test]
    fn ordinary_context_keeps_ai_state_unallocated() {
        let ctx = EdgionHttpContext::new();
        assert!(ctx.ai_state().is_none());
    }

    #[test]
    fn ordinary_context_size_stays_within_budget() {
        assert!(
            std::mem::size_of::<EdgionHttpContext>() <= 1280,
            "EdgionHttpContext grew beyond the ordinary-request hot-path size budget"
        );
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn ordinary_context_construction_is_allocation_free() {
        let (ctx, allocations) = crate::test_allocation_counter::count_allocation_stats(EdgionHttpContext::new);

        assert_eq!(std::mem::size_of_val(&ctx), 1120);
        assert!(ctx.gateway_info.is_none());
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }
}

#[cfg(test)]
mod context_benchmark_tests {
    use super::*;
    use std::hint::black_box;

    const ITERATIONS: usize = 250_000;

    fn run_scenario(label: &str, mut scenario: impl FnMut()) {
        let started = std::time::Instant::now();
        for _ in 0..ITERATIONS {
            scenario();
        }
        let elapsed = started.elapsed();
        let nanos_per_op = elapsed.as_nanos() / ITERATIONS as u128;
        eprintln!("context-benchmark {label}: {nanos_per_op} ns/op ({ITERATIONS} iterations)");
    }

    /// Internal, dependency-free request-context microbenchmark.
    ///
    /// Run on the target architecture with:
    /// `cargo test -p edgion-gateway --release benchmark_http_context_scenarios -- --ignored --nocapture`
    #[test]
    #[ignore = "manual performance benchmark"]
    fn benchmark_http_context_scenarios() {
        eprintln!(
            "context-benchmark layout: {} bytes",
            std::mem::size_of::<EdgionHttpContext>()
        );
        run_scenario("default", || {
            black_box(EdgionHttpContext::new());
        });
        run_scenario("hot-plugin-latch", || {
            let mut ctx = EdgionHttpContext::new();
            ctx.terminate_plugin_execution();
            black_box(ctx);
        });
        run_scenario("sparse-plugin-state", || {
            let mut ctx = EdgionHttpContext::new();
            black_box(ctx.claim_json_schema_validation());
            black_box(ctx);
        });

        let owner = Arc::new(());
        run_scenario("selected-waf", || {
            let mut ctx = EdgionHttpContext::new();
            black_box(ctx.select_waf(Some(WafSelection::new(
                Arc::clone(&owner),
                WafLogSource::Route,
                0,
                WafLogMode::On,
            ))));
            black_box(ctx);
        });
    }
}

#[cfg(test)]
mod sp_config_tests {
    use super::pick_effective_sp;
    use edgion_resources::resources::http_route::SessionPersistence;

    fn sp(name: &str) -> SessionPersistence {
        SessionPersistence {
            session_name: Some(name.to_string()),
            absolute_timeout: None,
            idle_timeout: None,
            persistence_type: None,
            cookie_config: None,
            strict: None,
        }
    }

    #[test]
    fn http_only_uses_http() {
        let http = sp("http-sp");
        let picked = pick_effective_sp(Some(&http), None).expect("http-only must resolve");
        assert_eq!(picked.session_name.as_deref(), Some("http-sp"));
    }

    #[test]
    fn grpc_only_falls_back_to_grpc() {
        let grpc = sp("grpc-sp");
        let picked = pick_effective_sp(None, Some(&grpc)).expect("grpc-only must resolve");
        assert_eq!(picked.session_name.as_deref(), Some("grpc-sp"));
    }

    #[test]
    fn neither_yields_none() {
        assert!(pick_effective_sp(None, None).is_none());
    }

    #[test]
    fn http_wins_over_grpc() {
        // Only one route unit is ever set per request; this pins the tie-break
        // order for consistency with retry_attempts()/retry_backoff().
        let http = sp("http-sp");
        let grpc = sp("grpc-sp");
        let picked = pick_effective_sp(Some(&http), Some(&grpc)).expect("must resolve");
        assert_eq!(picked.session_name.as_deref(), Some("http-sp"));
    }
}
