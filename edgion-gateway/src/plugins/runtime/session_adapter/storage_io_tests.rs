use super::*;
use crate::request_body::{StorageIoOperation, STORAGE_IO_TEST_HOOK};
use parking_lot::Mutex;
use pingora_core::protocols::http::v2::server::{handshake, H2Accept, HttpSession as H2Session};
use pingora_core::protocols::Digest;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn h1_capture_storage_timeout_is_local_failure_and_disables_reuse() {
    let (mut client, server) = tokio::io::duplex(4096);
    let mut session = Session::new_h1(Box::new(server));
    client
        .write_all(b"POST /upload HTTP/1.1\r\nHost: test\r\nContent-Length: 4\r\n\r\nbody")
        .await
        .unwrap();
    assert!(session.read_request().await.unwrap());
    assert!(session.as_ref().get_keepalive().is_some());

    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    ctx.request_info.body_declared = Some(true);
    ctx.request_body_config.memory_buffer_bytes = 1;
    ctx.request_body_config.max_body_size_bytes = 64;
    ctx.request_body_config.storage_operation_timeout = Duration::from_millis(100);

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let hook: Arc<dyn Fn(StorageIoOperation) + Send + Sync> = Arc::new(move |operation| {
        if operation == StorageIoOperation::SpillCreate {
            entered_tx.send(()).unwrap();
            release_rx.lock().recv().unwrap();
        }
    });

    let result = {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        tokio::time::timeout(
            Duration::from_secs(2),
            STORAGE_IO_TEST_HOOK.scope(hook, adapter.capture_request_body(None)),
        )
        .await
        .expect("capture must return within its storage deadline")
    };
    assert_eq!(result, Err(BodyReadError::StorageFailure));
    entered_rx
        .try_recv()
        .expect("spill operation reached the blocking closure");
    assert_eq!(session.as_ref().get_keepalive(), None);
    release_tx.send(()).expect("release late spill operation");
}

#[tokio::test]
async fn h2_capture_cleanup_keeps_the_connection_available_to_a_sibling_stream() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let client = tokio::spawn(async move {
        let (mut sender, connection) = h2::client::handshake(client_io).await.unwrap();
        let driver = tokio::spawn(connection);

        sender = sender.ready().await.unwrap();
        let first = http::Request::builder()
            .method(http::Method::POST)
            .uri("https://example.test/first")
            .header(http::header::CONTENT_LENGTH, "4")
            .body(())
            .unwrap();
        let (_first_response, mut first_body) = sender.send_request(first, false).unwrap();
        first_body.send_data(bytes::Bytes::from_static(b"body"), true).unwrap();

        sender = sender.ready().await.unwrap();
        let second = http::Request::builder()
            .uri("https://example.test/second")
            .body(())
            .unwrap();
        let (second_response, _) = sender.send_request(second, true).unwrap();
        assert_eq!(second_response.await.unwrap().status(), http::StatusCode::NO_CONTENT);
        drop(sender);
        driver.abort();
    });

    let mut connection = handshake(Box::new(server_io), None).await.unwrap();
    let digest = Arc::new(Digest::default());
    let Some(H2Accept::Session(first)) = H2Session::from_h2_conn(&mut connection, Arc::clone(&digest))
        .await
        .unwrap()
    else {
        panic!("first H2 stream reaches the application");
    };
    let mut first = pingora_core::protocols::http::server::Session::new_http2(first);
    let mut ctx = EdgionHttpContext::default();
    ctx.request_body_config.memory_buffer_bytes = 1;
    ctx.request_body_config.max_body_size_bytes = 64;
    ctx.request_body_config.storage_operation_timeout = Duration::from_millis(100);

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let hook: Arc<dyn Fn(StorageIoOperation) + Send + Sync> = Arc::new(move |operation| {
        if operation == StorageIoOperation::SpillCreate {
            entered_tx.send(()).unwrap();
            release_rx.lock().recv().unwrap();
        }
    });
    let result = {
        tokio::time::timeout(
            Duration::from_secs(2),
            STORAGE_IO_TEST_HOOK.scope(hook, capture_request_body_core(&mut first, &mut ctx, None)),
        )
        .await
        .expect("capture must return within its storage deadline")
    };
    assert_eq!(result, Err(BodyReadError::StorageFailure));
    entered_rx
        .try_recv()
        .expect("H2 capture reached the blocked spill operation");
    assert_eq!(ctx.request_body.cached_len(), Ok(None));
    assert!(first.is_http2());
    release_tx.send(()).expect("release late H2 spill operation");
    drop(first);

    let Some(H2Accept::Session(mut second)) = H2Session::from_h2_conn(&mut connection, Arc::clone(&digest))
        .await
        .unwrap()
    else {
        panic!("sibling H2 stream remains usable after first-stream cleanup");
    };
    second
        .write_response_header(
            Box::new(pingora_http::ResponseHeader::build(http::StatusCode::NO_CONTENT, Some(0)).unwrap()),
            true,
        )
        .unwrap();
    drop(second);
    let (client_result, connection_result) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(client, H2Session::from_h2_conn(&mut connection, digest))
    })
    .await
    .expect("second response flushes and the client closes the H2 connection");
    client_result.unwrap();
    assert!(connection_result.unwrap().is_none());
}
