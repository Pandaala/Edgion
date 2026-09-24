use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn test_tcp_probe_success() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    tokio::spawn(async move {
        let _ = listener.accept().await;
    });

    let res = probe_tcp(addr, None, Duration::from_secs(1)).await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_tcp_probe_failure_connection_refused() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    drop(listener);

    let res = probe_tcp(addr, None, Duration::from_millis(300)).await;
    assert!(matches!(
        res,
        Err(ProbeError::ConnectionRefused(_)) | Err(ProbeError::Timeout)
    ));
}

#[tokio::test]
async fn test_http_probe_matching_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut req_buf = [0_u8; 1024];
            let _ = stream.read(&mut req_buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        }
    });

    let res = probe_http(addr, "/healthz", None, None, &[200, 204], Duration::from_secs(1)).await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_http_probe_unexpected_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut req_buf = [0_u8; 1024];
            let _ = stream.read(&mut req_buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        }
    });

    let res = probe_http(addr, "/healthz", None, None, &[200], Duration::from_secs(1)).await;
    assert!(matches!(res, Err(ProbeError::UnexpectedStatus(503))));
}

#[tokio::test]
async fn test_http_probe_does_not_follow_redirects() {
    let redirect_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind redirect listener");
    let redirect_addr = redirect_listener.local_addr().expect("redirect listener addr");
    let target_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind target listener");
    let target_addr = target_listener.local_addr().expect("target listener addr");

    let (target_contact_tx, mut target_contact_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = target_listener.accept().await {
            let _ = target_contact_tx.send(());
            let mut req_buf = [0_u8; 1024];
            let _ = stream.read(&mut req_buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        }
    });

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = redirect_listener.accept().await {
            let mut req_buf = [0_u8; 1024];
            let _ = stream.read(&mut req_buf).await;
            let response = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });

    let res = probe_http(redirect_addr, "/healthz", None, None, &[200], Duration::from_secs(1)).await;
    assert!(matches!(res, Err(ProbeError::UnexpectedStatus(302))));
    assert!(matches!(
        target_contact_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn test_http_probe_ipv6_url_is_bracketed() {
    // Bind an IPv6 loopback listener; skip if the host has no IPv6 support.
    let Ok(listener) = TcpListener::bind("[::1]:0").await else {
        eprintln!("skipping test_http_probe_ipv6_url_is_bracketed: IPv6 loopback unavailable");
        return;
    };
    let addr = listener.local_addr().expect("listener addr");
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut req_buf = [0_u8; 1024];
            let _ = stream.read(&mut req_buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        }
    });

    let res = probe_http(addr, "/healthz", None, None, &[200], Duration::from_secs(1)).await;
    assert!(res.is_ok(), "IPv6 probe should succeed, got {res:?}");
}

/// A Service carrying an `upstreamAuthority` policy resolves its probe host
/// to `healthCheckHost` (see `manager::health_check_loop`); this test verifies
/// the request builder in `probes.rs` actually sends it as the `Host` header
/// once the override reaches `probe_http`.
#[tokio::test]
async fn test_http_probe_sends_authority_health_check_host() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut req_buf = [0_u8; 1024];
            let n = stream.read(&mut req_buf).await.unwrap_or(0);
            let _ = tx.send(req_buf[..n].to_vec());
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        }
    });

    let res = probe_http(
        addr,
        "/healthz",
        None,
        Some("probe.sandbox.internal"),
        &[200],
        Duration::from_secs(1),
    )
    .await;
    assert!(res.is_ok(), "probe should succeed, got {res:?}");

    let req_bytes = rx.await.expect("request must be captured");
    let req = String::from_utf8_lossy(&req_bytes).to_ascii_lowercase();
    assert!(
        req.contains("host: probe.sandbox.internal"),
        "expected Host override in request, got: {req}"
    );
}

/// A Service with no `upstreamAuthority` policy passes `host: None` all the
/// way through, so the probe is byte-for-byte unchanged from today: the
/// `Host` header falls back to reqwest's default (the dialed socket address).
#[tokio::test]
async fn test_http_probe_host_header_unchanged_without_authority() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut req_buf = [0_u8; 1024];
            let n = stream.read(&mut req_buf).await.unwrap_or(0);
            let _ = tx.send(req_buf[..n].to_vec());
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        }
    });

    let res = probe_http(addr, "/healthz", None, None, &[200], Duration::from_secs(1)).await;
    assert!(res.is_ok(), "probe should succeed, got {res:?}");

    let req_bytes = rx.await.expect("request must be captured");
    let req = String::from_utf8_lossy(&req_bytes).to_ascii_lowercase();
    let expected_host = format!("host: {addr}");
    assert!(
        req.contains(&expected_host),
        "expected default socket-address Host header, got: {req}"
    );
}

#[tokio::test]
async fn test_grpc_probe_ipv6_does_not_panic() {
    // No gRPC server is listening; the probe must return an error rather than panic.
    // No working IPv6 stack required: the URI must parse (bracketed `::1`) and the
    // failure must come from the network layer, not URI construction.
    let addr: SocketAddr = "[::1]:1".parse().expect("parse ipv6 addr");
    let res = probe_grpc(addr, None, "", Duration::from_millis(300)).await;
    assert!(matches!(
        res,
        Err(ProbeError::GrpcConnectionFailed(_)) | Err(ProbeError::Timeout)
    ));
}

#[test]
fn test_grpc_call_error_class_does_not_expose_status_message() {
    const SENTINEL: &str = "sentinel tonic status detail";
    let err = ProbeError::GrpcCallFailed(Box::new(tonic::Status::internal(SENTINEL)));

    assert_eq!(err.class(), "grpc-call-failed");
    assert!(!err.class().contains(SENTINEL));
}
