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

    if let Some(mutations) = ctx.deferred_credential_mutations() {
        validate_deferred_credential_connection(upstream_request, mutations).map_err(|_| {
            pingora_core::Error::explain(
                pingora_core::ErrorType::InternalError,
                "deferred credential target conflicts with Connection header",
            )
        })?;
    }

    if !ctx.deferred_credentials_all_unexpired(std::time::Instant::now()) {
        ctx.record_error(edgion_resources::EdgionErrorCode::CredentialExpired);
        return Err(pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            "deferred credential expired before upstream dispatch",
        ));
    }

    ctx.apply_deferred_credentials(upstream_request).map_err(|_| {
        pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            "failed to apply deferred credential header",
        )
    })?;

    // This is the final owned-header mutation. It intentionally runs after
    // gRPC deadline, authority, framing, and every request-plugin stage so a
    // configured RemoteIp header remains authoritative even when its name
    // overlaps another locally managed header.
    finalize_forwarded_headers(&ctx.remote_ip_header, upstream_request, ctx);

    // All fallible local header/framing preparation has succeeded. Crossing
    // the boundary here keeps a local preparation failure pre-dispatch.
    crate::ai::dispatch::mark_ai_dispatch_started(ctx).map_err(|_| {
        pingora_core::Error::explain(pingora_core::ErrorType::InternalError, "AI dispatch state is invalid")
    })?;

    Ok(())
}

/// Recheck the final attempt-local header block after every request plugin.
/// A later plugin may have changed `Connection` after an injector ran; invalid
/// UTF-8 or a dynamically nominated credential target must fail closed before
/// the credential is materialized.
fn validate_deferred_credential_connection(
    upstream_request: &RequestHeader,
    mutations: &DeferredCredentialMutations,
) -> Result<(), ()> {
    for value in upstream_request.headers.get_all(http::header::CONNECTION) {
        let value = value.to_str().map_err(|_| ())?;
        if connection_listed_hop_by_hop(value)
            .iter()
            .any(|name| mutations.contains_target(name))
        {
            return Err(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod deferred_credential_connection_tests {
    use super::validate_deferred_credential_connection;
    use crate::ctx::DeferredCredentialMutations;
    use http::{HeaderName, HeaderValue};
    use pingora_http::RequestHeader;
    use std::sync::Arc;

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
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request
            .insert_header("connection", "keep-alive, X-Provider-Token")
            .unwrap();
        assert!(validate_deferred_credential_connection(&request, &mutations()).is_err());
    }

    #[test]
    fn accepts_unrelated_options_and_rejects_non_utf8_connection_values() {
        let mut unrelated = RequestHeader::build("GET", b"/", None).unwrap();
        unrelated.insert_header("connection", "keep-alive, X-Other").unwrap();
        assert!(validate_deferred_credential_connection(&unrelated, &mutations()).is_ok());

        let mut invalid = RequestHeader::build("GET", b"/", None).unwrap();
        invalid
            .insert_header("connection", HeaderValue::from_bytes(b"X-Other, \xff").unwrap())
            .unwrap();
        assert!(validate_deferred_credential_connection(&invalid, &mutations()).is_err());
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
