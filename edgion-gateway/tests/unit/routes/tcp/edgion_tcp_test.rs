use super::*;
use edgion_resources::resources::tcp_route::{TCPRoute, TCPRouteSpec};
use futures::poll;
use kube::api::ObjectMeta;
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use tokio::time::advance;

fn context() -> TcpContext {
    TcpContext {
        listener_port: 9000,
        direct_peer_ip: "192.0.2.1".to_string(),
        direct_peer_port: 1234,
        remote_ip: "192.0.2.2".to_string(),
        remote_port: 4321,
        upstream_addr: Some("192.0.2.3:9001".to_string()),
        start_time: Instant::now(),
        bytes_sent: 0,
        bytes_received: 0,
        status: TcpStatus::Success,
        connection_established: true,
        deny_reason: None,
    }
}

fn route_unit(name: &str) -> Arc<TcpRouteUnit> {
    Arc::new(TcpRouteUnit::from_route(Arc::new(
        crate::test_current_status::processed(TCPRoute {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TCPRouteSpec {
                current_status: None,
                resolved_status_controller: Some("edgion.io/gateway-controller".into()),
                parent_refs: None,
                rules: None,
                resolved_listeners: None,
                resolved_listener_attachments: None,
            },
            status: None,
        }),
    )))
}

#[test]
fn selected_unit_survives_reload_without_retaining_old_table() {
    let manager = TcpPortRouteManager::new();
    assert!(select_route_unit(&manager).is_none());

    let old_unit = route_unit("a");
    let sibling = route_unit("b");
    let old_unit_weak = Arc::downgrade(&old_unit);
    let sibling_weak = Arc::downgrade(&sibling);
    manager.rebuild(&HashMap::from([
        ("default/a".to_string(), old_unit),
        ("default/b".to_string(), sibling),
    ]));
    let old_table = Arc::downgrade(&manager.load_route_table());
    let selected = select_route_unit(&manager).unwrap();
    assert!(Arc::ptr_eq(&selected, &old_unit_weak.upgrade().unwrap()));

    let new_unit = route_unit("a");
    manager.rebuild(&HashMap::from([("default/a".to_string(), new_unit.clone())]));
    assert!(old_table.upgrade().is_none(), "selection must release the table guard");
    assert!(
        sibling_weak.upgrade().is_none(),
        "unselected units must not be retained"
    );
    assert!(
        old_unit_weak.upgrade().is_some(),
        "the connection keeps its selected generation"
    );
    assert!(Arc::ptr_eq(&select_route_unit(&manager).unwrap(), &new_unit));
    assert!(!Arc::ptr_eq(&selected, &new_unit));
    drop(selected);
    assert!(old_unit_weak.upgrade().is_none());
}

#[tokio::test(start_paused = true)]
async fn tcp_hooks_record_bidirectional_bytes_and_clean_completion() {
    let (mut client, downstream) = duplex(16);
    let (upstream, mut server) = duplex(16);
    let mut ctx = context();
    let relay = bidirectional_relay(
        Box::new(downstream),
        Box::new(upstream),
        TcpDuplexHooks { ctx: &mut ctx },
        Some(Duration::from_secs(1)),
    );
    let exchange = async {
        client.write_all(b"request").await.unwrap();
        let mut request = [0; 7];
        server.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"request");
        server.write_all(b"response").await.unwrap();
        let mut response = [0; 8];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"response");
        client.shutdown().await.unwrap();
    };
    tokio::join!(relay, exchange);
    assert_eq!((ctx.bytes_sent, ctx.bytes_received), (7, 8));
    assert!(matches!(ctx.status, TcpStatus::Success));
    assert!(ctx.connection_established);
    assert!(ctx.deny_reason.is_none());
}

#[tokio::test(start_paused = true)]
async fn tcp_hooks_idle_timeout_preserves_bytes_read_before_blocked_write() {
    let (mut client, downstream) = duplex(16);
    let (upstream, _server) = duplex(1);
    client.write_all(b"request").await.unwrap();
    let mut ctx = context();
    let mut relay = Box::pin(bidirectional_relay(
        Box::new(downstream),
        Box::new(upstream),
        TcpDuplexHooks { ctx: &mut ctx },
        Some(Duration::from_secs(10)),
    ));
    assert!(poll!(&mut relay).is_pending());
    advance(Duration::from_secs(9)).await;
    assert!(poll!(&mut relay).is_pending(), "idle timeout must not fire early");
    advance(Duration::from_secs(1)).await;
    assert!(poll!(&mut relay).is_ready());
    drop(relay);
    assert!(matches!(ctx.status, TcpStatus::IdleTimeout));
    assert_eq!((ctx.bytes_sent, ctx.bytes_received), (7, 0));
    assert!(ctx.connection_established);
}

#[test]
fn tcp_hooks_map_every_relay_error_without_changing_accounting() {
    for (error, status) in [
        (DuplexError::UpstreamWrite, "UpstreamWriteError"),
        (DuplexError::DownstreamRead, "DownstreamReadError"),
        (DuplexError::DownstreamWrite, "DownstreamWriteError"),
        (DuplexError::DownstreamFlush, "DownstreamWriteError"),
        (DuplexError::UpstreamRead, "UpstreamReadError"),
        (DuplexError::IdleTimeout, "IdleTimeout"),
    ] {
        let mut ctx = context();
        let mut hooks = TcpDuplexHooks { ctx: &mut ctx };
        hooks.record_sent(7);
        hooks.record_sent(11);
        hooks.record_received(13);
        hooks.record_received(17);
        hooks.on_error(error);
        assert_eq!(ctx.status.as_str(), status, "{error:?}");
        assert_eq!((ctx.bytes_sent, ctx.bytes_received), (18, 30));
        assert!(ctx.connection_established);
        assert!(ctx.deny_reason.is_none());
    }
}

#[tokio::test]
async fn fail_close_when_store_key_missing_in_tcp() {
    // Rule references an ESP but the admitted scope has no executable view.
    // Must reject (fail-close) with the bounded reason.
    let key = "default/__tcp_fail_close_missing__";

    let result = check_stream_plugins(Some(key), None, "127.0.0.1", 9000).await;
    assert_eq!(
        result,
        StreamPluginCheck::Deny("stream plugin resource not found"),
        "missing ESP must fail-close"
    );
}

#[tokio::test]
async fn fail_close_when_tcp_direct_peer_ip_unparseable() {
    // Rule references an ESP but DirectPeerIp is not a valid IP literal
    // (e.g. peer addr extraction returned "unknown"). Must reject.
    let result = check_stream_plugins(Some("default/something"), None, "unknown", 9000).await;
    assert_eq!(
        result,
        StreamPluginCheck::Deny("direct peer IP unparseable"),
        "unparseable client addr must fail-close"
    );
}

#[tokio::test]
async fn pass_through_when_store_key_none() {
    // No ESP attached at all — section 3 is a no-op. Must allow.
    let result = check_stream_plugins(None, None, "127.0.0.1", 9000).await;
    assert_eq!(
        result,
        StreamPluginCheck::Allow,
        "rule with no store_key must pass through"
    );
}
