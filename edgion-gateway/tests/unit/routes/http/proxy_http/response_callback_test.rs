use super::*;
use crate::plugins::runtime::traits::{ResponseStreamSession, StreamingResponseProcessor};
use crate::response::driver::{BodyExecutionLease, ResponseProcessorDriver};
use async_trait::async_trait;
use bytes::Bytes;
use pingora_proxy::ResponseBodySink;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio::sync::Notify;

async fn session(request: &[u8]) -> (Session, DuplexStream) {
    let (mut client, server) = tokio::io::duplex(4096);
    client.write_all(request).await.unwrap();
    let mut session = Session::new_h1(Box::new(server));
    assert!(session.read_request().await.unwrap());
    (session, client)
}

struct Probe(Arc<AtomicUsize>);
#[async_trait]
impl StreamingResponseProcessor for Probe {
    async fn on_chunk(
        &mut self,
        _: &mut Option<Bytes>,
        _: bool,
        _: crate::ctx::ResponseStreamKind,
        _: &mut ResponseBodySink,
        _: &mut dyn ResponseStreamSession,
    ) {
    }
    fn release_inflight(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

async fn owned_wait(_lease: BodyExecutionLease<'_>, entered: &Notify, calls: &AtomicUsize) -> Result<()> {
    calls.fetch_add(1, Ordering::SeqCst);
    entered.notify_one();
    pending().await
}

#[tokio::test]
async fn close_before_first_poll_releases_owned_lease_without_entering_callback() {
    let (mut session, client) = session(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let released = Arc::new(AtomicUsize::new(0));
    let driver = ResponseProcessorDriver::new(None, None, vec![Box::new(Probe(released.clone()))], vec![]).unwrap();
    let lease = driver.body_lease().await.unwrap();
    drop(client);
    let calls = AtomicUsize::new(0);
    let entered = Notify::new();
    let error = run(&mut session, owned_wait(lease, &entered, &calls))
        .await
        .unwrap_err();
    assert_eq!(error.esource(), &pingora_core::ErrorSource::Downstream);
    assert!(!error.retry.retry());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert!(driver.body_lease().await.is_none());
}

#[tokio::test]
async fn close_drops_pending_callback_before_return_and_releases_only_once() {
    let (mut session, client) = session(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let released = Arc::new(AtomicUsize::new(0));
    let driver = ResponseProcessorDriver::new(None, None, vec![Box::new(Probe(released.clone()))], vec![]).unwrap();
    let lease = driver.body_lease().await.unwrap();
    let calls = AtomicUsize::new(0);
    let entered = Notify::new();
    let close = async {
        entered.notified().await;
        drop(client);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(run(&mut session, owned_wait(lease, &entered, &calls)), close)
    })
    .await
    .unwrap();
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert!(driver.body_lease().await.is_none());
}

#[tokio::test]
async fn tolerated_h1_fin_keeps_response_callback_alive() {
    let (mut session, mut client) = session(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    session.set_abort_on_close(false);
    client.shutdown().await.unwrap();
    run(&mut session, async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(())
    })
    .await
    .unwrap();
    assert!(session.as_downstream().as_http1().unwrap().is_half_closed());
}

#[tokio::test]
async fn incomplete_upload_is_not_consumed_by_idle_monitor() {
    let (mut session, mut client) = session(b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 4\r\n\r\n").await;
    assert!(!session.is_body_done());
    client.write_all(b"body").await.unwrap();
    run(&mut session, async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(session.read_request_body().await.unwrap().unwrap().as_ref(), b"body");
}

#[tokio::test]
async fn pipelined_probe_remains_owned_by_canonical_reader() {
    let (mut session, mut client) = session(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    session.set_pipelining_enabled(true);
    client
        .write_all(b"GET /next HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    run(&mut session, async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(())
    })
    .await
    .unwrap();
    // A second monitor must not consume the remaining pipelined request.
    run(&mut session, async { Ok(()) }).await.unwrap();
    let mut response = pingora_http::ResponseHeader::build(200, None).unwrap();
    response.insert_header("content-length", "0").unwrap();
    session.write_response_header(Box::new(response), true).await.unwrap();
    let reusable = (*session.downstream_session).finish().await.unwrap().unwrap();
    let (stream, prefix) = reusable.into_parts();
    assert_eq!(prefix.as_ref().unwrap().as_ref(), b"G");
    let mut next = Session::new_h1(stream);
    next.set_pipelined_prefix(prefix.unwrap());
    assert!(next.read_request().await.unwrap());
    assert_eq!(next.req_header().uri.path(), "/next");
}

struct FinalizationProbe {
    released: Arc<AtomicUsize>,
    entered: Arc<Notify>,
}
#[async_trait]
impl StreamingResponseProcessor for FinalizationProbe {
    async fn on_chunk(
        &mut self,
        _: &mut Option<Bytes>,
        _: bool,
        _: crate::ctx::ResponseStreamKind,
        _: &mut ResponseBodySink,
        _: &mut dyn ResponseStreamSession,
    ) {
    }
    async fn on_actual_upstream_trailers(&mut self, _: Option<&http::HeaderMap>, _: bool) -> Result<()> {
        self.entered.notify_one();
        pending().await
    }
    fn release_inflight(&mut self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn close_cancels_pending_grpc_terminal_finalization() {
    let (mut session, client) = session(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let released = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let driver = ResponseProcessorDriver::new(
        None,
        None,
        vec![Box::new(FinalizationProbe {
            released: released.clone(),
            entered: entered.clone(),
        })],
        vec![],
    )
    .unwrap();
    let close = async {
        entered.notified().await;
        drop(client);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(run(&mut session, driver.finalize_grpc_transcoding(None, false)), close)
    })
    .await
    .unwrap();
    assert!(result.is_err());
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert!(driver.body_lease().await.is_none());
}
