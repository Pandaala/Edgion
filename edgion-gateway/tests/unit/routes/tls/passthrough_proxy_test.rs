use super::*;

#[test]
fn tls_log_masks_only_backend_addresses() {
    super::super::sensitive_log_tests::assert_log_modes(
        concat!(module_path!(), "::tls_log_masks_only_backend_addresses"),
        EdgionTlsPassthroughProxy::listener_log_entry,
    );
}
use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

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
    Arc::new(TlsRouteUnit::from_route(Arc::new(route)))
}

#[test]
fn selected_unit_survives_reload_without_retaining_old_table() {
    let manager = TlsRouteManager::new(ListenerKey::new("0.0.0.0:443".parse().unwrap(), "TLS"));
    assert!(select_route_unit(&manager, "a.example.com").is_none());
    let old_unit = route_unit("a", "a.example.com");
    let sibling = route_unit("b", "b.example.com");
    let old_unit_weak = Arc::downgrade(&old_unit);
    let selector_weak = Arc::downgrade(&old_unit.backend_finder);
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
    assert!(Arc::ptr_eq(&selected.backend_finder, &selector_weak.upgrade().unwrap()));
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

fn context() -> TlsContext {
    let mut ctx = TlsContext::new(8443, "192.0.2.1".to_string(), 1234, "192.0.2.1".to_string(), 1234);
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
struct ReplayWriter {
    bytes: Vec<u8>,
    fail_after: Option<usize>,
    fail_flush: bool,
    flushes: usize,
}

impl AsyncWrite for ReplayWriter {
    fn poll_write(mut self: Pin<&mut Self>, _: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        let remaining = self.fail_after.unwrap_or(usize::MAX).saturating_sub(self.bytes.len());
        if remaining == 0 {
            return Poll::Ready(Err(io::Error::other("replay write failed")));
        }
        let count = bytes.len().min(3).min(remaining);
        self.bytes.extend_from_slice(&bytes[..count]);
        Poll::Ready(Ok(count))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.flushes += 1;
        Poll::Ready(if self.fail_flush {
            Err(io::Error::other("replay flush failed"))
        } else {
            Ok(())
        })
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn replay_forwards_record_and_trailing_bytes_once_before_accounting() {
    // A complete handshake record plus a following application-data record.
    let bytes = [0x16, 3, 3, 0, 1, 1, 0x17, 3, 3, 0, 2, 42, 43];
    let mut initial = vec![0; sni_parser::MAX_CLIENT_HELLO_LEN];
    initial[..bytes.len()].copy_from_slice(&bytes);
    initial.truncate(bytes.len());
    assert!(matches!(
        sni_parser::client_hello_record_complete(&initial),
        sni_parser::RecordCompleteness::Complete { record_end: 6 }
    ));
    let mut writer = ReplayWriter::default();
    let mut ctx = context();
    assert!(replay_initial_bytes(&mut writer, &mut ctx, initial).await);
    assert_eq!(writer.bytes, bytes);
    assert_eq!(writer.flushes, 1);
    assert_eq!(ctx.upstream_info[0].bytes_sent, 7 + bytes.len() as u64);
    assert!(ctx.err_log.is_none());
}

#[tokio::test]
async fn replay_write_failure_is_terminal_without_flush_or_accounting() {
    let mut writer = ReplayWriter {
        fail_after: Some(4),
        ..Default::default()
    };
    let mut ctx = context();
    assert!(!replay_initial_bytes(&mut writer, &mut ctx, b"initial bytes".to_vec()).await);
    assert_eq!(writer.bytes, b"init");
    assert_eq!(writer.flushes, 0);
    assert_eq!(ctx.upstream_info[0].bytes_sent, 7);
    assert_eq!(
        ctx.err_log.as_deref(),
        Some("upstream write initial bytes failed: replay write failed")
    );
}

#[tokio::test]
async fn replay_flush_failure_is_terminal_without_accounting() {
    let mut writer = ReplayWriter {
        fail_flush: true,
        ..Default::default()
    };
    let mut ctx = context();
    assert!(!replay_initial_bytes(&mut writer, &mut ctx, b"initial bytes".to_vec()).await);
    assert_eq!(writer.bytes, b"initial bytes");
    assert_eq!(writer.flushes, 1);
    assert_eq!(ctx.upstream_info[0].bytes_sent, 7);
    assert_eq!(ctx.err_log.as_deref(), Some("upstream flush initial bytes failed"));
}
