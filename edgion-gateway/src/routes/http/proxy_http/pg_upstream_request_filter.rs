use super::EdgionHttpProxy;
use crate::routes::grpc::format_grpc_timeout;
use crate::EdgionHttpContext;
use pingora_http::RequestHeader;
use pingora_proxy::Session;

/// Pingora `upstream_request_filter` hook.
///
/// Runs just before the request is written to the upstream connection. The
/// only mutation today is rewriting `grpc-timeout` for requests that carry
/// a parsed deadline, so the upstream sees the remaining time rather than
/// the original client value (which may be stale after route matching,
/// plugin execution, retries, and queueing).
///
/// When no deadline is set (non-gRPC, missing header, or unparseable
/// header) the upstream request is passed through unchanged.
///
/// Zero-remaining edge case: `upstream_peer`'s entry check normally aborts
/// before this hook runs, but a deadline can decay to zero in the small
/// window between `upstream_peer` returning OK and Pingora invoking this
/// hook (TLS handshake, connection establishment). In that race we still
/// insert `grpc-timeout: 1n` so the upstream sees "give up immediately"
/// rather than the stale original client value.
#[inline]
pub async fn upstream_request_filter(
    _edgion_http: &EdgionHttpProxy,
    _session: &mut Session,
    upstream_request: &mut RequestHeader,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    if let Some(remaining) = ctx.grpc_remaining() {
        let value = if remaining.is_zero() {
            "1n".to_string()
        } else {
            format_grpc_timeout(remaining)
        };
        let _ = upstream_request.insert_header("grpc-timeout", &value);
    }
    Ok(())
}
