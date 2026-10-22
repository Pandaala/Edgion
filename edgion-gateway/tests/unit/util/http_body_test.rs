use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn collector_does_not_allocate_before_nonempty_data() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    for cap in [0, 1, 8192, 16384] {
        for chunks in [vec![], vec![Ok(Bytes::new())], vec![Err("read failed")]] {
            let failed = chunks.first().is_some_and(Result::is_err);
            let stream = futures::stream::iter(chunks);
            let mut future = std::pin::pin!(stream_body_chunks_capped(stream, cap));
            let mut context = Context::from_waker(Waker::noop());
            let (result, allocations) =
                crate::test_allocation_counter::count_allocations(|| future.as_mut().poll(&mut context));
            if failed {
                assert_eq!(result, Poll::Ready(Err("read failed")));
            } else {
                assert_eq!(result, Poll::Ready(Ok((Bytes::new(), false))));
            }
            assert_eq!(allocations, 0, "cap={cap}, failed={failed}");
        }

        let stream = futures::stream::pending::<Result<Bytes, ()>>();
        let mut future = std::pin::pin!(stream_body_chunks_capped(stream, cap));
        let mut context = Context::from_waker(Waker::noop());
        let (result, allocations) =
            crate::test_allocation_counter::count_allocations(|| future.as_mut().poll(&mut context));
        assert!(result.is_pending());
        assert_eq!(allocations, 0, "pending cap={cap}");
    }
}

#[tokio::test]
async fn collector_preserves_empty_zero_and_exact_eof_contracts() {
    for (chunks, cap, expected) in [
        (vec![], 0, b"".as_slice()),
        (vec![Bytes::new()], 0, b"".as_slice()),
        (
            vec![Bytes::new(), Bytes::from_static(b"abc"), Bytes::new()],
            3,
            b"abc".as_slice(),
        ),
        (
            vec![Bytes::from_static(b"ab"), Bytes::from_static(b"c")],
            3,
            b"abc".as_slice(),
        ),
    ] {
        let stream = futures::stream::iter(chunks.into_iter().map(Ok::<_, ()>));
        let (body, truncated) = stream_body_chunks_capped(stream, cap).await.unwrap();
        assert_eq!(body.as_ref(), expected);
        assert!(!truncated);
    }
}

#[tokio::test]
async fn collector_stops_polling_at_first_overflow_and_keeps_capped_prefix() {
    for chunks in [
        vec![b"abcdef".as_slice()],
        vec![b"ab".as_slice(), b"cdef".as_slice()],
        vec![b"".as_slice(), b"ab".as_slice(), b"".as_slice(), b"cdef".as_slice()],
    ] {
        let stream = futures::stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok::<_, ()>(Bytes::copy_from_slice(chunk))),
        )
        .chain(futures::stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<Bytes, ()>>> { panic!("collector polled after overflow") },
        ));
        let (body, truncated) = stream_body_chunks_capped(stream, 3).await.unwrap();
        assert_eq!(body.as_ref(), b"abc");
        assert!(truncated);
    }
    let (body, truncated) =
        stream_body_chunks_capped(futures::stream::iter([Ok::<_, ()>(Bytes::from_static(b"x"))]), 0)
            .await
            .unwrap();
    assert!(body.is_empty());
    assert!(truncated);
}

#[tokio::test]
async fn collector_propagates_read_error() {
    let result = stream_body_chunks_capped(
        futures::stream::iter([Ok(Bytes::from_static(b"ab")), Err("read failed")]),
        3,
    )
    .await;
    assert_eq!(result, Err("read failed"));
}

struct PendingStream(Arc<AtomicBool>);
impl Stream for PendingStream {
    type Item = Result<Bytes, ()>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Pending
    }
}
impl Drop for PendingStream {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn dropping_pending_collector_releases_stream() {
    let dropped = Arc::new(AtomicBool::new(false));
    let mut future = Box::pin(stream_body_chunks_capped(PendingStream(Arc::clone(&dropped)), 10));
    assert!(futures::poll!(&mut future).is_pending());
    drop(future);
    assert!(dropped.load(Ordering::SeqCst));
}
