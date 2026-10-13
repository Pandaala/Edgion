//! Optional capture and terminal publication; Session retains transport ownership.

use std::time::{Duration, Instant};

use bytes::Bytes;
use edgion_resources::EdgionErrorCode;
use pingora_core::{Error, ErrorType};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::EdgionHttpContext;

use super::storage::Hit;

pub(crate) fn record_discovery(ctx: &mut EdgionHttpContext, stamp: Option<super::key::DiscoveryStamp>) {
    if let Some(state) = ctx.response_cache.as_mut() {
        state.record_discovery(stamp);
    }
}
pub(crate) fn record_traffic(ctx: &mut EdgionHttpContext, stamp: Option<super::key::TrafficStamp>) {
    if let Some(state) = ctx.response_cache.as_mut() {
        state.record_traffic(stamp);
    }
}
pub(crate) fn record_tls(ctx: &mut EdgionHttpContext, stamp: super::key::TlsStamp) {
    if let Some(state) = ctx.response_cache.as_mut() {
        state.record_tls(stamp);
    }
}

fn compatible(ctx: &EdgionHttpContext) -> bool {
    !ctx.plugin_terminated()
        && !ctx.response_stage_settled()
        && !ctx.request_info.is_grpc_request
        && !ctx.is_grpc_route_matched
        && !matches!(
            ctx.request_info.discover_protocol.as_deref(),
            Some("grpc-web" | "websocket")
        )
        && ctx.grpc_transcoding.is_none()
        && ctx.ai_state().is_none()
        && !ctx.has_request_body_stream_handlers()
        && !ctx.has_request_body_observers()
        && !ctx.has_response_stream_processors()
        && !ctx.has_response_framing_reservation()
        && ctx.session_persistence().is_none()
}

pub(crate) async fn bind_request(session: &mut Session, ctx: &mut EdgionHttpContext) {
    if ctx.response_cache.is_none() {
        return;
    }
    if !compatible(ctx) {
        ctx.response_cache = None;
        return;
    }
    let Some(mut state) = ctx.response_cache.take() else {
        return;
    };
    if state.bind(session, ctx).await.is_some() {
        ctx.response_cache = Some(state);
    }
}

pub(crate) fn prepare_hit(session: &mut Session, ctx: &mut EdgionHttpContext) -> Option<Hit> {
    if ctx.response_cache.is_none() || !compatible(ctx) {
        return None;
    }
    if !matches!(session.req_header().method.as_str(), "GET" | "POST")
        || !super::policy::request_is_eligible(&session.req_header().headers)
        || !session.as_downstream_mut().is_body_done()
    {
        return None;
    }
    ctx.response_cache.as_ref()?.lookup()
}

pub(crate) fn capture_headers(ctx: &mut EdgionHttpContext, response: &ResponseHeader) {
    if ctx.response_cache.is_none() {
        return;
    }
    if !compatible(ctx) {
        abandon(ctx);
        return;
    }
    let request_at = ctx
        .get_current_upstream()
        .map_or(ctx.start_time, |upstream| upstream.start_time);
    if let Some(state) = ctx.response_cache.as_mut() {
        state.capture(response, request_at);
    }
}

/// Seal the selected final onion header before Pingora applies transport headers.
pub(crate) fn seal_headers(ctx: &mut EdgionHttpContext, chosen_header: &ResponseHeader) {
    if ctx.response_cache.is_none() {
        return;
    }
    if !compatible(ctx) {
        abandon(ctx);
        return;
    }
    if let Some(state) = ctx.response_cache.as_mut() {
        state.seal_pending_headers(chosen_header);
    }
}

pub(crate) fn capture_body(ctx: &mut EdgionHttpContext, body: Option<&Bytes>, eos: bool) {
    if let Some(state) = ctx.response_cache.as_mut() {
        state.push(body, eos);
    }
}

pub(crate) fn abandon(ctx: &mut EdgionHttpContext) {
    if let Some(state) = ctx.response_cache.as_mut() {
        state.abandon();
    }
}

fn output_bound(ctx: &EdgionHttpContext) -> Duration {
    let client = ctx.egc.parsed_timeouts.client.write_timeout;
    ctx.request_timeout().map_or(client, |timeout| {
        client.min(timeout.saturating_sub(Instant::now().saturating_duration_since(ctx.start_time)))
    })
}

pub(crate) async fn deliver_hit(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    mut hit: Hit,
) -> pingora_core::Result<()> {
    if !compatible(ctx) {
        return Err(Error::explain(
            ErrorType::HTTPStatus(500),
            "cached response processing failed",
        ));
    }
    let Some(state) = ctx.response_cache.as_mut() else {
        return Err(Error::explain(
            ErrorType::HTTPStatus(500),
            "cached response state unavailable",
        ));
    };
    state.retain_header_charge(&mut hit);
    let age = hit
        .entry
        .freshness
        .age(Instant::now())
        .ok_or_else(|| Error::explain(ErrorType::HTTPStatus(500), "cached response age unavailable"))?;
    hit.header.insert_header("age", age.as_secs().to_string())?;
    let bound = output_bound(ctx);
    if bound.is_zero() {
        return Err(Error::explain(ErrorType::WriteTimedout, "cached response deadline exhausted").into_down());
    }
    let result = tokio::time::timeout(bound, async {
        session.write_response_header(Box::new(hit.header), false).await?;
        for range in (0..hit.body.len()).step_by(16 * 1024) {
            let end = (range + 16 * 1024).min(hit.body.len());
            session
                .write_response_body(Some(hit.body.slice(range..end)), false)
                .await?;
        }
        session.write_response_body(None, true).await?;
        session.as_downstream_mut().finish_body().await
    })
    .await;
    match result {
        Ok(result) => result.map_err(Error::into_down),
        Err(_) => Err(Error::explain(ErrorType::WriteTimedout, "cached response delivery timed out").into_down()),
    }
}

/// EOS remains private until final response delivery is accepted by the transport.
pub(crate) async fn finalize(session: &mut Session, ctx: &mut EdgionHttpContext, error: Option<&Error>) {
    if ctx.response_cache.is_none() {
        return;
    }
    if error.is_some() || ctx.response_stage_settled() {
        abandon(ctx);
        return;
    }
    if !ctx
        .response_cache
        .as_mut()
        .is_some_and(|state| state.prepare_publication(session.response_written()))
    {
        return;
    }
    // The awaited terminal operation owns the writer. Cancellation drops it
    // immediately, even when the request context survives the cancelled future.
    let Some(pending) = ctx.response_cache.as_mut().and_then(|state| state.take_pending()) else {
        return;
    };
    let bound = output_bound(ctx);
    if bound.is_zero() {
        abandon(ctx);
        ctx.record_error(EdgionErrorCode::RequestTimeoutExceeded);
        return;
    }
    match tokio::time::timeout(bound, session.as_downstream_mut().finish_body()).await {
        Ok(Ok(())) => {
            if let Some(state) = ctx.response_cache.as_mut() {
                state.publish_pending(pending);
            }
        }
        _ => {
            abandon(ctx);
            ctx.record_error(EdgionErrorCode::ClientClosed);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/response_cache/lifecycle_test.rs"]
mod tests;
