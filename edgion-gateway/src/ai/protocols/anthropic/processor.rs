//! Native response observation preserves bytes and the existing terminal/quota owners.
use super::usage::{Components, Kind, Observation, Observer};
use crate::ai::accounting::usage::terminate_ai_stream;
use crate::ai::protocols::openai::json_usage::ResponseOutcome;
use crate::ai::quota::QuotaAccumulator;
use crate::ai::{AiProxyError, AiUsageShared};
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::{ResponseStreamSession, StreamingResponseProcessor};
use crate::sse::{SseBlockStatus, SseDecoder, SseRecordRef};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::EdgionErrorCode;
use pingora_http::ResponseHeader;
use pingora_proxy::ResponseBodySink;
use std::sync::Arc;
pub(crate) struct MessagesProcessor {
    shared: Arc<AiUsageShared>,
    quota: Option<Arc<QuotaAccumulator>>,
    requested_streaming: bool,
    reject: bool,
    model: String,
    sse: SseDecoder,
    json: Option<Box<Observer>>,
    components: Components,
    started: bool,
    stopped: bool,
    stop_metadata: bool,
    delta_output: bool,
    finished: bool,
    diagnostic: bool,
    degraded_reported: bool,
    malformed_reported: bool,
}
impl MessagesProcessor {
    pub(crate) fn new(
        shared: Arc<AiUsageShared>,
        quota: Option<Arc<QuotaAccumulator>>,
        stream: bool,
        reject: bool,
        model: String,
    ) -> Self {
        Self {
            shared,
            quota,
            requested_streaming: stream,
            reject,
            model,
            sse: SseDecoder::with_default_limit(),
            json: None,
            components: Components::default(),
            started: false,
            stopped: false,
            stop_metadata: false,
            delta_output: false,
            finished: false,
            diagnostic: false,
            degraded_reported: false,
            malformed_reported: false,
        }
    }
    fn publish(shared: &AiUsageShared, quota: Option<&Arc<QuotaAccumulator>>, components: Components) {
        if shared.observation_unavailable() {
            return;
        }
        if let Some(usage) = components.normalized() {
            shared.observe_reported_usage(
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens,
                usage.cache_write_input_tokens,
            );
            if let Some(quota) = quota {
                quota.observe_output_total(usage.output_tokens);
            }
        } else if components.input.is_some() && components.output.is_some() {
            shared.mark_observation_unavailable();
        }
    }
    // Borrow disjoint decoder/progress fields without allocating callback state.
    #[allow(clippy::too_many_arguments)]
    fn event(
        shared: &AiUsageShared,
        quota: Option<&Arc<QuotaAccumulator>>,
        components: &mut Components,
        started: &mut bool,
        stopped: &mut bool,
        stop_metadata: &mut bool,
        delta_output: &mut bool,
        observed: Observation,
    ) {
        if observed.invalid || observed.composite {
            shared.mark_observation_unavailable();
        }
        if *stopped && observed.kind != Kind::Other {
            shared.mark_observation_unavailable();
        }
        match observed.kind {
            Kind::Start => {
                if *started {
                    shared.mark_observation_unavailable();
                }
                *started = true;
                components.merge(observed.components);
            }
            Kind::Delta => {
                if !*started {
                    shared.mark_observation_unavailable();
                }
                *delta_output |= observed.components.output.is_some();
                *stop_metadata |= observed.stop;
                components.merge(observed.components);
            }
            Kind::Stop => {
                *stopped = true;
                if !*started || !*stop_metadata {
                    shared.mark_observation_unavailable();
                }
                if shared.response_outcome() != ResponseOutcome::Failed {
                    shared.observe_response_outcome(ResponseOutcome::Completed);
                }
                if !*delta_output {
                    shared.mark_observation_unavailable();
                }
            }
            Kind::Error => {
                shared.observe_response_outcome(ResponseOutcome::Failed);
                shared.mark_observation_unavailable();
            }
            _ => {}
        }
        Self::publish(shared, quota, *components);
    }
    fn finish(&mut self, kind: ResponseStreamKind) {
        if self.finished {
            return;
        }
        self.finished = true;
        if kind == ResponseStreamKind::Sse {
            let mut tail = false;
            self.sse.finish_records(|record| {
                tail |= matches!(record, SseRecordRef::EosTail(_));
            });
            if tail || !self.stopped || self.sse.degraded() || self.sse.malformed_events() != 0 {
                self.shared.mark_observation_unavailable();
            }
        } else if !self.shared.encoding_unavailable() {
            let observer = self.json.get_or_insert_with(|| Box::new(Observer::new()));
            let observed = observer.finish(&self.model);
            self.shared.mark_json_complete();
            if observed.kind == Kind::Error {
                self.shared.observe_response_outcome(ResponseOutcome::Failed);
            } else if observed.kind == Kind::Message && observed.stop && !observed.invalid {
                self.shared.observe_response_outcome(ResponseOutcome::Completed);
            } else {
                self.shared.mark_observation_unavailable();
            }
            if observed.invalid || observed.composite || observed.kind == Kind::Error {
                self.shared.mark_observation_unavailable();
            }
            Self::publish(&self.shared, self.quota.as_ref(), observed.components);
        }
    }
}
#[async_trait]
impl StreamingResponseProcessor for MessagesProcessor {
    fn on_response_header(&mut self, response: &mut ResponseHeader) {
        if !crate::routes::http::proxy_http::content_type::response_is_identity_encoded(&response.headers) {
            self.shared.mark_encoding_unavailable();
        }
    }
    fn changes_body_length(&self) -> bool {
        self.quota.is_some() && self.requested_streaming && self.reject
    }
    fn finalize_incomplete(&mut self, _: &'static str, _: &'static str) {
        if !self.finished {
            self.shared.mark_observation_unavailable();
        }
    }
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        eos: bool,
        kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
    ) {
        if !self.shared.encoding_unavailable() {
            if let Some(bytes) = chunk.as_ref() {
                if kind == ResponseStreamKind::Sse {
                    let shared = &self.shared;
                    let quota = self.quota.as_ref();
                    let components = &mut self.components;
                    let started = &mut self.started;
                    let stopped = &mut self.stopped;
                    let stop = &mut self.stop_metadata;
                    let output = &mut self.delta_output;
                    let model = &mut self.model;
                    self.sse.feed_records(bytes.clone(), |record| {
                        if let SseRecordRef::Block { status, event } = record {
                            match status {
                                SseBlockStatus::Dataless => return,
                                SseBlockStatus::Poisoned => {
                                    shared.mark_observation_unavailable();
                                    return;
                                }
                                SseBlockStatus::Data => {}
                            }
                            let envelope = match event.event().as_deref() {
                                Some("message_start") => Kind::Start,
                                Some("message_delta") => Kind::Delta,
                                Some("message_stop") => Kind::Stop,
                                Some("error") => Kind::Error,
                                _ => Kind::Other,
                            };
                            // Unknown events remain transparent and cannot contribute lifecycle metadata.
                            if envelope == Kind::Other {
                                return;
                            }
                            let mut observer = Observer::for_envelope(envelope);
                            let Ok(data) = event.semantic_data() else {
                                shared.mark_observation_unavailable();
                                return;
                            };
                            observer.feed(data.as_bytes());
                            let mut observed = observer.finish(model);
                            if observed.kind != envelope {
                                observed.invalid = true;
                            }
                            if envelope == Kind::Error {
                                observed.kind = Kind::Error;
                            }
                            if envelope == Kind::Start
                                && !*started
                                && !observed.invalid
                                && !observed.composite
                                && !shared.observation_unavailable()
                            {
                                // Transfer the bounded selected model; later deltas may omit it.
                                if let Some(actual_model) = observed.model.take() {
                                    *model = actual_model;
                                }
                            }
                            Self::event(shared, quota, components, started, stopped, stop, output, observed);
                        }
                    });
                    if self.sse.degraded() || self.sse.malformed_events() != 0 {
                        self.shared.mark_observation_unavailable();
                    }
                } else {
                    self.shared.mark_json_started();
                    let observer = self.json.get_or_insert_with(|| Box::new(Observer::new()));
                    observer.feed(bytes);
                    if observer.failed() {
                        self.shared.mark_observation_unavailable();
                    }
                }
            }
            if eos {
                self.finish(kind);
            }
        }
        if self.shared.observation_unavailable() && !self.diagnostic {
            self.diagnostic = true;
            if self.shared.claim_unavailable_diagnostic() {
                session.record_error(EdgionErrorCode::AiUsageObservationUnavailable);
            }
        }
        if self.sse.degraded() && !self.degraded_reported {
            self.degraded_reported = true;
            session.record_error(EdgionErrorCode::AiSseDecodeDegraded);
        }
        if self.sse.malformed_events() != 0 && !self.malformed_reported {
            self.malformed_reported = true;
            session.record_error(EdgionErrorCode::AiSseDecodeMalformed);
        }
        if eos || !self.changes_body_length() {
            return;
        }
        let (error, code) = if self.quota.as_ref().is_some_and(|q| q.over_limit()) {
            (AiProxyError::QuotaExceeded, EdgionErrorCode::AiQuotaOverLimitTerminated)
        } else if self.shared.observation_unavailable() {
            (
                AiProxyError::QuotaUnavailable,
                EdgionErrorCode::AiSseDegradedQuotaTerminated,
            )
        } else {
            return;
        };
        let event = super::error::event(error);
        terminate_ai_stream(
            &self.shared,
            sink,
            session,
            kind == ResponseStreamKind::Sse,
            Some(&event),
            code,
        );
    }
}
#[cfg(test)]
#[path = "../../../../tests/unit/ai/protocols/anthropic/processor_test.rs"]
mod tests;
