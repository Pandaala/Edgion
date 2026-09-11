use super::EdgionHttpProxy;
use crate::plugins::http::common::connection_listed_hop_by_hop;
use crate::routes::grpc::format_grpc_timeout;
use crate::{ctx::DeferredCredentialMutations, EdgionHttpContext};
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

    // This is the final owned-header mutation. It intentionally runs after
    // gRPC deadline, authority, framing, and every request-plugin stage so a
    // configured RemoteIp header remains authoritative even when its name
    // overlaps another locally managed header.
    finalize_forwarded_headers(&ctx.remote_ip_header, upstream_request, ctx);
    ctx.request_info.trace_id.reconcile_request(upstream_request);

    // All fallible local header/framing preparation has succeeded. Crossing
    // the boundary here keeps a local preparation failure pre-dispatch.
    crate::ai::dispatch::mark_ai_dispatch_started(ctx).map_err(|_| local_pre_dispatch_failure())?;

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
            if connection_listed_hop_by_hop(value)
                .iter()
                .any(|name| mutations.contains_target(name))
            {
                return Err(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod deferred_credential_connection_tests {
    use super::{local_pre_dispatch_failure, upstream_request_filter, validate_deferred_credential_connection};
    use crate::ctx::DeferredCredentialMutations;
    use crate::observe::AccessLogger;
    use crate::routes::grpc::GrpcPortRouteManager;
    use crate::routes::http::proxy_http::{EdgionHttpProxy, PreflightHandler};
    use crate::routes::http::routes_mgr::HttpPortRouteManager;
    use crate::EdgionHttpContext;
    use edgion_resources::{EdgionErrorCode, EdgionGatewayConfig, ListenerKey};
    use http::{HeaderName, HeaderValue};
    use pingora_core::ErrorType;
    use pingora_http::RequestHeader;
    use pingora_proxy::Session;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    fn mutations() -> DeferredCredentialMutations {
        let mut mutations = DeferredCredentialMutations::default();
        mutations.upsert(
            HeaderName::from_static("x-provider-token"),
            Arc::new(HeaderValue::from_static("private-token")),
            None,
        );
        mutations
    }

    #[test]
    fn rejects_target_nominated_by_a_late_connection_mutation() {
        let session_request = RequestHeader::build("GET", b"/", None).unwrap();
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request
            .insert_header("connection", "keep-alive, X-Provider-Token")
            .unwrap();
        assert!(validate_deferred_credential_connection(&session_request, &request, &mutations()).is_err());
    }

    #[test]
    fn accepts_unrelated_options_and_rejects_non_utf8_connection_values() {
        let session_request = RequestHeader::build("GET", b"/", None).unwrap();
        let mut unrelated = RequestHeader::build("GET", b"/", None).unwrap();
        unrelated.insert_header("connection", "keep-alive, X-Other").unwrap();
        assert!(validate_deferred_credential_connection(&session_request, &unrelated, &mutations()).is_ok());

        let mut invalid = RequestHeader::build("GET", b"/", None).unwrap();
        invalid
            .insert_header("connection", HeaderValue::from_bytes(b"X-Other, \xff").unwrap())
            .unwrap();
        assert!(validate_deferred_credential_connection(&session_request, &invalid, &mutations()).is_err());
    }

    #[test]
    fn checks_every_field_case_insensitively_and_accepts_upgrade() {
        let mut session_request = RequestHeader::build("GET", b"/", None).unwrap();
        session_request.append_header("connection", "keep-alive").unwrap();
        session_request
            .append_header("connection", "X-OTHER, x-PrOvIdEr-ToKeN")
            .unwrap();
        let mut attempt = RequestHeader::build("GET", b"/", None).unwrap();
        attempt.insert_header("connection", "Upgrade").unwrap();
        attempt.insert_header("upgrade", "websocket").unwrap();

        assert!(validate_deferred_credential_connection(&session_request, &attempt, &mutations()).is_err());

        session_request.remove_header(&http::header::CONNECTION);
        session_request
            .insert_header("connection", "keep-alive, X-Other")
            .unwrap();
        assert!(validate_deferred_credential_connection(&session_request, &attempt, &mutations()).is_ok());
    }

    #[test]
    fn rejects_non_utf8_in_the_session_request() {
        let mut session_request = RequestHeader::build("GET", b"/", None).unwrap();
        session_request
            .insert_header("connection", HeaderValue::from_bytes(b"X-Other, \xff").unwrap())
            .unwrap();
        let attempt = RequestHeader::build("GET", b"/", None).unwrap();
        assert!(validate_deferred_credential_connection(&session_request, &attempt, &mutations()).is_err());
    }

    #[test]
    fn local_pre_dispatch_error_is_static_500_and_nonretryable() {
        let error = local_pre_dispatch_failure();
        assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
        assert!(!error.retry());
    }

    fn proxy() -> EdgionHttpProxy {
        EdgionHttpProxy {
            listener_port: 80,
            is_https_listener: false,
            listener_key: ListenerKey("127.0.0.1:80/HTTP".to_string()),
            server_header_opts: Default::default(),
            access_logger: Arc::new(AccessLogger::default()),
            edgion_gateway_config: Arc::new(EdgionGatewayConfig::new("test", Default::default())),
            request_body_config_name: None,
            parsed_timeouts: Default::default(),
            enable_http2: false,
            real_ip_extractor: None,
            preflight_handler: PreflightHandler::new(None),
            http_route_manager: Arc::new(HttpPortRouteManager::default()),
            grpc_route_manager: Arc::new(GrpcPortRouteManager::default()),
        }
    }

    async fn session_from_request(raw_request: &[u8]) -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        client.write_all(raw_request).await.unwrap();
        drop(client);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        session.read_request().await.unwrap();
        session
    }

    #[tokio::test]
    async fn full_hook_rejects_session_nomination_before_materialization_or_ai_dispatch() {
        let mut session =
            session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\nConnection: keep-alive, X-Provider-Token\r\n\r\n")
                .await;
        let mut attempt = session.req_header().clone();
        attempt.remove_header(&http::header::CONNECTION);
        let mut ctx = EdgionHttpContext::default();
        ctx.upsert_deferred_credential_mutation(
            HeaderName::from_static("x-provider-token"),
            Arc::new(HeaderValue::from_static("private-token")),
            None,
        );

        let error = upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
            .await
            .unwrap_err();

        assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
        assert!(!error.retry());
        assert!(ctx
            .diagnostics
            .contains_error(EdgionErrorCode::CredentialConnectionConflict));
        assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::CredentialExpired));
        assert!(!attempt.headers.contains_key("x-provider-token"));
        assert!(ctx
            .ai_state()
            .is_none_or(|state| !state.attempt_runtime.ever_dispatched()));
    }

    #[tokio::test]
    async fn full_hook_rejects_attempt_local_nomination() {
        let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut attempt = session.req_header().clone();
        attempt.insert_header("connection", "x-provider-token").unwrap();
        let mut ctx = EdgionHttpContext::default();
        ctx.upsert_deferred_credential_mutation(
            HeaderName::from_static("x-provider-token"),
            Arc::new(HeaderValue::from_static("private-token")),
            None,
        );

        let error = upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
            .await
            .unwrap_err();
        assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
        assert!(!error.retry());
        assert!(ctx
            .diagnostics
            .contains_error(EdgionErrorCode::CredentialConnectionConflict));
        assert!(!attempt.headers.contains_key("x-provider-token"));
    }

    #[tokio::test]
    async fn full_hook_preserves_expiry_diagnostic_without_materializing_secret() {
        let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut attempt = session.req_header().clone();
        let mut ctx = EdgionHttpContext::default();
        ctx.upsert_deferred_credential_mutation(
            HeaderName::from_static("x-provider-token"),
            Arc::new(HeaderValue::from_static("private-token")),
            Some(std::time::Instant::now()),
        );

        let error = upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
            .await
            .unwrap_err();

        assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
        assert!(!error.retry());
        assert!(ctx.diagnostics.contains_error(EdgionErrorCode::CredentialExpired));
        assert!(!ctx
            .diagnostics
            .contains_error(EdgionErrorCode::CredentialConnectionConflict));
        assert!(!attempt.headers.contains_key("x-provider-token"));
    }

    #[tokio::test]
    async fn full_hook_without_credentials_completes_local_preparation() {
        let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut attempt = session.req_header().clone();
        let mut ctx = EdgionHttpContext::default();

        upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
            .await
            .unwrap();

        assert!(ctx.diagnostics.is_empty());
        assert!(ctx.deferred_credential_mutations().is_none());
        assert!(ctx
            .ai_state()
            .is_none_or(|state| !state.attempt_runtime.ever_dispatched()));
    }

    #[tokio::test]
    async fn full_hook_materializes_a_valid_unexpired_credential() {
        let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut attempt = session.req_header().clone();
        let mut ctx = EdgionHttpContext::default();
        ctx.upsert_deferred_credential_mutation(
            HeaderName::from_static("x-provider-token"),
            Arc::new(HeaderValue::from_static("private-token")),
            Some(std::time::Instant::now() + std::time::Duration::from_secs(60)),
        );

        upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
            .await
            .unwrap();

        assert_eq!(attempt.headers.get("x-provider-token").unwrap(), "private-token");
        assert!(ctx.diagnostics.is_empty());
    }
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

        let mut ctx = EdgionHttpContext::default();
        ctx.request_body = request_body;
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
