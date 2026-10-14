use super::*;
use crate::backends::degrade_threshold::GLOBAL_STORE_TEST_LOCK;
use edgion_resources::sensitive::{SensitiveMode, SENSITIVE_MODE};
use edgion_resources::EdgionGatewayConfig;
use pingora_core::apps::{HttpPersistentSettings, ReusedHttpStream};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn assert_sensitive_log_in_subprocess(test_name: &str, assertion: impl FnOnce()) {
    const CHILD_ENV: &str = "EDGION_REDIRECT_SENSITIVE_LOG_CHILD";
    const TEST_MODULE: &str = "routes::http::redirect_http::tests";
    crate::test_subprocess::assert_in_subprocess(TEST_MODULE, test_name, CHILD_ENV, test_name, assertion);
}

fn redirect_log_json(error: Option<String>) -> serde_json::Value {
    serde_json::to_value(RedirectLogEntry {
        ts: 0,
        log_type: "redirect",
        request_info: RedirectRequestInfo {
            direct_peer_ip: "192.0.2.1",
            remote_ip: "198.51.100.2",
            host: "example.com",
            path: "/account/reset",
            status: 301,
        },
        redirect_url: "https://example.com/account/reset",
        latency_ms: 1,
        error,
    })
    .unwrap()
}

fn admit_redirect_gateway(hostname: Option<&str>) {
    let gateway: edgion_resources::Gateway = crate::test_current_status::from_value(serde_json::json!({
        "metadata": {"name": "redirect", "namespace": "test"},
        "spec": {"gatewayClassName": "shared", "listeners": [{"name": "http", "port": 8080, "protocol": "HTTP", "hostname": hostname}]}
    })).unwrap();
    crate::runtime::store::listener_gateway_info::rebuild_listener_gateway_infos(&[gateway]);
}

fn test_access_logger() -> Arc<AccessLogger> {
    Arc::new(AccessLogger::new())
}

async fn redirect_listener_wire_request_count(
    redirect: &EdgionHttpRedirectProxy,
    request_limit: u32,
    request_cap: usize,
) -> (usize, String) {
    let (mut client, server) = tokio::io::duplex(8192);
    let mut next_stream: pingora_core::protocols::Stream = Box::new(server);
    let mut persistent_settings: Option<HttpPersistentSettings> = None;
    let mut served = 0;

    for request_index in 0..request_cap {
        let client_closes = request_limit == 0 && request_index + 1 == request_cap;
        let connection = if client_closes { "Connection: close\r\n" } else { "" };
        client
            .write_all(format!("GET /{request_index} HTTP/1.1\r\nHost: example.com\r\n{connection}\r\n").as_bytes())
            .await
            .unwrap();

        let mut session = Session::new_h1(next_stream);
        if let Some(settings) = persistent_settings.take() {
            settings.apply_to_session(&mut session.downstream_session);
        }
        assert!(session.read_request().await.unwrap());

        let previous = session.downstream_session.take_connection_user_context();
        let mut ctx = redirect.new_ctx();
        if let Some(previous) = previous {
            redirect.on_connection_reuse(&mut session, &mut ctx, previous);
        }
        redirect.early_request_filter(&mut session, &mut ctx).await.unwrap();
        assert!(redirect.request_filter(&mut session, &mut ctx).await.unwrap());

        let mut settings = HttpPersistentSettings::for_session(&session);
        if let Some(user_context) = redirect.persist_connection_context(&session, &ctx) {
            settings.set_user_context(user_context);
        }
        let reusable = session.downstream_session.finish().await.unwrap();
        served += 1;
        let Some(reusable) = reusable else {
            break;
        };
        let (stream, carried) = ReusedHttpStream::from_reusable_stream(reusable, settings).consume();
        next_stream = stream;
        persistent_settings = carried;
    }

    let mut wire = String::new();
    client.read_to_string(&mut wire).await.unwrap();
    (served, wire)
}

#[test]
fn test_redirect_creation() {
    let redirect = EdgionHttpRedirectProxy::new(
        443,
        test_access_logger(),
        crate::config::bind_gateway_config(None),
        edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
    );
    assert_eq!(redirect.https_port, 443);

    let redirect = EdgionHttpRedirectProxy::new(
        8443,
        test_access_logger(),
        crate::config::bind_gateway_config(None),
        edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
    );
    assert_eq!(redirect.https_port, 8443);
}

#[test]
fn test_build_redirect_url_default_port() {
    let redirect = EdgionHttpRedirectProxy::new(
        443,
        test_access_logger(),
        crate::config::bind_gateway_config(None),
        edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
    );

    // Simple path
    assert_eq!(
        redirect.build_redirect_url_from_parts("example.com", "/"),
        "https://example.com/"
    );

    // Path with query string
    assert_eq!(
        redirect.build_redirect_url_from_parts("example.com", "/api/users?page=1&limit=10"),
        "https://example.com/api/users?page=1&limit=10"
    );

    // Host with port should strip the port
    assert_eq!(
        redirect.build_redirect_url_from_parts("example.com:80", "/path"),
        "https://example.com/path"
    );
}

#[test]
fn test_build_redirect_url_custom_port() {
    let redirect = EdgionHttpRedirectProxy::new(
        8443,
        test_access_logger(),
        crate::config::bind_gateway_config(None),
        edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
    );

    // Simple path
    assert_eq!(
        redirect.build_redirect_url_from_parts("example.com", "/"),
        "https://example.com:8443/"
    );

    // Path with query string
    assert_eq!(
        redirect.build_redirect_url_from_parts("example.com", "/api/users?page=1"),
        "https://example.com:8443/api/users?page=1"
    );

    // Host with port should be replaced with custom port
    assert_eq!(
        redirect.build_redirect_url_from_parts("example.com:8080", "/path"),
        "https://example.com:8443/path"
    );
}

#[test]
fn test_build_redirect_url_special_cases() {
    let redirect = EdgionHttpRedirectProxy::new(
        443,
        test_access_logger(),
        crate::config::bind_gateway_config(None),
        edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
    );

    // IPv4 address
    assert_eq!(
        redirect.build_redirect_url_from_parts("192.168.1.1", "/"),
        "https://192.168.1.1/"
    );

    // IPv4 with port
    assert_eq!(
        redirect.build_redirect_url_from_parts("192.168.1.1:8080", "/api"),
        "https://192.168.1.1/api"
    );

    // Subdomain
    assert_eq!(
        redirect.build_redirect_url_from_parts("api.example.com", "/v1/users"),
        "https://api.example.com/v1/users"
    );

    // Complex query string
    assert_eq!(
        redirect.build_redirect_url_from_parts("example.com", "/search?q=hello+world&lang=zh"),
        "https://example.com/search?q=hello+world&lang=zh"
    );

    // Path with special characters
    assert_eq!(
        redirect.build_redirect_url_from_parts("example.com", "/path/to/%C3%A9%C3%B1"),
        "https://example.com/path/to/%C3%A9%C3%B1"
    );
}

#[test]
fn test_host_is_missing() {
    // Missing / empty / whitespace-only Host must be treated as absent (→ 400).
    assert!(EdgionHttpRedirectProxy::host_is_missing(""));
    assert!(EdgionHttpRedirectProxy::host_is_missing("   "));
    assert!(EdgionHttpRedirectProxy::host_is_missing("\t"));
    // A bare `:port` with no authority collapses to empty after port strip.
    assert!(EdgionHttpRedirectProxy::host_is_missing(":8080"));

    // Valid authorities are accepted (→ 301).
    assert!(!EdgionHttpRedirectProxy::host_is_missing("example.com"));
    assert!(!EdgionHttpRedirectProxy::host_is_missing("example.com:80"));
    assert!(!EdgionHttpRedirectProxy::host_is_missing("192.168.1.1"));
    // IPv6 literal (bracketed) with and without port.
    assert!(!EdgionHttpRedirectProxy::host_is_missing("[::1]"));
    assert!(!EdgionHttpRedirectProxy::host_is_missing("[2001:db8::1]:8443"));
}

#[test]
fn test_new_ctx() {
    let redirect = EdgionHttpRedirectProxy::new(
        443,
        test_access_logger(),
        crate::config::bind_gateway_config(None),
        edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
    );
    let _ctx = redirect.new_ctx(); // Should not panic
}

#[test]
fn redirect_log_sensitive_fields_follow_the_shared_masking_contract() {
    assert_sensitive_log_in_subprocess(
        "redirect_log_sensitive_fields_follow_the_shared_masking_contract",
        || {
            for (mode, expected_path, expected_error) in [
                (SensitiveMode::Replace, "***", "***"),
                (SensitiveMode::Wrap, "<E>/account/reset</E>", "<E>private failure</E>"),
                (SensitiveMode::Off, "/account/reset", "private failure"),
            ] {
                SENSITIVE_MODE.store(mode as u8, Ordering::SeqCst);
                let value = redirect_log_json(Some("private failure".to_string()));
                assert_eq!(value["request_info"]["path"], expected_path);
                assert_eq!(
                    value["redirect_url"],
                    match mode {
                        SensitiveMode::Replace => "***",
                        SensitiveMode::Wrap => "<E>https://example.com/account/reset</E>",
                        SensitiveMode::Off => "https://example.com/account/reset",
                    }
                );
                assert_eq!(value["error"], expected_error);
            }
        },
    );
}

#[test]
fn absent_redirect_error_stays_null_under_masking() {
    assert_sensitive_log_in_subprocess("absent_redirect_error_stays_null_under_masking", || {
        SENSITIVE_MODE.store(SensitiveMode::Replace as u8, Ordering::SeqCst);
        assert!(redirect_log_json(None)["error"].is_null());
    });
}

#[tokio::test]
async fn redirect_connection_policy_matches_wire_and_stream_reuse() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    admit_redirect_gateway(None);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        for (host_headers, expected_status) in [
            ("Host: example.com\r\n", 301),
            ("Host: example.com\r\nHost: other.example.com\r\n", 400),
            ("Host: :8080\r\n", 400),
        ] {
            for client_close in [false, true] {
                let redirect = EdgionHttpRedirectProxy::new(
                    443,
                    test_access_logger(),
                    crate::config::bind_gateway_config(None),
                    edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
                );
                let (mut client, server) = tokio::io::duplex(4096);
                let connection = if client_close { "Connection: close\r\n" } else { "" };
                client
                    .write_all(format!("GET /first HTTP/1.1\r\n{host_headers}{connection}\r\n").as_bytes())
                    .await
                    .unwrap();
                let mut session = Session::new_h1(Box::new(server));
                if host_headers.matches("Host:").count() > 1 {
                    // Pingora 0.9 rejects malformed authorities before the
                    // redirect service receives a request session.
                    assert!(session.read_request().await.is_err());
                    continue;
                }
                assert!(session.read_request().await.unwrap());
                let mut ctx = redirect.new_ctx();
                redirect.early_request_filter(&mut session, &mut ctx).await.unwrap();
                assert!(redirect.request_filter(&mut session, &mut ctx).await.unwrap());
                assert_eq!(ctx.status, expected_status);

                let reusable = session.downstream_session.finish().await.unwrap();
                assert_eq!(reusable.is_some(), !client_close);
                let mut wire = Vec::new();
                while !wire.ends_with(b"\r\n\r\n") {
                    wire.push(client.read_u8().await.unwrap());
                }
                let wire = String::from_utf8(wire).unwrap();
                assert!(wire.starts_with(&format!("HTTP/1.1 {expected_status}")));
                let expected_connection = if client_close { "close" } else { "keep-alive" };
                assert!(wire
                    .to_ascii_lowercase()
                    .contains(&format!("\r\nconnection: {expected_connection}\r\n")));
                assert!(wire.to_ascii_lowercase().contains("\r\ncontent-length: 0\r\n"));

                if let Some(reusable) = reusable {
                    // Send only after finishing the first request: this tests reuse,
                    // without relying on pipelining or dropping the live session.
                    let (stream, prefix) = reusable.into_parts();
                    assert!(prefix.is_none());
                    client
                        .write_all(b"GET /second HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
                        .await
                        .unwrap();
                    let mut session = Session::new_h1(stream);
                    assert!(session.read_request().await.unwrap());
                    let mut ctx = redirect.new_ctx();
                    assert!(redirect.request_filter(&mut session, &mut ctx).await.unwrap());
                    assert!(session.downstream_session.finish().await.unwrap().is_none());
                    let mut second_wire = String::new();
                    client.read_to_string(&mut second_wire).await.unwrap();
                    assert!(second_wire.starts_with("HTTP/1.1 301"));
                    assert!(second_wire.contains("https://example.com/second"));
                    assert!(second_wire.to_ascii_lowercase().contains("\r\nconnection: close\r\n"));
                } else {
                    assert_eq!(client.read(&mut [0; 1]).await.unwrap(), 0);
                }
            }
        }
    })
    .await
    .expect("redirect connection-policy exchanges must finish without waiting for peer EOF");
}

#[tokio::test(flavor = "current_thread")]
async fn redirect_listener_enforces_total_request_limit_on_the_wire() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    admit_redirect_gateway(None);
    let handler = crate::config::edgion_gateway::create_edgion_gateway_config_handler();
    let configs = [0_u32, 1, 2]
        .into_iter()
        .map(|limit| {
            let name = format!("redirect-keepalive-{limit}");
            let config = serde_json::from_value::<EdgionGatewayConfig>(serde_json::json!({
                "apiVersion": "edgion.io/v1",
                "kind": "EdgionGatewayConfig",
                "metadata": {"name": name},
                "spec": {"server": {"downstreamKeepaliveRequestLimit": limit}}
            }))
            .unwrap();
            (name, Arc::new(config))
        })
        .collect::<HashMap<_, _>>();
    handler.full_set(&configs);

    for (limit, request_cap, expected_served, expected_keepalive) in [(0, 3, 3, 2), (1, 2, 1, 0), (2, 3, 2, 1)] {
        let redirect = EdgionHttpRedirectProxy::new(
            443,
            test_access_logger(),
            crate::config::bind_gateway_config(Some(&format!("redirect-keepalive-{limit}"))),
            edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
        );
        let (served, wire) = redirect_listener_wire_request_count(&redirect, limit, request_cap).await;
        let wire_lower = wire.to_ascii_lowercase();
        assert_eq!(served, expected_served, "request_limit={limit}");
        assert_eq!(
            wire.matches("HTTP/1.1 301").count(),
            expected_served,
            "request_limit={limit}, wire={wire:?}"
        );
        assert_eq!(
            wire_lower.matches("\r\nconnection: keep-alive\r\n").count(),
            expected_keepalive,
            "request_limit={limit}, wire={wire:?}"
        );
        assert_eq!(
            wire_lower.matches("\r\nconnection: close\r\n").count(),
            1,
            "request_limit={limit}, wire={wire:?}"
        );
    }

    handler.full_set(&HashMap::new());
}

#[tokio::test]
async fn redirect_duplicate_host_is_rejected_by_protocol_validation() {
    let _redirect = EdgionHttpRedirectProxy::new(
        443,
        test_access_logger(),
        crate::config::bind_gateway_config(None),
        edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
    );
    let request = b"GET /path?query=1 HTTP/1.1\r\nHost: first.test\r\nHost: second.test\r\nConnection: close\r\n\r\n";
    let (mut client, server) = tokio::io::duplex(4096);
    client.write_all(request).await.unwrap();
    let mut session = Session::new_h1(Box::new(server));
    assert!(session.read_request().await.is_err());
}

#[tokio::test]
async fn redirect_wire_preserves_query_percent_encoding_ipv6_port_and_client_requested_close() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    admit_redirect_gateway(None);
    let redirect = EdgionHttpRedirectProxy::new(
        8443,
        test_access_logger(),
        crate::config::bind_gateway_config(None),
        edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
    );
    let (mut client, server) = tokio::io::duplex(4096);
    // Pingora derives the response Connection header from session keepalive;
    // exercise the client-requested close branch without changing that policy.
    client
        .write_all(
            b"GET /account%20reset?token=secret HTTP/1.1\r\nHost: [2001:db8::1]:8080\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    assert!(session.read_request().await.unwrap());
    let mut ctx = redirect.new_ctx();

    assert!(redirect.request_filter(&mut session, &mut ctx).await.unwrap());
    assert_eq!(ctx.path, "/account%20reset");
    assert_eq!(ctx.redirect_url, "https://[2001:db8::1]:8443/account%20reset");
    drop(session);

    let mut wire = Vec::new();
    client.read_to_end(&mut wire).await.unwrap();
    let wire = String::from_utf8(wire).unwrap();
    assert!(wire.starts_with("HTTP/1.1 301"));
    let header = |name: &str| {
        wire.split("\r\n")
            .filter_map(|line| line.split_once(": "))
            .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value))
    };
    assert_eq!(
        header("Location"),
        Some("https://[2001:db8::1]:8443/account%20reset?token=secret")
    );
    assert_eq!(header("Content-Length"), Some("0"));
    assert_eq!(header("Connection"), Some("close"));
    assert!(wire.ends_with("\r\n\r\n"));
    assert!(!ctx.path.contains("token="));
    assert!(!ctx.redirect_url.contains("token="));
}

#[tokio::test]
async fn redirect_rejected_or_unowned_hostname_returns_wire_404() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    admit_redirect_gateway(Some("allowed.example.com"));
    let redirect = EdgionHttpRedirectProxy::new(
        443,
        test_access_logger(),
        crate::config::bind_gateway_config(None),
        edgion_resources::ListenerKey("0.0.0.0:8080/HTTP".into()),
    );
    assert!(redirect.hostname_is_admitted("ALLOWED.EXAMPLE.COM:8080"));
    for clear_owner in [false, true] {
        if clear_owner {
            crate::runtime::store::listener_gateway_info::rebuild_listener_gateway_infos(&[]);
        }
        let host = if clear_owner {
            "allowed.example.com"
        } else {
            "unowned.example.com"
        };
        let (mut client, server) = tokio::io::duplex(4096);
        client
            .write_all(format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut session = Session::new_h1(Box::new(server));
        assert!(session.read_request().await.unwrap());
        let mut ctx = redirect.new_ctx();
        assert!(redirect.request_filter(&mut session, &mut ctx).await.unwrap());
        assert_eq!(ctx.status, 404);
        drop(session);
        let mut wire = String::new();
        client.read_to_string(&mut wire).await.unwrap();
        assert!(wire.starts_with("HTTP/1.1 404"));
        assert!(!wire.to_ascii_lowercase().contains("location:"));
    }
}
