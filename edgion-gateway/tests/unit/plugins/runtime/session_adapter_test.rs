use super::*;

struct ConsumerSnapshot {
    len: u64,
    chunks: std::collections::VecDeque<Result<Bytes, BodyReadError>>,
    reads: Arc<std::sync::atomic::AtomicUsize>,
}
#[async_trait]
impl RequestBodySnapshotReader for ConsumerSnapshot {
    fn len(&self) -> u64 {
        self.len
    }
    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.chunks.pop_front().transpose()
    }
}
struct CollectChunks(Vec<u8>);
impl RequestBodyChunkConsumer for CollectChunks {
    fn consume(&mut self, chunk: &[u8]) {
        self.0.extend_from_slice(chunk);
    }
}

#[tokio::test]
async fn captured_snapshot_consumer_requires_real_eof_exact_parity_and_admission() {
    for (len, cap, chunks, expected, delivered, expected_reads) in [
        (
            3,
            3,
            vec![Ok(Bytes::from_static(b"a")), Ok(Bytes::from_static(b"bc"))],
            Ok(3),
            b"abc".as_slice(),
            3,
        ),
        (
            3,
            2,
            vec![Ok(Bytes::from_static(b"abc"))],
            Err(BodyReadError::BufferLimitExceeded { limit: 2 }),
            b"".as_slice(),
            0,
        ),
        (
            3,
            3,
            vec![Ok(Bytes::from_static(b"ab"))],
            Err(BodyReadError::Incomplete),
            b"ab".as_slice(),
            2,
        ),
        (
            2,
            4,
            vec![Ok(Bytes::from_static(b"abc"))],
            Err(BodyReadError::Incomplete),
            b"".as_slice(),
            1,
        ),
        (
            3,
            3,
            vec![Ok(Bytes::from_static(b"abc")), Err(BodyReadError::Timeout)],
            Err(BodyReadError::Timeout),
            b"abc".as_slice(),
            2,
        ),
    ] {
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reader = ConsumerSnapshot {
            len,
            chunks: chunks.into(),
            reads: Arc::clone(&reads),
        };
        let mut consumer = CollectChunks(Vec::new());
        assert_eq!(
            consume_request_body_snapshot(Box::new(reader), cap, &mut consumer).await,
            expected
        );
        assert_eq!(consumer.0, delivered);
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), expected_reads);
    }
}

#[tokio::test]
async fn contiguous_consumer_preserves_admission_parity_and_real_eof() {
    struct FastSnapshot {
        inner: ConsumerSnapshot,
        fast: Option<Result<Option<Bytes>, BodyReadError>>,
        attempts: Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait]
    impl RequestBodySnapshotReader for FastSnapshot {
        fn len(&self) -> u64 {
            self.inner.len
        }
        fn try_take_contiguous_memory(&mut self, _: usize) -> Result<Option<Bytes>, BodyReadError> {
            self.attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.fast.take().unwrap()
        }
        async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
            self.inner.next_chunk().await
        }
    }
    struct Consumer {
        limit: Option<usize>,
        bytes: Vec<u8>,
    }
    impl RequestBodyChunkConsumer for Consumer {
        fn cached_contiguous_limit(&self) -> Option<usize> {
            self.limit
        }
        fn consume(&mut self, bytes: &[u8]) {
            self.bytes.extend_from_slice(bytes);
        }
    }
    let abc = Bytes::from_static(b"abc");
    for (cap, limit, fast, chunks, expected, delivered, attempts, reads) in [
        (
            3,
            Some(3),
            Ok(Some(abc.clone())),
            vec![],
            Ok(3),
            b"abc".as_slice(),
            1,
            1,
        ),
        (
            2,
            Some(3),
            Ok(Some(abc.clone())),
            vec![],
            Err(BodyReadError::BufferLimitExceeded { limit: 2 }),
            b"".as_slice(),
            0,
            0,
        ),
        (
            3,
            None,
            Ok(Some(abc.clone())),
            vec![Ok(abc.clone())],
            Ok(3),
            b"abc".as_slice(),
            0,
            2,
        ),
        (
            3,
            Some(2),
            Ok(Some(abc.clone())),
            vec![Ok(abc.clone())],
            Ok(3),
            b"abc".as_slice(),
            0,
            2,
        ),
        (
            3,
            Some(3),
            Ok(None),
            vec![Ok(abc.clone())],
            Ok(3),
            b"abc".as_slice(),
            1,
            2,
        ),
        (
            3,
            Some(3),
            Ok(Some(Bytes::from_static(b"ab"))),
            vec![],
            Err(BodyReadError::Incomplete),
            b"".as_slice(),
            1,
            0,
        ),
        (
            4,
            Some(3),
            Ok(Some(Bytes::from_static(b"abcd"))),
            vec![],
            Err(BodyReadError::Incomplete),
            b"".as_slice(),
            1,
            0,
        ),
        (
            3,
            Some(3),
            Err(BodyReadError::Incomplete),
            vec![],
            Err(BodyReadError::Incomplete),
            b"".as_slice(),
            1,
            0,
        ),
        (
            4,
            Some(3),
            Ok(Some(abc.clone())),
            vec![Ok(Bytes::from_static(b"d"))],
            Err(BodyReadError::Incomplete),
            b"abc".as_slice(),
            1,
            1,
        ),
        (
            3,
            Some(3),
            Ok(Some(abc.clone())),
            vec![Err(BodyReadError::Timeout)],
            Err(BodyReadError::Timeout),
            b"abc".as_slice(),
            1,
            1,
        ),
    ] {
        let read_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempt_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reader = FastSnapshot {
            inner: ConsumerSnapshot {
                len: 3,
                chunks: chunks.into(),
                reads: Arc::clone(&read_count),
            },
            fast: Some(fast),
            attempts: Arc::clone(&attempt_count),
        };
        let mut consumer = Consumer {
            limit,
            bytes: Vec::new(),
        };
        assert_eq!(
            consume_request_body_snapshot(Box::new(reader), cap, &mut consumer).await,
            expected
        );
        assert_eq!(consumer.bytes, delivered);
        assert_eq!(attempt_count.load(std::sync::atomic::Ordering::Relaxed), attempts);
        assert_eq!(read_count.load(std::sync::atomic::Ordering::Relaxed), reads);
    }
}

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

    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    crate::plugins::http::common::auth_common::strip_query_param(&mut adapter, "api_key");
    assert_eq!(adapter.request_query(), Some("keep=1"));
    assert!(adapter.ctx().request_query.projection_is_dirty());
    drop(adapter);
    assert_eq!(session.req_header().uri.query(), Some("api_key=secret&keep=1"));
    ctx.ensure_target_synced(session.req_header_mut()).unwrap();

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

    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    crate::plugins::http::common::auth_common::strip_query_param(&mut adapter, "api_key");
    assert_eq!(adapter.request_query(), Some("keep=1"));
    assert!(adapter.ctx().request_query.projection_is_dirty());
    drop(adapter);
    assert_eq!(session.req_header().uri.query(), Some("api_key=secret&keep=1"));
    ctx.ensure_target_synced(session.req_header_mut()).unwrap();

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

    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.set_upstream_uri("/rewritten?scope=read").unwrap();

    let header = session.req_header();
    assert_eq!(header.uri.to_string(), "https://example.com/rewritten?scope=read");
    assert!(header.headers.get("Host").is_none());
    assert!(ctx.uri_finalized);
}

#[tokio::test]
async fn path_only_rewrite_keeps_h1_absolute_request_target() {
    use tokio::io::AsyncWriteExt;

    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"GET http://example.com/original?scope=read HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();

    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.set_upstream_uri("/rewritten?scope=read").unwrap();

    assert_eq!(session.req_header().uri.to_string(), "/rewritten?scope=read");
    assert_eq!(
        session.req_header().raw_path(),
        b"http://example.com/rewritten?scope=read"
    );
    assert!(ctx.uri_finalized);
}

#[tokio::test]
async fn asterisk_rewrite_rejects_h1_absolute_request_target_without_mutation() {
    use tokio::io::AsyncWriteExt;

    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"OPTIONS http://example.com/original HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();

    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(adapter.set_upstream_uri("*").is_err());

    assert_eq!(session.req_header().uri.to_string(), "/original");
    assert_eq!(session.req_header().raw_path(), b"http://example.com/original");
    assert!(!ctx.uri_finalized);
}

#[tokio::test]
async fn upstream_host_rewrite_updates_h2_authority_and_host() {
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
        .set_uri("https://downstream.example.com/original?scope=read".parse().unwrap());
    session.req_header_mut().set_version(http::Version::HTTP_2);
    session.req_header_mut().remove_header("Host");
    let mut ctx = EdgionHttpContext::default();

    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.set_upstream_uri("/rewritten?scope=read").unwrap();
    adapter.set_upstream_host("127.0.0.1:30001").unwrap();

    let header = session.req_header();
    assert_eq!(header.uri.to_string(), "https://127.0.0.1:30001/rewritten?scope=read");
    assert_eq!(header.headers.get("Host").unwrap(), "127.0.0.1:30001");
}

#[tokio::test]
async fn invalid_upstream_host_leaves_uri_and_header_unchanged() {
    use tokio::io::AsyncWriteExt;

    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: original.example.com\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    session
        .req_header_mut()
        .set_uri("https://original.example.com/original".parse().unwrap());
    let mut ctx = EdgionHttpContext::default();

    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(adapter.set_upstream_host("bad host").is_err());

    let header = session.req_header();
    assert_eq!(header.uri.to_string(), "https://original.example.com/original");
    assert_eq!(header.headers.get("Host").unwrap(), "original.example.com");
}

#[tokio::test]
async fn plugin_created_userinfo_host_fails_pingora_egress_validation() {
    use tokio::io::AsyncWriteExt;

    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: original.example.com\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();

    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.set_upstream_host("user@evil.example.com").unwrap();

    assert!(pingora_core::protocols::http::authority::validate_request_authority(session.req_header()).is_err());
}

#[tokio::test]
async fn upstream_host_rewrite_preserves_h1_absolute_target() {
    use tokio::io::AsyncWriteExt;

    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(
            b"GET http://original.example.com/raw%2Fpath?scope=read HTTP/1.1\r\nHost: original.example.com\r\n\r\n",
        )
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();

    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.set_upstream_host("backend.example.com:8443").unwrap();

    assert_eq!(
        session.req_header().raw_path(),
        b"http://original.example.com/raw%2Fpath?scope=read"
    );
    assert_eq!(
        session.req_header().headers.get("Host").unwrap(),
        "backend.example.com:8443"
    );
    assert!(!ctx.uri_finalized);
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
fn admission_ctx(declared: Option<bool>, proto: Option<&'static str>, grpc: bool) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.body_declared = declared;
    ctx.request_info.discover_protocol = proto;
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
    ctx.request_info.discover_protocol = Some("websocket");
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

#[tokio::test]
async fn disabling_guard_preserves_another_owner_and_pending_unary_possibility() {
    use tokio::io::AsyncWriteExt;
    let (mut client, server) = tokio::io::duplex(1024);
    client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::new();
    ctx.ensure_ai_state().unary_possible = true;
    use crate::ai::conversion::chat_to_anthropic::{budget::UnaryBudget, head::UnaryHead, limits::UnaryLimits};
    let pool = UnaryBudget::new(1);
    let unary = std::sync::Arc::new(UnaryHead::new(
        UnaryLimits::resolve(&crate::ai::config::UnaryConfig::default()).unwrap(),
        pool.acquire().unwrap(),
    ));
    ctx.ensure_ai_state().unary_owner = Some(unary.clone());
    let limits = pingora_proxy::ResponseHeadHoldLimits::new(1, 1, 1, 1, 1, 1, std::time::Duration::from_secs(1));
    for owner in [
        crate::ctx::ResponseHeadHoldOwner::AnthropicUnary,
        crate::ctx::ResponseHeadHoldOwner::Guardrail,
    ] {
        ctx.ensure_ai_state().response_head_hold_claim = Some(crate::ctx::ResponseHeadHoldClaim {
            owner,
            limits,
            active: false,
        });
        PingoraSessionAdapter::new(&mut session, &mut ctx)
            .ai()
            .disable_semantic_guard();
        assert!(ctx.ai_state().unwrap().unary_possible);
        assert!(std::sync::Arc::ptr_eq(
            ctx.ai_state().unwrap().unary_owner.as_ref().unwrap(),
            &unary
        ));
        assert!(pool.acquire().is_none());
        assert_eq!(
            ctx.ai_state().unwrap().response_head_hold_claim.is_some(),
            owner == crate::ctx::ResponseHeadHoldOwner::AnthropicUnary
        );
    }
}

#[tokio::test]
async fn inspection_window_skips_only_proven_tail_and_keeps_complete_eof_validation() {
    struct WindowConsumer {
        bytes: Vec<u8>,
        window: u64,
    }
    impl RequestBodyChunkConsumer for WindowConsumer {
        fn source_window(&self) -> Option<u64> {
            Some(self.window)
        }
        fn consume(&mut self, bytes: &[u8]) {
            self.bytes.extend_from_slice(bytes);
        }
    }
    for (len, cap, window, expected, expected_reads) in [
        (6, 6, 2, Ok(6), 1),
        (3, 6, 3, Err(BodyReadError::Timeout), 2),
        (6, 5, 2, Err(BodyReadError::BufferLimitExceeded { limit: 5 }), 0),
    ] {
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reader = ConsumerSnapshot {
            len,
            chunks: vec![Ok(Bytes::from_static(b"abc")), Err(BodyReadError::Timeout)].into(),
            reads: Arc::clone(&reads),
        };
        let mut consumer = WindowConsumer {
            bytes: Vec::new(),
            window,
        };
        assert_eq!(
            consume_request_body_snapshot(Box::new(reader), cap, &mut consumer).await,
            expected
        );
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), expected_reads);
        assert!(consumer.bytes.len() as u64 <= window);
    }
}

async fn ai_capability_session() -> Session {
    use tokio::io::AsyncWriteExt;
    let (mut client, server) = tokio::io::duplex(1024);
    client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    session
}

fn ai_capability_batch(raw: Bytes) -> crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch {
    let mut batch = crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch::new();
    batch.push(crate::sse::window::WindowRecord::raw(vec![raw])).unwrap();
    batch
}

#[tokio::test]
async fn ai_capability_absent_reads_and_accessor_do_not_allocate() {
    let mut session = ai_capability_session().await;
    let mut ctx = EdgionHttpContext::new();
    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    let ((), allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(!adapter.ai().semantic_guard_active());
        assert!(adapter.ai().ai_terminal_render_context().is_none());
        assert!(adapter.ai().attached_guardrail("missing").is_none());
        assert!(adapter.ai().take_canonical_records().is_none());
        adapter.ai().disable_semantic_guard();
    });
    assert!(ctx.ai_state().is_none());
    assert_eq!(allocations.calls, 0);
}

#[tokio::test]
async fn ai_capability_first_writers_and_guard_generation_remain_request_owned() {
    use crate::ai::{AiPlanError, AiProviderLabel, AiUsageShared};
    use edgion_resources::resources::edgion_plugins::AiApi;
    let mut session = ai_capability_session().await;
    let mut ctx = EdgionHttpContext::new();
    let context = |model: &str| AiRequestContext {
        provider: AiProviderLabel::OpenAi,
        model: model.to_owned(),
        pricing: None,
        requested_streaming: false,
        usage: Arc::new(AiUsageShared::default()),
        quota: None,
        quota_status: None,
    };
    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(adapter.ai().capture_ai_client_api(AiApi::OpenaiChat));
    assert!(!adapter.ai().capture_ai_client_api(AiApi::AnthropicMessages));
    assert!(adapter.ai().set_ai_request_context(context("first")));
    assert!(!adapter.ai().set_ai_request_context(context("second")));
    assert!(adapter
        .ai()
        .set_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend)));
    assert!(!adapter
        .ai()
        .set_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidRouteConfiguration)));
    adapter.ai().record_ai_guard_decision("guard".to_owned(), Some(7));
    adapter.ai().record_ai_guard_decision("guard".to_owned(), None);
    adapter.ai().record_ai_guard_decision("other".to_owned(), Some(8));
    let state = ctx.ai_state().unwrap();
    assert_eq!(state.client_api, Some(AiApi::OpenaiChat));
    assert_eq!(state.request.as_ref().unwrap().model, "first");
    assert!(matches!(
        state.attempt_plan,
        Some(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend))
    ));
    assert_eq!(state.guard_decided, ["guard", "other"]);
    assert_eq!(state.guard_body_generation, Some(8));
}

#[tokio::test]
async fn ai_capability_returns_original_rejected_batch_and_preserves_resident() {
    let mut session = ai_capability_session().await;
    let mut ctx = EdgionHttpContext::new();
    let offered = Bytes::from(vec![b'r'; 3]);
    let pointer = offered.as_ptr();
    let returned = PingoraSessionAdapter::new(&mut session, &mut ctx)
        .ai()
        .publish_canonical_records(ai_capability_batch(offered))
        .unwrap_err();
    assert!(ctx.ai_state().is_some());
    let raw = returned.into_records().pop().unwrap().into_raw_slices();
    assert_eq!(raw[0].as_ptr(), pointer);
    assert_eq!(raw[0].as_ref(), b"rrr");
    ctx.ensure_ai_state().semantic_guard_claimed = true;
    let accepted = Bytes::from(vec![b'a'; 3]);
    let accepted_pointer = accepted.as_ptr();
    let rejected = Bytes::from(vec![b'b'; 4]);
    let rejected_pointer = rejected.as_ptr();
    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(adapter
        .ai()
        .publish_canonical_records(ai_capability_batch(accepted))
        .is_ok());
    let returned = adapter
        .ai()
        .publish_canonical_records(ai_capability_batch(rejected))
        .unwrap_err();
    let raw = returned.into_records().pop().unwrap().into_raw_slices();
    assert_eq!(raw[0].as_ptr(), rejected_pointer);
    assert_eq!(raw[0].as_ref(), b"bbbb");
    let raw = adapter
        .ai()
        .take_canonical_records()
        .unwrap()
        .into_records()
        .pop()
        .unwrap()
        .into_raw_slices();
    assert_eq!(raw[0].as_ptr(), accepted_pointer);
    assert_eq!(raw[0].as_ref(), b"aaa");
    assert!(adapter.ai().take_canonical_records().is_none());
}

struct AiCapabilityProcessor(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for AiCapabilityProcessor {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
#[async_trait]
impl crate::plugins::runtime::traits::StreamingResponseProcessor for AiCapabilityProcessor {
    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _eos: bool,
        _kind: crate::ctx::ResponseStreamKind,
        _sink: &mut pingora_proxy::ResponseBodySink,
        _session: &mut dyn crate::plugins::runtime::traits::ResponseStreamSession,
    ) {
    }
}

#[tokio::test]
async fn ai_capability_guard_claim_processor_rejection_and_disable_keep_owners() {
    use crate::ai::guards::semantic::AttachedGuardrail;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut session = ai_capability_session().await;
    let mut ctx = EdgionHttpContext::new();
    let owner = Arc::new(AttachedGuardrail::new());
    let limits = pingora_proxy::ResponseHeadHoldLimits::new(1, 1, 1, 1, 1, 1, std::time::Duration::from_secs(1));
    let accepted_drops = Arc::new(AtomicUsize::new(0));
    let rejected_drops = Arc::new(AtomicUsize::new(0));
    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(adapter
        .ai()
        .install_attached_guardrail("first".to_owned(), Arc::clone(&owner), Some(limits)));
    assert!(!adapter
        .ai()
        .install_attached_guardrail("second".to_owned(), Arc::new(AttachedGuardrail::new()), None));
    assert!(Arc::ptr_eq(&adapter.ai().attached_guardrail("first").unwrap(), &owner));
    assert!(adapter.ai().attached_guardrail("second").is_none());
    assert!(adapter
        .ai()
        .install_semantic_response_processor(Box::new(AiCapabilityProcessor(Arc::clone(&accepted_drops)))));
    assert!(!adapter
        .ai()
        .install_semantic_response_processor(Box::new(AiCapabilityProcessor(Arc::clone(&rejected_drops)))));
    assert_eq!(accepted_drops.load(Ordering::Relaxed), 0);
    assert_eq!(rejected_drops.load(Ordering::Relaxed), 1);
    assert!(adapter.ctx.ai_state().unwrap().response_head_hold_claim.unwrap().active);
    adapter.ctx.response_body_processing_started = true;
    adapter.ai().disable_semantic_guard();
    assert!(adapter.ai().semantic_guard_active());
    assert!(!adapter
        .ai()
        .install_semantic_response_processor(Box::new(AiCapabilityProcessor(Arc::clone(&rejected_drops)))));
    assert_eq!(rejected_drops.load(Ordering::Relaxed), 2);
    adapter.ctx.response_body_processing_started = false;
    adapter.ai().disable_semantic_guard();
    assert!(!adapter.ai().semantic_guard_active());
    assert!(adapter.ctx.ai_state().unwrap().response_head_hold_claim.is_none());
    assert_eq!(accepted_drops.load(Ordering::Relaxed), 1);
    assert!(Arc::ptr_eq(&adapter.ai().attached_guardrail("first").unwrap(), &owner));
}

#[tokio::test]
async fn ai_capability_frozen_empty_processor_slot_rejects_without_state_allocation() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut session = ai_capability_session().await;
    let mut ctx = EdgionHttpContext::new();
    ctx.response_body_processing_started = true;
    let drops = Arc::new(AtomicUsize::new(0));
    assert!(!PingoraSessionAdapter::new(&mut session, &mut ctx)
        .ai()
        .install_semantic_response_processor(Box::new(AiCapabilityProcessor(Arc::clone(&drops)))));
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    assert!(ctx.ai_state().is_none());
}

#[tokio::test]
async fn key_set_context_writes_share_store_and_preserve_request_isolation() {
    use tokio::io::AsyncWriteExt;

    let (mut client, server) = tokio::io::duplex(1024);
    client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    let key = KeySet::Ctx {
        name: "selection".into(),
    };
    {
        if !ctx.request_query.is_captured() {
            ctx.request_query
                .capture(session.req_header().uri.query().map(str::to_owned))
                .unwrap();
        }
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.key_set(&key, Some("first".into())).unwrap();
        assert_eq!(adapter.get_ctx_var_ref("selection"), Some("first"));
        assert_eq!(adapter.get_ctx_var("selection").as_deref(), Some("first"));
        adapter.set_ctx_var("selection", "replacement").unwrap();
        assert_eq!(adapter.get_ctx_var_ref("selection"), Some("replacement"));
        adapter.set_ctx_var("other", "retained").unwrap();
        adapter.key_set(&key, Some(String::new())).unwrap();
        assert_eq!(adapter.get_ctx_var_ref("selection"), Some(""));
        adapter.key_set(&key, None).unwrap();
        assert_eq!(adapter.get_ctx_var_ref("selection"), None);
        adapter.key_set(&key, None).unwrap();
        assert_eq!(adapter.get_ctx_var_ref("other"), Some("retained"));
        adapter.key_set(&key, Some("final".into())).unwrap();
    }
    assert_eq!(ctx.get_ctx_var("selection"), Some("final"));
    let mut next_ctx = EdgionHttpContext::default();
    if !next_ctx.request_query.is_captured() {
        next_ctx
            .request_query
            .capture(session.req_header().uri.query().map(str::to_owned))
            .unwrap();
    }
    let next = PingoraSessionAdapter::new(&mut session, &mut next_ctx);
    assert_eq!(next.get_ctx_var_ref("selection"), None);
    assert_eq!(next.get_ctx_var_ref("other"), None);
}

#[tokio::test]
async fn deferred_query_edits_snapshots_authority_and_finalized_target_are_coherent() {
    let mut session = super::record_outcome_tests::test_session_raw(
        "GET /raw?token=one&secret=two&keep=a%26b HTTP/1.1\r\nHost: entry.test\r\n\r\n",
    )
    .await;
    let mut ctx = EdgionHttpContext::new();
    ctx.request_query
        .capture(session.req_header().uri.query().map(str::to_owned))
        .unwrap();
    ctx.request_info.normalized_current_path = "/raw".into();
    ctx.uri_finalized = true;
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert!(adapter.remove_request_query_param("token").unwrap());
        adapter.set_upstream_host("next.test").unwrap();
        assert_eq!(adapter.request_query(), Some("secret=two&keep=a%26b"));
        assert_eq!(adapter.get_uri(), "/raw?secret=two&keep=a%26b");
    }
    assert!(ctx.request_query.projection_is_dirty());
    let snapshot = ctx.snapshot_current_request(session.req_header()).unwrap();
    assert_eq!(snapshot.uri.query(), Some("secret=two&keep=a%26b"));
    assert!(session.req_header().uri.query().unwrap().contains("token="));
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.remove_request_query_param("secret").unwrap();
        assert_eq!(adapter.request_query(), Some("keep=a%26b"));
    }
    assert_eq!(snapshot.uri.query(), Some("secret=two&keep=a%26b"));
    ctx.ensure_target_synced(session.req_header_mut()).unwrap();
    assert_eq!(session.req_header().uri.query(), Some("keep=a%26b"));
    assert!(!ctx.request_query.projection_is_dirty());
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.set_upstream_uri("/replacement").unwrap();
    assert_eq!(adapter.request_query(), None);
    assert_eq!(adapter.get_query_param_ref("token"), Some("one"));
    assert!(adapter.set_upstream_uri("bad uri with spaces").is_err());
    assert_eq!(adapter.get_uri(), "/replacement");
}

#[tokio::test]
async fn query_setter_imports_the_validated_transport_query_in_every_target_form() {
    for form in ["origin", "absolute", "h2"] {
        for raw in ["a=1#fragment&token=x", "a=1%23fragment&token=x", "", "#fragment"] {
            let wire = if form == "absolute" {
                "GET http://example.test/path?entry=1 HTTP/1.1\r\nHost: example.test\r\n\r\n"
            } else {
                "GET /path?entry=1 HTTP/1.1\r\nHost: example.test\r\n\r\n"
            };
            let mut session = super::record_outcome_tests::test_session_raw(wire).await;
            if form == "h2" {
                session
                    .req_header_mut()
                    .set_uri("https://example.test/path?entry=1".parse().unwrap());
                session.req_header_mut().set_version(http::Version::HTTP_2);
                session.req_header_mut().remove_header("Host");
            }
            let mut ctx = EdgionHttpContext::new();
            ctx.request_query.capture(Some("entry=1".into())).unwrap();
            let mut expected = session.req_header().clone();
            let expected_result = crate::ctx::overlay_request_query(&mut expected, (!raw.is_empty()).then_some(raw));
            let result = PingoraSessionAdapter::new(&mut session, &mut ctx).set_request_query(raw);
            assert_eq!(result.is_ok(), expected_result.is_ok());
            if result.is_ok() {
                assert_eq!(ctx.query(), expected.uri.query());
                ctx.ensure_target_synced(session.req_header_mut()).unwrap();
                assert_eq!(ctx.query(), session.req_header().uri.query());
                assert_eq!(ctx.entry_query_param("entry"), Some("1"));
            } else {
                assert_eq!(ctx.query(), Some("entry=1"));
            }
        }
    }
}
