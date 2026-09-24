use super::{
    opaque_body_state, opaque_body_stream, settle_opaque_body_upload, BodyReadError, RequestBodySnapshotReader,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{poll, StreamExt};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct ScriptedReader {
    len: u64,
    results: VecDeque<Result<Option<Bytes>, BodyReadError>>,
}

#[async_trait]
impl RequestBodySnapshotReader for ScriptedReader {
    fn len(&self) -> u64 {
        self.len
    }

    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
        self.results.pop_front().unwrap_or(Ok(None))
    }
}

#[tokio::test]
async fn midstream_snapshot_failure_is_recorded_after_fetched_bytes() {
    let reader = ScriptedReader {
        len: 8,
        results: VecDeque::from([
            Ok(Some(Bytes::from_static(b"abcd"))),
            Err(BodyReadError::StorageFailure),
        ]),
    };
    let tracking = opaque_body_state(Box::new(reader));
    let mut stream = Box::pin(opaque_body_stream(Arc::clone(&tracking)));

    assert_eq!(stream.next().await.unwrap().unwrap(), Bytes::from_static(b"abcd"));
    assert!(matches!(
        stream.next().await.unwrap(),
        Err(BodyReadError::StorageFailure)
    ));
    assert_eq!(tracking.fetched.load(Ordering::Acquire), 4);
    assert!(matches!(
        tracking.read_error.lock().as_ref(),
        Some(BodyReadError::StorageFailure)
    ));
    let (refund, error) = settle_opaque_body_upload(&tracking, 8).await;
    assert_eq!(refund, 4);
    assert!(matches!(error, Some(BodyReadError::StorageFailure)));
}

struct PendingReader {
    dropped: Arc<AtomicBool>,
}

impl Drop for PendingReader {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

#[async_trait]
impl RequestBodySnapshotReader for PendingReader {
    fn len(&self) -> u64 {
        1
    }

    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_stops_and_drops_a_pending_snapshot_reader() {
    let dropped = Arc::new(AtomicBool::new(false));
    let reader = PendingReader {
        dropped: Arc::clone(&dropped),
    };
    let tracking = opaque_body_state(Box::new(reader));
    let mut stream = Box::pin(opaque_body_stream(Arc::clone(&tracking)));

    let next = stream.next();
    futures::pin_mut!(next);
    assert!(poll!(next.as_mut()).is_pending());

    let (refund, item) = tokio::join!(settle_opaque_body_upload(&tracking, 1), next.as_mut());
    assert_eq!(refund, (0, None));
    assert!(item.is_none());
    assert!(dropped.load(Ordering::Acquire));
    assert!(tracking.ambiguous_read_cancelled.load(Ordering::Acquire));
}

struct FirstChunkThenPendingReader {
    first: bool,
    dropped: Arc<AtomicBool>,
}

impl Drop for FirstChunkThenPendingReader {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

#[async_trait]
impl RequestBodySnapshotReader for FirstChunkThenPendingReader {
    fn len(&self) -> u64 {
        128 * 1024
    }

    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
        if self.first {
            self.first = false;
            return Ok(Some(Bytes::from(vec![b'x'; 64 * 1024])));
        }
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_releases_reader_without_another_stream_poll() {
    let dropped = Arc::new(AtomicBool::new(false));
    let reader = FirstChunkThenPendingReader {
        first: true,
        dropped: Arc::clone(&dropped),
    };
    let tracking = opaque_body_state(Box::new(reader));
    let mut stream = Box::pin(opaque_body_stream(Arc::clone(&tracking)));

    assert_eq!(stream.next().await.unwrap().unwrap().len(), 64 * 1024);
    let (refund, error) = settle_opaque_body_upload(&tracking, 128 * 1024).await;

    assert!(dropped.load(Ordering::Acquire));
    assert_eq!(tracking.fetched.load(Ordering::Acquire), 64 * 1024);
    assert!(!tracking.ambiguous_read_cancelled.load(Ordering::Acquire));
    assert_eq!(refund, 64 * 1024);
    assert_eq!(error, None);
}

#[tokio::test]
async fn dropping_a_pending_stream_future_aborts_the_chunk_task() {
    let dropped = Arc::new(AtomicBool::new(false));
    let reader = PendingReader {
        dropped: Arc::clone(&dropped),
    };
    let tracking = opaque_body_state(Box::new(reader));
    let mut stream = Box::pin(opaque_body_stream(Arc::clone(&tracking)));

    {
        let next = stream.next();
        futures::pin_mut!(next);
        assert!(poll!(next.as_mut()).is_pending());
    }
    drop(stream);

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !dropped.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropping the upload future must abort its chunk task");

    let settled = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        settle_opaque_body_upload(&tracking, 1),
    )
    .await
    .expect("dropped upload future must not leave in_flight stuck");

    assert_eq!(settled, (0, None));
    assert!(dropped.load(Ordering::Acquire));
    assert!(tracking.ambiguous_read_cancelled.load(Ordering::Acquire));
}
