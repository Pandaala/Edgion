use super::*;
use async_trait::async_trait;
use futures::FutureExt;
use parking_lot::Mutex;
use pingora_core::protocols::raw_connect::ProxyDigest;
use pingora_core::protocols::{
    GetProxyDigest, GetSocketDigest, GetTimingDigest, Peek, Shutdown, SocketDigest, Ssl, TimingDigest, UniqueID,
    UniqueIDType,
};
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::task::Waker;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio::time::{advance, Instant};

#[derive(Default)]
struct RecordedHooks {
    sent: usize,
    received: usize,
    errors: Vec<DuplexError>,
}

impl DuplexHooks for &mut RecordedHooks {
    fn record_sent(&mut self, n: usize) {
        self.sent += n;
    }
    fn record_received(&mut self, n: usize) {
        self.received += n;
    }
    fn on_error(&mut self, err: DuplexError) {
        self.errors.push(err);
    }
}

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(futures::task::noop_waker_ref()))
}

async fn assert_reverse_progress(block_upload: bool) {
    let (mut client, downstream) = tokio::io::duplex(if block_upload { 128 } else { 8 });
    let (upstream, mut backend) = tokio::io::duplex(if block_upload { 8 } else { 128 });
    let mut hooks = RecordedHooks::default();
    if block_upload {
        client.write_all(&[7; 64]).await.unwrap();
    } else {
        backend.write_all(&[7; 64]).await.unwrap();
    }
    let relay = bidirectional_relay(Box::new(downstream), Box::new(upstream), &mut hooks, None);
    tokio::pin!(relay);
    for _ in 0..64 {
        assert!(poll_once(relay.as_mut()).is_pending());
    }
    // Only make reverse bytes available after the first write is blocked.
    if block_upload {
        backend.write_all(b"ack").await.unwrap();
    } else {
        client.write_all(b"ack").await.unwrap();
    }
    for _ in 0..64 {
        assert!(poll_once(relay.as_mut()).is_pending());
    }
    let mut reply = [0; 3];
    let result = if block_upload {
        client.read_exact(&mut reply).now_or_never()
    } else {
        backend.read_exact(&mut reply).now_or_never()
    };
    assert!(
        matches!(result, Some(Ok(3))),
        "reverse ACK must progress while the first write stays blocked"
    );
    assert_eq!(&reply, b"ack");
}

#[tokio::test]
async fn blocked_upload_does_not_block_download() {
    assert_reverse_progress(true).await;
}

#[tokio::test]
async fn blocked_download_does_not_block_upload() {
    assert_reverse_progress(false).await;
}

const IDLE: Duration = Duration::from_millis(200);

// Replaces write_all_idle_completes_when_reader_drains at the relay boundary;
// real executor wakeups, rather than manual polling, drive this case.
#[tokio::test]
async fn relay_completes_when_reader_drains() {
    let (mut client, downstream) = tokio::io::duplex(16);
    let (upstream, mut backend) = tokio::io::duplex(8);
    let mut hooks = RecordedHooks::default();
    let (done, wait_done) = tokio::sync::oneshot::channel();
    let payload: Vec<u8> = (0..40_000).map(|n| (n % 251) as u8).collect();
    let relay = bidirectional_relay(
        Box::new(downstream),
        Box::new(upstream),
        &mut hooks,
        Some(Duration::from_secs(5)),
    );
    let client_work = async {
        client.write_all(&payload).await.unwrap();
        let mut reply = [0; 3];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"ack");
        done.send(()).unwrap();
    };
    let backend_work = async {
        let mut received = vec![0; payload.len()];
        backend.read_exact(&mut received).await.unwrap();
        assert_eq!(received, payload);
        backend.write_all(b"ack").await.unwrap();
        wait_done.await.unwrap();
        drop(backend);
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(relay, client_work, backend_work);
    })
    .await
    .unwrap();
    assert_eq!((hooks.sent, hooks.received), (payload.len(), 3));
    assert!(hooks.errors.is_empty());
}

#[tokio::test]
async fn ready_streams_self_wake_after_poll_budget() {
    let payload: Vec<u8> = (0..BUFFER_SIZE * 40).map(|n| (n % 251) as u8).collect();
    let (mut client, downstream) = tokio::io::duplex(payload.len());
    let (upstream, mut backend) = tokio::io::duplex(payload.len());
    // Both buffers fit the entire transfer: no blocked writer's I/O wakeup
    // can rescue a relay that yields its poll budget without self-waking.
    client.write_all(&payload).await.unwrap();
    let mut hooks = RecordedHooks::default();
    let relay = bidirectional_relay(Box::new(downstream), Box::new(upstream), &mut hooks, None);
    let receiver = async {
        let mut received = vec![0; payload.len()];
        backend.read_exact(&mut received).await.unwrap();
        assert_eq!(received, payload);
        drop(backend);
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(relay, receiver);
    })
    .await
    .unwrap();
    assert_eq!(hooks.sent, payload.len());
    assert!(hooks.errors.is_empty());
}

async fn assert_blocked_write_expires(upload: bool) {
    let (mut client, downstream) = tokio::io::duplex(if upload { 128 } else { 8 });
    let (upstream, mut backend) = tokio::io::duplex(if upload { 8 } else { 128 });
    if upload {
        client.write_all(&[1; 64]).await.unwrap();
    } else {
        backend.write_all(&[1; 64]).await.unwrap();
    }
    let mut hooks = RecordedHooks::default();
    let started = Instant::now();
    let mut relay = Box::pin(bidirectional_relay(
        Box::new(downstream),
        Box::new(upstream),
        &mut hooks,
        Some(IDLE),
    ));
    assert!(poll_once(relay.as_mut()).is_pending());
    advance(IDLE - Duration::from_millis(1)).await;
    assert!(poll_once(relay.as_mut()).is_pending(), "must not expire early");
    advance(Duration::from_millis(1)).await;
    assert!(poll_once(relay.as_mut()).is_ready());
    drop(relay);
    assert_eq!(hooks.errors, [DuplexError::IdleTimeout]);
    assert_eq!(if upload { hooks.sent } else { hooks.received }, 64);
    assert!(Instant::now().saturating_duration_since(started) >= IDLE);
}

// Replaces write_all_idle_expires_when_peer_never_reads, in both directions.
#[tokio::test(start_paused = true)]
async fn blocked_upload_expires_without_early_timeout() {
    assert_blocked_write_expires(true).await;
}
#[tokio::test(start_paused = true)]
async fn blocked_download_expires_without_early_timeout() {
    assert_blocked_write_expires(false).await;
}

// Replaces write_all_idle_survives_slow_but_progressing_peer, checking exact
// bytes across repeated partial writes as well as elapsed idle windows.
#[tokio::test(start_paused = true)]
async fn partial_writes_reset_idle_and_preserve_payload() {
    let (mut client, downstream) = tokio::io::duplex(64);
    let (upstream, mut backend) = tokio::io::duplex(8);
    let payload: Vec<u8> = (0..40).collect();
    client.write_all(&payload).await.unwrap();
    let mut hooks = RecordedHooks::default();
    let mut relay = Box::pin(bidirectional_relay(
        Box::new(downstream),
        Box::new(upstream),
        &mut hooks,
        Some(IDLE),
    ));
    assert!(poll_once(relay.as_mut()).is_pending());
    let started = Instant::now();
    let mut received = Vec::new();
    for _ in 0..5 {
        advance(Duration::from_millis(80)).await;
        let mut chunk = [0; 8];
        backend.read_exact(&mut chunk).await.unwrap();
        received.extend_from_slice(&chunk);
        assert!(poll_once(relay.as_mut()).is_pending());
    }
    assert!(Instant::now().saturating_duration_since(started) > IDLE);
    assert_eq!(received, payload);
    drop(backend);
    assert!(poll_once(relay.as_mut()).is_ready());
    drop(relay);
    assert_eq!(hooks.sent, 40);
    assert!(hooks.errors.is_empty());
}

#[tokio::test(start_paused = true)]
async fn reverse_activity_resets_shared_idle_while_upload_is_blocked() {
    let (mut client, downstream) = tokio::io::duplex(128);
    let (upstream, mut backend) = tokio::io::duplex(8);
    client.write_all(&[1; 64]).await.unwrap();
    let mut hooks = RecordedHooks::default();
    let mut relay = Box::pin(bidirectional_relay(
        Box::new(downstream),
        Box::new(upstream),
        &mut hooks,
        Some(IDLE),
    ));
    assert!(poll_once(relay.as_mut()).is_pending());
    for _ in 0..5 {
        advance(Duration::from_millis(100)).await;
        backend.write_all(b"ack").await.unwrap();
        assert!(poll_once(relay.as_mut()).is_pending());
        let mut reply = [0; 3];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"ack");
    }
    advance(IDLE - Duration::from_millis(1)).await;
    assert!(poll_once(relay.as_mut()).is_pending());
    advance(Duration::from_millis(1)).await;
    assert!(poll_once(relay.as_mut()).is_ready());
    drop(relay);
    assert_eq!((hooks.sent, hooks.received), (64, 15));
    assert_eq!(hooks.errors, [DuplexError::IdleTimeout]);
}

#[tokio::test(start_paused = true)]
async fn idle_read_expires_once() {
    let (_client, downstream) = tokio::io::duplex(8);
    let (upstream, _backend) = tokio::io::duplex(8);
    let mut hooks = RecordedHooks::default();
    let started = Instant::now();
    bidirectional_relay(Box::new(downstream), Box::new(upstream), &mut hooks, Some(IDLE)).await;
    assert!(Instant::now().saturating_duration_since(started) >= IDLE);
    assert_eq!(hooks.errors, [DuplexError::IdleTimeout]);
    assert_eq!((hooks.sent, hooks.received), (0, 0));
}

#[tokio::test(start_paused = true)]
async fn disabled_idle_allows_blocked_write_to_resume() {
    let (mut client, downstream) = tokio::io::duplex(64);
    let (upstream, mut backend) = tokio::io::duplex(8);
    client.write_all(&[2; 16]).await.unwrap();
    let mut hooks = RecordedHooks::default();
    let mut relay = Box::pin(bidirectional_relay(
        Box::new(downstream),
        Box::new(upstream),
        &mut hooks,
        None,
    ));
    assert!(poll_once(relay.as_mut()).is_pending());
    advance(Duration::from_secs(86_400)).await;
    assert!(poll_once(relay.as_mut()).is_pending());
    let mut received = [0; 16];
    backend.read_exact(&mut received[..8]).await.unwrap();
    assert!(poll_once(relay.as_mut()).is_pending());
    backend.read_exact(&mut received[8..]).await.unwrap();
    assert_eq!(received, [2; 16]);
    drop(backend);
    assert!(poll_once(relay.as_mut()).is_ready());
    drop(relay);
    assert_eq!(hooks.sent, 16);
    assert!(hooks.errors.is_empty());
}

#[derive(Debug, Default)]
struct Control {
    read_error: bool,
    write_error: bool,
    write_zero: bool,
    flush_error: bool,
    flush_pending: bool,
    flushes: usize,
    flush_waker: Option<Waker>,
    written: Vec<u8>,
}

#[derive(Debug)]
struct ControlledStream {
    inner: DuplexStream,
    control: Arc<Mutex<Control>>,
}

impl AsyncRead for ControlledStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.control.lock().read_error {
            return Poll::Ready(Err(io::ErrorKind::Other.into()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
impl AsyncWrite for ControlledStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        {
            let control = self.control.lock();
            if control.write_error {
                return Poll::Ready(Err(io::ErrorKind::Other.into()));
            }
            if control.write_zero {
                return Poll::Ready(Ok(0));
            }
        }
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = result {
            self.control.lock().written.extend_from_slice(&buf[..n]);
        }
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut control = self.control.lock();
        control.flushes += 1;
        if control.flush_error {
            Poll::Ready(Err(io::ErrorKind::Other.into()))
        } else if control.flush_pending {
            control.flush_waker = Some(cx.waker().clone());
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
#[async_trait]
impl Shutdown for ControlledStream {
    async fn shutdown(&mut self) {}
}
impl UniqueID for ControlledStream {
    fn id(&self) -> UniqueIDType {
        0
    }
}
impl Ssl for ControlledStream {}
impl Peek for ControlledStream {}
impl GetTimingDigest for ControlledStream {
    fn get_timing_digest(&self) -> Vec<Option<TimingDigest>> {
        vec![]
    }
}
impl GetProxyDigest for ControlledStream {
    fn get_proxy_digest(&self) -> Option<Arc<ProxyDigest>> {
        None
    }
}
impl GetSocketDigest for ControlledStream {
    fn get_socket_digest(&self) -> Option<Arc<SocketDigest>> {
        None
    }
}

fn controlled_pair(capacity: usize) -> (DuplexStream, Stream, Arc<Mutex<Control>>) {
    let (peer, inner) = tokio::io::duplex(capacity);
    let control = Arc::new(Mutex::new(Control::default()));
    (
        peer,
        Box::new(ControlledStream {
            inner,
            control: control.clone(),
        }),
        control,
    )
}

async fn assert_error(error: DuplexError, zero: bool) {
    let (mut client, downstream, down_control) = controlled_pair(64);
    let (mut backend, upstream, up_control) = controlled_pair(64);
    match error {
        DuplexError::DownstreamRead => down_control.lock().read_error = true,
        DuplexError::UpstreamRead => up_control.lock().read_error = true,
        DuplexError::UpstreamWrite => {
            client.write_all(b"upload").await.unwrap();
            if zero {
                up_control.lock().write_zero = true;
            } else {
                up_control.lock().write_error = true;
            }
        }
        DuplexError::DownstreamWrite => {
            backend.write_all(b"download").await.unwrap();
            if zero {
                down_control.lock().write_zero = true;
            } else {
                down_control.lock().write_error = true;
            }
        }
        DuplexError::DownstreamFlush => {
            backend.write_all(b"download").await.unwrap();
            down_control.lock().flush_error = true;
        }
        DuplexError::IdleTimeout => unreachable!("idle tests use the shared timer"),
    }
    let mut hooks = RecordedHooks::default();
    bidirectional_relay(downstream, upstream, &mut hooks, Some(IDLE)).await;
    assert_eq!(hooks.errors, [error]);
    assert_eq!(hooks.sent, if error == DuplexError::UpstreamWrite { 6 } else { 0 });
    assert_eq!(
        hooks.received,
        if matches!(error, DuplexError::DownstreamWrite | DuplexError::DownstreamFlush) {
            8
        } else {
            0
        }
    );
    assert_eq!(up_control.lock().flushes, 0);
}

#[tokio::test]
async fn downstream_read_error_is_reported_once() {
    assert_error(DuplexError::DownstreamRead, false).await;
}
#[tokio::test]
async fn upstream_read_error_is_reported_once() {
    assert_error(DuplexError::UpstreamRead, false).await;
}
#[tokio::test]
async fn upstream_write_error_retains_read_accounting() {
    assert_error(DuplexError::UpstreamWrite, false).await;
}
#[tokio::test]
async fn downstream_write_error_retains_read_accounting() {
    assert_error(DuplexError::DownstreamWrite, false).await;
}
#[tokio::test]
async fn downstream_flush_error_is_distinct() {
    assert_error(DuplexError::DownstreamFlush, false).await;
}
#[tokio::test]
async fn upstream_write_zero_is_a_write_error() {
    assert_error(DuplexError::UpstreamWrite, true).await;
}
#[tokio::test]
async fn downstream_write_zero_is_a_write_error() {
    assert_error(DuplexError::DownstreamWrite, true).await;
}

#[tokio::test(start_paused = true)]
async fn pending_flush_allows_reverse_progress_and_shares_idle() {
    let (mut client, downstream, control) = controlled_pair(64);
    let (mut backend, upstream, up_control) = controlled_pair(64);
    control.lock().flush_pending = true;
    backend.write_all(b"reply").await.unwrap();
    let mut hooks = RecordedHooks::default();
    let mut relay = Box::pin(bidirectional_relay(downstream, upstream, &mut hooks, Some(IDLE)));
    assert!(poll_once(relay.as_mut()).is_pending());
    assert!(control.lock().flushes > 0);
    for _ in 0..5 {
        advance(Duration::from_millis(100)).await;
        client.write_all(b"ack").await.unwrap();
        assert!(poll_once(relay.as_mut()).is_pending());
        let mut ack = [0; 3];
        backend.read_exact(&mut ack).await.unwrap();
        assert_eq!(&ack, b"ack");
    }
    assert_eq!(control.lock().written, b"reply");
    assert_eq!(up_control.lock().flushes, 0);
    advance(IDLE - Duration::from_millis(1)).await;
    assert!(poll_once(relay.as_mut()).is_pending());
    advance(Duration::from_millis(1)).await;
    assert!(poll_once(relay.as_mut()).is_ready());
    drop(relay);
    assert_eq!((hooks.sent, hooks.received), (15, 5));
    assert_eq!(hooks.errors, [DuplexError::IdleTimeout]);
}

#[tokio::test(start_paused = true)]
async fn disabled_idle_preserves_flush_before_next_read() {
    let (_client, downstream, control) = controlled_pair(64);
    let (mut backend, upstream, up_control) = controlled_pair(64);
    control.lock().flush_pending = true;
    backend.write_all(b"first").await.unwrap();
    let mut hooks = RecordedHooks::default();
    let mut relay = Box::pin(bidirectional_relay(downstream, upstream, &mut hooks, None));
    assert!(poll_once(relay.as_mut()).is_pending());
    backend.write_all(b"second").await.unwrap();
    advance(Duration::from_secs(86_400)).await;
    assert!(poll_once(relay.as_mut()).is_pending());
    assert_eq!(control.lock().written, b"first");
    let waker = {
        let mut control = control.lock();
        control.flush_pending = false;
        control.flush_waker.take().unwrap()
    };
    waker.wake();
    assert!(poll_once(relay.as_mut()).is_pending());
    assert_eq!(control.lock().written, b"firstsecond");
    assert_eq!(up_control.lock().flushes, 0);
    drop(backend);
    assert!(poll_once(relay.as_mut()).is_ready());
    drop(relay);
    assert_eq!(hooks.received, 11);
    assert!(hooks.errors.is_empty());
}

#[tokio::test]
async fn downstream_eof_does_not_wait_for_pending_flush() {
    let (mut client, downstream, control) = controlled_pair(64);
    let (mut backend, upstream, _) = controlled_pair(64);
    control.lock().flush_pending = true;
    backend.write_all(b"reply").await.unwrap();
    let mut hooks = RecordedHooks::default();
    let mut relay = Box::pin(bidirectional_relay(downstream, upstream, &mut hooks, None));
    assert!(poll_once(relay.as_mut()).is_pending());
    assert!(control.lock().flushes > 0);
    AsyncWriteExt::shutdown(&mut client).await.unwrap();
    assert!(poll_once(relay.as_mut()).is_ready());
    drop(relay);
    assert_eq!(hooks.received, 5);
    assert!(hooks.errors.is_empty());
}

#[tokio::test]
async fn eof_ends_without_error_or_draining_other_direction() {
    for downstream_eof in [true, false] {
        let (mut client, downstream) = tokio::io::duplex(128);
        let (upstream, mut backend) = tokio::io::duplex(8);
        // Block upload before making EOF observable in the reverse reader.
        if !downstream_eof {
            client.write_all(&[3; 64]).await.unwrap();
        }
        let mut hooks = RecordedHooks::default();
        let mut relay = Box::pin(bidirectional_relay(
            Box::new(downstream),
            Box::new(upstream),
            &mut hooks,
            None,
        ));
        assert!(poll_once(relay.as_mut()).is_pending());
        if downstream_eof {
            AsyncWriteExt::shutdown(&mut client).await.unwrap();
        } else {
            AsyncWriteExt::shutdown(&mut backend).await.unwrap();
        }
        assert!(poll_once(relay.as_mut()).is_ready());
        drop(relay);
        assert!(hooks.errors.is_empty());
        assert_eq!(hooks.sent, if downstream_eof { 0 } else { 64 });
    }
}
