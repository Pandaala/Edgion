// Standalone transport fixture: this tests the Common HTTP client without a
// Gateway/Controller topology and does not start or restart shared services.
use edgion_common::dns::http_fetcher::ReqwestHttpDnsFetcher;
use edgion_common::dns::httpdns::HttpDnsFetcher;
use edgion_common::dns::DnsError;
use edgion_resources::resources::link_sys::httpdns::HttpDnsConnection;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn owner(
    config: edgion_resources::resources::link_sys::HttpDnsResolverConfig,
) -> std::sync::Arc<edgion_resources::LinkSys> {
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    let mut owner = edgion_resources::LinkSys::new(
        "resolver",
        LinkSysSpec {
            config: SystemConfig::HttpDns(config),
            current_status: None,
            unknown_fields: vec![],
        },
    );
    owner.metadata.namespace = Some("apps".into());
    std::sync::Arc::new(owner)
}
fn new_fetcher(connection: Option<&HttpDnsConnection>) -> Result<ReqwestHttpDnsFetcher, DnsError> {
    use edgion_resources::resources::link_sys::httpdns::{
        HttpDnsFallback, HttpDnsPreset, HttpDnsResolverConfig, HttpDnsResponseFormat,
    };
    ReqwestHttpDnsFetcher::new(owner(HttpDnsResolverConfig {
        preset: Some(HttpDnsPreset::Aliyun),
        url_template: None,
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::None,
        connection: connection.cloned(),
    }))
}
fn prepare(
    config: edgion_resources::resources::link_sys::HttpDnsResolverConfig,
) -> std::sync::Arc<edgion_common::dns::PreparedHttpDns> {
    edgion_common::dns::HttpDnsCandidate::from_link_sys(owner(config))
        .unwrap()
        .prepare()
}

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
        ..Default::default()
    }
}

#[tokio::test]
async fn production_default_rejects_loopback_literals_and_hostname_before_connect() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let fetcher = new_fetcher(None).unwrap();
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
    let allowed = new_fetcher(Some(&connection)).unwrap();
    assert_eq!(allowed.get(&url).await.unwrap(), "answer");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://localhost:{}/d", listener.local_addr().unwrap().port());
    connection.allow_loopback = false;
    let denied = new_fetcher(Some(&connection)).unwrap();
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
        let fetcher = new_fetcher(Some(&connection)).unwrap();
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
    assert!(resolve(&ResolverSpec::HttpDns(prepare(config.clone())), "localhost")
        .await
        .is_err());
    config.fallback = HttpDnsFallback::System;
    let addresses = resolve(&ResolverSpec::HttpDns(prepare(config)), "localhost")
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
    let fetcher = new_fetcher(Some(&local_connection())).unwrap();
    assert!(matches!(fetcher.get(&url).await, Err(DnsError::Http(message)) if message.contains("302")));
    server.await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(50), target.accept())
        .await
        .is_err());
    let connection = HttpDnsConnection {
        timeout_ms: Some(50),
        ..local_connection()
    };
    let fetcher = new_fetcher(Some(&connection)).unwrap();
    let url = format!("http://{target_addr}/d");
    assert!(tokio::time::timeout(Duration::from_secs(2), fetcher.get(&url))
        .await
        .unwrap()
        .is_err());
}

#[tokio::test]
async fn fetches_body_from_stub() {
    let url = spawn_stub(r#"{"ips":["1.2.3.4"]}"#.to_string()).await;
    let fetcher = new_fetcher(Some(&local_connection())).unwrap();
    let body = fetcher.get(&url).await.unwrap();
    assert!(body.contains("1.2.3.4"));
}

#[tokio::test]
async fn rejects_body_over_limit() {
    let url = spawn_stub("x".repeat(64 * 1024 + 1)).await;
    let fetcher = new_fetcher(Some(&local_connection())).unwrap();
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
    let fetcher = new_fetcher(cfg.connection.as_ref()).unwrap();
    let ips = edgion_common::dns::httpdns::resolve_http_dns_with(&fetcher, &cfg, "api.example.com")
        .await
        .unwrap();
    assert_eq!(ips.len(), 2);
}

#[tokio::test]
async fn configured_body_limit_and_runtime_validation_are_enforced() {
    let connection = HttpDnsConnection {
        max_response_bytes: Some(4),
        ..local_connection()
    };
    let url = spawn_stub("12345".into()).await;
    let fetcher = new_fetcher(Some(&connection)).unwrap();
    assert!(matches!(fetcher.get(&url).await, Err(DnsError::Http(message)) if message.contains("4-byte limit")));
    for input in [
        serde_json::json!({"timeoutMs": 0}),
        serde_json::json!({"timeoutMs": 50, "connectTimeoutMs": 51}),
        serde_json::json!({"poolMaxIdlePerHost": 257}),
        serde_json::json!({"poolIdleTimeoutMs": 0}),
        serde_json::json!({"maxResponseBytes": 65537}),
    ] {
        let connection: HttpDnsConnection = serde_json::from_value(input).unwrap();
        assert!(
            new_fetcher(Some(&connection)).is_err(),
            "runtime must reject admission-bypassed settings"
        );
    }
}

async fn spawn_keepalive_stub() -> (String, tokio::task::JoinHandle<usize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/d?host={{domain}}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut accepts = 0;
        let mut answers = 0;
        while answers < 2 {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
            accepts += 1;
            loop {
                let mut request = [0u8; 2048];
                let bytes = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut request))
                    .await
                    .unwrap()
                    .unwrap();
                if bytes == 0 {
                    break;
                }
                let body = r#"{"ips":["1.2.3.4"]}"#;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
                answers += 1;
                if answers == 2 {
                    return accepts;
                }
            }
        }
        accepts
    });
    (url, server)
}

#[tokio::test]
async fn prepared_queries_reuse_native_pool_and_config_can_disable_it() {
    use edgion_common::dns::{resolve, ResolverSpec};
    use edgion_resources::resources::link_sys::httpdns::HttpDnsResolverConfig;
    for (pool_size, expected_connections) in [(8, 1), (0, 2)] {
        let (url, server) = spawn_keepalive_stub().await;
        let config: HttpDnsResolverConfig = serde_json::from_value(serde_json::json!({
            "urlTemplate": url, "response": {"kind": "json", "ipPath": "ips"},
            "fallback": {"type": "none"},
            "connection": {"allowLoopback": true, "poolMaxIdlePerHost": pool_size}
        }))
        .unwrap();
        let prepared = prepare(config);
        assert!(prepared.is_ready());
        let spec = ResolverSpec::HttpDns(prepared);
        for domain in ["first.example", "second.example"] {
            assert_eq!(
                resolve(&spec, domain).await.unwrap(),
                vec!["1.2.3.4".parse::<std::net::IpAddr>().unwrap()]
            );
        }
        assert_eq!(server.await.unwrap(), expected_connections);
    }
}

#[tokio::test]
async fn configured_connect_deadline_bounds_stalled_tls_handshake() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://{}/d", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 1024];
        assert!(socket.read(&mut bytes).await.unwrap() > 0);
        std::future::pending::<()>().await;
    });
    let connection = HttpDnsConnection {
        timeout_ms: Some(1000),
        connect_timeout_ms: Some(50),
        ..local_connection()
    };
    let fetcher = new_fetcher(Some(&connection)).unwrap();
    let started = std::time::Instant::now();
    assert!(tokio::time::timeout(Duration::from_millis(600), fetcher.get(&url))
        .await
        .expect("connect deadline must precede total timeout")
        .is_err());
    assert!(started.elapsed() < Duration::from_millis(600));
    server.abort();
}

#[tokio::test]
async fn configured_idle_lifetime_releases_pooled_connection() {
    let (url, server) = spawn_keepalive_stub().await;
    let connection = HttpDnsConnection {
        pool_idle_timeout_ms: Some(1),
        ..local_connection()
    };
    let fetcher = new_fetcher(Some(&connection)).unwrap();
    let url = url.replace("{domain}", "example.com");
    fetcher.get(&url).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    fetcher.get(&url).await.unwrap();
    assert_eq!(server.await.unwrap(), 2, "expired idle sockets cannot be reused");
}

#[tokio::test]
async fn cancelled_query_does_not_poison_shared_prepared_generation() {
    use edgion_common::dns::{resolve, ResolverSpec};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/d?host={{domain}}", listener.local_addr().unwrap());
    let (started, request_started) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let mut scratch = [0; 2048];
        assert!(first.read(&mut scratch).await.unwrap() > 0);
        started.send(()).unwrap();
        // The first request never receives headers. Cancellation must release
        // its exchange while allowing another query on the same client.
        let (mut second, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        assert!(second.read(&mut scratch).await.unwrap() > 0);
        let body = r#"{"ips":["1.2.3.4"]}"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        second.write_all(reply.as_bytes()).await.unwrap();
    });
    let config = serde_json::from_value(serde_json::json!({
        "urlTemplate": url, "response": {"ipPath": "ips"}, "fallback": {"type": "none"},
        "connection": {"allowLoopback": true, "timeoutMs": 2000}
    }))
    .unwrap();
    let prepared = prepare(config);
    let first_spec = ResolverSpec::HttpDns(prepared.clone());
    let first = tokio::spawn(async move { resolve(&first_spec, "first.example").await });
    tokio::time::timeout(Duration::from_secs(2), request_started)
        .await
        .unwrap()
        .unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(prepared.is_ready());
    assert_eq!(
        resolve(&ResolverSpec::HttpDns(prepared), "second.example")
            .await
            .unwrap()
            .len(),
        1
    );
    server.await.unwrap();
}

#[tokio::test]
async fn invalid_preparation_uses_the_latest_explicit_dns_fallback() {
    use edgion_common::dns::{resolve, ResolverSpec};
    use hickory_resolver::proto::{
        op::Message,
        rr::{rdata::A, RData, Record, RecordType},
    };
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut bytes = [0; 2048];
        loop {
            let (len, peer) = socket.recv_from(&mut bytes).await.unwrap();
            let query = Message::from_vec(&bytes[..len]).unwrap();
            let mut response = Message::response(query.id, query.op_code);
            response.metadata.recursion_available = true;
            response.metadata.authoritative = true;
            for question in &query.queries {
                response.add_query(question.clone());
                if question.query_type() == RecordType::A {
                    response.add_answer(Record::from_rdata(
                        question.name().clone(),
                        30,
                        RData::A(A(std::net::Ipv4Addr::new(1, 2, 3, 4))),
                    ));
                }
            }
            socket.send_to(&response.to_vec().unwrap(), peer).await.unwrap();
        }
    });
    let config = serde_json::from_value(serde_json::json!({
        "preset": "aliyun", "connection": {"timeoutMs": 0},
        "fallback": {"type": "dns", "servers": [address.to_string()]}
    }))
    .unwrap();
    let prepared = prepare(config);
    assert!(!prepared.is_ready());
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        resolve(&ResolverSpec::HttpDns(prepared), "fallback.example"),
    )
    .await;
    server.abort();
    let addresses = result.unwrap().unwrap();
    assert!(
        addresses.contains(&"1.2.3.4".parse().unwrap()),
        "failed preparation must honor configured DNS server rather than System"
    );
}

#[tokio::test]
async fn signed_urls_are_sent_unchanged_but_transport_errors_are_redacted() {
    use edgion_common::dns::{resolve, ResolverSpec};
    use edgion_resources::resources::link_sys::httpdns::{
        HttpDnsFallback, HttpDnsResolverConfig, HttpDnsResponseFormat,
    };
    for body_error in [false, true] {
        for system_fallback in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!(
                "http://{}/PRIVATE_PATH/d?host={{domain}}&signature=PRIVATE_SIGNATURE",
                listener.local_addr().unwrap()
            );
            let server = tokio::spawn(async move {
                let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let request = tokio::time::timeout(Duration::from_secs(2), async {
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        let read = socket.read(&mut chunk).await.unwrap();
                        assert!(read > 0 && request.len() + read <= 8192);
                        request.extend_from_slice(&chunk[..read]);
                    }
                    request
                })
                .await
                .unwrap();
                let response = if body_error {
                    "HTTP/1.1 200 OK\r\nContent-Length: 32\r\nConnection: close\r\n\r\nshort"
                } else {
                    "invalid HTTP response\r\n\r\n"
                };
                tokio::time::timeout(Duration::from_secs(2), socket.write_all(response.as_bytes()))
                    .await
                    .unwrap()
                    .unwrap();
                request
            });
            let config = HttpDnsResolverConfig {
                preset: None,
                url_template: Some(url.clone()),
                response: HttpDnsResponseFormat::default(),
                fallback: if system_fallback {
                    HttpDnsFallback::System
                } else {
                    HttpDnsFallback::None
                },
                connection: Some(local_connection()),
            };
            let result = resolve(&ResolverSpec::HttpDns(prepare(config)), "localhost").await;
            let request = String::from_utf8(server.await.unwrap()).unwrap();
            assert_eq!(
                request.lines().next().unwrap(),
                "GET /PRIVATE_PATH/d?host=localhost&signature=PRIVATE_SIGNATURE HTTP/1.1"
            );
            if system_fallback {
                assert!(result.unwrap().iter().any(|ip| ip.is_loopback()));
            } else {
                let error = result.unwrap_err();
                let expected = if body_error {
                    "error decoding response body"
                } else {
                    "error sending request"
                };
                assert!(
                    matches!(&error, DnsError::Http(message) if message == expected),
                    "{error}"
                );
                let formatted = error.to_string();
                assert_eq!(formatted, format!("http dns error: {expected}"));
                for private in [url.as_str(), "PRIVATE_PATH", "PRIVATE_SIGNATURE"] {
                    assert!(!formatted.contains(private));
                }
            }
        }
    }
}

#[tokio::test]
async fn response_metadata_changes_with_generation_while_borrowed_generation_survives() {
    use edgion_common::dns::{resolve, HttpDnsCandidate, ResolverSpec};
    use edgion_resources::resources::link_sys::HttpDnsResolverConfig;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/d?host={{domain}}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = [0u8; 1024];
            assert!(
                tokio::time::timeout(Duration::from_secs(2), socket.read(&mut request))
                    .await
                    .unwrap()
                    .unwrap()
                    > 0
            );
            let body = r#"{"old":["192.0.2.1"],"new":["192.0.2.2"]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            tokio::time::timeout(Duration::from_secs(2), socket.write_all(reply.as_bytes()))
                .await
                .unwrap()
                .unwrap();
        }
    });
    let mut config: HttpDnsResolverConfig = serde_json::from_value(serde_json::json!({
        "urlTemplate": url, "response": {"kind": "json", "ipPath": "old"},
        "fallback": {"type": "none"}, "connection": {"allowLoopback": true}
    }))
    .unwrap();
    let old = prepare(config.clone());
    assert!(old.is_ready());
    assert!(HttpDnsCandidate::from_link_sys(owner(config.clone()))
        .unwrap()
        .matches(&old));
    config.response.ip_path = Some("new".into());
    let candidate = HttpDnsCandidate::from_link_sys(owner(config)).unwrap();
    assert!(!candidate.matches(&old));
    let new = candidate.prepare();
    assert!(new.is_ready());
    for (generation, expected) in [(&old, "192.0.2.1"), (&new, "192.0.2.2"), (&old, "192.0.2.1")] {
        assert_eq!(
            resolve(&ResolverSpec::HttpDns(generation.clone()), "example.com")
                .await
                .unwrap(),
            vec![expected.parse::<std::net::IpAddr>().unwrap()]
        );
    }
    server.await.unwrap();
}
