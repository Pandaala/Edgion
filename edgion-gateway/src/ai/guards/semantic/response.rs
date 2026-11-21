//! Canonical semantic SSE window processor for Guardrail unary decisions.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use pingora_proxy::{ResponseBodySink, ResponseHeadHoldLimits};
use tokio::sync::Semaphore;

use super::attached::{AttachedGuardrail, GuardrailSummary};
use super::protocol::{encode_request, is_json_response, parse_decision, ParsedDecision, RequestWindow};
use crate::ai::protocols::openai::canonical_sse::{
    CanonicalRecordBatch, CanonicalSourceStatus, MAX_CANONICAL_SOURCE_CHUNK_BYTES,
};
use crate::ai::protocols::openai::terminal::emit_guardrail_rejection;
use crate::ctx::ResponseStreamKind;
use crate::link_sys::template::TemplateContext;
use crate::plugins::http::common::outbound::call::outbound_call_with_template_context;
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::runtime::traits::StreamingResponseProcessor;
use crate::plugins::runtime::ResponseStreamSession;
use crate::response::terminal::{ResponseTerminalCause, TerminalEmission};
use crate::sse::window::{
    RawWindow, RawWindowPolicy, SemanticProgress, SemanticReplacement, WindowDecision, WindowError, WindowOutput,
    MAX_RAW_BYTES,
};
use crate::sse::DEFAULT_MAX_PENDING_EVENT_BYTES;

const MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES: usize =
    MAX_RAW_BYTES + DEFAULT_MAX_PENDING_EVENT_BYTES + MAX_CANONICAL_SOURCE_CHUNK_BYTES;
const MAX_DECISION_RESPONSE_BYTES: usize = 1024 * 1024;
const RESPONSE_HEAD_METADATA_BYTES: usize = 256 * 1024;
const RESPONSE_HEAD_EVENT_SLACK: usize = 4;
const RESPONSE_HEAD_WORK_SLACK: u64 = 8;

/// Generic transport-retention limits that compose with Guardrail's more
/// detailed RawWindow bounds. The configured head timeout is an outer absolute
/// deadline, so waiting for source bytes and the first dependency call consume
/// one bounded precommit budget. The dependency client's own timeout remains
/// an independent, potentially shorter, call bound.
pub(super) fn response_head_hold_limits(timeout: Duration) -> ResponseHeadHoldLimits {
    let events = crate::sse::window::MAX_EVENTS + RESPONSE_HEAD_EVENT_SLACK;
    ResponseHeadHoldLimits::new(
        MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES,
        MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES,
        events,
        events,
        RESPONSE_HEAD_METADATA_BYTES,
        events as u64 + RESPONSE_HEAD_WORK_SLACK,
        timeout,
    )
}

pub(super) struct GuardrailResponseProcessor {
    client: Arc<ManagedOutboundClient>,
    permits: Arc<Semaphore>,
    owner: Arc<AttachedGuardrail>,
    window: RawWindow,
    semantic_progress: SemanticProgress,
    window_bytes: usize,
    max_request_bytes: usize,
    max_replacement_bytes: usize,
    fail_open: bool,
    first_head_window_pending: bool,
    disabled: bool,
    summary: GuardrailSummary,
}

/// The precommit decision surface is deliberately narrower than the body
/// sink. Keeping it private lets the Guardrail state machine be tested without
/// exposing framework-owned Hold arming to product code.
trait HeldResponseHeadSink {
    fn response_head_is_held(&self) -> bool;
    fn reserve_response_head_work(&mut self, units: u64) -> pingora_core::Result<()>;
    fn replace_response_head(
        &mut self,
        replacement: pingora_proxy::ResponseHeadReplacement,
    ) -> pingora_core::Result<()>;
    fn fail_response_head(&mut self, error: Box<pingora_core::Error>) -> pingora_core::Result<()>;
    fn terminate(&mut self);
}

impl HeldResponseHeadSink for ResponseBodySink {
    fn response_head_is_held(&self) -> bool {
        ResponseBodySink::response_head_is_held(self)
    }

    fn reserve_response_head_work(&mut self, units: u64) -> pingora_core::Result<()> {
        ResponseBodySink::reserve_response_head_work(self, units)
    }

    fn replace_response_head(
        &mut self,
        replacement: pingora_proxy::ResponseHeadReplacement,
    ) -> pingora_core::Result<()> {
        ResponseBodySink::replace_response_head(self, replacement)
    }

    fn fail_response_head(&mut self, error: Box<pingora_core::Error>) -> pingora_core::Result<()> {
        ResponseBodySink::fail_response_head(self, error)
    }

    fn terminate(&mut self) {
        ResponseBodySink::terminate(self);
    }
}

impl GuardrailResponseProcessor {
    fn terminate_dependency(
        &mut self,
        sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
        reason: &'static str,
    ) {
        if self.stage_head_failure(sink, session, reason) {
            return;
        }
        if session.try_claim_response_terminal(ResponseTerminalCause::GuardrailDependency) {
            session.mark_response_terminal_emission(TerminalEmission::NoEvent);
            sink.terminate();
            self.publish_terminal("fail-close", reason);
        }
    }

    /// Ensure `output` can hold `bound` bytes in total. The bound is admitted
    /// against the callback limit before allocation, and growth goes straight to
    /// the exact bound instead of amortized doubling, so capacity never exceeds
    /// the limit. Callbacks start unallocated and size output to what they emit.
    fn reserve_output(output: &mut BytesMut, bound: usize) -> Result<(), ()> {
        if bound > MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES {
            return Err(());
        }
        if output.capacity() >= bound {
            return Ok(());
        }
        let mut grown = BytesMut::with_capacity(bound);
        if grown.capacity() > MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES {
            return Err(());
        }
        grown.extend_from_slice(output);
        *output = grown;
        Ok(())
    }

    fn resolved_len(resolved: &WindowOutput) -> Option<usize> {
        match resolved {
            WindowOutput::Release(slices) => slices
                .iter()
                .try_fold(0usize, |total, slice| total.checked_add(slice.len())),
            WindowOutput::Replace(bytes) => Some(bytes.len()),
            WindowOutput::Discard => Some(0),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        client: Arc<ManagedOutboundClient>,
        permits: Arc<Semaphore>,
        owner: Arc<AttachedGuardrail>,
        window_bytes: usize,
        overlap_bytes: usize,
        max_request_bytes: usize,
        max_replacement_bytes: usize,
        fail_open: bool,
        hold_first_window: bool,
    ) -> Result<Self, WindowError> {
        Ok(Self {
            client,
            permits,
            owner,
            window: RawWindow::with_policy(RawWindowPolicy {
                per_bucket_overlap_bytes: overlap_bytes,
                global_overlap_bytes: crate::sse::window::MAX_OVERLAP_BYTES,
                max_replacement_bytes,
            })?,
            semantic_progress: SemanticProgress::default(),
            window_bytes,
            max_request_bytes,
            max_replacement_bytes,
            fail_open,
            first_head_window_pending: hold_first_window,
            disabled: false,
            summary: GuardrailSummary::default(),
        })
    }

    fn start_next_window(&mut self) -> Result<(), WindowError> {
        self.window.start_next()?;
        self.semantic_progress = SemanticProgress::default();
        Ok(())
    }

    fn append_output(output: &mut BytesMut, resolved: WindowOutput) -> Result<(), ()> {
        let added = Self::resolved_len(&resolved).ok_or(())?;
        Self::reserve_output(output, output.len().checked_add(added).ok_or(())?)?;
        match resolved {
            WindowOutput::Release(slices) => {
                for slice in slices {
                    output.extend_from_slice(&slice);
                }
            }
            WindowOutput::Replace(bytes) => output.extend_from_slice(&bytes),
            WindowOutput::Discard => {}
        }
        Ok(())
    }

    fn publish_terminal(&mut self, outcome: &'static str, reason: &'static str) {
        self.summary.outcome = outcome;
        self.summary.reason = reason;
        self.owner.publish(self.summary.clone());
    }

    fn stage_head_failure<S: HeldResponseHeadSink>(
        &mut self,
        sink: &mut S,
        session: &mut dyn ResponseStreamSession,
        reason: &'static str,
    ) -> bool {
        if !sink.response_head_is_held() {
            return false;
        }
        self.first_head_window_pending = false;
        let claimed = session.try_claim_response_terminal(ResponseTerminalCause::GuardrailDependency);
        let _ = self.window.resolve(WindowDecision::Discard);
        session.mark_response_terminal_emission(TerminalEmission::NoEvent);
        let mut error = pingora_core::Error::explain(
            pingora_core::ErrorType::HTTPStatus(503),
            if claimed {
                format!("Guardrail first-window dependency failure: {reason}")
            } else {
                "Guardrail first-window terminal owner conflict".to_string()
            },
        );
        error.set_retry(false);
        if sink.fail_response_head(error).is_err() {
            sink.terminate();
        }
        self.publish_terminal("fail-close", reason);
        true
    }

    fn release_first_head_window(
        &mut self,
        sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
    ) -> bool {
        if !self.first_head_window_pending {
            return true;
        }
        if sink.response_head_is_held() && sink.release_response_head() {
            self.first_head_window_pending = false;
            return true;
        }
        let _ = self.stage_head_failure(sink, session, "head-release");
        false
    }

    fn stage_head_policy_reject<S: HeldResponseHeadSink>(
        &mut self,
        sink: &mut S,
        session: &mut dyn ResponseStreamSession,
    ) -> bool {
        if !sink.response_head_is_held() {
            return false;
        }
        if !session.try_claim_response_terminal(ResponseTerminalCause::GuardrailRejected) {
            return self.stage_head_failure(sink, session, "terminal-owner-conflict");
        }
        let Some(replacement) = self.owner.head_replacement() else {
            return self.stage_head_failure(sink, session, "replacement-template");
        };
        let _ = self.window.resolve(WindowDecision::Discard);
        self.first_head_window_pending = false;
        if sink.replace_response_head(replacement).is_err() {
            sink.terminate();
            self.publish_terminal("fail-close", "head-replacement");
            return true;
        }
        session.mark_response_terminal_emission(TerminalEmission::Rendered);
        self.publish_terminal("reject", "policy");
        true
    }

    async fn decide<S: HeldResponseHeadSink>(
        &mut self,
        session: &dyn ResponseStreamSession,
        sink: &mut S,
    ) -> Result<ParsedDecision, &'static str> {
        if sink.response_head_is_held() {
            sink.reserve_response_head_work(1).map_err(|_| "head-work-limit")?;
        }
        let snapshot = self.window.snapshot_all().map_err(|_| "semantic-source")?;
        if snapshot.iter().all(|entry| entry.current.is_empty()) {
            return Ok(ParsedDecision::Pass);
        }
        let windows: Vec<_> = snapshot
            .into_iter()
            .map(|entry| {
                let (choice_index, field) = entry.key.into();
                RequestWindow {
                    choice_index,
                    field,
                    context: entry.context,
                    current: entry.current,
                }
            })
            .collect();
        let request = encode_request(&windows, self.max_request_bytes)?;
        let timeout = self.client.timeout;
        let call = async {
            let _permit = self.permits.acquire().await.map_err(|_| "concurrency-closed")?;
            outbound_call_with_template_context(
                &self.client,
                TemplateContext::Trusted(session),
                None,
                MAX_DECISION_RESPONSE_BYTES,
                &[("content-type", "application/json")],
                Some(request),
                // Guardrail validation forbids all origin fields. In
                // particular, never create PluginLog entries in body callbacks.
                &mut Default::default(),
            )
            .await
            .map_err(|_| "dependency")
        };
        let response = tokio::time::timeout(timeout, call).await.map_err(|_| "timeout")??;
        if !is_json_response(response.status, &response.headers) {
            return Err("invalid-http-response");
        }
        parse_decision(&response.body, self.max_replacement_bytes)
    }

    fn apply_decision(&mut self, decision: ParsedDecision) -> Result<(WindowOutput, bool), &'static str> {
        self.summary.windows = self.summary.windows.saturating_add(1);
        match decision {
            ParsedDecision::Pass => {
                self.summary.passed = self.summary.passed.saturating_add(1);
                self.window
                    .resolve(WindowDecision::Release)
                    .map(|output| (output, false))
                    .map_err(|_| "window")
            }
            ParsedDecision::Replace(replacements) => {
                self.summary.replaced = self.summary.replaced.saturating_add(1);
                let replacements = replacements
                    .into_iter()
                    .map(|(key, current)| SemanticReplacement { key, current })
                    .collect();
                self.window
                    .resolve_semantic_replace(replacements)
                    .map(|output| (output, false))
                    .map_err(|_| "replacement")
            }
            ParsedDecision::Reject => self
                .window
                .resolve(WindowDecision::Discard)
                .map(|output| (output, true))
                .map_err(|_| "window"),
        }
    }

    fn apply_final_decision(&mut self, decision: ParsedDecision) -> Result<(WindowOutput, bool), &'static str> {
        self.summary.windows = self.summary.windows.saturating_add(1);
        match decision {
            ParsedDecision::Pass => {
                self.summary.passed = self.summary.passed.saturating_add(1);
                self.window
                    .finish(WindowDecision::Release)
                    .map(|output| (output, false))
                    .map_err(|_| "window")
            }
            ParsedDecision::Replace(replacements) => {
                self.summary.replaced = self.summary.replaced.saturating_add(1);
                let replacements = replacements
                    .into_iter()
                    .map(|(key, current)| SemanticReplacement { key, current })
                    .collect();
                self.window
                    .finish_semantic_replace(replacements)
                    .map(|output| (output, false))
                    .map_err(|_| "replacement")
            }
            ParsedDecision::Reject => self
                .window
                .finish(WindowDecision::Discard)
                .map(|output| (output, true))
                .map_err(|_| "window"),
        }
    }

    fn fail_dependency(
        &mut self,
        output: &mut BytesMut,
        records: VecDeque<crate::sse::window::WindowRecord>,
        sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
        reason: &'static str,
    ) {
        if self.stage_head_failure(sink, session, reason) {
            output.clear();
            return;
        }
        if self.fail_open {
            match self.window.resolve(WindowDecision::Release) {
                Ok(resolved) => {
                    // Admit the released window and the untouched tail together so
                    // the fail-open output is allocated once at its exact size.
                    let tail = records
                        .iter()
                        .try_fold(0usize, |total, record| total.checked_add(record.raw_len()));
                    let bound = Self::resolved_len(&resolved)
                        .zip(tail)
                        .and_then(|(window, tail)| window.checked_add(tail))
                        .and_then(|added| output.len().checked_add(added));
                    if bound.is_none_or(|bound| Self::reserve_output(output, bound).is_err())
                        || Self::append_output(output, resolved).is_err()
                    {
                        output.clear();
                        self.terminate_dependency(sink, session, "callback-output-limit");
                        return;
                    }
                }
                Err(_) => {
                    output.clear();
                    self.terminate_dependency(sink, session, "window-release");
                    return;
                }
            }
            for record in records {
                for slice in record.into_raw_slices() {
                    let next = output.len().checked_add(slice.len());
                    if next.is_none_or(|next| next > MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES) {
                        output.clear();
                        self.terminate_dependency(sink, session, "callback-output-limit");
                        return;
                    }
                    output.extend_from_slice(&slice);
                }
            }
            self.disabled = true;
            self.summary.output_bytes = self.summary.output_bytes.saturating_add(output.len() as u64);
            self.publish_terminal("fail-open", reason);
        } else {
            if !session.try_claim_response_terminal(ResponseTerminalCause::GuardrailDependency) {
                output.clear();
                let _ = self.window.resolve(WindowDecision::Discard);
                return;
            }
            output.clear();
            let _ = self.window.resolve(WindowDecision::Discard);
            session.mark_response_terminal_emission(TerminalEmission::NoEvent);
            sink.terminate();
            self.publish_terminal("fail-close", reason);
        }
    }

    fn emit_policy_reject(&mut self, sink: &mut ResponseBodySink, session: &mut dyn ResponseStreamSession) {
        if self.stage_head_policy_reject(sink, session) {
            return;
        }
        if !session.try_claim_response_terminal(ResponseTerminalCause::GuardrailRejected) {
            self.publish_terminal("superseded", "terminal-owner-conflict");
            return;
        }
        let emission = session
            .ai_terminal_render_context()
            .map_or(TerminalEmission::NoEvent, |(dialect, eligibility)| {
                emit_guardrail_rejection(dialect, eligibility, sink)
            });
        session.mark_response_terminal_emission(emission);
        sink.terminate();
        self.publish_terminal("reject", "policy");
    }

    /// Inclusive rollback bound: emitted output, the proposed decision, and every
    /// remaining raw record. `None` when it overflows or exceeds the callback limit.
    fn rollback_bound(output_len: usize, emitted_len: usize, remaining_raw_bytes: Option<usize>) -> Option<usize> {
        output_len
            .checked_add(emitted_len)
            .and_then(|value| value.checked_add(remaining_raw_bytes?))
            .filter(|value| *value <= MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES)
    }

    /// Admit the inclusive rollback bound, then reserve only the decision
    /// output. Remaining records are usually retained rather than emitted, so
    /// reserving them would ship unused capacity downstream with the frozen
    /// chunk; a later fail-open tail reserves its own exact size.
    fn reserve_rollback(output: &mut BytesMut, emitted_len: usize, remaining_raw_bytes: Option<usize>) -> bool {
        let decision_bound = output.len().checked_add(emitted_len);
        Self::rollback_bound(output.len(), emitted_len, remaining_raw_bytes).is_some()
            && decision_bound.is_some_and(|bound| Self::reserve_output(output, bound).is_ok())
    }

    async fn process_batch(
        &mut self,
        batch: CanonicalRecordBatch,
        end_of_stream: bool,
        chunk: &mut Option<Bytes>,
        sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
    ) {
        if self.disabled {
            match batch.into_raw_bytes() {
                Ok(bytes) => *chunk = Some(bytes),
                Err(()) => {
                    *chunk = None;
                    self.terminate_dependency(sink, session, "raw-output-limit");
                }
            }
            return;
        }
        if batch.status() == CanonicalSourceStatus::Unavailable {
            self.summary.input_bytes = self.summary.input_bytes.saturating_add(batch.raw_len() as u64);
            let records = VecDeque::from(batch.into_records());
            let mut output = BytesMut::new();
            self.fail_dependency(&mut output, records, sink, session, "semantic-source");
            *chunk = (!output.is_empty()).then(|| output.freeze());
            return;
        }
        // Available batches already carry the checked sum of their records.
        // A rejected retain keeps the current record in this total until retry succeeds.
        let mut remaining_raw_bytes = Some(batch.raw_len());
        let mut records = VecDeque::from(batch.into_records());
        let mut output = BytesMut::new();
        while let Some(mut record) = records.pop_front() {
            let record_raw_len = record.raw_len();
            self.summary.input_bytes = self.summary.input_bytes.saturating_add(record_raw_len as u64);
            loop {
                match self.window.try_retain(record) {
                    Ok(()) => {
                        remaining_raw_bytes = remaining_raw_bytes.and_then(|bytes| bytes.checked_sub(record_raw_len));
                        break;
                    }
                    Err((_error, returned)) => {
                        record = returned;
                        let Ok(stats) = self.window.stats() else {
                            records.push_front(record);
                            self.fail_dependency(&mut output, records, sink, session, "semantic-source");
                            *chunk = (!output.is_empty()).then(|| output.freeze());
                            return;
                        };
                        let has_raw = stats.raw_bytes != 0;
                        if !has_raw {
                            records.push_front(record);
                            self.fail_dependency(&mut output, records, sink, session, "window-limit");
                            *chunk = (!output.is_empty()).then(|| output.freeze());
                            return;
                        }
                        let semantic = stats.current_semantic_bytes != 0;
                        let decision = if semantic {
                            match self.decide(session, sink).await {
                                Ok(decision) => decision,
                                Err(reason) => {
                                    records.push_front(record);
                                    self.fail_dependency(&mut output, records, sink, session, reason);
                                    *chunk = (!output.is_empty()).then(|| output.freeze());
                                    return;
                                }
                            }
                        } else {
                            ParsedDecision::Pass
                        };
                        records.push_front(record);
                        let emitted_len = match &decision {
                            ParsedDecision::Pass => stats.raw_bytes,
                            ParsedDecision::Replace(replacements) => {
                                match self.window.preview_semantic_replace(
                                    replacements.iter().map(|(key, text)| (*key, text.as_str())),
                                ) {
                                    Ok(len) => len,
                                    Err(_) => {
                                        self.fail_dependency(&mut output, records, sink, session, "replacement");
                                        *chunk = (!output.is_empty()).then(|| output.freeze());
                                        return;
                                    }
                                }
                            }
                            ParsedDecision::Reject => 0,
                        };
                        if !Self::reserve_rollback(&mut output, emitted_len, remaining_raw_bytes) {
                            self.fail_dependency(&mut output, records, sink, session, "callback-output-limit");
                            *chunk = (!output.is_empty()).then(|| output.freeze());
                            return;
                        }
                        let resolved_result = if semantic {
                            self.apply_decision(decision)
                        } else {
                            self.window
                                .resolve(WindowDecision::Release)
                                .map(|output| (output, false))
                                .map_err(|_| "window")
                        };
                        let (resolved, reject) = match resolved_result {
                            Ok(result) => result,
                            Err(reason) => {
                                self.fail_dependency(&mut output, records, sink, session, reason);
                                *chunk = (!output.is_empty()).then(|| output.freeze());
                                return;
                            }
                        };
                        if reject {
                            if sink.response_head_is_held() {
                                output.clear();
                            }
                            self.summary.output_bytes = self.summary.output_bytes.saturating_add(output.len() as u64);
                            self.emit_policy_reject(sink, session);
                            *chunk = (!output.is_empty()).then(|| output.freeze());
                            return;
                        }
                        if Self::append_output(&mut output, resolved).is_err() || self.start_next_window().is_err() {
                            self.fail_dependency(&mut output, records, sink, session, "window-state");
                            *chunk = (!output.is_empty()).then(|| output.freeze());
                            return;
                        }
                        if semantic && !self.release_first_head_window(sink, session) {
                            output.clear();
                            *chunk = None;
                            return;
                        }
                        record = records.pop_front().expect("failed record was restored");
                    }
                }
            }
            let Ok(stats) = self.window.incremental_stats(&mut self.semantic_progress) else {
                self.fail_dependency(&mut output, records, sink, session, "semantic-source");
                *chunk = (!output.is_empty()).then(|| output.freeze());
                return;
            };
            let should_decide = stats.current_semantic_bytes >= self.window_bytes;
            if should_decide {
                let decision = match self.decide(session, sink).await {
                    Ok(decision) => decision,
                    Err(reason) => {
                        self.fail_dependency(&mut output, records, sink, session, reason);
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                };
                let emitted_len = match &decision {
                    ParsedDecision::Pass => stats.raw_bytes,
                    ParsedDecision::Replace(replacements) => {
                        match self
                            .window
                            .preview_semantic_replace(replacements.iter().map(|(key, text)| (*key, text.as_str())))
                        {
                            Ok(len) => len,
                            Err(_) => {
                                self.fail_dependency(&mut output, records, sink, session, "replacement");
                                *chunk = (!output.is_empty()).then(|| output.freeze());
                                return;
                            }
                        }
                    }
                    ParsedDecision::Reject => 0,
                };
                if !Self::reserve_rollback(&mut output, emitted_len, remaining_raw_bytes) {
                    self.fail_dependency(&mut output, records, sink, session, "callback-output-limit");
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
                let (resolved, reject) = match self.apply_decision(decision) {
                    Ok(result) => result,
                    Err(reason) => {
                        self.fail_dependency(&mut output, records, sink, session, reason);
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                };
                if reject {
                    if sink.response_head_is_held() {
                        output.clear();
                    }
                    self.summary.output_bytes = self.summary.output_bytes.saturating_add(output.len() as u64);
                    self.emit_policy_reject(sink, session);
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
                if Self::append_output(&mut output, resolved).is_err() {
                    self.fail_dependency(&mut output, records, sink, session, "callback-output-limit");
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
                if self.start_next_window().is_err() {
                    self.fail_dependency(&mut output, records, sink, session, "window-state");
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
                if !self.release_first_head_window(sink, session) {
                    output.clear();
                    *chunk = None;
                    return;
                }
            }
        }
        if end_of_stream {
            let Ok(stats) = self.window.stats() else {
                self.fail_dependency(&mut output, VecDeque::new(), sink, session, "semantic-source");
                *chunk = (!output.is_empty()).then(|| output.freeze());
                return;
            };
            let semantic = stats.current_semantic_bytes != 0;
            let resolved = if semantic {
                let decision = match self.decide(session, sink).await {
                    Ok(decision) => decision,
                    Err(reason) => {
                        self.fail_dependency(&mut output, VecDeque::new(), sink, session, reason);
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                };
                let emitted_len = match &decision {
                    ParsedDecision::Pass => stats.raw_bytes,
                    ParsedDecision::Replace(replacements) => {
                        match self
                            .window
                            .preview_semantic_replace(replacements.iter().map(|(key, text)| (*key, text.as_str())))
                        {
                            Ok(len) => len,
                            Err(_) => {
                                self.fail_dependency(&mut output, VecDeque::new(), sink, session, "replacement");
                                *chunk = (!output.is_empty()).then(|| output.freeze());
                                return;
                            }
                        }
                    }
                    ParsedDecision::Reject => 0,
                };
                if !Self::reserve_rollback(&mut output, emitted_len, Some(0)) {
                    self.fail_dependency(&mut output, VecDeque::new(), sink, session, "callback-output-limit");
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
                match self.apply_final_decision(decision) {
                    Ok((output, false)) => output,
                    Ok((_output, true)) => {
                        if sink.response_head_is_held() {
                            output.clear();
                        }
                        self.summary.output_bytes = self.summary.output_bytes.saturating_add(output.len() as u64);
                        self.emit_policy_reject(sink, session);
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                    Err(reason) => {
                        self.fail_dependency(&mut output, VecDeque::new(), sink, session, reason);
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                }
            } else {
                match self.window.finish(WindowDecision::Release) {
                    Ok(output) => output,
                    Err(_) => {
                        self.fail_dependency(&mut output, VecDeque::new(), sink, session, "window-state");
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                }
            };
            if Self::append_output(&mut output, resolved).is_err() {
                output.clear();
                self.terminate_dependency(sink, session, "callback-output-limit");
                *chunk = None;
                return;
            }
            if !self.release_first_head_window(sink, session) {
                output.clear();
                *chunk = None;
                return;
            }
        }
        self.summary.output_bytes = self.summary.output_bytes.saturating_add(output.len() as u64);
        if end_of_stream {
            let outcome = if self.summary.replaced != 0 { "replace" } else { "pass" };
            self.publish_terminal(outcome, "normal-eos");
        }
        *chunk = (!output.is_empty()).then(|| output.freeze());
    }
}

#[async_trait]
impl StreamingResponseProcessor for GuardrailResponseProcessor {
    fn changes_body_length(&self) -> bool {
        true
    }

    fn finalize_incomplete(&mut self, outcome: &'static str, reason: &'static str) {
        self.publish_terminal(outcome, reason);
    }

    fn on_terminal_preempted(&mut self, winner: ResponseTerminalCause) {
        let _ = self.window.resolve(WindowDecision::Discard);
        self.disabled = true;
        self.publish_terminal("superseded", winner.as_str());
    }

    fn release_inflight(&mut self) {
        let _ = self.window.resolve(WindowDecision::Discard);
        self.disabled = true;
        self.publish_terminal("cancelled", "callback-dropped");
    }

    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
    ) {
        if stream_kind != ResponseStreamKind::Sse {
            let records = chunk
                .take()
                .map(|bytes| VecDeque::from([crate::sse::window::WindowRecord::raw(vec![bytes])]))
                .unwrap_or_default();
            let mut output = BytesMut::new();
            self.fail_dependency(&mut output, records, sink, session, "not-canonical-sse");
            *chunk = (!output.is_empty()).then(|| output.freeze());
            return;
        }
        let Some(batch) = session.take_canonical_records() else {
            // A split event legitimately produces no record until a later
            // callback. The canonical source already owns and cleared bytes.
            if end_of_stream {
                let records = chunk
                    .take()
                    .map(|bytes| VecDeque::from([crate::sse::window::WindowRecord::raw(vec![bytes])]))
                    .unwrap_or_default();
                let mut output = BytesMut::new();
                self.fail_dependency(&mut output, records, sink, session, "missing-eos-source");
                *chunk = (!output.is_empty()).then(|| output.freeze());
            }
            return;
        };
        self.process_batch(batch, end_of_stream, chunk, sink, session).await;
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/guards/semantic/response_test.rs"]
mod tests;
