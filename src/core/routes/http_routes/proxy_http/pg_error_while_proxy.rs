use super::EdgionHttp;
use crate::types::EdgionHttpContext;
use pingora_core::prelude::HttpPeer;
use pingora_core::Error;
use pingora_proxy::Session;

#[inline]
pub fn error_while_proxy(
    _edgion_http: &EdgionHttp,
    peer: &HttpPeer,
    session: &mut Session,
    e: Box<Error>,
    ctx: &mut EdgionHttpContext,
    client_reused: bool,
) -> Box<Error> {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.set_response_body_size(session.upstream_body_bytes_received());
    }
    let mut e = e.more_context(format!("Peer: {}", peer));
    // only reused client connections where retry buffer is not truncated
    e.retry
        .decide_reuse(client_reused && !session.as_ref().retry_buffer_truncated());
    // todo need add retry logic?
    e
}
