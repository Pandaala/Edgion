use super::*;
use edgion_resources::sensitive::{init_sensitive, SensitiveMaskConfig, SensitiveMode};
use pingora_core::upstreams::peer::BasicPeer;
use proxy::TlsUpstreamInfo;
use serde_json::Value;

pub(super) fn assert_log_modes(test_name: &str, listener: impl Fn(&TlsContext) -> Value) {
    const CHILD: &str = "EDGION_TLS_ADDRESS_LOG_CHILD";
    let exact_test_name = test_name.strip_prefix("edgion_gateway::").unwrap();
    let (test_module, test_name) = exact_test_name.rsplit_once("::").unwrap();
    for mode in ["off", "replace", "wrap"] {
        crate::test_subprocess::assert_in_subprocess(test_module, test_name, CHILD, mode, || {
            assert_log_mode(mode, &listener);
        });
    }
}

fn assert_log_mode(mode: &str, listener: &impl Fn(&TlsContext) -> Value) {
    let mode = match mode {
        "off" => SensitiveMode::Off,
        "replace" => SensitiveMode::Replace,
        "wrap" => SensitiveMode::Wrap,
        _ => panic!("unexpected isolated test mode"),
    };
    init_sensitive(SensitiveMaskConfig {
        mode,
        replace_with: "[hidden]".into(),
        wrap_prefix: "<mask>".into(),
        wrap_suffix: "</mask>".into(),
    });
    // SslLogger serializes this handshake metadata directly. Its SNI and
    // nonsensitive diagnostics are intentionally outside address masking.
    let mut meta = crate::TlsConnMeta {
        ts: 0,
        start_at: std::time::Instant::now(),
        handshake_complete_time: Some(1),
        port: Some(443),
        tls_id: None,
        sni: Some("public.example.com".into()),
        log: None,
        matched: None,
        err_log: Some("cert_expired: not_after=Jan 1 00:00:00 2026 GMT".into()),
        client_cert_info: None,
        is_mtls: false,
    };
    let ssl_json = serde_json::to_value(&meta).unwrap();
    assert_eq!(ssl_json["sni"], "public.example.com");
    assert_eq!(ssl_json["err_log"], "cert_expired: not_after=Jan 1 00:00:00 2026 GMT");
    meta.sni = None;
    meta.err_log = None;
    let ssl_json = serde_json::to_value(&meta).unwrap();
    assert!(ssl_json.get("sni").is_none());
    assert!(ssl_json.get("err_log").is_none());
    for address in ["192.0.2.40:8443", "[2001:db8::40]:8443"] {
        let socket: std::net::SocketAddr = address.parse().unwrap();
        let ip = socket.ip().to_string();
        let peer = BasicPeer::new(address);
        // Reproduce the actual nested Pingora connector error shape.
        let cause = pingora_core::Error::because(
            pingora_core::ErrorType::ConnectError,
            format!("Fail to connect to {address}"),
            std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
        );
        let error = cause.more_context(format!("Fail to connect to {peer}"));
        let diagnostic = format_connect_error(address, &error);
        assert!(diagnostic.contains("ConnectError"));
        assert!(diagnostic.contains("connection refused"));
        assert!(diagnostic.starts_with("connect failed: "));
        let expected_ip = match mode {
            SensitiveMode::Off => {
                assert_eq!(diagnostic, format!("connect failed: {address}: {error}"));
                ip.clone()
            }
            SensitiveMode::Replace => {
                assert!(!diagnostic.contains(&ip), "{diagnostic}");
                assert_eq!(diagnostic.matches("[hidden]").count(), 3);
                "[hidden]".into()
            }
            SensitiveMode::Wrap => {
                assert_eq!(diagnostic.matches(&format!("<mask>{address}</mask>")).count(), 3);
                format!("<mask>{ip}</mask>")
            }
        };
        let mut ctx = TlsContext::new(443, "192.0.2.1".into(), 1234, "192.0.2.1".into(), 1234);
        ctx.sni = Some("public.example.com".into());
        ctx.err_log = Some(diagnostic.clone());
        ctx.push_log(&format!("retry 1/2: {diagnostic}"));
        ctx.upstream_info.push(TlsUpstreamInfo {
            addr: Some(ip.clone()),
            port: Some(socket.port()),
            backend: None,
            bytes_sent: 0,
            bytes_received: 0,
            connect_time: None,
            connection_established: false,
            proxy_protocol_sent: false,
            upstream_protocol: Some("TCP".into()),
        });
        for json in [serde_json::to_value(&ctx).unwrap(), listener(&ctx)] {
            assert_eq!(json["sni"], "public.example.com");
            assert_eq!(json["err_log"], diagnostic);
            // LogBuffer may reject a long entry; accepted entries preserve diagnostics.
            for entry in json["log"].as_array().unwrap() {
                assert_eq!(entry, &format!("retry 1/2: {diagnostic}"));
            }
            assert_eq!(json["upstream_info"][0]["addr"], expected_ip);
            assert_eq!(json["upstream_info"][0]["port"], 8443);
        }
        assert_eq!(ctx.upstream_info[0].addr.as_deref(), Some(ip.as_str()));
        assert_eq!(ctx.sni.as_deref(), Some("public.example.com"));
        // Short retry entries must survive the byte cap and must not be masked twice.
        let short = format_connect_error(
            address,
            &pingora_core::Error::new(pingora_core::ErrorType::ConnectTimedout),
        );
        ctx.log = None;
        ctx.push_log(&format!("retry 1/2: {short}"));
        assert_eq!(listener(&ctx)["log"][0], format!("retry 1/2: {short}"));
        for message in ["idle timeout", "no endpoint: default/service", "no SNI"] {
            ctx.err_log = Some(message.into());
            assert_eq!(listener(&ctx)["err_log"], message);
            assert_eq!(serde_json::to_value(&ctx).unwrap()["err_log"], message);
        }
        ctx.err_log = None;
        ctx.sni = None;
        ctx.upstream_info[0].addr = None;
        let global = serde_json::to_value(&ctx).unwrap();
        assert!(global.get("sni").is_none());
        assert!(global.get("err_log").is_none());
        let local = listener(&ctx);
        assert!(local["sni"].is_null());
        assert!(local["err_log"].is_null());
        for json in [global, local] {
            assert!(json["upstream_info"][0].get("addr").is_none());
        }
    }
}
