use super::*;

#[tokio::test]
async fn stripping_query_credential_preserves_absolute_request_target() {
    use tokio::io::AsyncWriteExt;

    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"GET http://example.com/raw%2Fpath?api_key=secret&keep=1 HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    crate::plugins::http::common::auth_common::strip_query_param(&mut adapter, "api_key");

    assert_eq!(session.req_header().raw_path(), b"http://example.com/raw%2Fpath?keep=1");
    assert_eq!(session.req_header().uri.path(), "/raw%2Fpath");
    assert_eq!(session.req_header().uri.query(), Some("keep=1"));
    assert!(!ctx.uri_finalized);
}

#[tokio::test]
async fn stripping_query_credential_preserves_h2_authority_without_host() {
    use tokio::io::AsyncWriteExt;

    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: placeholder\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    session
        .req_header_mut()
        .set_uri("https://example.com/raw%2Fpath?api_key=secret&keep=1".parse().unwrap());
    session.req_header_mut().set_version(http::Version::HTTP_2);
    session.req_header_mut().remove_header("Host");
    let mut ctx = EdgionHttpContext::default();

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    crate::plugins::http::common::auth_common::strip_query_param(&mut adapter, "api_key");

    let header = session.req_header();
    assert_eq!(header.uri.scheme_str(), Some("https"));
    assert_eq!(header.uri.authority().unwrap().as_str(), "example.com");
    assert_eq!(header.raw_path(), b"/raw%2Fpath?keep=1");
    assert!(header.headers.get("Host").is_none());
    assert!(!ctx.uri_finalized);
}

#[tokio::test]
async fn path_only_rewrite_keeps_h2_authority_without_host() {
    use tokio::io::AsyncWriteExt;

    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: placeholder\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    session
        .req_header_mut()
        .set_uri("https://example.com/original?scope=read".parse().unwrap());
    session.req_header_mut().set_version(http::Version::HTTP_2);
    session.req_header_mut().remove_header("Host");
    let mut ctx = EdgionHttpContext::default();

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.set_upstream_uri("/rewritten?scope=read").unwrap();

    let header = session.req_header();
    assert_eq!(header.uri.to_string(), "https://example.com/rewritten?scope=read");
    assert!(header.headers.get("Host").is_none());
    assert!(ctx.uri_finalized);
}

#[test]
fn request_body_buffer_install_failure_is_storage_failure() {
    assert_eq!(body_buffer_install_error(), BodyReadError::StorageFailure);
}

#[test]
fn cookie_value_finds_named_cookie_among_others() {
    let mut headers = http::HeaderMap::new();
    headers.insert("Cookie", http::HeaderValue::from_static("a=1; b=2"));
    assert_eq!(cookie_value(&headers, "b").as_deref(), Some("2"));
    assert_eq!(cookie_value(&headers, "c"), None);
}

#[test]
fn cookie_value_finds_cookie_in_later_header_field() {
    let mut headers = http::HeaderMap::new();
    headers.append(http::header::COOKIE, http::HeaderValue::from_static("a=1"));
    headers.append(
        http::header::COOKIE,
        http::HeaderValue::from_static("session=later; b=2"),
    );

    assert_eq!(cookie_value(&headers, "session").as_deref(), Some("later"));
}

#[test]
fn cookie_value_uses_first_matching_cookie_across_fields() {
    let mut headers = http::HeaderMap::new();
    headers.append(
        http::header::COOKIE,
        http::HeaderValue::from_static("Session=upper; session=first"),
    );
    headers.append(http::header::COOKIE, http::HeaderValue::from_static("session=second"));

    assert_eq!(cookie_value(&headers, "session").as_deref(), Some("first"));
    assert_eq!(cookie_value(&headers, "SESSION"), None);
}

#[test]
fn cookie_value_handles_malformed_fields_conservatively_in_order() {
    let malformed = http::HeaderValue::from_bytes(&[0xff]).expect("obs-text is a valid raw header value");

    let mut malformed_first = http::HeaderMap::new();
    malformed_first.append(http::header::COOKIE, malformed.clone());
    malformed_first.append(http::header::COOKIE, http::HeaderValue::from_static("session=later"));
    assert_eq!(cookie_value(&malformed_first, "session"), None);

    let mut match_first = http::HeaderMap::new();
    match_first.append(http::header::COOKIE, http::HeaderValue::from_static("session=first"));
    match_first.append(http::header::COOKIE, malformed);
    assert_eq!(cookie_value(&match_first, "session").as_deref(), Some("first"));
}

#[test]
fn utf8_header_values_preserves_duplicates_and_rejects_invalid_values() {
    let mut headers = http::HeaderMap::new();
    headers.append("content-type", http::HeaderValue::from_static("application/json"));
    headers.append(
        "content-type",
        http::HeaderValue::from_static("application/problem+json"),
    );
    assert_eq!(
        utf8_header_values(&headers, "content-type"),
        Some(vec![
            "application/json".to_string(),
            "application/problem+json".to_string()
        ])
    );
    assert_eq!(utf8_header_values(&headers, "content-encoding"), Some(Vec::new()));

    headers.append(
        "content-type",
        http::HeaderValue::from_bytes(&[0xff]).expect("obs-text is a valid raw header value"),
    );
    assert_eq!(utf8_header_values(&headers, "content-type"), None);
}

#[test]
fn test_parse_path_params_single_param() {
    let pattern = "/api/:uid/profile";
    let actual = "/api/123/profile";
    let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

    assert_eq!(params.get("uid"), Some(&"123".to_string()));
    assert_eq!(params.len(), 1);
}

#[test]
fn test_parse_path_params_multiple_params() {
    let pattern = "/users/:uid/posts/:post_id";
    let actual = "/users/456/posts/789";
    let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

    assert_eq!(params.get("uid"), Some(&"456".to_string()));
    assert_eq!(params.get("post_id"), Some(&"789".to_string()));
    assert_eq!(params.len(), 2);
}

#[test]
fn test_parse_path_params_no_params() {
    let pattern = "/api/v1/users";
    let actual = "/api/v1/users";
    let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

    assert!(params.is_empty());
}

#[test]
fn test_parse_path_params_escaped_colon() {
    // :: is escaped colon, not a parameter
    let pattern = "/api/::version/data";
    let actual = "/api/:v1/data";
    let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

    assert!(params.is_empty());
}

#[test]
fn test_parse_path_params_param_at_end() {
    let pattern = "/users/:id";
    let actual = "/users/999";
    let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

    assert_eq!(params.get("id"), Some(&"999".to_string()));
}

#[test]
fn test_parse_path_params_prefix_match() {
    // For prefix match, actual path may have more segments
    let pattern = "/api/:version";
    let actual = "/api/v2/users/123";
    let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

    assert_eq!(params.get("version"), Some(&"v2".to_string()));
}

#[test]
fn test_parse_path_params_empty_param_name() {
    // Empty param name should be skipped
    let pattern = "/api/:/data";
    let actual = "/api/test/data";
    let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

    assert!(params.is_empty());
}

#[test]
fn test_parse_path_params_special_characters_in_value() {
    let pattern = "/search/:query";
    let actual = "/search/hello%20world";
    let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

    assert_eq!(params.get("query"), Some(&"hello%20world".to_string()));
}

#[test]
fn resolve_cap_never_treats_none_as_unbounded() {
    // Three production call sites pass `None`. It means "gateway default",
    // never "unbounded".
    assert_eq!(resolve_cap(None, 1024), 1024);
    // A plugin may tighten...
    assert_eq!(resolve_cap(Some(512), 1024), 512);
    // ...but never enlarge past the gateway-wide cap.
    assert_eq!(resolve_cap(Some(4096), 1024), 1024);
}

/// Build a request-stage ctx with the given snapshot + protocol facts.
fn admission_ctx(declared: Option<bool>, proto: Option<&str>, grpc: bool) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.body_declared = declared;
    ctx.request_info.discover_protocol = proto.map(str::to_string);
    ctx.request_info.is_grpc_request = grpc;
    ctx
}

#[test]
fn request_admission_no_declared_body_passes_through() {
    let ctx = admission_ctx(Some(false), None, false);
    assert_eq!(request_admission(&ctx, false), RequestAdmission::PassThrough);
}

#[test]
fn request_admission_missing_snapshot_refuses() {
    let ctx = admission_ctx(None, None, false);
    assert_eq!(
        request_admission(&ctx, false),
        RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedNoSnapshot)
    );
}

#[test]
fn missing_snapshot_and_wrong_stage_are_distinguishable() {
    // Both refuse, but they must not share one status: the missing-snapshot
    // arm is a code-path bug, the wrong-stage arm is expected traffic for
    // any body consumer in a post-request stage. A shared variant would make
    // the bug undetectable in the access log.
    let bug = request_admission(&admission_ctx(None, None, false), false);
    let expected = request_admission(&admission_ctx(Some(true), None, false), true);
    assert_eq!(bug, RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedNoSnapshot));
    assert_eq!(expected, RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedStage));
    assert_ne!(bug, expected);
}

#[test]
fn request_admission_response_stage_refuses() {
    // After the request body is proxied the H1 reader is Complete(n>0), so a
    // naive predicate would fall through to "buffer" and answer a zero-length
    // Ok. That is a silent violation of the Empty/Absent distinction.
    let ctx = admission_ctx(Some(true), None, false);
    assert_eq!(
        request_admission(&ctx, true),
        RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedStage)
    );
}

#[test]
fn request_admission_websocket_with_body_refuses() {
    let ctx = admission_ctx(Some(true), Some("websocket"), false);
    assert_eq!(
        request_admission(&ctx, false),
        RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedWebsocket)
    );
}

#[test]
fn request_admission_bodyless_websocket_handshake_passes_through() {
    // A genuine handshake declares no body, so step 1 answers before the
    // websocket check is ever consulted. Order is the semantics.
    let ctx = admission_ctx(Some(false), Some("websocket"), false);
    assert_eq!(request_admission(&ctx, false), RequestAdmission::PassThrough);
}

#[test]
fn request_admission_grpc_refuses_via_either_fact() {
    // Forged content-type (client-controlled) — safe to widen a REFUSAL with.
    let ctx = admission_ctx(Some(true), None, true);
    assert_eq!(
        request_admission(&ctx, false),
        RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedGrpc)
    );

    // Route-derived, with no gRPC content-type at all.
    let mut ctx = admission_ctx(Some(true), None, false);
    ctx.is_grpc_route_matched = true;
    assert_eq!(
        request_admission(&ctx, false),
        RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedGrpc)
    );
}

#[test]
fn request_admission_ordinary_post_buffers() {
    let ctx = admission_ctx(Some(true), None, false);
    assert_eq!(request_admission(&ctx, false), RequestAdmission::Buffer);
}

#[test]
fn response_admission_request_stage_refuses() {
    // No response header bound means this was called from the request stage.
    // Without this guard steps 2-4 all miss and step 5 would block on a
    // response that does not exist yet.
    let ctx = EdgionHttpContext::new();
    assert_eq!(
        response_admission(&ctx, None, false, false),
        RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedStage)
    );
}

#[test]
fn response_admission_real_101_refuses() {
    let ctx = EdgionHttpContext::new();
    assert_eq!(
        response_admission(&ctx, Some(101), false, false),
        RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedWebsocket)
    );
}

#[test]
fn response_admission_upgrade_request_answered_200_buffers() {
    // The request's protocol label is irrelevant to whether the RESPONSE body
    // is bufferable. Only a real 101 is a tunnel.
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.discover_protocol = Some("websocket".to_string());
    assert_eq!(
        response_admission(&ctx, Some(200), false, false),
        RequestAdmission::Buffer
    );
}

#[test]
fn response_admission_grpc_refuses() {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.is_grpc_request = true;
    assert_eq!(
        response_admission(&ctx, Some(200), false, false),
        RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedGrpc)
    );
}

#[test]
fn response_admission_bodyless_statuses_pass_through() {
    let ctx = EdgionHttpContext::new();
    for status in [100u16, 199, 204, 304] {
        assert_eq!(
            response_admission(&ctx, Some(status), false, true),
            RequestAdmission::PassThrough,
            "status {status} carries no body"
        );
    }
}

#[test]
fn response_admission_bodyless_status_without_agreement_buffers() {
    // The silent PassThrough needs `bodyless_agreed`, not just a bodyless
    // live status: `read_response_body` only sets it when the upstream's own
    // stamped status agrees, so a plugin rewriting 200 -> 204 ahead of a body
    // consumer cannot fabricate "this response has no body".
    let ctx = EdgionHttpContext::new();
    for status in [100u16, 199, 204, 304] {
        assert_eq!(
            response_admission(&ctx, Some(status), false, false),
            RequestAdmission::Buffer,
            "status {status} must not short-circuit without agreement"
        );
    }
}

#[test]
fn response_admission_head_response_passes_through() {
    // RFC 9110 9.3.2: a HEAD response carries Content-Length but no body.
    // Same class as 204/304 — the one place a request fact legitimately
    // enters the response predicate.
    let ctx = EdgionHttpContext::new();
    assert_eq!(
        response_admission(&ctx, Some(200), true, false),
        RequestAdmission::PassThrough
    );
}

#[test]
fn response_admission_ordinary_200_buffers() {
    let ctx = EdgionHttpContext::new();
    assert_eq!(
        response_admission(&ctx, Some(200), false, false),
        RequestAdmission::Buffer
    );
}
