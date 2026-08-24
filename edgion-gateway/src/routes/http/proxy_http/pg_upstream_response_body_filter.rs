use std::sync::Arc;
use std::time::{Duration, Instant};

use super::EdgionHttpProxy;
use crate::ai::canonical_sse::MAX_CANONICAL_SOURCE_CHUNK_BYTES;
use crate::ai::terminal::{ResponseClose, ResponseProcessorSlot, ResponseTerminalCause};
use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
use crate::plugins::runtime::traits::StreamingResponseProcessor;
use crate::plugins::runtime::PluginSession;
use crate::EdgionHttpContext;
use pingora_http::ResponseHeader;
use pingora_proxy::{ResponseBodySink, Session};

struct ResponseProcessorRunSet {
    ai: Option<Box<dyn StreamingResponseProcessor>>,
    semantic: Option<Box<dyn StreamingResponseProcessor>>,
    ordinary: Vec<Box<dyn StreamingResponseProcessor>>,
    released: bool,
}

impl ResponseProcessorRunSet {
    fn take(ctx: &mut EdgionHttpContext) -> Self {
        let (ai, semantic) = match ctx.ai_state_mut() {
            Some(state) => (state.stream_processor.take(), state.semantic_processor.take()),
            None => (None, None),
        };
        Self {
            ai,
            semantic,
            ordinary: std::mem::take(&mut ctx.stream_processors),
            released: false,
        }
    }

    fn preempt(&mut self, winner: ResponseProcessorSlot, cause: ResponseTerminalCause) {
        if winner != ResponseProcessorSlot::Ai {
            if let Some(processor) = self.ai.as_mut() {
                processor.on_terminal_preempted(cause);
            }
        }
        if winner != ResponseProcessorSlot::Semantic {
            if let Some(processor) = self.semantic.as_mut() {
                processor.on_terminal_preempted(cause);
            }
        }
        for (index, processor) in self.ordinary.iter_mut().enumerate() {
            if winner != ResponseProcessorSlot::Ordinary(index) {
                processor.on_terminal_preempted(cause);
            }
        }
    }

    fn restore(mut self, ctx: &mut EdgionHttpContext) {
        if let Some(state) = ctx.ai_state_mut() {
            state.stream_processor = self.ai.take();
            state.semantic_processor = self.semantic.take();
        }
        ctx.stream_processors = std::mem::take(&mut self.ordinary);
        self.released = true;
    }
}

impl Drop for ResponseProcessorRunSet {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        if let Some(processor) = self.ai.as_mut() {
            processor.release_inflight();
        }
        if let Some(processor) = self.semantic.as_mut() {
            processor.release_inflight();
        }
        for processor in &mut self.ordinary {
            processor.release_inflight();
        }
    }
}

fn settle_driver_terminal(ctx: &mut EdgionHttpContext, winner: ResponseProcessorSlot, cause: ResponseTerminalCause) {
    let _ = ctx.response_terminal_state.try_claim(winner, cause);
    let mut processors = ResponseProcessorRunSet::take(ctx);
    processors.preempt(winner, cause);
    let winner_processor = match winner {
        ResponseProcessorSlot::Ai => processors.ai.as_mut(),
        ResponseProcessorSlot::Semantic => processors.semantic.as_mut(),
        ResponseProcessorSlot::Ordinary(index) => processors.ordinary.get_mut(index),
    };
    if let Some(processor) = winner_processor {
        processor.finalize_incomplete("fail-close", cause.as_str());
        processor.release_inflight();
    }
    if let Some(state) = ctx.ai_state_mut() {
        state.canonical_record_batch = None;
    }
    ctx.response_terminal_state.close_gateway();
    processors.restore(ctx);
    ctx.active_response_processor_slot = None;
}

/// Run private response-processor header callbacks after raw response retry
/// arbitration. The processor set is fixed during the request stage.
pub(crate) fn run_stream_processor_headers(response: &mut ResponseHeader, ctx: &mut EdgionHttpContext) {
    if let Some(state) = ctx.ai_state_mut() {
        if let Some(processor) = state.stream_processor.as_mut() {
            processor.on_response_header(response);
        }
        if let Some(processor) = state.semantic_processor.as_mut() {
            processor.on_response_header(response);
        }
    }
    let mut processors = std::mem::take(&mut ctx.stream_processors);
    for processor in processors.iter_mut() {
        processor.on_response_header(response);
    }
    ctx.stream_processors = processors;
}

/// Remove framing metadata that cannot remain valid after a body-mutating
/// processor. This is called at both response-header boundaries: the first
/// call lets H1 choose chunked framing, and the second prevents later async
/// plugins from restoring a stale length.
pub(crate) fn enforce_stream_processor_framing(response: &mut ResponseHeader, ctx: &EdgionHttpContext) {
    if ctx.ai_state().is_some_and(|state| {
        state
            .stream_processor
            .as_ref()
            .is_some_and(|processor| processor.changes_body_length())
            || state
                .semantic_processor
                .as_ref()
                .is_some_and(|processor| processor.changes_body_length())
    }) || ctx
        .stream_processors
        .iter()
        .any(|processor| processor.changes_body_length())
        || ctx.attached_response_framing_reservations > 0
    {
        let _ = response.remove_header("content-length");
    }
}

/// Run the per-request streaming processors, in installation order, with
/// in-place chunk mutation. `mem::take` sidesteps borrowing ctx mutably twice
/// (once for the session adapter, once for the processor Vec); consequently a
/// processor cannot install further processors mid-stream — that is
/// intentional (the set is fixed at request stage).
///
/// A terminate stops the chain immediately: the response is ending, so later
/// processors have nothing to contribute. Settlement belongs in the logging
/// phase, which fires on this path too.
pub(crate) async fn run_stream_processors(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    body: &mut Option<bytes::Bytes>,
    end_of_stream: bool,
    sink: &mut ResponseBodySink,
) {
    ctx.response_body_processing_started = true;
    if ctx.ai_state().is_some_and(|state| state.semantic_guard_claimed)
        && body
            .as_ref()
            .is_some_and(|chunk| chunk.len() > MAX_CANONICAL_SOURCE_CHUNK_BYTES)
    {
        // The locked Pingora H1/H2 readers both cap one physical response
        // callback at 64 KiB. Treat a larger custom transport callback as an
        // internal invariant violation; fail-open cannot relax a memory bound.
        settle_driver_terminal(ctx, ResponseProcessorSlot::Ai, ResponseTerminalCause::ProcessorFailure);
        *body = None;
        sink.terminate();
        return;
    }
    if ctx
        .ai_state_mut()
        .is_some_and(|state| state.semantic_guard_claimed && state.canonical_record_batch.take().is_some())
    {
        settle_driver_terminal(
            ctx,
            ResponseProcessorSlot::Semantic,
            ResponseTerminalCause::GuardrailDependency,
        );
        *body = None;
        sink.terminate();
        return;
    }
    if ctx.stream_processors.is_empty()
        && ctx
            .ai_state()
            .is_none_or(|state| state.stream_processor.is_none() && state.semantic_processor.is_none())
    {
        return;
    }
    let stream_kind = ctx.response_stream_kind;
    let mut processors = ResponseProcessorRunSet::take(ctx);
    let mut winner = None;
    {
        let mut adapter = PingoraSessionAdapter::new(session, ctx);
        // Guarded like the plain chain below: a caller that re-enters this
        // function on an already-terminated sink (unreachable today -- the
        // fork returns `Terminate` before another `on_chunk` call can
        // happen) must not resume the AI processor either. The driver holds
        // its own contract rather than depending solely on the fork's pump.
        if !sink.is_terminated() {
            if let Some(processor) = processors.ai.as_mut() {
                adapter.set_response_processor_slot(ResponseProcessorSlot::Ai);
                processor
                    .on_chunk(body, end_of_stream, stream_kind, sink, &mut adapter)
                    .await;
                if sink.is_terminated() && adapter.response_terminal_winner().is_none() {
                    let _ = adapter.try_claim_response_terminal(ResponseTerminalCause::LegacyProcessor);
                }
                winner = adapter.response_terminal_winner();
            }
        }
        if winner.is_none() && !sink.is_terminated() {
            if let Some(processor) = processors.semantic.as_mut() {
                adapter.set_response_processor_slot(ResponseProcessorSlot::Semantic);
                processor
                    .on_chunk(body, end_of_stream, stream_kind, sink, &mut adapter)
                    .await;
                if sink.is_terminated() && adapter.response_terminal_winner().is_none() {
                    let _ = adapter.try_claim_response_terminal(ResponseTerminalCause::LegacyProcessor);
                }
                winner = adapter.response_terminal_winner();
            }
        }
        let semantic_is_withholding = adapter.semantic_guard_active() && body.is_none() && !end_of_stream;
        if winner.is_none() && !sink.is_terminated() && !semantic_is_withholding {
            for (index, processor) in processors.ordinary.iter_mut().enumerate() {
                adapter.set_response_processor_slot(ResponseProcessorSlot::Ordinary(index));
                processor
                    .on_chunk(body, end_of_stream, stream_kind, sink, &mut adapter)
                    .await;
                if sink.is_terminated() && adapter.response_terminal_winner().is_none() {
                    let _ = adapter.try_claim_response_terminal(ResponseTerminalCause::LegacyProcessor);
                }
                winner = adapter.response_terminal_winner();
                if sink.is_terminated() {
                    break;
                }
            }
        }
    }
    if let Some(winner_slot) = winner {
        let cause = ctx
            .response_terminal_state
            .winner_cause()
            .unwrap_or(ResponseTerminalCause::LegacyProcessor);
        processors.preempt(winner_slot, cause);
        if let Some(state) = ctx.ai_state_mut() {
            state.canonical_record_batch = None;
        }
        if !sink.is_terminated() {
            ctx.response_terminal_state.settle_pending_without_event();
            sink.terminate();
        }
        ctx.response_terminal_state.close_gateway();
    } else if end_of_stream {
        ctx.response_terminal_state.close_open(ResponseClose::NaturalEos);
    }
    processors.restore(ctx);
    ctx.active_response_processor_slot = None;
    if ctx
        .ai_state_mut()
        .is_some_and(|state| state.semantic_guard_claimed && state.canonical_record_batch.take().is_some())
    {
        settle_driver_terminal(
            ctx,
            ResponseProcessorSlot::Semantic,
            ResponseTerminalCause::GuardrailDependency,
        );
        *body = None;
        sink.terminate();
    }
}

/// Process upstream response body chunks
///
/// Returns:
/// - `Ok(None)` - continue processing normally
/// - `Ok(Some(duration))` - rate limit for the given duration (for bandwidth throttling)
#[inline]
pub async fn upstream_response_body_filter(
    _edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    body: &mut Option<bytes::Bytes>,
    end_of_stream: bool,
    sink: &mut ResponseBodySink,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Option<Duration>> {
    record_first_body_time(ctx, body.as_ref());

    // Writable streaming-processor pipeline (installed per-request by
    // request-stage plugins). Runs before the read-only filter chains so
    // e.g. bandwidth limiting sees post-mutation bytes.
    run_stream_processors(session, ctx, body, end_of_stream, sink).await;

    // Run upstream_response_body_filter plugins for bandwidth throttling
    let mut max_delay: Option<Duration> = None;

    // Run route-level body filter plugins
    if let Some(route_unit) = ctx.route_unit.clone() {
        if route_unit.plugin_runtime.upstream_response_body_plugins_count() > 0 {
            if let Some(delay) =
                route_unit
                    .plugin_runtime
                    .run_upstream_response_body_plugins(session, ctx, body, end_of_stream)
            {
                max_delay = Some(delay);
            }
        }
    }

    // Gateway-level plugins (response onion: before GatewayClass-global).
    if let Some(plugins) = ctx.resolved_gateway_plugins.as_ref().map(Arc::clone) {
        for runtime in plugins.iter() {
            if runtime.upstream_response_body_plugins_count() == 0 {
                continue;
            }
            if let Some(delay) = runtime.run_upstream_response_body_plugins(session, ctx, body, end_of_stream) {
                max_delay = Some(match max_delay {
                    Some(current) => current.max(delay),
                    None => delay,
                });
            }
        }
    }

    // Run global plugins body filter (read from per-request snapshot; zero
    // format/store cost on the per-chunk hot path).
    if let Some(plugins) = ctx.resolved_global_plugins.as_ref().map(Arc::clone) {
        for runtime in plugins.iter() {
            if runtime.upstream_response_body_plugins_count() == 0 {
                continue;
            }
            if let Some(delay) = runtime.run_upstream_response_body_plugins(session, ctx, body, end_of_stream) {
                max_delay = Some(match max_delay {
                    Some(current) => current.max(delay),
                    None => delay,
                });
            }
        }
    }

    Ok(max_delay)
}

/// Stamp TTFT only when a non-empty response-body chunk actually arrived.
/// Header-only responses and the terminal `body=None` callback keep `bt`
/// absent, while later chunks never overwrite the first observation.
#[inline]
fn record_first_body_time(ctx: &mut EdgionHttpContext, body: Option<&bytes::Bytes>) {
    if !body.is_some_and(|bytes| !bytes.is_empty()) {
        return;
    }
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        if upstream.bt.is_none() {
            upstream.bt = Some(
                Instant::now()
                    .saturating_duration_since(upstream.start_time)
                    .as_millis() as u64,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::anthropic::AnthropicResponseProcessor;
    use crate::ai::quota::{QuotaAccumulator, QuotaFinalStatus};
    use crate::ai::{AiUsageShared, UsageExtractionProcessor};
    use crate::ctx::ResponseStreamKind;
    use crate::plugins::http::ext_proc::{test_response_processor, AttachedExtProc, TEST_RESPONSE_WINDOW_CAP};
    use crate::plugins::runtime::traits::session::PluginSession;
    use crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseProcessor;
    use async_trait::async_trait;
    use bytes::Bytes;
    use parking_lot::Mutex;
    use tokio::io::AsyncWriteExt;

    fn ctx_with_upstream() -> crate::EdgionHttpContext {
        let mut ctx = crate::EdgionHttpContext {
            backend_context: Some(crate::BackendContext {
                source: None,
                rsc: None,
                service_key: None,
                service_kind: None,
                upstreams: Vec::new(),
                current_upstream_id: None,
                backend_ref_idx: None,
            }),
            ..Default::default()
        };
        ctx.push_upstream(None, None);
        ctx
    }

    /// Minimal live Session (pingora's `Session::new_h1` over a duplex pair);
    /// same pattern as `session_adapter.rs`'s test_session helper.
    async fn test_session() -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(b"GET /sse-test HTTP/1.1\r\nHost: test\r\n\r\n")
            .await
            .expect("write minimal request into duplex pair");
        drop(client);
        session
            .read_request()
            .await
            .expect("read_request should succeed")
            .then_some(())
            .expect("client must not have closed before sending the request");
        session
    }

    struct Uppercase;
    #[async_trait]
    impl StreamingResponseProcessor for Uppercase {
        fn name(&self) -> &str {
            "uppercase"
        }
        async fn on_chunk(
            &mut self,
            chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
            if let Some(b) = chunk.take() {
                *chunk = Some(Bytes::from(b.to_ascii_uppercase()));
            }
        }
    }

    struct DropChunk;
    #[async_trait]
    impl StreamingResponseProcessor for DropChunk {
        fn name(&self) -> &str {
            "drop"
        }
        async fn on_chunk(
            &mut self,
            chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
            *chunk = None;
        }
    }

    struct DropSseChunk;
    #[async_trait]
    impl StreamingResponseProcessor for DropSseChunk {
        fn name(&self) -> &str {
            "drop-sse"
        }
        async fn on_chunk(
            &mut self,
            chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            stream_kind: ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
            if stream_kind == ResponseStreamKind::Sse {
                *chunk = None;
            }
        }
    }

    struct HeaderMutator;
    #[async_trait]
    impl StreamingResponseProcessor for HeaderMutator {
        fn name(&self) -> &str {
            "header-mutator"
        }

        fn on_response_header(&mut self, response: &mut ResponseHeader) {
            response.set_status(http::StatusCode::SERVICE_UNAVAILABLE).unwrap();
            response.insert_header("content-type", "application/json").unwrap();
        }

        fn changes_body_length(&self) -> bool {
            true
        }

        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
        }
    }

    struct Injector;
    #[async_trait]
    impl StreamingResponseProcessor for Injector {
        fn name(&self) -> &str {
            "injector"
        }
        fn changes_body_length(&self) -> bool {
            true
        }
        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
            sink.push(Bytes::from_static(b"-injected")).unwrap();
        }
    }

    /// Records every byte it is shown, into a handle the test still owns.
    struct SeesOnlyBody {
        saw: Arc<Mutex<Vec<u8>>>,
    }

    #[async_trait]
    impl StreamingResponseProcessor for SeesOnlyBody {
        fn name(&self) -> &str {
            "sees-only-body"
        }
        async fn on_chunk(
            &mut self,
            chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
            if let Some(bytes) = chunk.as_deref() {
                self.saw.lock().extend_from_slice(bytes);
            }
        }
    }

    struct Terminator;
    #[async_trait]
    impl StreamingResponseProcessor for Terminator {
        fn name(&self) -> &str {
            "terminator"
        }
        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
            sink.terminate();
        }
    }

    /// Flips a shared flag if it ever runs, into a handle the test still owns.
    struct MarkRan {
        ran: Arc<Mutex<bool>>,
    }
    #[async_trait]
    impl StreamingResponseProcessor for MarkRan {
        fn name(&self) -> &str {
            "mark-ran"
        }
        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
            *self.ran.lock() = true;
        }
    }

    struct OrderMarker {
        label: &'static str,
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    #[derive(Default)]
    struct LifecycleCounts {
        ran: usize,
        preempted: usize,
        released: usize,
    }

    struct LifecycleMarker {
        counts: Arc<Mutex<LifecycleCounts>>,
        terminate: bool,
    }

    struct ClaimWithoutTerminate;

    #[async_trait]
    impl StreamingResponseProcessor for ClaimWithoutTerminate {
        fn name(&self) -> &str {
            "claim-without-terminate"
        }

        async fn on_chunk(
            &mut self,
            chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            session: &mut dyn PluginSession,
        ) {
            assert!(session.try_claim_response_terminal(ResponseTerminalCause::GuardrailRejected));
            *chunk = None;
        }
    }

    struct PendingLifecycle {
        released: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl StreamingResponseProcessor for PendingLifecycle {
        fn name(&self) -> &str {
            "pending-lifecycle"
        }

        fn release_inflight(&mut self) {
            *self.released.lock() += 1;
        }

        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
            std::future::pending::<()>().await;
        }
    }

    #[async_trait]
    impl StreamingResponseProcessor for LifecycleMarker {
        fn name(&self) -> &str {
            "lifecycle-marker"
        }

        fn on_terminal_preempted(&mut self, _winner: ResponseTerminalCause) {
            self.counts.lock().preempted += 1;
        }

        fn release_inflight(&mut self) {
            self.counts.lock().released += 1;
        }

        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
            self.counts.lock().ran += 1;
            if self.terminate {
                sink.terminate();
            }
        }
    }

    #[async_trait]
    impl StreamingResponseProcessor for OrderMarker {
        fn name(&self) -> &str {
            self.label
        }

        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
            self.order.lock().push(self.label);
        }
    }

    #[test]
    fn header_callbacks_run_and_processors_survive() {
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stream_processors.push(Box::new(HeaderMutator));
        let mut response = ResponseHeader::build(200, None).unwrap();

        run_stream_processor_headers(&mut response, &mut ctx);

        assert_eq!(response.status, http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers.get("content-type").unwrap(), "application/json");
        assert_eq!(ctx.stream_processors.len(), 1);
    }

    #[test]
    fn framing_guard_applies_only_to_body_mutators() {
        let mut response = ResponseHeader::build(200, None).unwrap();
        response.insert_header("content-length", "42").unwrap();
        let mut observational = crate::EdgionHttpContext::default();
        observational.stream_processors.push(Box::new(Uppercase));
        enforce_stream_processor_framing(&mut response, &observational);
        assert_eq!(response.headers.get("content-length").unwrap(), "42");

        let mut mutating = crate::EdgionHttpContext::default();
        mutating.stream_processors.push(Box::new(HeaderMutator));
        enforce_stream_processor_framing(&mut response, &mutating);
        assert!(response.headers.get("content-length").is_none());

        let reserved = crate::EdgionHttpContext {
            attached_response_framing_reservations: 1,
            ..Default::default()
        };
        let mut response = ResponseHeader::build(200, None).unwrap();
        response.insert_header("content-length", "42").unwrap();
        enforce_stream_processor_framing(&mut response, &reserved);
        assert!(response.headers.get("content-length").is_none());
    }

    #[tokio::test]
    async fn processors_run_in_install_order_and_mutate_in_place() {
        let mut session = test_session().await;
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stream_processors.push(Box::new(Uppercase));

        let mut body = Some(Bytes::from_static(b"data: hi\n\n"));
        let mut sink = ResponseBodySink::new();
        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;
        assert_eq!(body.as_deref(), Some(b"DATA: HI\n\n".as_slice()));
        // Processors survive the call (they run once per chunk, not once per request).
        assert_eq!(ctx.stream_processors.len(), 1);
    }

    #[tokio::test]
    async fn later_processor_sees_earlier_processors_output() {
        let mut session = test_session().await;
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stream_processors.push(Box::new(Uppercase));
        ctx.stream_processors.push(Box::new(DropChunk));

        let mut body = Some(Bytes::from_static(b"payload"));
        let mut sink = ResponseBodySink::new();
        run_stream_processors(&mut session, &mut ctx, &mut body, true, &mut sink).await;
        assert!(body.is_none(), "second processor dropped the chunk");
    }

    #[tokio::test]
    async fn empty_pipeline_is_a_no_op() {
        let mut session = test_session().await;
        let mut ctx = crate::EdgionHttpContext::default();
        let mut body = Some(Bytes::from_static(b"untouched"));
        let mut sink = ResponseBodySink::new();
        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;
        assert_eq!(body.as_deref(), Some(b"untouched".as_slice()));
    }

    #[tokio::test]
    async fn processor_observes_response_stream_kind() {
        let mut session = test_session().await;
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stream_processors.push(Box::new(DropSseChunk));

        let mut body = Some(Bytes::from_static(b"ordinary"));
        let mut sink = ResponseBodySink::new();
        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;
        assert_eq!(body.as_deref(), Some(b"ordinary".as_slice()));

        ctx.response_stream_kind = ResponseStreamKind::Sse;
        let mut body = Some(Bytes::from_static(b"data: event\n\n"));
        let mut sink = ResponseBodySink::new();
        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;
        assert!(body.is_none(), "processor observed the SSE classification");
    }

    #[tokio::test]
    async fn injected_content_is_not_visible_to_later_processors() {
        // Injector pushes to the sink; the processor after it must see only the
        // original body, never the injected bytes.
        let mut session = test_session().await;
        let saw = Arc::new(Mutex::new(Vec::new()));
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stream_processors.push(Box::new(Injector));
        ctx.stream_processors.push(Box::new(SeesOnlyBody { saw: saw.clone() }));
        let mut sink = ResponseBodySink::new();
        let mut body = Some(Bytes::from_static(b"orig"));

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        assert_eq!(sink.peek_extra(), &[Bytes::from_static(b"-injected")]);
        assert_eq!(
            saw.lock().as_slice(),
            b"orig",
            "the sink is an injection channel, not part of the transform chain"
        );
    }

    #[tokio::test]
    async fn semantic_withholding_does_not_expose_empty_callbacks_to_ext_proc_slot() {
        let mut session = test_session().await;
        let ran = Arc::new(Mutex::new(false));
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.ensure_ai_state().semantic_guard_claimed = true;
        ctx.stream_processors.push(Box::new(MarkRan { ran: Arc::clone(&ran) }));
        let mut sink = ResponseBodySink::new();
        let mut body = None;

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        assert!(!*ran.lock());
        assert!(!sink.is_terminated());
    }

    #[tokio::test]
    async fn terminate_stops_the_chain_immediately() {
        // A processor that terminates must prevent every later processor from
        // running at all -- not merely from seeing further chunks.
        let mut session = test_session().await;
        let ran = Arc::new(Mutex::new(false));

        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stream_processors.push(Box::new(Terminator));
        ctx.stream_processors.push(Box::new(MarkRan { ran: ran.clone() }));
        let mut sink = ResponseBodySink::new();
        let mut body = Some(Bytes::from_static(b"orig"));

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        assert!(sink.is_terminated());
        assert!(!*ran.lock(), "processor after terminate must not run");
    }

    #[tokio::test]
    async fn ai_processor_terminate_skips_the_entire_plain_chain() {
        // The AI processor slot runs before the plain `stream_processors`
        // chain. If it terminates, the plain chain must not run at all --
        // this is the `!sink.is_terminated()` guard the driver holds on its
        // own entry into that chain, not merely on later iterations of it.
        let mut session = test_session().await;
        let ran = Arc::new(Mutex::new(false));

        let mut ctx = crate::EdgionHttpContext::default();
        ctx.ensure_ai_state().stream_processor = Some(Box::new(Terminator));
        ctx.stream_processors.push(Box::new(MarkRan { ran: ran.clone() }));
        let mut sink = ResponseBodySink::new();
        let mut body = Some(Bytes::from_static(b"orig"));

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        assert!(sink.is_terminated());
        assert!(
            !*ran.lock(),
            "the plain chain must not run once the AI processor terminated"
        );
    }

    #[tokio::test]
    async fn ai_semantic_guard_and_generic_processors_are_distinct_ordered_slots() {
        let mut session = test_session().await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.ensure_ai_state().stream_processor = Some(Box::new(OrderMarker {
            label: "ai",
            order: Arc::clone(&order),
        }));
        ctx.ensure_ai_state().semantic_processor = Some(Box::new(OrderMarker {
            label: "guardrail",
            order: Arc::clone(&order),
        }));
        ctx.stream_processors.push(Box::new(OrderMarker {
            label: "ext-proc",
            order: Arc::clone(&order),
        }));
        let mut body = Some(Bytes::from_static(b"data: event\n\n"));
        let mut sink = ResponseBodySink::new();

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        assert_eq!(&*order.lock(), &["ai", "guardrail", "ext-proc"]);
        assert!(ctx.ai_state().unwrap().stream_processor.is_some());
        assert!(ctx.ai_state().unwrap().semantic_processor.is_some());
        assert_eq!(ctx.stream_processors.len(), 1);
    }

    #[tokio::test]
    async fn terminal_cleans_every_non_winner_once_including_earlier_processors() {
        let mut session = test_session().await;
        let first = Arc::new(Mutex::new(LifecycleCounts::default()));
        let winner = Arc::new(Mutex::new(LifecycleCounts::default()));
        let later = Arc::new(Mutex::new(LifecycleCounts::default()));
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stream_processors.push(Box::new(LifecycleMarker {
            counts: Arc::clone(&first),
            terminate: false,
        }));
        ctx.stream_processors.push(Box::new(LifecycleMarker {
            counts: Arc::clone(&winner),
            terminate: true,
        }));
        ctx.stream_processors.push(Box::new(LifecycleMarker {
            counts: Arc::clone(&later),
            terminate: false,
        }));
        let mut body = Some(Bytes::from_static(b"data: event\n\n"));
        let mut sink = ResponseBodySink::new();

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        let first_counts = first.lock();
        assert_eq!((first_counts.ran, first_counts.preempted), (1, 1));
        drop(first_counts);
        let winner_counts = winner.lock();
        assert_eq!((winner_counts.ran, winner_counts.preempted), (1, 0));
        drop(winner_counts);
        let later_counts = later.lock();
        assert_eq!((later_counts.ran, later_counts.preempted), (0, 1));
        drop(later_counts);
        assert!(matches!(
            ctx.response_terminal_state,
            crate::ai::terminal::ResponseTerminalState::Closed {
                winner: Some(ResponseProcessorSlot::Ordinary(1)),
                close: ResponseClose::GatewayTerminate,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn real_quota_terminal_preempts_guardrail_and_ext_proc_slots() {
        let mut session = test_session().await;
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        quota.finish(QuotaFinalStatus::OverLimit);
        let semantic = Arc::new(Mutex::new(LifecycleCounts::default()));
        let ordinary = Arc::new(Mutex::new(LifecycleCounts::default()));
        let mut ctx = crate::EdgionHttpContext {
            response_stream_kind: ResponseStreamKind::Sse,
            ..Default::default()
        };
        ctx.ensure_ai_state().semantic_guard_claimed = true;
        ctx.ensure_ai_state().stream_processor = Some(Box::new(UsageExtractionProcessor::new_with_quota(
            Arc::clone(&shared),
            Some(quota),
            true,
            true,
        )));
        ctx.ensure_ai_state().semantic_processor = Some(Box::new(LifecycleMarker {
            counts: Arc::clone(&semantic),
            terminate: false,
        }));
        ctx.stream_processors.push(Box::new(LifecycleMarker {
            counts: Arc::clone(&ordinary),
            terminate: false,
        }));
        let mut body = Some(Bytes::from_static(
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"x\"}}]}\n\n",
        ));
        let original = body.clone();
        let mut sink = ResponseBodySink::new();

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        assert_eq!(body, original, "quota preserves the current canonical chunk");
        assert!(sink.is_terminated());
        assert_eq!(sink.peek_extra().len(), 1);
        assert!(sink.peek_extra()[0]
            .windows(b"token_quota_exceeded".len())
            .any(|part| part == b"token_quota_exceeded"));
        {
            let counts = semantic.lock();
            assert_eq!((counts.ran, counts.preempted), (0, 1));
        }
        {
            let counts = ordinary.lock();
            assert_eq!((counts.ran, counts.preempted), (0, 1));
        }
        assert!(ctx.ai_state().unwrap().canonical_record_batch.is_none());
        assert!(shared.gateway_terminated());
        assert!(matches!(
            ctx.response_terminal_state,
            crate::ai::terminal::ResponseTerminalState::Closed {
                winner: Some(ResponseProcessorSlot::Ai),
                cause: Some(ResponseTerminalCause::QuotaExceeded),
                close: ResponseClose::GatewayTerminate,
                ..
            }
        ));
    }

    fn anthropic_streaming_processor(quota: Option<Arc<QuotaAccumulator>>) -> AnthropicResponseProcessor {
        let mut processor = AnthropicResponseProcessor::new_with_quota(
            "claude-canonical".to_string(),
            true,
            false,
            1_785_772_800,
            Arc::new(AiUsageShared::default()),
            quota,
            true,
        );
        let mut header = ResponseHeader::build(200, None).unwrap();
        header.insert_header("content-type", "text/event-stream").unwrap();
        processor.on_response_header(&mut header);
        processor
    }

    #[tokio::test]
    async fn anthropic_quota_winner_reclaims_translated_bytes_before_guardrail_cleanup() {
        let mut session = test_session().await;
        let quota = Arc::new(QuotaAccumulator::new(16));
        quota.finish(QuotaFinalStatus::OverLimit);
        let semantic = Arc::new(Mutex::new(LifecycleCounts::default()));
        let mut ctx = crate::EdgionHttpContext {
            response_stream_kind: ResponseStreamKind::Sse,
            ..Default::default()
        };
        let state = ctx.ensure_ai_state();
        state.semantic_guard_claimed = true;
        state.stream_processor = Some(Box::new(anthropic_streaming_processor(Some(quota))));
        state.semantic_processor = Some(Box::new(LifecycleMarker {
            counts: Arc::clone(&semantic),
            terminate: false,
        }));
        let mut body = Some(Bytes::from_static(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"type\":\"message\",\"id\":\"msg_1\",\"model\":\"claude-provider\",\"role\":\"assistant\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n"));
        let mut sink = ResponseBodySink::new();

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        let body = body.expect("translated current chunk must remain client-visible");
        assert!(body
            .windows(b"chat.completion.chunk".len())
            .any(|part| part == b"chat.completion.chunk"));
        assert!(!body
            .windows(b"claude-provider".len())
            .any(|part| part == b"claude-provider"));
        assert_eq!(sink.peek_extra().len(), 1);
        assert!(sink.is_terminated());
        assert!(ctx.ai_state().unwrap().canonical_record_batch.is_none());
        let counts = semantic.lock();
        assert_eq!((counts.ran, counts.preempted), (0, 1));
    }

    #[tokio::test]
    async fn anthropic_translation_failure_keeps_only_fixed_error_and_preempts_guardrail() {
        let mut session = test_session().await;
        let semantic = Arc::new(Mutex::new(LifecycleCounts::default()));
        let mut ctx = crate::EdgionHttpContext {
            response_stream_kind: ResponseStreamKind::Sse,
            ..Default::default()
        };
        let state = ctx.ensure_ai_state();
        state.semantic_guard_claimed = true;
        state.stream_processor = Some(Box::new(anthropic_streaming_processor(None)));
        state.semantic_processor = Some(Box::new(LifecycleMarker {
            counts: Arc::clone(&semantic),
            terminate: false,
        }));
        let mut body = Some(Bytes::from_static(
            b"event: error\ndata: {\"type\":\"error\",\"private\":\"provider-payload\"}\n\n",
        ));
        let mut sink = ResponseBodySink::new();

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        let body = body.expect("fixed translation error must remain client-visible");
        assert!(body
            .windows(b"response_translation_failed".len())
            .any(|part| part == b"response_translation_failed"));
        assert!(!body
            .windows(b"provider-payload".len())
            .any(|part| part == b"provider-payload"));
        assert!(sink.peek_extra().is_empty());
        assert!(sink.is_terminated());
        assert!(ctx.ai_state().unwrap().canonical_record_batch.is_none());
        let counts = semantic.lock();
        assert_eq!((counts.ran, counts.preempted), (0, 1));
    }

    #[tokio::test]
    async fn first_real_ext_proc_terminal_preempts_second_attached_owner() {
        let mut session = test_session().await;
        let first_owner = Arc::new(AttachedExtProc::test_lazy());
        let second_owner = Arc::new(AttachedExtProc::test_lazy());
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stream_processors
            .push(test_response_processor(Arc::clone(&first_owner), false));
        ctx.stream_processors
            .push(test_response_processor(Arc::clone(&second_owner), false));
        let mut body = Some(Bytes::from(vec![b'x'; TEST_RESPONSE_WINDOW_CAP + 1]));
        let mut sink = ResponseBodySink::new();

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        assert!(body.is_none());
        assert!(sink.is_terminated());
        assert_eq!(first_owner.test_state(), ("fail-close", "input-cap", true));
        assert_eq!(second_owner.test_state(), ("superseded", "processor-failure", true));
        assert!(matches!(
            ctx.response_terminal_state,
            crate::ai::terminal::ResponseTerminalState::Closed {
                winner: Some(ResponseProcessorSlot::Ordinary(0)),
                cause: Some(ResponseTerminalCause::ProcessorFailure),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn driver_natural_eos_and_logging_are_idempotent_for_bodyless_output() {
        let mut session = test_session().await;
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.ensure_ai_state().stream_processor = Some(Box::new(UsageExtractionProcessor::new(Arc::new(
            AiUsageShared::default(),
        ))));
        let mut body = None;
        let mut sink = ResponseBodySink::new();

        run_stream_processors(&mut session, &mut ctx, &mut body, true, &mut sink).await;

        let expected = crate::ai::terminal::ResponseTerminalState::Closed {
            winner: None,
            cause: None,
            emission: None,
            close: ResponseClose::NaturalEos,
        };
        assert_eq!(ctx.response_terminal_state, expected);
        assert!(sink.peek_extra().is_empty());
        assert!(!sink.is_terminated());
        super::super::pg_logging::finalize_response_terminal_for_logging(&mut ctx, None);
        super::super::pg_logging::finalize_response_terminal_for_logging(
            &mut ctx,
            Some(pingora_core::ErrorSource::Upstream),
        );
        assert_eq!(ctx.response_terminal_state, expected);
    }

    #[tokio::test]
    async fn claimed_terminal_is_force_terminated_when_winner_returns_early() {
        let mut session = test_session().await;
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.ensure_ai_state().semantic_processor = Some(Box::new(ClaimWithoutTerminate));
        let mut body = Some(Bytes::from_static(b"rejected"));
        let mut sink = ResponseBodySink::new();

        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

        assert!(body.is_none());
        assert!(sink.is_terminated());
        assert!(matches!(
            ctx.response_terminal_state,
            crate::ai::terminal::ResponseTerminalState::Closed {
                winner: Some(ResponseProcessorSlot::Semantic),
                emission: Some(crate::ai::terminal::TerminalEmission::NoEvent),
                close: ResponseClose::GatewayTerminate,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn cancelling_an_awaiting_driver_releases_the_owned_run_set() {
        let released = Arc::new(Mutex::new(0));
        let mut session = test_session().await;
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stream_processors.push(Box::new(PendingLifecycle {
            released: Arc::clone(&released),
        }));
        let mut body = Some(Bytes::from_static(b"pending"));
        let mut sink = ResponseBodySink::new();

        let result = tokio::time::timeout(
            Duration::from_millis(10),
            run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(*released.lock(), 1);
        assert!(ctx.stream_processors.is_empty());
    }

    #[test]
    fn cancelled_run_set_releases_all_inflight_owners() {
        let ai = Arc::new(Mutex::new(LifecycleCounts::default()));
        let semantic = Arc::new(Mutex::new(LifecycleCounts::default()));
        let ordinary = Arc::new(Mutex::new(LifecycleCounts::default()));
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.ensure_ai_state().stream_processor = Some(Box::new(LifecycleMarker {
            counts: Arc::clone(&ai),
            terminate: false,
        }));
        ctx.ensure_ai_state().semantic_processor = Some(Box::new(LifecycleMarker {
            counts: Arc::clone(&semantic),
            terminate: false,
        }));
        ctx.stream_processors.push(Box::new(LifecycleMarker {
            counts: Arc::clone(&ordinary),
            terminate: false,
        }));

        drop(ResponseProcessorRunSet::take(&mut ctx));

        assert_eq!(ai.lock().released, 1);
        assert_eq!(semantic.lock().released, 1);
        assert_eq!(ordinary.lock().released, 1);
        assert!(ctx.ai_state().unwrap().stream_processor.is_none());
        assert!(ctx.ai_state().unwrap().semantic_processor.is_none());
        assert!(ctx.stream_processors.is_empty());
    }

    #[tokio::test]
    async fn guarded_physical_callback_limit_is_inclusive_and_stale_handoff_fails_closed() {
        let mut session = test_session().await;
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.ensure_ai_state().semantic_guard_claimed = true;
        let mut exact = Some(Bytes::from(vec![b'x'; MAX_CANONICAL_SOURCE_CHUNK_BYTES]));
        let mut sink = ResponseBodySink::new();
        run_stream_processors(&mut session, &mut ctx, &mut exact, false, &mut sink).await;
        assert_eq!(exact.as_ref().map(Bytes::len), Some(MAX_CANONICAL_SOURCE_CHUNK_BYTES));
        assert!(!sink.is_terminated());

        let mut plus_one = Some(Bytes::from(vec![b'x'; MAX_CANONICAL_SOURCE_CHUNK_BYTES + 1]));
        let mut sink = ResponseBodySink::new();
        run_stream_processors(&mut session, &mut ctx, &mut plus_one, false, &mut sink).await;
        assert!(plus_one.is_none());
        assert!(sink.is_terminated());

        let mut stale_ctx = crate::EdgionHttpContext::default();
        let state = stale_ctx.ensure_ai_state();
        state.semantic_guard_claimed = true;
        state.canonical_record_batch = Some(crate::ai::canonical_sse::CanonicalRecordBatch::new());
        let mut body = None;
        let mut sink = ResponseBodySink::new();
        run_stream_processors(&mut session, &mut stale_ctx, &mut body, true, &mut sink).await;
        assert!(sink.is_terminated());
        assert!(stale_ctx.ai_state().unwrap().canonical_record_batch.is_none());
    }

    #[test]
    fn ttft_requires_first_nonempty_body_chunk_and_never_overwrites() {
        let mut ctx = ctx_with_upstream();
        record_first_body_time(&mut ctx, None);
        record_first_body_time(&mut ctx, Some(&Bytes::new()));
        assert_eq!(ctx.get_current_upstream().and_then(|upstream| upstream.bt), None);

        record_first_body_time(&mut ctx, Some(&Bytes::from_static(b"x")));
        let first = ctx.get_current_upstream().and_then(|upstream| upstream.bt);
        assert!(first.is_some());

        ctx.get_current_upstream_mut().unwrap().start_time = Instant::now() - Duration::from_secs(1);
        record_first_body_time(&mut ctx, Some(&Bytes::from_static(b"later")));
        assert_eq!(ctx.get_current_upstream().and_then(|upstream| upstream.bt), first);
    }
}
