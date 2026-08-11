//! Writable, async per-chunk processors for upstream response streams.
//!
//! This is the AI-gateway streaming pipeline contract (spec:
//! `docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md`). It is
//! deliberately NOT an operator-facing EdgionPlugins stage: request-stage
//! plugins that need to observe or transform the response stream push a
//! processor onto `EdgionHttpContext::stream_processors`, and the proxy's
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

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use pingora_proxy::ResponseBodySink;

use super::session::PluginSession;
use crate::ctx::ResponseStreamKind;

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

    /// Called for every upstream response body chunk, including the
    /// end-of-stream call (where `chunk` may be `None`). `stream_kind` is the
    /// allocation-free classification copied from the request context.
    ///
    /// Mutating `chunk` **transforms** the upstream byte stream and IS visible
    /// to later processors in the chain. `sink.push()` **injects**
    /// self-generated content and is NOT visible to them -- a processor whose
    /// output must be seen by the next one has to mutate `chunk`.
    /// The sink's byte budget counts only chunks passed to `sink.push()`; it
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
