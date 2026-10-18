use super::EdgionHttpProxy;
use crate::plugins::http::common::http_client::connection_option_tokens;
use crate::routes::grpc::format_grpc_timeout;
use crate::{ctx::DeferredCredentialMutations, EdgionHttpContext};
use pingora_http::RequestHeader;
use pingora_proxy::Session;

/// Pingora `upstream_request_filter` hook.
///
/// Runs just before each attempt is written to the upstream connection. It
/// publishes the remaining gRPC deadline, selected authority, final body
/// framing, deferred credentials, managed request IDs, and canonical forwarded
/// identity in their required ownership order before marking AI dispatch.
///
/// Zero-remaining edge case: `upstream_peer`'s entry check normally aborts
/// before this hook runs, but a deadline can decay to zero in the small
/// window between `upstream_peer` returning OK and Pingora invoking this
/// hook (TLS handshake, connection establishment). In that race we still
/// insert `grpc-timeout: 1n` so the upstream sees "give up immediately"
/// rather than the stale original client value.
///
/// Request-body policy has already been frozen at the end of
/// `pg_request_filter`; this hook is attempt-local and cannot change its source
/// or processor set.
#[inline]
pub async fn upstream_request_filter(
    _edgion_http: &EdgionHttpProxy,
    session: &mut Session,
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

    crate::plugins::http::grpc_json_transcoder::prepare_attempt(upstream_request, ctx)?;
    let rpc_timeout = ctx
        .grpc_transcoding
        .as_ref()
        .and_then(|_| upstream_request.headers.get("grpc-timeout").cloned());
    apply_upstream_authority(upstream_request, ctx);

    sync_body_framing_headers(upstream_request, ctx).map_err(|_| local_pre_dispatch_failure())?;

    if let Some(mutations) = ctx.deferred_credential_mutations() {
        if validate_deferred_credential_connection(session.req_header(), upstream_request, mutations).is_err() {
            ctx.record_error(edgion_resources::EdgionErrorCode::CredentialConnectionConflict);
            return Err(local_pre_dispatch_failure());
        }
        if !ctx.deferred_credentials_all_unexpired(std::time::Instant::now()) {
            ctx.record_error(edgion_resources::EdgionErrorCode::CredentialExpired);
            return Err(local_pre_dispatch_failure());
        }
        ctx.apply_deferred_credentials(upstream_request)
            .map_err(|_| local_pre_dispatch_failure())?;
    }

    // Reconcile correlation headers after every request-plugin stage, then
    // publish proxy-owned identity last. A configured RemoteIp header remains
    // authoritative even when its name overlaps a managed RequestId header.
    ctx.request_info.trace_id.reconcile_request(upstream_request);
    finalize_forwarded_headers(&ctx.remote_ip_header, upstream_request, ctx);

    crate::plugins::http::grpc_json_transcoder::transport::validate_attempt(upstream_request, ctx)?;
    if ctx.grpc_transcoding.is_some() && upstream_request.headers.get("grpc-timeout") != rpc_timeout.as_ref() {
        return Err(crate::plugins::http::grpc_json_transcoder::failure());
    }

    // All fallible local header/framing preparation has succeeded. Crossing
    // the boundary here keeps a local preparation failure pre-dispatch.
    crate::ai::runtime::dispatch::mark_ai_dispatch_started(ctx).map_err(|_| local_pre_dispatch_failure())?;

    Ok(())
}

/// One non-retryable error shape for failures owned by this local preparation
/// hook. The static detail is intentionally free of credential and topology
/// information; request diagnostics carry the actionable classification.
#[inline]
fn local_pre_dispatch_failure() -> Box<pingora_core::Error> {
    let mut error = pingora_core::Error::explain(
        pingora_core::ErrorType::HTTPStatus(500),
        "upstream request preparation failed",
    );
    error.set_retry(false);
    error
}

/// Recheck the final attempt-local header block after every request plugin.
/// A later plugin may have changed `Connection` after an injector ran; invalid
/// UTF-8 or a dynamically nominated credential target must fail closed before
/// the credential is materialized.
fn validate_deferred_credential_connection(
    session_request: &RequestHeader,
    upstream_request: &RequestHeader,
    mutations: &DeferredCredentialMutations,
) -> Result<(), ()> {
    for request in [session_request, upstream_request] {
        for value in request.headers.get_all(http::header::CONNECTION) {
            let value = value.to_str().map_err(|_| ())?;
            if connection_option_tokens(value).any(|name| mutations.contains_target(name)) {
                return Err(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_upstream_request_filter_test/deferred_credential_connection_tests.rs"]
mod deferred_credential_connection_tests;

/// Authoritatively publish the canonical downstream identity after every
/// request plugin, including backendRef-scoped runtimes, has completed.
fn finalize_forwarded_headers(
    remote_ip_header: &http::HeaderName,
    upstream_request: &mut RequestHeader,
    ctx: &EdgionHttpContext,
) {
    let peer = &ctx.request_info.downstream_peer_ip;
    if let Some(existing) = &ctx.request_info.x_forwarded_for {
        let mut value = String::with_capacity(existing.len() + 2 + peer.len());
        value.push_str(existing);
        value.push_str(", ");
        value.push_str(peer);
        let _ = upstream_request.insert_header("X-Forwarded-For", value);
    } else {
        let _ = upstream_request.insert_header("X-Forwarded-For", peer);
    }

    let _ = upstream_request.insert_header(remote_ip_header.clone(), &ctx.request_info.remote_ip);
}

fn sync_body_framing_headers(upstream_request: &mut RequestHeader, ctx: &mut EdgionHttpContext) -> Result<(), ()> {
    let Some(len) = ctx.request_body.modified_len_for_framing().map_err(|_| ())? else {
        return Ok(());
    };
    upstream_request.remove_header(&http::header::TRANSFER_ENCODING);
    upstream_request
        .insert_header(http::header::CONTENT_LENGTH, len.to_string())
        .map_err(|_| ())
}

/// Overwrite the upstream `Host` with the authority composed in `upstream_peer`
/// (which also set the same value as the peer's TLS SNI — see
/// `pg_upstream_peer::apply_upstream_authority`). A no-op when the target
/// Service carries no `upstreamAuthority` policy.
pub(crate) fn apply_upstream_authority(req: &mut RequestHeader, ctx: &EdgionHttpContext) {
    if let Some(authority) = &ctx.upstream_authority {
        // The authority is `<validated label><fixed suffix>`, so it cannot contain
        // a header-splitting byte; `insert_header` cannot fail on it.
        let _ = req.insert_header("Host", authority.as_str());
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_upstream_request_filter_test/authority_tests.rs"]
mod authority_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_upstream_request_filter_test/body_framing_tests.rs"]
mod body_framing_tests;
