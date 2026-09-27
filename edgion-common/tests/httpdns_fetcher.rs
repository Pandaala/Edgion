// Standalone transport fixture: this tests the Common HTTP client without a
// Gateway/Controller topology and does not start or restart shared services.
use edgion_common::dns::http_fetcher::ReqwestHttpDnsFetcher;
use edgion_common::dns::httpdns::HttpDnsFetcher;
use edgion_common::dns::DnsError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Spawn a one-shot HTTP/1.1 stub that replies with `body` and returns its URL.
async fn spawn_stub(body: String) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut scratch = [0u8; 1024];
            let _ = sock.read(&mut scratch).await; // drain the request line/headers
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    format!("http://{addr}/d")
}

#[tokio::test]
async fn fetches_body_from_stub() {
    let url = spawn_stub(r#"{"ips":["1.2.3.4"]}"#.to_string()).await;
    let fetcher = ReqwestHttpDnsFetcher::new(None).unwrap();
    let body = fetcher.get(&url).await.unwrap();
    assert!(body.contains("1.2.3.4"));
}

#[tokio::test]
async fn rejects_body_over_limit() {
    let url = spawn_stub("x".repeat(64 * 1024 + 1)).await;
    let fetcher = ReqwestHttpDnsFetcher::new(None).unwrap();
    let err = fetcher.get(&url).await.unwrap_err();
    assert!(
        matches!(&err, DnsError::Http(message) if message.contains("response exceeds the 65536-byte limit")),
        "expected size-limit error, got {err:?}"
    );
}

#[tokio::test]
async fn end_to_end_preset_parse_via_stub() {
    use edgion_resources::resources::link_sys::httpdns::{
        HttpDnsFallback, HttpDnsResolverConfig, HttpDnsResponseFormat,
    };

    // A generic (non-preset) config pointed at the stub, JSON `ips` shape.
    let url = spawn_stub(r#"{"ips":["93.184.216.34","93.184.216.35"]}"#.to_string()).await;
    let cfg = HttpDnsResolverConfig {
        preset: None,
        url_template: Some(format!("{url}?host={{domain}}")),
        response: HttpDnsResponseFormat {
            kind: edgion_resources::resources::link_sys::httpdns::HttpDnsBodyKind::Json,
            ip_path: Some("ips".to_string()),
            delimiter: None,
            ttl_path: None,
        },
        fallback: HttpDnsFallback::None,
        connection: None,
    };
    let fetcher = ReqwestHttpDnsFetcher::new(None).unwrap();
    let ips = edgion_common::dns::httpdns::resolve_http_dns_with(&fetcher, &cfg, "api.example.com")
        .await
        .unwrap();
    assert_eq!(ips.len(), 2);
}
