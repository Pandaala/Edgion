use std::sync::Arc;
use std::time::{Duration, Instant};

use super::EdgionHttpProxy;
use crate::ai::canonical_sse::MAX_CANONICAL_SOURCE_CHUNK_BYTES;
use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
use crate::EdgionHttpContext;
use pingora_http::ResponseHeader;
use pingora_proxy::{ResponseBodySink, Session};

/// Run private response-processor header callbacks after raw response retry
/// arbitration. The processor set is fixed during the request stage.
pub(crate) fn run_stream_processor_headers(response: &mut ResponseHeader, ctx: &mut EdgionHttpContext) {
    if let Some(processor) = ctx.ai_stream_processor.as_mut() {
        processor.on_response_header(response);
    }
    if let Some(processor) = ctx.semantic_response_processor.as_mut() {
        processor.on_response_header(response);
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
    if ctx
        .ai_stream_processor
        .as_ref()
        .is_some_and(|processor| processor.changes_body_length())
        || ctx
            .semantic_response_processor
            .as_ref()
            .is_some_and(|processor| processor.changes_body_length())
        || ctx
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
    if ctx.semantic_response_guard_claimed
        && body
            .as_ref()
            .is_some_and(|chunk| chunk.len() > MAX_CANONICAL_SOURCE_CHUNK_BYTES)
    {
        // The locked Pingora H1/H2 readers both cap one physical response
        // callback at 64 KiB. Treat a larger custom transport callback as an
        // internal invariant violation; fail-open cannot relax a memory bound.
        *body = None;
        sink.terminate();
        return;
    }
    if ctx.semantic_response_guard_claimed && ctx.canonical_record_batch.take().is_some() {
        *body = None;
        sink.terminate();
        return;
    }
    if ctx.stream_processors.is_empty()
        && ctx.ai_stream_processor.is_none()
        && ctx.semantic_response_processor.is_none()
    {
        return;
    }
    let stream_kind = ctx.response_stream_kind;
    let mut ai_processor = ctx.ai_stream_processor.take();
    let mut semantic_processor = ctx.semantic_response_processor.take();
    let mut processors = std::mem::take(&mut ctx.stream_processors);
    {
        let mut adapter = PingoraSessionAdapter::new(session, ctx);
        // Guarded like the plain chain below: a caller that re-enters this
        // function on an already-terminated sink (unreachable today -- the
        // fork returns `Terminate` before another `on_chunk` call can
        // happen) must not resume the AI processor either. The driver holds
        // its own contract rather than depending solely on the fork's pump.
        if !sink.is_terminated() {
            if let Some(processor) = ai_processor.as_mut() {
                processor
                    .on_chunk(body, end_of_stream, stream_kind, sink, &mut adapter)
                    .await;
            }
        }
        if !sink.is_terminated() {
            if let Some(processor) = semantic_processor.as_mut() {
                processor
                    .on_chunk(body, end_of_stream, stream_kind, sink, &mut adapter)
                    .await;
            }
        }
        if !sink.is_terminated() {
            for processor in processors.iter_mut() {
                processor
                    .on_chunk(body, end_of_stream, stream_kind, sink, &mut adapter)
                    .await;
                if sink.is_terminated() {
                    break;
                }
            }
        }
    }
    ctx.ai_stream_processor = ai_processor;
    ctx.semantic_response_processor = semantic_processor;
    ctx.stream_processors = processors;
    if ctx.semantic_response_guard_claimed && ctx.canonical_record_batch.take().is_some() {
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
    use crate::ctx::ResponseStreamKind;
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
        ctx.ai_stream_processor = Some(Box::new(Terminator));
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
        ctx.ai_stream_processor = Some(Box::new(OrderMarker {
            label: "ai",
            order: Arc::clone(&order),
        }));
        ctx.semantic_response_processor = Some(Box::new(OrderMarker {
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
        assert!(ctx.ai_stream_processor.is_some());
        assert!(ctx.semantic_response_processor.is_some());
        assert_eq!(ctx.stream_processors.len(), 1);
    }

    #[tokio::test]
    async fn guarded_physical_callback_limit_is_inclusive_and_stale_handoff_fails_closed() {
        let mut session = test_session().await;
        let mut ctx = crate::EdgionHttpContext {
            semantic_response_guard_claimed: true,
            ..Default::default()
        };
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

        let mut stale_ctx = crate::EdgionHttpContext {
            semantic_response_guard_claimed: true,
            canonical_record_batch: Some(crate::ai::canonical_sse::CanonicalRecordBatch::new()),
            ..Default::default()
        };
        let mut body = None;
        let mut sink = ResponseBodySink::new();
        run_stream_processors(&mut session, &mut stale_ctx, &mut body, true, &mut sink).await;
        assert!(sink.is_terminated());
        assert!(stale_ctx.canonical_record_batch.is_none());
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
