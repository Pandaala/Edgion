use super::*;
use crate::ctx::ClientCertInfo;
use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

#[test]
fn tls_log_masks_only_backend_addresses() {
    super::super::sensitive_log_tests::assert_log_modes(
        concat!(module_path!(), "::tls_log_masks_only_backend_addresses"),
        EdgionTlsTcpProxy::listener_log_entry,
    );
}

fn route_unit(name: &str, hostname: &str) -> Arc<TlsRouteUnit> {
    let route = crate::test_current_status::from_value(serde_json::json!({
            "metadata": {"namespace": "default", "name": name},
            "spec": {
                "hostnames": [hostname], "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{"name": "gateway", "sectionName": "tls"}],
                "resolvedListeners": [{"listenerKey": "0.0.0.0:443/TLS", "gatewayNamespace": "default",
                    "gatewayName": "gateway", "listenerName": "tls", "hostnames": [hostname]}]
            }
        }))
        .unwrap();
    Arc::new(TlsRouteUnit::from_route(Arc::new(route)).unwrap())
}

#[test]
fn selected_unit_survives_reload_without_retaining_old_table() {
    let manager = TlsRouteManager::new(ListenerKey::new("0.0.0.0:443".parse().unwrap(), "TLS"));
    assert!(select_route_unit(&manager, "a.example.com").is_none());
    let old_unit = route_unit("a", "a.example.com");
    let sibling = route_unit("b", "b.example.com");
    let old_unit_weak = Arc::downgrade(&old_unit);
    let selector_weak = Arc::downgrade(old_unit.backend_generation());
    let sibling_weak = Arc::downgrade(&sibling);
    manager.rebuild(&HashMap::from([
        ("default/a".to_string(), old_unit),
        ("default/b".to_string(), sibling),
    ]));
    let old_table = Arc::downgrade(&manager.load_route_table());
    assert!(select_route_unit(&manager, "missing.example.com").is_none());
    let selected = select_route_unit(&manager, "A.EXAMPLE.COM").unwrap();
    assert!(Arc::ptr_eq(&selected, &old_unit_weak.upgrade().unwrap()));
    assert!(select_route_unit(&manager, "b.example.com").is_some());

    let new_unit = route_unit("a", "a.example.com");
    manager.rebuild(&HashMap::from([("default/a".to_string(), new_unit.clone())]));
    assert!(old_table.upgrade().is_none(), "selection must release the table guard");
    assert!(
        sibling_weak.upgrade().is_none(),
        "unselected hostname winner is released"
    );
    assert!(Arc::ptr_eq(
        selected.backend_generation(),
        &selector_weak.upgrade().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &select_route_unit(&manager, "a.example.com").unwrap(),
        &new_unit
    ));
    assert!(!Arc::ptr_eq(&selected, &new_unit));
    assert!(select_route_unit(&manager, "b.example.com").is_none());
    drop(selected);
    assert!(old_unit_weak.upgrade().is_none());
    assert!(selector_weak.upgrade().is_none());
}

#[test]
fn metadata_install_preserves_fields_and_leaves_source_unchanged() {
    let mut ctx = TlsContext::new(8443, "192.0.2.1".into(), 1234, "192.0.2.2".into(), 5678);
    let mut meta = TlsConnMeta {
        ts: 42,
        start_at: Instant::now(),
        handshake_complete_time: Some(17),
        port: Some(9443),
        tls_id: Some("tls-connection".into()),
        sni: Some("A.EXAMPLE.COM".into()),
        log: None,
        matched: Some(MatchedInfo {
            rsc: Rsc {
                kind: "EdgionTls".into(),
                ns: "default".into(),
                name: "certificate".into(),
                labels: None,
            },
            section: Some("secure".into()),
            sv: 123,
        }),
        err_log: Some("handshake diagnostic".into()),
        client_cert_info: Some(ClientCertInfo {
            subject: "CN=client.example.com".into(),
            sans: vec!["client.example.com".into()],
            cn: Some("client.example.com".into()),
            fingerprint: "test-fingerprint".into(),
        }),
        is_mtls: true,
    };
    let before = serde_json::to_value(&meta).unwrap();
    install_tls_metadata(&mut ctx, Some(&meta));
    assert_eq!(ctx.tls_id, meta.tls_id);
    assert_eq!(ctx.sni, meta.sni);
    assert_eq!(serde_json::to_value(&ctx.matched).unwrap(), before["matched"]);
    assert!(ctx.is_mtls);
    assert_eq!(ctx.listener_port, 8443);
    assert!(ctx.log.is_none());
    assert!(ctx.err_log.is_none());
    assert_eq!(serde_json::to_value(&meta).unwrap(), before);
    let cert = meta.client_cert_info.as_ref().unwrap();
    assert_eq!(cert.subject, "CN=client.example.com");
    assert_eq!(cert.sans, ["client.example.com"]);
    assert_eq!(cert.cn.as_deref(), Some("client.example.com"));
    assert_eq!(cert.fingerprint, "test-fingerprint");

    let installed = serde_json::to_value(&ctx).unwrap();
    install_tls_metadata(&mut ctx, None);
    assert_eq!(serde_json::to_value(&ctx).unwrap(), installed);

    meta.tls_id = None;
    meta.sni = None;
    meta.matched = None;
    meta.is_mtls = false;
    install_tls_metadata(&mut ctx, Some(&meta));
    assert!(ctx.tls_id.is_none());
    assert!(ctx.sni.is_none());
    assert!(ctx.matched.is_none());
    assert!(!ctx.is_mtls);
}

fn pp2_context(remote_ip: &str) -> TlsContext {
    let mut ctx = TlsContext::new(8443, "192.0.2.1".into(), 4321, remote_ip.into(), 12345);
    ctx.sni = Some("example.com".into());
    ctx.upstream_info.push(TlsUpstreamInfo {
        addr: None,
        port: None,
        backend: None,
        bytes_sent: 7,
        bytes_received: 0,
        connect_time: None,
        connection_established: true,
        proxy_protocol_sent: false,
        upstream_protocol: None,
    });
    ctx
}

#[derive(Default)]
struct Pp2Writer {
    bytes: Vec<u8>,
    fail_after: Option<usize>,
    fail_flush: bool,
    flushes: usize,
}

impl AsyncWrite for Pp2Writer {
    fn poll_write(mut self: Pin<&mut Self>, _: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        let remaining = self.fail_after.unwrap_or(usize::MAX).saturating_sub(self.bytes.len());
        if remaining == 0 {
            return Poll::Ready(Err(io::Error::other("test write failed")));
        }
        let count = bytes.len().min(5).min(remaining);
        self.bytes.extend_from_slice(&bytes[..count]);
        Poll::Ready(Ok(count))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.flushes += 1;
        Poll::Ready(if self.fail_flush {
            Err(io::Error::other("test flush failed"))
        } else {
            Ok(())
        })
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn proxy_protocol_v2_emits_exact_ipv6_header_and_marks_success() {
    let sni = "secure.example.com";
    let source: std::net::Ipv6Addr = "2001:db8::10".parse().unwrap();
    let destination: std::net::Ipv6Addr = "2001:db8::20".parse().unwrap();
    let mut writer = Pp2Writer::default();
    let mut ctx = pp2_context(&source.to_string());
    ctx.sni = Some(sni.into());

    assert!(send_proxy_protocol_v2(Some(2), &mut ctx, "[2001:db8::20]:8443", &mut writer,).await);

    let mut expected = Vec::new();
    expected.extend_from_slice(&crate::util::proxy_protocol::PP2_SIGNATURE);
    expected.extend_from_slice(&[0x21, 0x21]);
    expected.extend_from_slice(&((36 + 3 + sni.len()) as u16).to_be_bytes());
    expected.extend_from_slice(&source.octets());
    expected.extend_from_slice(&destination.octets());
    expected.extend_from_slice(&12345u16.to_be_bytes());
    expected.extend_from_slice(&8443u16.to_be_bytes());
    expected.push(crate::util::proxy_protocol::PP2_TYPE_AUTHORITY);
    expected.extend_from_slice(&(sni.len() as u16).to_be_bytes());
    expected.extend_from_slice(sni.as_bytes());

    assert_eq!(writer.bytes, expected);
    assert_eq!(writer.flushes, 1);
    assert!(ctx.upstream_info[0].proxy_protocol_sent);
    assert_eq!(ctx.upstream_info[0].bytes_sent, 7);
    assert_eq!(ctx.log.as_ref().unwrap().entries().collect::<Vec<_>>(), ["PP2 sent"]);
    assert!(ctx.err_log.is_none());
}

#[tokio::test]
async fn proxy_protocol_v2_skips_disabled_or_unparseable_source() {
    for version in [None, Some(1)] {
        let mut writer = Pp2Writer::default();
        let mut ctx = pp2_context("192.0.2.10");
        assert!(send_proxy_protocol_v2(version, &mut ctx, "192.0.2.20:443", &mut writer).await);
        assert!(writer.bytes.is_empty());
        assert_eq!(writer.flushes, 0);
        assert!(!ctx.upstream_info[0].proxy_protocol_sent);
        assert_eq!(ctx.upstream_info[0].bytes_sent, 7);
        assert!(ctx.log.is_none());
        assert!(ctx.err_log.is_none());
    }

    let mut writer = Pp2Writer::default();
    let mut ctx = pp2_context("unknown");
    assert!(send_proxy_protocol_v2(Some(2), &mut ctx, "192.0.2.20:443", &mut writer).await);
    assert!(writer.bytes.is_empty());
    assert_eq!(writer.flushes, 0);
    assert!(!ctx.upstream_info[0].proxy_protocol_sent);
    assert_eq!(ctx.upstream_info[0].bytes_sent, 7);
    assert!(ctx.log.is_none());
    assert!(ctx.err_log.is_none());
}

#[tokio::test]
async fn proxy_protocol_v2_preserves_unspecified_destination_fallback() {
    let mut writer = Pp2Writer::default();
    let mut ctx = pp2_context("192.0.2.10");
    assert!(send_proxy_protocol_v2(Some(2), &mut ctx, "invalid", &mut writer).await);
    assert_eq!(writer.bytes[13], 0x11);
    assert_eq!(&writer.bytes[20..24], &[0, 0, 0, 0]);
    assert_eq!(&writer.bytes[26..28], &[0, 0]);
    assert!(ctx.upstream_info[0].proxy_protocol_sent);
}

#[tokio::test]
async fn proxy_protocol_v2_failures_do_not_mark_or_account_success() {
    let mut writer = Pp2Writer {
        fail_after: Some(4),
        ..Default::default()
    };
    let mut ctx = pp2_context("192.0.2.10");
    assert!(!send_proxy_protocol_v2(Some(2), &mut ctx, "192.0.2.20:443", &mut writer).await);
    assert_eq!(writer.bytes.len(), 4);
    assert_eq!(writer.flushes, 0);
    assert!(!ctx.upstream_info[0].proxy_protocol_sent);
    assert_eq!(ctx.upstream_info[0].bytes_sent, 7);
    assert!(ctx.log.is_none());
    assert_eq!(ctx.err_log.as_deref(), Some("PP2 write failed: test write failed"));

    let mut writer = Pp2Writer {
        fail_flush: true,
        ..Default::default()
    };
    let mut ctx = pp2_context("192.0.2.10");
    assert!(!send_proxy_protocol_v2(Some(2), &mut ctx, "192.0.2.20:443", &mut writer).await);
    assert!(!writer.bytes.is_empty());
    assert_eq!(writer.flushes, 1);
    assert!(!ctx.upstream_info[0].proxy_protocol_sent);
    assert_eq!(ctx.upstream_info[0].bytes_sent, 7);
    assert!(ctx.log.is_none());
    assert_eq!(ctx.err_log.as_deref(), Some("PP2 flush failed"));
}
