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

    // Publish the configured forwarded identity before MCP reconstructs its
    // protected protocol and credential headers. The configurable RemoteIp
    // header name may overlap an MCP-owned name; MCP must win that conflict.
    finalize_forwarded_headers(&ctx.remote_ip_header, upstream_request, ctx);

    // This is the final owned-header mutation. It intentionally runs after
    // every request plugin and all other local header writers so neither a
    // client/plugin value nor a conflicting RemoteIp header can replace the
    // pinned MCP protocol headers or selected backend credential.
    apply_mcp_request_headers(upstream_request, ctx)?;

    // All fallible local header/framing preparation has succeeded. Crossing
    // the boundary here keeps a local preparation failure pre-dispatch.
    crate::ai::dispatch::mark_ai_dispatch_started(ctx).map_err(|_| {
        pingora_core::Error::explain(pingora_core::ErrorType::InternalError, "AI dispatch state is invalid")
    })?;

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

fn apply_mcp_request_headers(
    upstream_request: &mut RequestHeader,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    let Some(plan) = ctx.mcp_request_plan.as_ref() else {
        return Ok(());
    };
    if !plan.body_generation_matches(ctx) || !plan.selected_matches_context(ctx) {
        ctx.add_error(edgion_resources::EdgionStatus::UpstreamNotBackendRefs);
        return Err(pingora_core::Error::new(pingora_core::ErrorType::InternalError));
    }

    let bearer = plan
        .selected()
        .and_then(|selected| selected.backend.bearer.as_ref())
        .map(|bearer| bearer.authorization_header_value());
    rewrite_mcp_owned_headers(upstream_request, plan, bearer)?;
    Ok(())
}

fn rewrite_mcp_owned_headers(
    upstream_request: &mut RequestHeader,
    plan: &crate::mcp::request::McpRequestPlan,
    bearer: Option<http::HeaderValue>,
) -> pingora_core::Result<()> {
    let owned = upstream_request
        .headers
        .keys()
        .filter(|name| name.as_str().starts_with("mcp-"))
        .cloned()
        .collect::<Vec<_>>();
    for name in owned {
        upstream_request.remove_header(&name);
    }
    for name in ["authorization", "cookie", "mcp-session-id", "last-event-id"] {
        upstream_request.remove_header(name);
    }

    upstream_request.set_method(http::Method::POST);
    upstream_request.insert_header("content-type", "application/json")?;
    upstream_request.insert_header("accept", "application/json, text/event-stream")?;
    upstream_request.insert_header("mcp-protocol-version", crate::mcp::request::MCP_PROTOCOL_VERSION)?;
    upstream_request.insert_header("mcp-method", plan.method.as_wire_name())?;
    if let Some(name) = &plan.name_header {
        upstream_request.headers.insert("mcp-name", name.clone());
    }
    for header in &plan.passthrough_param_headers {
        upstream_request
            .headers
            .append(header.name.clone(), header.value.clone());
    }
    if let Some(bearer) = bearer {
        upstream_request.headers.insert(http::header::AUTHORIZATION, bearer);
    }
    Ok(())
}

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
    use super::{apply_upstream_authority, finalize_forwarded_headers};
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

    #[test]
    fn canonical_forwarded_headers_overwrite_plugin_values() {
        let mut req = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
        req.insert_header("X-Forwarded-For", "spoofed").unwrap();
        req.insert_header("X-Edgion-Remote-IP", "spoofed").unwrap();

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.x_forwarded_for = Some("198.51.100.8".to_string());
        ctx.request_info.downstream_peer_ip = "10.0.0.5".to_string();
        ctx.request_info.remote_ip = "203.0.113.9".to_string();
        let name = http::HeaderName::from_static("x-edgion-remote-ip");

        finalize_forwarded_headers(&name, &mut req, &ctx);

        assert_eq!(req.headers.get("x-forwarded-for").unwrap(), "198.51.100.8, 10.0.0.5");
        assert_eq!(req.headers.get(&name).unwrap(), "203.0.113.9");
    }
}

#[cfg(test)]
mod mcp_header_tests {
    use super::{finalize_forwarded_headers, rewrite_mcp_owned_headers};

    #[test]
    fn pinned_mcp_headers_replace_plugin_and_client_mutations() {
        let plan = crate::mcp::request::test_unselected_plan();
        let mut request = pingora_http::RequestHeader::build("POST", b"/rewritten", None).unwrap();
        request.insert_header("authorization", "Bearer downstream").unwrap();
        request.insert_header("cookie", "session=client").unwrap();
        request.insert_header("mcp-session-id", "legacy").unwrap();
        request.insert_header("last-event-id", "resume").unwrap();
        request.insert_header("mcp-protocol-version", "wrong").unwrap();
        request.insert_header("mcp-method", "server/discover").unwrap();
        request.insert_header("mcp-name", "changed").unwrap();
        request.insert_header("mcp-injected", "bad").unwrap();
        request.insert_header("mcp-param-region", "changed").unwrap();
        request.insert_header("content-type", "text/plain").unwrap();
        request.insert_header("accept", "*/*").unwrap();
        request.set_method(http::Method::DELETE);

        rewrite_mcp_owned_headers(&mut request, &plan, None).unwrap();

        assert!(!request.headers.contains_key("authorization"));
        assert!(!request.headers.contains_key("cookie"));
        assert!(!request.headers.contains_key("mcp-session-id"));
        assert!(!request.headers.contains_key("last-event-id"));
        assert!(!request.headers.contains_key("mcp-injected"));
        assert_eq!(request.headers.get("mcp-protocol-version").unwrap(), "2026-07-28");
        assert_eq!(request.headers.get("mcp-method").unwrap(), "tools/call");
        assert_eq!(request.headers.get("mcp-name").unwrap(), "weather");
        assert_eq!(request.method, http::Method::POST);
        assert_eq!(request.headers.get("content-type").unwrap(), "application/json");
        assert_eq!(
            request.headers.get("accept").unwrap(),
            "application/json, text/event-stream"
        );
        assert_eq!(request.headers.get("mcp-param-region").unwrap(), "west");
        assert_eq!(request.headers.get("mcp-param-zone").unwrap(), "east");
    }

    #[test]
    fn injected_bearer_remains_sensitive_and_replaces_downstream_value() {
        let plan = crate::mcp::request::test_unselected_plan();
        let mut request = pingora_http::RequestHeader::build("POST", b"/", None).unwrap();
        request.insert_header("authorization", "Bearer downstream").unwrap();
        let mut bearer = http::HeaderValue::from_static("Bearer upstream");
        bearer.set_sensitive(true);

        rewrite_mcp_owned_headers(&mut request, &plan, Some(bearer)).unwrap();

        let value = request.headers.get("authorization").unwrap();
        assert_eq!(value, "Bearer upstream");
        assert!(value.is_sensitive());
    }

    #[test]
    fn mcp_reconstruction_wins_conflicting_remote_ip_header_names() {
        let plan = crate::mcp::request::test_unselected_plan();
        let mut request = pingora_http::RequestHeader::build("POST", b"/", None).unwrap();
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.request_info.remote_ip = "203.0.113.9".to_string();

        let authorization = http::HeaderName::from_static("authorization");
        finalize_forwarded_headers(&authorization, &mut request, &ctx);
        let mut bearer = http::HeaderValue::from_static("Bearer upstream");
        bearer.set_sensitive(true);
        rewrite_mcp_owned_headers(&mut request, &plan, Some(bearer)).unwrap();
        assert_eq!(request.headers.get("authorization").unwrap(), "Bearer upstream");
        assert!(request.headers.get("authorization").unwrap().is_sensitive());

        let method = http::HeaderName::from_static("mcp-method");
        finalize_forwarded_headers(&method, &mut request, &ctx);
        rewrite_mcp_owned_headers(&mut request, &plan, None).unwrap();
        assert_eq!(request.headers.get("mcp-method").unwrap(), "tools/call");
        assert!(!request.headers.contains_key("authorization"));
    }
}

#[cfg(test)]
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
