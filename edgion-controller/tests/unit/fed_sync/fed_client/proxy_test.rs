use super::*;
use axum::{body::Body, routing::get, Router};

fn request() -> HttpProxyRequest {
    HttpProxyRequest {
        request_id: "test".into(),
        method: "GET".into(),
        path: "/api/v1/test".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn configserver_is_not_a_federation_proxy_surface() {
    let mut req = request();
    req.path = "/configserver/EdgionTls/list".into();
    let router = Router::new().route("/configserver/EdgionTls/list", get(|| async { "must-not-be-exported" }));
    let result = execute(router, req).await;
    assert_eq!(result.status_code, 404);
    assert!(!String::from_utf8_lossy(&result.body).contains("must-not-be-exported"));
}

#[tokio::test]
async fn response_failure_replaces_status_and_entity_headers() {
    let router = Router::new().route(
        "/api/v1/test",
        get(|| async {
            http::Response::builder()
                .header("content-encoding", "gzip")
                .body(Body::from_stream(futures::stream::iter([Err::<bytes::Bytes, _>(
                    std::io::Error::other("sensitive failure"),
                )])))
                .unwrap()
        }),
    );
    let response = execute(router, request()).await;
    assert_eq!(response.status_code, 502);
    assert!(response.headers.is_empty());
    assert_eq!(response.body, b"Failed to read proxy response");
}

#[tokio::test]
async fn response_limit_is_not_the_default_four_megabyte_grpc_limit() {
    let router = Router::new().route("/api/v1/test", get(|| async { vec![b'a'; 5 * 1024 * 1024] }));
    assert_eq!(execute(router, request()).await.body.len(), 5 * 1024 * 1024);
    let router = Router::new().route("/api/v1/test", get(|| async { vec![b'a'; MAX_RESPONSE_BYTES + 1] }));
    let response = execute(router, request()).await;
    assert_eq!(response.status_code, 413);
    assert!(response.headers.is_empty());
}

#[tokio::test]
async fn envelope_and_final_status_are_checked() {
    let mut req = request();
    req.headers.insert("ignored".into(), "x".repeat(MAX_HEADER_BYTES));
    assert_eq!(execute(Router::new(), req).await.status_code, 413);
    let router = Router::new().route("/api/v1/test", get(|| async { http::StatusCode::SWITCHING_PROTOCOLS }));
    assert_eq!(execute(router, request()).await.status_code, 502);
}

#[tokio::test]
async fn full_response_channel_obeys_deadline() {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    tx.send(ControllerMessage::default()).await.unwrap();
    let deadline = Instant::now() + Duration::from_millis(10);
    tokio::time::timeout(
        Duration::from_secs(1),
        send(&tx, error("test".into(), 504, "Timeout"), deadline),
    )
    .await
    .unwrap();
    send(&tx, error("test".into(), 429, "Busy"), deadline).await;
}

#[tokio::test]
async fn deadline_drops_slow_router_and_body_work() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    struct MarkDrop(Arc<AtomicBool>);
    impl Drop for MarkDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    for slow_body in [false, true] {
        let dropped = Arc::new(AtomicBool::new(false));
        let marker = dropped.clone();
        let router = Router::new().route(
            "/api/v1/test",
            get(move || {
                let marker = MarkDrop(marker.clone());
                async move {
                    if slow_body {
                        Body::from_stream(futures::stream::once(async move {
                            let _marker = marker;
                            std::future::pending::<Result<bytes::Bytes, std::io::Error>>().await
                        }))
                    } else {
                        let _marker = marker;
                        std::future::pending::<Body>().await
                    }
                }
            }),
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), execute(router, request()))
                .await
                .is_err()
        );
        assert!(dropped.load(Ordering::SeqCst));
    }
}

#[test]
fn overload_and_failure_envelopes_do_not_echo_oversized_identifiers() {
    for status in [400, 429, 504] {
        let response = error("é".repeat(128), status, "Fixed error");
        assert!(response.request_id.is_empty());
        assert_eq!(response.body, b"Fixed error");
    }
}
