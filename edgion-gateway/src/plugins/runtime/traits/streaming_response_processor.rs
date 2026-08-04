//! Writable, synchronous per-chunk processors for upstream response streams.
//!
//! This is the AI-gateway streaming pipeline contract (spec:
//! `docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md`). It is
//! deliberately NOT an operator-facing EdgionPlugins stage: request-stage
//! plugins that need to observe or transform the response stream push a
//! processor onto `EdgionHttpContext::stream_processors`, and the proxy's
//! upstream-response-body hook runs them in installation order, BEFORE the
//! read-only `UpstreamResponseBodyFilter` chains (so bandwidth limiting sees
//! post-mutation bytes).
//!
//! Contract:
//! - Sync, per transport chunk, writable: a processor may replace, drop, or
//!   extend the current chunk in place. It must NOT accumulate unbounded
//!   state across chunks (see `crate::sse::SseDecoder` for the bounded
//!   framing substrate).
//! - The pipeline runs whenever processors are installed, regardless of
//!   `ResponseStreamKind` — classification tells a processor HOW to interpret
//!   chunks, not WHETHER it runs.
//! - No per-chunk logging or tracing; aggregate per-request state and emit
//!   once outside the body hot path (same rule as the read-only trait).
//! - No termination/pause/resume: the proxy pump owns the stream. A
//!   terminate action arrives with AI-4 together with fork support.

use bytes::Bytes;
use pingora_http::ResponseHeader;

use super::session::PluginSession;
use crate::ctx::ResponseStreamKind;

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
    fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        stream_kind: ResponseStreamKind,
        session: &mut dyn PluginSession,
    );
}
