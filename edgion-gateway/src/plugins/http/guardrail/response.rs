//! Canonical semantic SSE window processor for Guardrail unary decisions.

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use pingora_proxy::ResponseBodySink;
use tokio::sync::Semaphore;

use super::attached::{AttachedGuardrail, GuardrailSummary};
use super::protocol::{encode_request, is_json_response, parse_decision, ParsedDecision, RequestWindow};
use crate::ai::canonical_sse::{CanonicalRecordBatch, CanonicalSourceStatus, MAX_CANONICAL_SOURCE_CHUNK_BYTES};
use crate::ctx::ResponseStreamKind;
use crate::plugins::http::common::outbound::call::outbound_call_with_body;
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::runtime::traits::StreamingResponseProcessor;
use crate::plugins::runtime::PluginSession;
use crate::sse::window::{
    RawWindow, RawWindowPolicy, SemanticReplacement, WindowDecision, WindowError, WindowOutput, MAX_RAW_BYTES,
};
use crate::sse::DEFAULT_MAX_PENDING_EVENT_BYTES;

const MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES: usize =
    MAX_RAW_BYTES + DEFAULT_MAX_PENDING_EVENT_BYTES + MAX_CANONICAL_SOURCE_CHUNK_BYTES;
const MAX_DECISION_RESPONSE_BYTES: usize = 1024 * 1024;

pub(super) struct GuardrailResponseProcessor {
    client: Arc<ManagedOutboundClient>,
    permits: Arc<Semaphore>,
    owner: Arc<AttachedGuardrail>,
    window: RawWindow,
    window_bytes: usize,
    max_request_bytes: usize,
    max_replacement_bytes: usize,
    fail_open: bool,
    disabled: bool,
    summary: GuardrailSummary,
}

impl GuardrailResponseProcessor {
    fn output_buffer() -> Result<BytesMut, ()> {
        let output = BytesMut::with_capacity(MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES);
        (output.capacity() <= MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES)
            .then_some(output)
            .ok_or(())
    }

    pub(super) fn new(
        client: Arc<ManagedOutboundClient>,
        permits: Arc<Semaphore>,
        owner: Arc<AttachedGuardrail>,
        window_bytes: usize,
        overlap_bytes: usize,
        max_request_bytes: usize,
        max_replacement_bytes: usize,
        fail_open: bool,
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
            window_bytes,
            max_request_bytes,
            max_replacement_bytes,
            fail_open,
            disabled: false,
            summary: GuardrailSummary::default(),
        })
    }

    fn append_output(output: &mut BytesMut, resolved: WindowOutput) -> Result<(), ()> {
        match resolved {
            WindowOutput::Release(slices) => {
                for slice in slices {
                    let next = output.len().checked_add(slice.len()).ok_or(())?;
                    if next > MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES {
                        return Err(());
                    }
                    output.extend_from_slice(&slice);
                }
            }
            WindowOutput::Replace(bytes) => {
                let next = output.len().checked_add(bytes.len()).ok_or(())?;
                if next > MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES {
                    return Err(());
                }
                output.extend_from_slice(&bytes);
            }
            WindowOutput::Discard => {}
        }
        Ok(())
    }

    fn publish_terminal(&mut self, outcome: &'static str, reason: &'static str) {
        self.summary.outcome = outcome;
        self.summary.reason = reason;
        self.owner.publish(self.summary.clone());
    }

    async fn decide(&mut self, session: &dyn PluginSession) -> Result<ParsedDecision, &'static str> {
        let snapshot = self.window.snapshot_all().map_err(|_| "semantic-source")?;
        if snapshot.is_empty() || snapshot.iter().all(|entry| entry.current.is_empty()) {
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
            let _permit = Arc::clone(&self.permits)
                .acquire_owned()
                .await
                .map_err(|_| "concurrency-closed")?;
            outbound_call_with_body(
                &self.client,
                session,
                None,
                self.client.max_response_bytes.min(MAX_DECISION_RESPONSE_BYTES),
                &[("content-type", "application/json")],
                Some(request),
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
        reason: &'static str,
    ) {
        if self.fail_open {
            match self.window.resolve(WindowDecision::Release) {
                Ok(resolved) => {
                    if Self::append_output(output, resolved).is_err() {
                        output.clear();
                        sink.terminate();
                        self.publish_terminal("fail-close", "callback-output-limit");
                        return;
                    }
                }
                Err(_) => {
                    output.clear();
                    sink.terminate();
                    self.publish_terminal("fail-close", "window-release");
                    return;
                }
            }
            for record in records {
                for slice in record.into_raw_slices() {
                    let next = output.len().checked_add(slice.len());
                    if !next.is_some_and(|next| next <= MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES) {
                        output.clear();
                        sink.terminate();
                        self.publish_terminal("fail-close", "callback-output-limit");
                        return;
                    }
                    output.extend_from_slice(&slice);
                }
            }
            self.disabled = true;
            self.summary.output_bytes = self.summary.output_bytes.saturating_add(output.len() as u64);
            self.publish_terminal("fail-open", reason);
        } else {
            output.clear();
            let _ = self.window.resolve(WindowDecision::Discard);
            sink.terminate();
            self.publish_terminal("fail-close", reason);
        }
    }

    fn remaining_raw_bytes(records: &VecDeque<crate::sse::window::WindowRecord>) -> Option<usize> {
        records
            .iter()
            .try_fold(0usize, |total, record| total.checked_add(record.raw_len()))
    }

    fn rollback_fits(
        &mut self,
        output_len: usize,
        emitted_len: usize,
        remaining: &VecDeque<crate::sse::window::WindowRecord>,
    ) -> bool {
        output_len
            .checked_add(emitted_len)
            .and_then(|value| value.checked_add(Self::remaining_raw_bytes(remaining)?))
            .is_some_and(|value| value <= MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES)
    }

    async fn process_batch(
        &mut self,
        batch: CanonicalRecordBatch,
        end_of_stream: bool,
        chunk: &mut Option<Bytes>,
        sink: &mut ResponseBodySink,
        session: &dyn PluginSession,
    ) {
        if self.disabled {
            match batch.into_raw_bytes() {
                Ok(bytes) => *chunk = Some(bytes),
                Err(()) => {
                    *chunk = None;
                    sink.terminate();
                }
            }
            return;
        }
        if batch.status() == CanonicalSourceStatus::Unavailable {
            self.summary.input_bytes = self.summary.input_bytes.saturating_add(batch.raw_len() as u64);
            let records = VecDeque::from(batch.into_records());
            let Ok(mut output) = Self::output_buffer() else {
                sink.terminate();
                self.publish_terminal("fail-close", "callback-output-limit");
                *chunk = None;
                return;
            };
            self.fail_dependency(&mut output, records, sink, "semantic-source");
            *chunk = (!output.is_empty()).then(|| output.freeze());
            return;
        }
        let mut records = VecDeque::from(batch.into_records());
        let Ok(mut output) = Self::output_buffer() else {
            sink.terminate();
            self.publish_terminal("fail-close", "callback-output-limit");
            *chunk = None;
            return;
        };
        while let Some(mut record) = records.pop_front() {
            self.summary.input_bytes = self.summary.input_bytes.saturating_add(record.raw_len() as u64);
            loop {
                match self.window.try_retain(record) {
                    Ok(()) => break,
                    Err((_error, returned)) => {
                        record = returned;
                        let stats = match self.window.stats() {
                            Ok(stats) => stats,
                            Err(_) => {
                                records.push_front(record);
                                self.fail_dependency(&mut output, records, sink, "semantic-source");
                                *chunk = (!output.is_empty()).then(|| output.freeze());
                                return;
                            }
                        };
                        let has_raw = stats.raw_bytes != 0;
                        if !has_raw {
                            records.push_front(record);
                            self.fail_dependency(&mut output, records, sink, "window-limit");
                            *chunk = (!output.is_empty()).then(|| output.freeze());
                            return;
                        }
                        let semantic = stats.current_semantic_bytes != 0;
                        let decision = if semantic {
                            match self.decide(session).await {
                                Ok(decision) => decision,
                                Err(reason) => {
                                    records.push_front(record);
                                    self.fail_dependency(&mut output, records, sink, reason);
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
                                let replacements: Vec<_> = replacements
                                    .iter()
                                    .map(|(key, current)| SemanticReplacement {
                                        key: *key,
                                        current: current.clone(),
                                    })
                                    .collect();
                                match self.window.preview_semantic_replace(&replacements) {
                                    Ok(len) => len,
                                    Err(_) => {
                                        self.fail_dependency(&mut output, records, sink, "replacement");
                                        *chunk = (!output.is_empty()).then(|| output.freeze());
                                        return;
                                    }
                                }
                            }
                            ParsedDecision::Reject => 0,
                        };
                        if !self.rollback_fits(output.len(), emitted_len, &records) {
                            self.fail_dependency(&mut output, records, sink, "callback-output-limit");
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
                                self.fail_dependency(&mut output, records, sink, reason);
                                *chunk = (!output.is_empty()).then(|| output.freeze());
                                return;
                            }
                        };
                        if reject {
                            output.clear();
                            sink.terminate();
                            self.publish_terminal("reject", "policy");
                            *chunk = None;
                            return;
                        }
                        if Self::append_output(&mut output, resolved).is_err() || self.window.start_next().is_err() {
                            self.fail_dependency(&mut output, records, sink, "window-state");
                            *chunk = (!output.is_empty()).then(|| output.freeze());
                            return;
                        }
                        record = records.pop_front().expect("failed record was restored");
                    }
                }
            }
            let stats = match self.window.stats() {
                Ok(stats) => stats,
                Err(_) => {
                    self.fail_dependency(&mut output, records, sink, "semantic-source");
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
            };
            let should_decide = stats.current_semantic_bytes >= self.window_bytes;
            if should_decide {
                let decision = match self.decide(session).await {
                    Ok(decision) => decision,
                    Err(reason) => {
                        self.fail_dependency(&mut output, records, sink, reason);
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                };
                let emitted_len = match &decision {
                    ParsedDecision::Pass => stats.raw_bytes,
                    ParsedDecision::Replace(replacements) => {
                        let replacements: Vec<_> = replacements
                            .iter()
                            .map(|(key, current)| SemanticReplacement {
                                key: *key,
                                current: current.clone(),
                            })
                            .collect();
                        match self.window.preview_semantic_replace(&replacements) {
                            Ok(len) => len,
                            Err(_) => {
                                self.fail_dependency(&mut output, records, sink, "replacement");
                                *chunk = (!output.is_empty()).then(|| output.freeze());
                                return;
                            }
                        }
                    }
                    ParsedDecision::Reject => 0,
                };
                if !self.rollback_fits(output.len(), emitted_len, &records) {
                    self.fail_dependency(&mut output, records, sink, "callback-output-limit");
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
                let (resolved, reject) = match self.apply_decision(decision) {
                    Ok(result) => result,
                    Err(reason) => {
                        self.fail_dependency(&mut output, records, sink, reason);
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                };
                if reject {
                    output.clear();
                    sink.terminate();
                    self.publish_terminal("reject", "policy");
                    *chunk = None;
                    return;
                }
                if Self::append_output(&mut output, resolved).is_err() {
                    self.fail_dependency(&mut output, records, sink, "callback-output-limit");
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
                if self.window.start_next().is_err() {
                    self.fail_dependency(&mut output, records, sink, "window-state");
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
            }
        }
        if end_of_stream {
            let stats = match self.window.stats() {
                Ok(stats) => stats,
                Err(_) => {
                    self.fail_dependency(&mut output, VecDeque::new(), sink, "semantic-source");
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
            };
            let semantic = stats.current_semantic_bytes != 0;
            let resolved = if semantic {
                let decision = match self.decide(session).await {
                    Ok(decision) => decision,
                    Err(reason) => {
                        self.fail_dependency(&mut output, VecDeque::new(), sink, reason);
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                };
                let emitted_len = match &decision {
                    ParsedDecision::Pass => stats.raw_bytes,
                    ParsedDecision::Replace(replacements) => {
                        let replacements: Vec<_> = replacements
                            .iter()
                            .map(|(key, current)| SemanticReplacement {
                                key: *key,
                                current: current.clone(),
                            })
                            .collect();
                        match self.window.preview_semantic_replace(&replacements) {
                            Ok(len) => len,
                            Err(_) => {
                                self.fail_dependency(&mut output, VecDeque::new(), sink, "replacement");
                                *chunk = (!output.is_empty()).then(|| output.freeze());
                                return;
                            }
                        }
                    }
                    ParsedDecision::Reject => 0,
                };
                if !self.rollback_fits(output.len(), emitted_len, &VecDeque::new()) {
                    self.fail_dependency(&mut output, VecDeque::new(), sink, "callback-output-limit");
                    *chunk = (!output.is_empty()).then(|| output.freeze());
                    return;
                }
                match self.apply_final_decision(decision) {
                    Ok((output, false)) => output,
                    Ok((_output, true)) => {
                        output.clear();
                        sink.terminate();
                        self.publish_terminal("reject", "policy");
                        *chunk = None;
                        return;
                    }
                    Err(reason) => {
                        self.fail_dependency(&mut output, VecDeque::new(), sink, reason);
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                }
            } else {
                match self.window.finish(WindowDecision::Release) {
                    Ok(output) => output,
                    Err(_) => {
                        self.fail_dependency(&mut output, VecDeque::new(), sink, "window-state");
                        *chunk = (!output.is_empty()).then(|| output.freeze());
                        return;
                    }
                }
            };
            if Self::append_output(&mut output, resolved).is_err() {
                output.clear();
                sink.terminate();
                self.publish_terminal("fail-close", "callback-output-limit");
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
    fn name(&self) -> &str {
        "guardrail"
    }

    fn changes_body_length(&self) -> bool {
        true
    }

    fn finalize_incomplete(&mut self, outcome: &'static str, reason: &'static str) {
        self.publish_terminal(outcome, reason);
    }

    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        if stream_kind != ResponseStreamKind::Sse {
            let records = chunk
                .take()
                .map(|bytes| VecDeque::from([crate::sse::window::WindowRecord::raw(vec![bytes])]))
                .unwrap_or_default();
            let Ok(mut output) = Self::output_buffer() else {
                sink.terminate();
                self.publish_terminal("fail-close", "callback-output-limit");
                *chunk = None;
                return;
            };
            self.fail_dependency(&mut output, records, sink, "not-canonical-sse");
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
                let Ok(mut output) = Self::output_buffer() else {
                    sink.terminate();
                    self.publish_terminal("fail-close", "callback-output-limit");
                    *chunk = None;
                    return;
                };
                self.fail_dependency(&mut output, records, sink, "missing-eos-source");
                *chunk = (!output.is_empty()).then(|| output.freeze());
            }
            return;
        };
        self.process_batch(batch, end_of_stream, chunk, sink, session).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::sse::window::{LogicalBucketKey, LogicalField, SemanticFragment, SemanticSource, WindowRecord};
    use crate::sse::ByteRange;

    fn processor(fail_open: bool) -> GuardrailResponseProcessor {
        GuardrailResponseProcessor::new(
            Arc::new(ManagedOutboundClient::test_with_static_target("http://localhost:1")),
            Arc::new(Semaphore::new(1)),
            Arc::new(AttachedGuardrail::new()),
            1,
            0,
            4096,
            4096,
            fail_open,
        )
        .unwrap()
    }

    fn semantic_record(text: &str) -> WindowRecord {
        let raw = Bytes::from(format!(
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{}}}}}]}}\n\n",
            serde_json::to_string(text).unwrap()
        ));
        let encoded = serde_json::to_vec(text).unwrap();
        let start = raw.windows(encoded.len()).position(|part| part == encoded).unwrap();
        WindowRecord::raw(vec![raw]).with_fragments(vec![SemanticFragment {
            bucket: LogicalBucketKey {
                choice_index: 0,
                field: LogicalField::Content,
            },
            source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(start, start + encoded.len())]),
        }])
    }

    fn mock_request_session() -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("GET".to_string());
        session.expect_get_request_path().return_const("/".to_string());
        session
    }

    fn localhost_uri(uri: &str) -> String {
        uri.replace("127.0.0.1", "localhost")
    }

    #[test]
    fn pass_replace_and_reject_resolve_one_generation_atomically() {
        let mut pass = processor(false);
        let record = semantic_record("original");
        let expected: Vec<u8> = record.raw_slices_cloned().into_iter().flatten().collect();
        pass.window.retain(record).unwrap();
        let (released, reject) = pass.apply_decision(ParsedDecision::Pass).unwrap();
        assert!(!reject);
        let WindowOutput::Release(slices) = released else {
            panic!("Pass must release original slices")
        };
        assert_eq!(slices.into_iter().flatten().collect::<Vec<_>>(), expected);

        let key = LogicalBucketKey {
            choice_index: 0,
            field: LogicalField::Content,
        };
        let mut replace = processor(false);
        replace.window.retain(semantic_record("original")).unwrap();
        let (rewritten, reject) = replace
            .apply_decision(ParsedDecision::Replace(vec![(key, "reviewed".to_string())]))
            .unwrap();
        assert!(!reject);
        let WindowOutput::Replace(rewritten) = rewritten else {
            panic!("Replace must emit rewritten SSE")
        };
        let rewritten = String::from_utf8(rewritten.to_vec()).unwrap();
        assert!(rewritten.contains("\"content\":\"reviewed\""));
        assert!(!rewritten.contains("original"));

        let mut reject = processor(true);
        reject.window.retain(semantic_record("blocked")).unwrap();
        let (discarded, policy_reject) = reject.apply_decision(ParsedDecision::Reject).unwrap();
        assert!(policy_reject, "policy Reject is independent of fail-open");
        assert_eq!(discarded, WindowOutput::Discard);
    }

    #[test]
    fn rollback_reservation_is_inclusive_and_checked() {
        let mut processor = processor(true);
        let remaining = VecDeque::from([WindowRecord::raw(vec![Bytes::from_static(b"tail")])]);
        assert!(processor.rollback_fits(MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES - 8, 4, &remaining));
        assert!(!processor.rollback_fits(MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES - 7, 4, &remaining));
        assert!(!processor.rollback_fits(usize::MAX, 1, &remaining));
        let output = GuardrailResponseProcessor::output_buffer().unwrap();
        assert_eq!(output.capacity(), MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES);
    }

    #[test]
    fn incomplete_finalization_preserves_counters_and_is_exactly_once() {
        let mut processor = processor(false);
        processor.summary.windows = 3;
        processor.summary.passed = 2;
        processor.finalize_incomplete("upstream-abort", "stream-error");
        processor.finalize_incomplete("cancelled", "downstream-cancel");
        let line = processor.owner.take_summary_line().unwrap();
        assert!(line.contains("windows=3"));
        assert!(line.contains("passed=2"));
        assert!(line.contains("outcome=upstream-abort"));
        assert!(processor.owner.take_summary_line().is_none());
    }

    #[tokio::test]
    async fn non_sse_and_missing_eos_source_fail_open_byte_identically() {
        let original = Bytes::from_static(b"unclassified response bytes");
        let mut non_sse = processor(true);
        let mut chunk = Some(original.clone());
        let mut sink = ResponseBodySink::new();
        let mut session = MockPluginSession::new();
        non_sse
            .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert_eq!(chunk, Some(original));
        assert!(!sink.is_terminated());

        let tail = Bytes::from_static(b"unterminated canonical tail");
        let mut missing_eos = processor(true);
        let mut chunk = Some(tail.clone());
        let mut sink = ResponseBodySink::new();
        let mut session = MockPluginSession::new();
        session.expect_take_canonical_records().once().return_once(|| None);
        missing_eos
            .on_chunk(&mut chunk, true, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;
        assert_eq!(chunk, Some(tail));
        assert!(!sink.is_terminated());
    }

    #[tokio::test]
    async fn semantic_stats_failure_obeys_fail_policy_without_losing_raw_bytes() {
        let raw = Bytes::from_static(b"\xff");
        let invalid = WindowRecord::raw(vec![raw.clone()]).with_fragments(vec![SemanticFragment {
            bucket: LogicalBucketKey {
                choice_index: 0,
                field: LogicalField::Content,
            },
            source: SemanticSource::Utf8Ranges(vec![ByteRange::new(0, 1)]),
        }]);

        let mut open_batch = CanonicalRecordBatch::new();
        open_batch.push(invalid.clone()).unwrap();
        let mut open = processor(true);
        let mut chunk = None;
        let mut sink = ResponseBodySink::new();
        open.process_batch(open_batch, false, &mut chunk, &mut sink, &MockPluginSession::new())
            .await;
        assert_eq!(chunk, Some(raw));
        assert!(!sink.is_terminated());

        let mut close_batch = CanonicalRecordBatch::new();
        close_batch.push(invalid).unwrap();
        let mut close = processor(false);
        let mut chunk = None;
        let mut sink = ResponseBodySink::new();
        close
            .process_batch(close_batch, false, &mut chunk, &mut sink, &MockPluginSession::new())
            .await;
        assert!(chunk.is_none());
        assert!(sink.is_terminated());
    }

    #[tokio::test]
    async fn eos_flushes_raw_tail_once_without_a_callout() {
        let first = Bytes::from_static(b": comment\n\n");
        let tail = Bytes::from_static(b"data: [DONE]\n\n");
        let mut batch = CanonicalRecordBatch::new();
        batch.push(WindowRecord::raw(vec![first.clone()])).unwrap();
        batch.push(WindowRecord::raw(vec![tail.clone()])).unwrap();
        let mut processor = processor(false);
        let mut chunk = None;
        let mut sink = ResponseBodySink::new();
        processor
            .process_batch(batch, true, &mut chunk, &mut sink, &MockPluginSession::new())
            .await;
        let expected = [first.as_ref(), tail.as_ref()].concat();
        assert_eq!(chunk.as_deref(), Some(expected.as_slice()));
        assert!(!sink.is_terminated());
        let summary = processor.owner.take_summary_line().unwrap();
        assert!(summary.contains("windows=0"));
        assert!(summary.contains("outcome=pass"));
    }

    #[tokio::test]
    async fn one_callback_can_make_multiple_serial_policy_calls() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("content-type", "application/json")
                    .set_body_json(serde_json::json!({"version":"v1","action":"pass"})),
            )
            .expect(2)
            .mount(&server)
            .await;
        let mut processor = processor(false);
        processor.client = Arc::new(ManagedOutboundClient::test_with_static_target(&localhost_uri(
            &server.uri(),
        )));
        let first = semantic_record("one");
        let second = semantic_record("two");
        let expected: Vec<u8> = first
            .raw_slices_cloned()
            .into_iter()
            .chain(second.raw_slices_cloned())
            .flatten()
            .collect();
        let mut batch = CanonicalRecordBatch::new();
        batch.push(first).unwrap();
        batch.push(second).unwrap();
        let mut chunk = None;
        let mut sink = ResponseBodySink::new();
        processor
            .process_batch(batch, false, &mut chunk, &mut sink, &mock_request_session())
            .await;
        assert_eq!(chunk.as_deref(), Some(expected.as_slice()));
        assert!(!sink.is_terminated());
        assert_eq!(processor.summary.windows, 2);
        assert_eq!(processor.summary.passed, 2);
    }

    #[tokio::test]
    async fn later_dependency_failure_preserves_accepted_replace_and_remaining_originals() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&calls);
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(move |_request: &wiremock::Request| {
                if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(200)
                        .append_header("content-type", "application/json")
                        .set_body_json(serde_json::json!({
                            "version":"v1",
                            "action":"replace",
                            "replacements":[{
                                "choiceIndex":0,
                                "field":{"type":"content"},
                                "current":"reviewed"
                            }]
                        }))
                } else {
                    ResponseTemplate::new(200)
                        .append_header("content-type", "application/json")
                        .set_body_bytes(b"not-json".to_vec())
                }
            })
            .expect(2)
            .mount(&server)
            .await;

        let first = semantic_record("first");
        let second = semantic_record("second");
        let second_raw: Vec<u8> = second.raw_slices_cloned().into_iter().flatten().collect();
        let mut batch = CanonicalRecordBatch::new();
        batch.push(first).unwrap();
        batch.push(second).unwrap();
        let mut processor = processor(true);
        processor.client = Arc::new(ManagedOutboundClient::test_with_static_target(&localhost_uri(
            &server.uri(),
        )));
        let mut chunk = None;
        let mut sink = ResponseBodySink::new();
        processor
            .process_batch(batch, false, &mut chunk, &mut sink, &mock_request_session())
            .await;

        let output = chunk.expect("fail-open must preserve every owned byte");
        assert!(output.windows(b"reviewed".len()).any(|part| part == b"reviewed"));
        assert!(!output.windows(b"first".len()).any(|part| part == b"first"));
        assert!(output.ends_with(&second_raw));
        assert!(!sink.is_terminated());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let summary = processor.owner.take_summary_line().unwrap();
        assert!(summary.contains("outcome=fail-open"));
        assert!(summary.contains("replaced=1"));
    }

    #[tokio::test]
    async fn policy_reject_discards_before_terminate_even_when_fail_open() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("content-type", "application/json")
                    .set_body_json(serde_json::json!({"version":"v1","action":"reject"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut processor = processor(true);
        processor.client = Arc::new(ManagedOutboundClient::test_with_static_target(&localhost_uri(
            &server.uri(),
        )));
        let mut batch = CanonicalRecordBatch::new();
        batch.push(semantic_record("blocked")).unwrap();
        let mut chunk = None;
        let mut sink = ResponseBodySink::new();
        processor
            .process_batch(batch, false, &mut chunk, &mut sink, &mock_request_session())
            .await;
        assert!(chunk.is_none());
        assert!(sink.is_terminated());
        let summary = processor.owner.take_summary_line().unwrap();
        assert!(summary.contains("outcome=reject"));
        assert!(!summary.contains("outcome=fail-open"));
    }

    #[tokio::test]
    async fn permit_saturation_uses_the_same_absolute_call_deadline() {
        let mut client = ManagedOutboundClient::test_with_static_target("http://localhost:1");
        client.timeout = std::time::Duration::from_millis(10);
        let mut processor = GuardrailResponseProcessor::new(
            Arc::new(client),
            Arc::new(Semaphore::new(0)),
            Arc::new(AttachedGuardrail::new()),
            1,
            0,
            4096,
            4096,
            false,
        )
        .unwrap();
        processor.window.retain(semantic_record("wait")).unwrap();
        assert_eq!(processor.decide(&MockPluginSession::new()).await, Err("timeout"));
    }

    #[tokio::test]
    async fn cancelling_an_inflight_decision_releases_the_snapshot_permit() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(1))
                    .append_header("content-type", "application/json")
                    .set_body_json(serde_json::json!({"version":"v1","action":"pass"})),
            )
            .mount(&server)
            .await;
        let permits = Arc::new(Semaphore::new(1));
        let mut processor = GuardrailResponseProcessor::new(
            Arc::new(ManagedOutboundClient::test_with_static_target(&localhost_uri(
                &server.uri(),
            ))),
            Arc::clone(&permits),
            Arc::new(AttachedGuardrail::new()),
            1,
            0,
            4096,
            4096,
            false,
        )
        .unwrap();
        processor.window.retain(semantic_record("cancel")).unwrap();
        let session = mock_request_session();
        let mut decision = Box::pin(processor.decide(&session));
        tokio::select! {
            result = &mut decision => panic!("decision unexpectedly completed: {result:?}"),
            () = tokio::time::sleep(std::time::Duration::from_millis(25)) => {}
        }
        assert_eq!(permits.available_permits(), 0);
        drop(decision);
        assert_eq!(permits.available_permits(), 1);
    }
}
