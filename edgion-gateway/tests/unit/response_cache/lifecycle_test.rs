use super::*;
use crate::response_cache::tests::{fixture, response};
use pingora_core::protocols::{GetProxyDigest, GetSocketDigest, GetTimingDigest, Peek, Shutdown, Ssl, UniqueID};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::task::{Context, Poll};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug)]
struct FlushStream {
    inner: tokio::io::DuplexStream,
    mode: Arc<AtomicUsize>,
    flushes: Arc<AtomicUsize>,
}

impl tokio::io::AsyncRead for FlushStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
impl tokio::io::AsyncWrite for FlushStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.flushes.fetch_add(1, Ordering::Relaxed);
        match self.mode.load(Ordering::Acquire) {
            1 => Poll::Pending,
            2 => Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
            _ => Poll::Ready(Ok(())),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
#[async_trait::async_trait]
impl Shutdown for FlushStream {
    async fn shutdown(&mut self) {}
}
impl UniqueID for FlushStream {
    fn id(&self) -> pingora_core::protocols::UniqueIDType {
        0
    }
}
impl Ssl for FlushStream {}
impl Peek for FlushStream {}
impl GetTimingDigest for FlushStream {
    fn get_timing_digest(&self) -> Vec<Option<pingora_core::protocols::TimingDigest>> {
        vec![]
    }
}
impl GetProxyDigest for FlushStream {
    fn get_proxy_digest(&self) -> Option<Arc<pingora_core::protocols::raw_connect::ProxyDigest>> {
        None
    }
}
impl GetSocketDigest for FlushStream {
    fn get_socket_digest(&self) -> Option<Arc<pingora_core::protocols::SocketDigest>> {
        None
    }
}

#[tokio::test]
async fn final_flush_error_timeout_and_cancellation_release_fill_without_publication() {
    use std::future::Future;
    for case in 0..3 {
        let (store, _epoch, state) = fixture(1024);
        let (mut client, inner) = tokio::io::duplex(64 * 1024);
        client
            .write_all(b"GET /cache HTTP/1.1\r\nHost: example.test\r\n\r\n")
            .await
            .unwrap();
        let mode = Arc::new(AtomicUsize::new(0));
        let flushes = Arc::new(AtomicUsize::new(0));
        let mut session = Session::new_h1(Box::new(FlushStream {
            inner,
            mode: mode.clone(),
            flushes: flushes.clone(),
        }));
        session.read_request().await.unwrap();
        let mut ctx = EdgionHttpContext::default();
        let mut config = (*ctx.egc).clone();
        config.parsed_timeouts.client.write_timeout = Duration::from_millis(10);
        ctx.egc = Arc::new(config);
        ctx.response_cache = Some(state);
        let mut header = response();
        header.insert_header("content-length", "5").unwrap();
        capture_headers(&mut ctx, &header);
        seal_headers(&mut ctx, &header);
        capture_body(&mut ctx, Some(&Bytes::from_static(b"hello")), true);
        session.write_response_header(Box::new(header), false).await.unwrap();
        session
            .write_response_body(Some(Bytes::from_static(b"hello")), true)
            .await
            .unwrap();
        let before = flushes.load(Ordering::Relaxed);
        mode.store(if case == 0 { 2 } else { 1 }, Ordering::Release);
        if case == 2 {
            let mut terminal = Box::pin(finalize(&mut session, &mut ctx, None));
            let mut poll_context = Context::from_waker(futures::task::noop_waker_ref());
            assert!(terminal.as_mut().poll(&mut poll_context).is_pending());
            drop(terminal);
        } else {
            tokio::time::timeout(Duration::from_secs(1), finalize(&mut session, &mut ctx, None))
                .await
                .unwrap();
        }
        assert!(
            flushes.load(Ordering::Relaxed) > before,
            "terminal transport flush was actually attempted"
        );
        assert_eq!(store.stats().1, 0);
        assert_eq!(store.stats().3, 0);
        // A surviving context cannot later publish the cancelled fill.
        mode.store(0, Ordering::Release);
        finalize(&mut session, &mut ctx, None).await;
        assert_eq!(store.stats().3, 0);
    }
}

#[tokio::test]
async fn postheader_hit_flush_failure_keeps_header_charge_and_never_starts_origin() {
    let (store, epoch, mut state) = fixture(1024);
    let header = response();
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"hello")), true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    let (mut client, inner) = tokio::io::duplex(64 * 1024);
    client
        .write_all(b"GET /cache HTTP/1.1\r\nHost: example.test\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(FlushStream {
        inner,
        mode: Arc::new(AtomicUsize::new(2)),
        flushes: Arc::new(AtomicUsize::new(0)),
    }));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    ctx.response_cache = Some(state);
    let baseline = store.stats().0;
    let hit = prepare_hit(&mut session, &mut ctx).unwrap();
    let error = deliver_hit(&mut session, &mut ctx, hit).await.unwrap_err();
    assert_eq!(error.esource(), &pingora_core::ErrorSource::Downstream);
    assert!(
        session.response_written().is_some(),
        "failure happens after the selected header is committed"
    );
    assert!(ctx.get_current_upstream().is_none());
    assert_eq!(ctx.try_cnt, 0);
    assert!(
        store.stats().0 >= baseline + 32 * 1024,
        "response_written metadata stays charged across failure"
    );
    assert_eq!(store.stats().2, 0);
    finalize(&mut session, &mut ctx, Some(&error)).await;
    assert_eq!(
        store.stats().3,
        1,
        "hit delivery failure cannot create a replacement fill"
    );
    drop(session);
    assert!(
        store.stats().0 >= baseline + 32 * 1024,
        "context keeps metadata charged through Session teardown"
    );
    drop(ctx);
    store.retire(&epoch);
    drop(epoch);
    let (total, pending, readers, entries, infrastructure) = store.stats();
    assert_eq!((total, pending, readers, entries), (infrastructure, 0, 0, 0));
}

async fn h1(request: &[u8]) -> (Session, tokio::io::DuplexStream) {
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    client.write_all(request).await.unwrap();
    let mut session = Session::new_h1(Box::new(server));
    assert!(session.read_request().await.unwrap());
    (session, client)
}

#[tokio::test]
async fn h1_terminal_flush_publishes_once_and_replays_without_origin() {
    let (store, _epoch, state) = fixture(1024);
    let (mut session, mut client) = h1(b"GET /cache HTTP/1.1\r\nHost: example.test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.response_cache = Some(state);
    assert!(prepare_hit(&mut session, &mut ctx).is_none());
    let mut header = response();
    header.insert_header("content-length", "5").unwrap();
    capture_headers(&mut ctx, &header);
    seal_headers(&mut ctx, &header);
    capture_body(&mut ctx, Some(&Bytes::from_static(b"hello")), true);
    assert_eq!(store.stats().3, 0);
    session.write_response_header(Box::new(header), false).await.unwrap();
    session
        .write_response_body(Some(Bytes::from_static(b"hello")), true)
        .await
        .unwrap();
    finalize(&mut session, &mut ctx, None).await;
    assert_eq!(store.stats().3, 1);
    finalize(&mut session, &mut ctx, None).await;
    assert_eq!(store.stats().3, 1);
    drop(session);
    let mut wire = String::new();
    client.read_to_string(&mut wire).await.unwrap();
    assert!(wire.ends_with("\r\n\r\nhello"));
    let (mut replay, mut reader) = h1(b"GET /cache HTTP/1.1\r\nHost: example.test\r\n\r\n").await;
    let before_headers = store.stats().0;
    let hit = prepare_hit(&mut replay, &mut ctx).unwrap();
    deliver_hit(&mut replay, &mut ctx, hit).await.unwrap();
    assert!(
        store.stats().0 >= before_headers + 32 * 1024,
        "decoded header charge survives replay return"
    );
    assert!(replay.response_written().is_some());
    assert!(ctx.get_current_upstream().is_none());
    replay.as_downstream_mut().finish_body().await.unwrap();
    drop(replay);
    let mut wire = String::new();
    reader.read_to_string(&mut wire).await.unwrap();
    assert!(wire.ends_with("\r\n\r\nhello"));
    assert!(wire.to_ascii_lowercase().contains("age: "));
}

#[tokio::test]
async fn unread_post_body_bypasses_hit_until_physical_eof() {
    let (_, _, mut state) = fixture(1024);
    let header = response();
    state.capture(&header, Instant::now());
    state.push(None, true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    let (mut session, _client) =
        h1(b"POST /graphql HTTP/1.1\r\nHost: example.test\r\nContent-Length: 2\r\n\r\n{}").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.response_cache = Some(state);
    assert!(prepare_hit(&mut session, &mut ctx).is_none());
    while session.read_request_body().await.unwrap().is_some() {}
    assert!(prepare_hit(&mut session, &mut ctx).is_some());
}

#[tokio::test]
async fn captured_rewritten_post_hit_allows_real_h1_connection_reuse() {
    use crate::plugins::runtime::{traits::PluginSession, PingoraSessionAdapter};
    use pingora_core::apps::{HttpPersistentSettings, ReusedHttpStream};
    let (_, _, mut state) = fixture(1024);
    let header = response();
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"hello")), true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    let (mut session, mut client) =
        h1(b"POST /graphql HTTP/1.1\r\nHost: example.test\r\nContent-Length: 2\r\n\r\n{}").await;
    session.as_downstream_mut().set_keepalive(Some(5));
    let mut ctx = EdgionHttpContext::default();
    ctx.response_cache = Some(state);
    {
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        ctx.request_body_config.enabled = true;
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(adapter.ensure_request_body(Some(64)).await.unwrap(), 2);
        adapter
            .overwrite_request_body(Bytes::from_static(b"{\"query\":\"{hello}\"}"))
            .await
            .unwrap();
    }
    let hit = prepare_hit(&mut session, &mut ctx).expect("Ready capture is physical EOF, before upstream replay");
    deliver_hit(&mut session, &mut ctx, hit).await.unwrap();
    let settings = HttpPersistentSettings::for_session(&session);
    let reusable = session
        .downstream_session
        .finish()
        .await
        .unwrap()
        .expect("consumed POST remains reusable");
    let (stream, _) = ReusedHttpStream::from_reusable_stream(reusable, settings).consume();
    client
        .write_all(b"GET /next HTTP/1.1\r\nHost: example.test\r\n\r\n")
        .await
        .unwrap();
    let mut next = Session::new_h1(stream);
    assert!(next.read_request().await.unwrap());
    assert_eq!(next.req_header().method, http::Method::GET);
}

#[tokio::test]
async fn terminal_error_discards_private_fill() {
    let (store, _, state) = fixture(1024);
    let (mut session, _client) = h1(b"GET /cache HTTP/1.1\r\nHost: example.test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.response_cache = Some(state);
    capture_headers(&mut ctx, &response());
    capture_body(&mut ctx, Some(&Bytes::from_static(b"hello")), true);
    let error = Error::explain(ErrorType::ReadError, "test transport failure");
    finalize(&mut session, &mut ctx, Some(&error)).await;
    assert_eq!(store.stats().1, 0);
    assert_eq!(store.stats().3, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h2_hit_accepts_clean_eos_for_empty_and_nonempty_payload() {
    use pingora_core::protocols::http::v2::server::{handshake, H2Accept, HttpSession};
    use pingora_core::protocols::Digest;
    use std::sync::Arc;
    for payload in [b"".as_slice(), b"hello".as_slice()] {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (client_io, server_io) = tokio::io::duplex(64 * 1024);
            let expected = payload.to_vec();
            let client = tokio::spawn(async move {
                let (mut sender, connection) = h2::client::handshake(client_io).await.unwrap();
                let driver = tokio::spawn(connection);
                sender = sender.ready().await.unwrap();
                let request = http::Request::builder()
                    .uri("https://example.test/cache")
                    .body(())
                    .unwrap();
                let (response, _) = sender.send_request(request, true).unwrap();
                let response = response.await.unwrap();
                assert_eq!(response.status(), 200);
                assert!(response.headers().contains_key("age"));
                let mut stream = response.into_body();
                let mut actual = Vec::new();
                while let Some(data) = stream.data().await {
                    let data = data.unwrap();
                    stream.flow_control().release_capacity(data.len()).unwrap();
                    actual.extend_from_slice(&data);
                }
                assert_eq!(actual, expected);
                assert!(stream.trailers().await.unwrap().is_none());
                assert!(stream.is_end_stream());
                driver.abort();
            });
            let mut connection = handshake(Box::new(server_io), None).await.unwrap();
            let Some(H2Accept::Session(downstream)) =
                HttpSession::from_h2_conn(&mut connection, Arc::new(Digest::default()))
                    .await
                    .unwrap()
            else {
                panic!("real H2 request reaches replay");
            };
            let (_unused, placeholder) = tokio::io::duplex(1);
            let mut session = Session::new_h1(Box::new(placeholder));
            *session.downstream_session = pingora_core::protocols::http::server::Session::new_http2(downstream);
            let driver = tokio::spawn(async move { while connection.accept().await.is_some() {} });
            let (store, _, mut state) = fixture(1024);
            let header = response();
            state.capture(&header, Instant::now());
            state.push(Some(&Bytes::copy_from_slice(payload)), true);
            assert!(state.seal_pending_headers(&header));
            assert!(state.prepare_publication(Some(&header)));
            state.publish();
            let mut ctx = EdgionHttpContext::default();
            ctx.response_cache = Some(state);
            let hit = prepare_hit(&mut session, &mut ctx).unwrap();
            deliver_hit(&mut session, &mut ctx, hit).await.unwrap();
            session.as_downstream_mut().finish_body().await.unwrap();
            assert_eq!(store.stats().3, 1);
            assert!(ctx.get_current_upstream().is_none());
            client.await.unwrap();
            driver.abort();
        })
        .await
        .unwrap();
    }
}
