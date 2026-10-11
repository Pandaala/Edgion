use std::sync::Arc;
use std::time::{Duration, Instant};

use super::EdgionHttpProxy;
use crate::ai::protocols::openai::canonical_sse::MAX_CANONICAL_SOURCE_CHUNK_BYTES;
use crate::response::terminal::{ResponseClose, ResponseProcessorSlot, ResponseTerminalCause};

use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
use crate::plugins::runtime::traits::StreamingResponseEvent;
use crate::plugins::runtime::PluginSession;
use crate::EdgionHttpContext;
use pingora_http::ResponseHeader;
use pingora_proxy::{ResponseBodySink, Session, UpstreamResponseBodyEvent};

fn preempt_processors(
    processors: &mut crate::response::driver::BodyProcessorSet,
    winner: ResponseProcessorSlot,
    cause: ResponseTerminalCause,
) {
    if winner != ResponseProcessorSlot::Ai {
        if let Some(processor) = processors.ai.as_mut() {
            processor.on_terminal_preempted(cause);
        }
    }
    if winner != ResponseProcessorSlot::Semantic {
        if let Some(processor) = processors.semantic.as_mut() {
            processor.on_terminal_preempted(cause);
        }
    }
    for (index, processor) in processors.ordinary.iter_mut().enumerate() {
        if winner != ResponseProcessorSlot::Ordinary(index) {
            processor.on_terminal_preempted(cause);
        }
    }
}

async fn settle_driver_terminal(
    ctx: &mut EdgionHttpContext,
    winner: ResponseProcessorSlot,
    cause: ResponseTerminalCause,
) {
    let _ = ctx.response_terminal_state.try_claim(winner, cause);
    if let Some(driver) = ctx.response_processor_driver() {
        if let Some(mut lease) = driver.body_lease().await {
            let processors = lease.processors();
            preempt_processors(processors, winner, cause);
            let winner_processor = match winner {
                ResponseProcessorSlot::Ai => processors.ai.as_mut(),
                ResponseProcessorSlot::Semantic => processors.semantic.as_mut(),
                ResponseProcessorSlot::Ordinary(index) => processors.ordinary.get_mut(index),
            };
            if let Some(processor) = winner_processor {
                processor.finalize_incomplete("fail-close", cause.as_str());
                processor.release_inflight();
            }
            lease.complete();
        }
    }
    if let Some(state) = ctx.ai_state_mut() {
        state.canonical_record_batch = None;
    }
    ctx.response_terminal_state.close_gateway();
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
    if let Some(processors) = ctx.response_stream_processors_mut() {
        for processor in processors {
            processor.on_response_header(response);
        }
    }
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
    }) || ctx.response_stream_processors_change_body_length()
        || ctx.has_response_framing_reservation()
    {
        let _ = response.remove_header("content-length");
    }
}

/// Run the frozen per-request streaming processors in installation order with
/// in-place chunk mutation. The driver lease owns the processor set across
/// awaits while the adapter borrows the request context; processor installation
/// is closed before this callback begins.
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
    run_stream_processors_event(session, ctx, body, StreamingResponseEvent::Data { end_of_stream }, sink).await;
}

/// Typed variant used by the fork response lifecycle. Unlike the legacy bool,
/// this keeps a pre-trailer boundary distinct all the way to trailer-aware
/// processors while legacy processors still flush through their default
/// delegation.
pub(crate) async fn run_stream_processors_event(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    body: &mut Option<bytes::Bytes>,
    event: StreamingResponseEvent,
    sink: &mut ResponseBodySink,
) {
    if !ctx.response_processors_frozen() {
        // The only legal freeze point is the successful final response-header
        // filter. Do not silently freeze an incomplete processor builder if a
        // body event arrives before that boundary.
        *body = None;
        sink.terminate();
        return;
    }
    if ctx.ai_state().is_some_and(|state| state.semantic_guard_claimed)
        && body
            .as_ref()
            .is_some_and(|chunk| chunk.len() > MAX_CANONICAL_SOURCE_CHUNK_BYTES)
    {
        // The locked Pingora H1/H2 readers both cap one physical response
        // callback at 64 KiB. Treat a larger custom transport callback as an
        // internal invariant violation; fail-open cannot relax a memory bound.
        settle_driver_terminal(ctx, ResponseProcessorSlot::Ai, ResponseTerminalCause::ProcessorFailure).await;
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
        )
        .await;
        *body = None;
        sink.terminate();
        return;
    }
    let Some(driver) = ctx.response_processor_driver() else {
        return;
    };
    if !driver.has_body_processors() {
        return;
    }
    let stream_kind = ctx.response_stream_kind;
    let Some(mut lease) = driver.body_lease().await else {
        *body = None;
        sink.terminate();
        return;
    };
    let mut winner = None;
    {
        let processors = lease.processors();
        let mut adapter = PingoraSessionAdapter::new(session, ctx);
        // Guarded like the plain chain below: a caller that re-enters this
        // function on an already-terminated sink (unreachable today -- the
        // fork returns `Terminate` before another `on_chunk` call can
        // happen) must not resume the AI processor either. The driver holds
        // its own contract rather than depending solely on the fork's pump.
        if !sink.is_terminated() {
            if let Some(processor) = processors.ai.as_mut() {
                adapter.set_response_processor_slot(ResponseProcessorSlot::Ai);
                processor.on_event(body, event, stream_kind, sink, &mut adapter).await;
                if sink.is_terminated() && adapter.response_terminal_winner().is_none() {
                    let _ = adapter.try_claim_response_terminal(ResponseTerminalCause::LegacyProcessor);
                }
                winner = adapter.response_terminal_winner();
            }
        }
        if winner.is_none() && !sink.is_terminated() {
            if let Some(processor) = processors.semantic.as_mut() {
                adapter.set_response_processor_slot(ResponseProcessorSlot::Semantic);
                processor.on_event(body, event, stream_kind, sink, &mut adapter).await;
                if sink.is_terminated() && adapter.response_terminal_winner().is_none() {
                    let _ = adapter.try_claim_response_terminal(ResponseTerminalCause::LegacyProcessor);
                }
                winner = adapter.response_terminal_winner();
            }
        }
        let semantic_is_withholding = adapter.ai().semantic_guard_active()
            && body.is_none()
            && matches!(event, StreamingResponseEvent::Data { end_of_stream: false });
        if winner.is_none() && !sink.is_terminated() && !semantic_is_withholding {
            for (index, processor) in processors.ordinary.iter_mut().enumerate() {
                adapter.set_response_processor_slot(ResponseProcessorSlot::Ordinary(index));
                processor.on_event(body, event, stream_kind, sink, &mut adapter).await;
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
        preempt_processors(lease.processors(), winner_slot, cause);
        if let Some(state) = ctx.ai_state_mut() {
            state.canonical_record_batch = None;
        }
        if !sink.is_terminated() && !sink.response_head_decision_pending() {
            ctx.response_terminal_state.settle_pending_without_event();
            sink.terminate();
        }
        ctx.response_terminal_state.close_gateway();
    } else if event.closes_without_trailers() {
        ctx.response_terminal_state.close_open(ResponseClose::NaturalEos);
    }
    lease.complete();
    if ctx
        .ai_state_mut()
        .is_some_and(|state| state.semantic_guard_claimed && state.canonical_record_batch.take().is_some())
    {
        settle_driver_terminal(
            ctx,
            ResponseProcessorSlot::Semantic,
            ResponseTerminalCause::GuardrailDependency,
        )
        .await;
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

    Ok(run_read_only_body_plugins(session, ctx, body, end_of_stream))
}

/// Adapt the fork's typed response lifecycle without collapsing the
/// pre-trailer boundary into a no-trailers EOS.
#[inline]
pub async fn upstream_response_body_filter_event(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    body: &mut Option<bytes::Bytes>,
    event: UpstreamResponseBodyEvent,
    sink: &mut ResponseBodySink,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Option<Duration>> {
    match event {
        UpstreamResponseBodyEvent::Data { end_of_stream } => {
            let delay = upstream_response_body_filter(edgion_http, session, body, end_of_stream, sink, ctx).await?;
            if end_of_stream {
                super::pg_upstream_response_trailer_filter::dispatch_upstream_response_trailers(session, ctx, None)
                    .await?;
                emit_grpc_terminal(ctx, sink)?;
            }
            Ok(delay)
        }
        UpstreamResponseBodyEvent::TerminalWithoutTrailers => {
            record_first_body_time(ctx, body.as_ref());
            run_stream_processors_event(
                session,
                ctx,
                body,
                StreamingResponseEvent::TerminalWithoutTrailers,
                sink,
            )
            .await;
            let delay = run_read_only_body_plugins(session, ctx, body, true);
            super::pg_upstream_response_trailer_filter::dispatch_upstream_response_trailers(session, ctx, None).await?;
            emit_grpc_terminal(ctx, sink)?;
            Ok(delay)
        }
        UpstreamResponseBodyEvent::TerminalBeforeTrailers => {
            record_first_body_time(ctx, body.as_ref());
            run_stream_processors_event(session, ctx, body, StreamingResponseEvent::TerminalBeforeTrailers, sink).await;
            // A processor may flush its final bytes here. They still pass through
            // the canonical readonly chain, while trailers retain ownership of EOS.
            let delay = if body.as_ref().is_some_and(|chunk| !chunk.is_empty()) {
                run_read_only_body_plugins(session, ctx, body, false)
            } else {
                None
            };
            let _ = ctx.mark_upstream_before_trailers();
            Ok(delay)
        }
    }
}

fn run_read_only_body_plugins(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    body: &Option<bytes::Bytes>,
    end_of_stream: bool,
) -> Option<Duration> {
    let mut max_delay: Option<Duration> = None;
    if let Some(runtime) = prefer_plugin_runtime(
        ctx.grpc_route_unit
            .as_ref()
            .map(|unit| Arc::clone(&unit.plugin_runtime)),
        || ctx.route_unit.as_ref().map(|unit| Arc::clone(&unit.plugin_runtime)),
    ) {
        run_and_merge_read_only_body_runtime(&mut max_delay, runtime.as_ref(), session, ctx, body, end_of_stream);
    }
    if let Some(runtime) = prefer_plugin_runtime(
        ctx.selected_grpc_backend
            .as_ref()
            .map(crate::routes::grpc::SelectedGrpcBackend::plugin_runtime),
        || {
            ctx.selected_backend
                .as_ref()
                .map(crate::backends::SelectedHttpBackend::plugin_runtime)
        },
    ) {
        run_and_merge_read_only_body_runtime(&mut max_delay, runtime.as_ref(), session, ctx, body, end_of_stream);
    }
    if let Some(plugins) = ctx.resolved_gateway_plugins() {
        for runtime in plugins.iter() {
            run_and_merge_read_only_body_runtime(&mut max_delay, runtime, session, ctx, body, end_of_stream);
        }
    }
    if let Some(plugins) = ctx.resolved_global_plugins() {
        for runtime in plugins.iter() {
            run_and_merge_read_only_body_runtime(&mut max_delay, runtime, session, ctx, body, end_of_stream);
        }
    }
    max_delay
}

#[inline]
fn prefer_plugin_runtime(
    preferred: Option<Arc<crate::plugins::runtime::PluginRuntime>>,
    fallback: impl FnOnce() -> Option<Arc<crate::plugins::runtime::PluginRuntime>>,
) -> Option<Arc<crate::plugins::runtime::PluginRuntime>> {
    preferred.or_else(fallback)
}

#[inline]
fn run_and_merge_read_only_body_runtime(
    max_delay: &mut Option<Duration>,
    runtime: &crate::plugins::runtime::PluginRuntime,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    body: &Option<bytes::Bytes>,
    end_of_stream: bool,
) {
    merge_max_delay(
        max_delay,
        runtime.run_upstream_response_body_plugins(session, ctx, body, end_of_stream),
    );
}

#[inline]
fn merge_max_delay(max_delay: &mut Option<Duration>, delay: Option<Duration>) {
    if let Some(delay) = delay {
        *max_delay = Some(max_delay.map_or(delay, |current| current.max(delay)));
    }
}

/// Stamp TTFT only when a non-empty response-body chunk actually arrived.
/// Header-only responses and the terminal `body=None` callback keep `bt`
/// absent, while later chunks never overwrite the first observation.
#[inline]
fn record_first_body_time(ctx: &mut EdgionHttpContext, body: Option<&bytes::Bytes>) {
    if body.is_none_or(|bytes| bytes.is_empty()) {
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
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_upstream_response_body_filter_test.rs"]
mod tests;

/// A no-trailers terminal has no downstream trailer callback; emit its fixed
/// failure record through the canonical bounded body sink exactly once.
fn emit_grpc_terminal(ctx: &EdgionHttpContext, sink: &mut ResponseBodySink) -> pingora_core::Result<()> {
    if let Some(plan) = &ctx.grpc_transcoding {
        if let Some(terminal) = plan.terminal.lock().take() {
            sink.push(terminal)?;
        }
    }
    Ok(())
}
