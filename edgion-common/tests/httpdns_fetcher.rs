// Standalone transport fixture: this tests the Common HTTP client without a
// Gateway/Controller topology and does not start or restart shared services.
use edgion_common::dns::http_fetcher::ReqwestHttpDnsFetcher;
use edgion_common::dns::httpdns::HttpDnsFetcher;
use edgion_common::dns::DnsError;
use edgion_resources::resources::link_sys::httpdns::HttpDnsConnection;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Spawn a one-shot HTTP/1.1 stub that replies with `body` and returns its URL.
async fn spawn_stub(body: String) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok(Ok((mut sock, _))) = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await {
            let mut scratch = [0u8; 1024];
            let _ = tokio::time::timeout(Duration::from_secs(2), sock.read(&mut scratch)).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = tokio::time::timeout(Duration::from_secs(2), sock.write_all(resp.as_bytes())).await;
        }
    });
    format!("http://{addr}/d")
}

fn local_connection() -> HttpDnsConnection {
    HttpDnsConnection {
        allow_loopback: true,
        timeout_ms: Some(1000),
        tls: None,
    }
}

#[tokio::test]
async fn production_default_rejects_loopback_literals_and_hostname_before_connect() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let fetcher = ReqwestHttpDnsFetcher::new(None).unwrap();
    for host in [
        "127.0.0.1",
        "127.1",
        "2130706433",
        "[::1]",
        "[::ffff:127.0.0.1]",
        "localhost",
    ] {
        let error = tokio::time::timeout(Duration::from_secs(4), fetcher.get(&format!("http://{host}:{port}/d")))
            .await
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, DnsError::Http(_)), "{host}: {error:?}");
        if host != "localhost" {
            assert!(matches!(error, DnsError::Http(message) if message == "HTTP DNS endpoint blocked: loopback"));
        }
    }
    assert!(tokio::time::timeout(Duration::from_millis(50), listener.accept())
        .await
        .is_err());
}

#[tokio::test]
async fn explicit_loopback_allows_hostname_and_revocation_blocks_new_generation() {
    let url = spawn_stub("answer".into()).await.replace("127.0.0.1", "localhost");
    let mut connection = local_connection();
    let allowed = ReqwestHttpDnsFetcher::new(Some(&connection)).unwrap();
    assert_eq!(allowed.get(&url).await.unwrap(), "answer");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://localhost:{}/d", listener.local_addr().unwrap().port());
    connection.allow_loopback = false;
    let denied = ReqwestHttpDnsFetcher::new(Some(&connection)).unwrap();
    assert!(denied.get(&url).await.is_err());
    assert!(tokio::time::timeout(Duration::from_millis(50), listener.accept())
        .await
        .is_err());
}

#[tokio::test]
async fn loopback_override_never_allows_metadata_or_unspecified_literals() {
    for allow_loopback in [false, true] {
        let connection = HttpDnsConnection {
            allow_loopback,
            ..local_connection()
        };
        let fetcher = ReqwestHttpDnsFetcher::new(Some(&connection)).unwrap();
        for host in [
            "169.254.169.254",
            "[fe80::1]",
            "[::ffff:169.254.169.254]",
            "0.0.0.0",
            "[::]",
            "[::ffff:0.0.0.0]",
        ] {
            let error = fetcher
                .get(&format!("http://{host}/d?secret=hidden"))
                .await
                .unwrap_err();
            assert!(
                matches!(&error, DnsError::Http(message) if message == "HTTP DNS endpoint blocked: link-local or unspecified"),
                "{host}: {error:?}"
            );
        }
    }
}

#[tokio::test]
async fn blocked_endpoint_preserves_configured_fallback() {
    use edgion_common::dns::{resolve, ResolverSpec};
    use edgion_resources::resources::link_sys::httpdns::{
        HttpDnsFallback, HttpDnsResolverConfig, HttpDnsResponseFormat,
    };
    let mut config = HttpDnsResolverConfig {
        preset: None,
        url_template: Some("http://169.254.169.254/d?host={domain}".into()),
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::None,
        connection: None,
    };
    assert!(resolve(&ResolverSpec::HttpDns(Box::new(config.clone())), "localhost")
        .await
        .is_err());
    config.fallback = HttpDnsFallback::System;
    let addresses = resolve(&ResolverSpec::HttpDns(Box::new(config)), "localhost")
        .await
        .unwrap();
    assert!(addresses.iter().any(|ip| ip.is_loopback()));
}

#[tokio::test]
async fn redirects_are_not_followed_and_timeout_still_applies() {
    let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/d", redirect.local_addr().unwrap());
    let target_addr = target.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), redirect.accept())
            .await
            .unwrap()
            .unwrap();
        let mut request = [0; 1024];
        tokio::time::timeout(Duration::from_secs(2), socket.read(&mut request))
            .await
            .unwrap()
            .unwrap();
        let reply = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/d\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        tokio::time::timeout(Duration::from_secs(2), socket.write_all(reply.as_bytes()))
            .await
            .unwrap()
            .unwrap();
    });
    let fetcher = ReqwestHttpDnsFetcher::new(Some(&local_connection())).unwrap();
    assert!(matches!(fetcher.get(&url).await, Err(DnsError::Http(message)) if message.contains("302")));
    server.await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(50), target.accept())
        .await
        .is_err());
    let connection = HttpDnsConnection {
        timeout_ms: Some(50),
        ..local_connection()
    };
    let fetcher = ReqwestHttpDnsFetcher::new(Some(&connection)).unwrap();
    let url = format!("http://{target_addr}/d");
    assert!(tokio::time::timeout(Duration::from_secs(2), fetcher.get(&url))
        .await
        .unwrap()
        .is_err());
}

#[tokio::test]
async fn fetches_body_from_stub() {
    let url = spawn_stub(r#"{"ips":["1.2.3.4"]}"#.to_string()).await;
    let fetcher = ReqwestHttpDnsFetcher::new(Some(&local_connection())).unwrap();
    let body = fetcher.get(&url).await.unwrap();
    assert!(body.contains("1.2.3.4"));
}

#[tokio::test]
async fn rejects_body_over_limit() {
    let url = spawn_stub("x".repeat(64 * 1024 + 1)).await;
    let fetcher = ReqwestHttpDnsFetcher::new(Some(&local_connection())).unwrap();
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
        connection: Some(local_connection()),
    };
    let fetcher = ReqwestHttpDnsFetcher::new(cfg.connection.as_ref()).unwrap();
    let ips = edgion_common::dns::httpdns::resolve_http_dns_with(&fetcher, &cfg, "api.example.com")
        .await
        .unwrap();
    assert_eq!(ips.len(), 2);
}
