//! Per-chunk streamed request-body handler seam.
//!
//! A request-stage plugin whose policy requires seeing the request body chunk
//! by chunk (without caching it) installs a [`RequestBodyStreamHandler`] into
//! the request context via
//! [`PluginSession::install_request_body_stream_handler`]. The `ProxyHttp`
//! hook `request_body_filter_action` then drives every installed handler for
//! each transport body event, before the chunk is forwarded upstream.
//!
//! Contract highlights (see the Pingora interface consumer contract):
//! - Handlers may mutate the current chunk in place; they must not accumulate
//!   the body (at most per-chunk state) — the request-body cache invariant
//!   allows only the `BodyStorage` cache.
//! - While any handler is installed the request is non-replayable
//!   (`request_retry_allowed == false`) and the upstream framing switches to
//!   the `Streamed` disposition. The context vector is therefore never cleared
//!   once populated, which keeps the retry predicate monotonic.
//! - A `Terminate` verdict makes the hook write the local reply and stop the
//!   proxying of this request; the handler only supplies status/body.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use pingora_proxy::RequestBodyEvent;

use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::DyeOutcome;

/// Verdict for one body event.
pub enum ChunkVerdict {
    /// Proceed with the (possibly mutated) chunk.
    Continue,
    /// Stop the request: the hook writes this local reply downstream and
    /// terminates proxying. The handler must already have recorded its log
    /// entry for the cause. `headers` are handler-authored reply headers,
    /// pre-filtered by the handler (protected/pseudo headers removed) —
    /// applied to the local reply where the reply shape permits (a gRPC
    /// trailers-only reply keeps only trailer-valid entries).
    Terminate {
        status: u16,
        body: Option<String>,
        headers: Vec<(String, String)>,
    },
}

/// Per-request handler driven once per transport body event.
///
/// Multiple handlers run in install order and compose: each sees the previous
/// handler's (possibly mutated) chunk. A `Terminate` verdict from handler N
/// short-circuits the chain — handlers N+1... are not driven for that event
/// and their resources are simply dropped with the request.
#[async_trait]
pub trait RequestBodyStreamHandler: Send {
    /// Plugin name for logs and diagnostics.
    fn name(&self) -> &str;

    /// Inspect/mutate the current chunk in place. `chunk` may be `None` for a
    /// terminal event. `Complete` means the real downstream EOF was observed;
    /// `Abandoned` means earlier chunks are only a prefix. Called serially;
    /// the pump does not read the next chunk until this returns.
    async fn on_chunk(&mut self, chunk: &mut Option<Bytes>, event: RequestBodyEvent) -> ChunkVerdict;

    /// Drain accumulated access-log entries. The hook pushes the drained log
    /// under the `request_body_filters` stage at EOS, on terminate, and as a
    /// leftover sweep in the logging phase (client-abort path).
    fn take_log(&mut self) -> Option<PluginLog>;

    /// Dye plan captured at install time. The hook re-installs it as the
    /// current plan before recording body-phase outcomes.
    fn dye_plan(&self) -> Option<Arc<CompiledDyePlan>>;

    /// A dye outcome produced by the last `on_chunk` call, if any (fail-open
    /// continuation or fail-close termination; a processor's immediate
    /// response is a verdict, not a failure, and records nothing). Polled by
    /// the hook after each call; returns at most once per event.
    fn take_pending_outcome(&mut self) -> Option<(DyeOutcome, String)> {
        None
    }
}
