use super::*;
use futures::StreamExt;

#[tokio::test]
async fn health_response_accepts_complete_normal_and_exact_limit_documents() {
    for length in [18, MAX_HEALTH_RESPONSE_BYTES] {
        let mut body = br#"{"status":"green"}"#.to_vec();
        body.resize(length, b' ');
        for declared in [None, Some(length as u64)] {
            let chunks = vec![
                Ok::<_, ()>(Bytes::copy_from_slice(&body[..8])),
                Ok(Bytes::copy_from_slice(&body[8..])),
            ];
            assert_eq!(
                read_cluster_health_stream(declared, futures::stream::iter(chunks))
                    .await
                    .unwrap(),
                "green"
            );
        }
    }
}

#[tokio::test]
async fn health_response_rejects_declared_overflow_without_polling() {
    let stream = futures::stream::poll_fn(|_| -> std::task::Poll<Option<Result<Bytes, ()>>> {
        panic!("oversized declared response must not be polled")
    });
    let error = read_cluster_health_stream(Some(MAX_HEALTH_RESPONSE_BYTES as u64 + 1), stream)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "ES cluster health response exceeds 1 MiB");
}

#[tokio::test]
async fn health_response_rejects_overflow_before_parsing_and_stops_polling() {
    for declared in [None, Some(1)] {
        // Both an otherwise valid JSON prefix and invalid JSON must fail as overflow.
        for prefix in [br#"{"status":"green"}"#.as_slice(), b"invalid JSON".as_slice()] {
            let mut body = prefix.to_vec();
            body.resize(MAX_HEALTH_RESPONSE_BYTES, b' ');
            let stream = futures::stream::iter([Ok::<_, ()>(Bytes::from(body)), Ok(Bytes::from_static(b" "))]).chain(
                futures::stream::poll_fn(|_| -> std::task::Poll<Option<Result<Bytes, ()>>> {
                    panic!("health reader polled after overflow")
                }),
            );
            let error = read_cluster_health_stream(declared, stream).await.unwrap_err();
            assert_eq!(error.to_string(), "ES cluster health response exceeds 1 MiB");
        }
    }
}

#[tokio::test]
async fn health_response_rejects_read_parse_and_missing_status_errors() {
    let error = read_cluster_health_stream(None, futures::stream::iter([Err::<Bytes, _>("private detail")]))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "ES cluster health response read failed");
    for (body, expected) in [
        (b"invalid".as_slice(), "ES cluster health parse error"),
        (b"{}".as_slice(), "ES cluster health: missing 'status' field"),
        (
            br#"{"status":null}"#.as_slice(),
            "ES cluster health: missing 'status' field",
        ),
    ] {
        let error =
            read_cluster_health_stream(None, futures::stream::iter([Ok::<_, ()>(Bytes::copy_from_slice(body))]))
                .await
                .unwrap_err();
        assert_eq!(error.to_string(), expected);
    }
}

#[tokio::test]
async fn cancelling_health_response_releases_stream_and_next_read_can_succeed() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    struct PendingBody(Arc<AtomicBool>);
    impl Stream for PendingBody {
        type Item = Result<Bytes, ()>;
        fn poll_next(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            std::task::Poll::Pending
        }
    }
    impl Drop for PendingBody {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let mut read = Box::pin(read_cluster_health_stream(None, PendingBody(dropped.clone())));
    assert!(futures::poll!(&mut read).is_pending());
    drop(read);
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(
        read_cluster_health_stream(
            None,
            futures::stream::iter([Ok::<_, ()>(Bytes::from_static(br#"{"status":"yellow"}"#))])
        )
        .await
        .unwrap(),
        "yellow"
    );
}

#[tokio::test]
async fn probe_cluster_health_false_on_unreachable() {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(200))
        .build()
        .unwrap();
    // Port 1 is unreachable -> the probe must report false, never panic.
    assert!(!probe_cluster_health(&client, "http://127.0.0.1:1").await);
}
