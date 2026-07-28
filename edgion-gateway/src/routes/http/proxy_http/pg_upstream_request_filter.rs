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
///
/// This hook also owns the exit point for `ctx.request_info.in_request_body_stage`
/// (see that field's doc for the full lifecycle): it runs only after a
/// successful upstream connect, which is guaranteed to be after both
/// backendRef-scoped request-plugin call sites (`pg_upstream_peer.rs`'s HTTP
/// path and `routes/grpc/integration.rs`'s gRPC path) have already executed,
/// and it runs right before Pingora starts pumping the downstream body
/// upstream.
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

    apply_upstream_authority(upstream_request, ctx);

    sync_body_framing_headers(upstream_request, ctx)?;

    // Body proxying is about to begin: both backendRef-scoped request-plugin
    // call sites have already run by this point (a prerequisite for reaching
    // a successful connect and this hook), so `read_request_body` must be
    // refused from here on UNLESS an earlier request plugin already populated
    // the shared cache. This is the single exit point paired with
    // `snapshot_body_declared`'s entry write; it closes capture, not cached
    // reads.
    ctx.request_info.in_request_body_stage = false;

    Ok(())
}

fn sync_body_framing_headers(
    upstream_request: &mut RequestHeader,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    let Some(len) = ctx.request_body.modified_len_for_framing().map_err(|_| {
        pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            "modified request body length is unavailable",
        )
    })?
    else {
        return Ok(());
    };
    upstream_request.remove_header(&http::header::TRANSFER_ENCODING);
    upstream_request
        .insert_header(http::header::CONTENT_LENGTH, len.to_string())
        .map_err(|_| {
            pingora_core::Error::explain(
                pingora_core::ErrorType::InternalError,
                "failed to synchronize modified request body framing",
            )
        })
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
mod authority_tests {
    use super::apply_upstream_authority;
    use crate::ctx::EdgionHttpContext;

    /// The filter writes the upstream Host from the ctx when an authority was
    /// composed, and leaves the request untouched when it was not.
    #[test]
    fn upstream_host_is_written_from_ctx() {
        let mut req = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
        req.insert_header("Host", "gw.example.com").unwrap();

        let mut ctx = EdgionHttpContext::default();
        ctx.upstream_authority = Some("tenant-a.sandbox.internal".to_string());
        apply_upstream_authority(&mut req, &ctx);
        assert_eq!(req.headers.get("host").unwrap(), "tenant-a.sandbox.internal");

        // No authority => Host untouched.
        let mut req2 = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
        req2.insert_header("Host", "gw.example.com").unwrap();
        let ctx2 = EdgionHttpContext::default();
        apply_upstream_authority(&mut req2, &ctx2);
        assert_eq!(req2.headers.get("host").unwrap(), "gw.example.com");
    }
}

#[cfg(all(test, feature = "body-buffering"))]
mod body_framing_tests {
    use super::sync_body_framing_headers;
    use crate::request_body::BodyStorage;
    use crate::EdgionHttpContext;
    use bytes::Bytes;
    use pingora_core::protocols::http::RequestBodyBuffer;

    #[tokio::test]
    async fn mutation_sets_content_length_and_removes_transfer_encoding() {
        let (mut storage, view) = BodyStorage::new(64, 64);
        storage.write(&Bytes::from_static(b"old")).await.unwrap();
        storage.finish().await.unwrap();
        let mut request_body = crate::request_body::RequestBodyState::default();
        request_body.install(view);
        request_body.overwrite(Bytes::from_static(b"new-body")).unwrap();

        let mut ctx = EdgionHttpContext {
            request_body,
            ..Default::default()
        };
        let mut request = pingora_http::RequestHeader::build("POST", b"/", None).unwrap();
        request.insert_header("transfer-encoding", "chunked").unwrap();
        request.insert_header("content-length", "3").unwrap();

        sync_body_framing_headers(&mut request, &mut ctx).unwrap();
        assert_eq!(request.headers.get("content-length").unwrap(), "8");
        assert!(!request.headers.contains_key("transfer-encoding"));

        ctx.request_body.release_view();
        let mut retry_request = pingora_http::RequestHeader::build("POST", b"/", None).unwrap();
        retry_request.insert_header("transfer-encoding", "chunked").unwrap();
        retry_request.insert_header("content-length", "3").unwrap();

        sync_body_framing_headers(&mut retry_request, &mut ctx).unwrap();
        assert_eq!(retry_request.headers.get("content-length").unwrap(), "8");
        assert!(!retry_request.headers.contains_key("transfer-encoding"));
    }
}
