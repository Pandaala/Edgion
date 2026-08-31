//! Writable, async per-chunk processors for upstream response streams.
//!
//! This is the AI-gateway streaming pipeline contract (spec:
//! `docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md`). It is
//! deliberately NOT an operator-facing EdgionPlugins stage: request-stage
//! plugins that need to observe or transform the response stream push a
//! processor through `EdgionHttpContext::push_response_stream_processor`, and the proxy's
//! upstream-response-body hook runs them in installation order, BEFORE the
//! read-only `UpstreamResponseBodyFilter` chains (so bandwidth limiting sees
//! post-mutation bytes). Those read-only chains are NOT terminate-aware: a
//! `sink.terminate()` called here stops this chain immediately, but the
//! `UpstreamResponseBodyFilter` chains that run afterward are unconditional
//! and still see the (now-terminating) chunk.
//!
//! Contract:
//! - Async, per transport chunk, writable: a processor may replace, drop, or
//!   extend the current chunk in place. It must NOT accumulate unbounded
//!   state across chunks (see `crate::sse::SseDecoder` for the bounded
//!   framing substrate).
//! - The pipeline runs whenever processors are installed, regardless of
//!   `ResponseStreamKind` — classification tells a processor HOW to interpret
//!   chunks, not WHETHER it runs.
//! - No per-chunk logging or tracing; aggregate per-request state and emit
//!   once outside the body hot path (same rule as the read-only trait).
//! - Termination: a processor may end the response via `sink.terminate()`.
//!   The chain stops immediately; see `on_chunk`.
//! - A bounded hold-and-release processor consumes exactly one decision for
//!   each generation. Pass releases held raw bytes in source order; replace
//!   discards held/current bytes before exposing the replacement; reject
//!   discards them before pushing a diagnostic and terminating; masking
//!   releases approved bytes before replacement/terminal bytes and terminate.
//!   EOS resolves the final generation before propagating the sole EOS.

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use pingora_proxy::ResponseBodySink;

use super::session::PluginSession;
use crate::ctx::ResponseStreamKind;

/// Typed completion evidence for the canonical upstream response stream.
///
/// Keeping the pre-trailer boundary distinct is essential: bounded processors
/// still need an opportunity to flush, while a trailer-aware processor must
/// not mistake that boundary for a response that ended without trailers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamingResponseEvent {
    Data { end_of_stream: bool },
    TerminalBeforeTrailers,
    TerminalWithoutTrailers,
}

impl StreamingResponseEvent {
    #[inline]
    pub fn end_of_stream_for_legacy(self) -> bool {
        match self {
            Self::Data { end_of_stream } => end_of_stream,
            Self::TerminalBeforeTrailers | Self::TerminalWithoutTrailers => true,
        }
    }

    #[inline]
    pub fn closes_without_trailers(self) -> bool {
        matches!(self, Self::Data { end_of_stream: true } | Self::TerminalWithoutTrailers)
    }
}

#[async_trait]
pub trait StreamingResponseProcessor: Send + Sync {
    /// Processor name for diagnostics (never logged per chunk).
    fn name(&self) -> &str;

    /// Called once for the final upstream response after retry arbitration and
    /// before operator response plugins. Implementations may normalize status
    /// and headers and retain a bounded body-processing mode.
    fn on_response_header(&mut self, _response: &mut ResponseHeader) {}

    /// Whether this processor may change the response body length.
    ///
    /// The proxy uses this declaration to remove stale Content-Length metadata
    /// at both response-header boundaries. The default keeps observational
    /// processors such as usage extraction framing-neutral.
    fn changes_body_length(&self) -> bool {
        false
    }

    /// Settle request-level aggregate state when the response pump ends
    /// without delivering EOS (for example an upstream error or downstream
    /// cancellation). Called once from the universal logging boundary.
    fn finalize_incomplete(&mut self, _outcome: &'static str, _reason: &'static str) {}

    /// Promptly settle a request-scoped owner after another processor instance
    /// wins the response terminal. The winning instance is never called.
    fn on_terminal_preempted(&mut self, _winner: crate::ai::terminal::ResponseTerminalCause) {}

    /// Release resources held across an async callback when its future is
    /// cancelled before its driver execution lease completes.
    fn release_inflight(&mut self) {}

    /// Typed response-stream callback.
    ///
    /// Existing processors delegate to [`Self::on_chunk`] and therefore flush
    /// at either terminal boundary. Trailer-aware processors override this
    /// method so `TerminalBeforeTrailers` can preserve protocol ownership for
    /// the following trailer phase.
    async fn on_event(
        &mut self,
        chunk: &mut Option<Bytes>,
        event: StreamingResponseEvent,
        stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        self.on_chunk(chunk, event.end_of_stream_for_legacy(), stream_kind, sink, session)
            .await;
    }

    /// Called for every upstream response body chunk, including the
    /// end-of-stream call (where `chunk` may be `None`). `stream_kind` is the
    /// allocation-free classification copied from the request context.
    ///
    /// Mutating `chunk` **transforms** the upstream byte stream and IS visible
    /// to later processors in the chain. `sink.push()` **injects**
    /// self-generated content and is NOT visible to them -- a processor whose
    /// output must be seen by the next one has to mutate `chunk`.
    /// The sink independently bounds bytes and nonempty chunks passed to
    /// `sink.push()` per pump batch; callers must handle a rejected push. It
    /// does not limit in-place growth of `chunk`. A processor that expands the
    /// current chunk must enforce its own output bound.
    ///
    /// Calling `sink.terminate()` ends the response after the queued chunks are
    /// written; the chain stops immediately and no further calls are made to
    /// any processor, including the end-of-stream call. Settle state in the
    /// `logging` phase, which fires on the terminate path too.
    ///
    /// A processor that calls `sink.terminate()` (or otherwise changes the
    /// response body length) MUST also override `changes_body_length()` to
    /// return `true`, so the framing layer strips `content-length` at both
    /// response-header boundaries. Terminating while `content-length` is still
    /// committed puts fewer bytes on the wire than declared, which the client
    /// reads as a broken connection rather than a normal end of stream.
    ///
    /// Any outbound call made here must be bounded by the processor itself with
    /// `tokio::time::timeout` (see `ai/quota/runtime.rs`). There is no
    /// fork-level timeout wrapper.
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    );
}
