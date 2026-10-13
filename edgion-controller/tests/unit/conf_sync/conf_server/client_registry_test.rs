use super::*;
use std::future::Future;
use std::task::{Context, Poll, Waker};

#[test]
fn test_register_and_count() {
    let registry = Arc::new(ClientRegistry::new());
    assert_eq!(registry.count(), 1);
    let _first = registry.register("client-1".into(), "gw-1".into());
    assert_eq!(registry.count(), 1);
    let _second = registry.register("client-2".into(), "gw-2".into());
    assert_eq!(registry.count(), 2);
    let _replacement = registry.register("client-1".into(), "gw-1-updated".into());
    assert_eq!(registry.count(), 2);
}

#[test]
fn test_unregister() {
    let registry = Arc::new(ClientRegistry::new());
    let first = registry.register("client-1".into(), "gw-1".into());
    let second = registry.register("client-2".into(), "gw-2".into());
    assert_eq!(registry.count(), 2);
    drop(first);
    assert_eq!(registry.count(), 1);
    drop(second);
    assert_eq!(registry.count(), 1);
    assert!(registry.clients.read().is_empty());
}

#[test]
fn overlapping_registrations_preserve_presence_in_both_release_orders() {
    for reverse in [false, true] {
        let registry = Arc::new(ClientRegistry::new());
        let h = registry.register("H".into(), "H".into());
        let j = registry.register("J".into(), "J".into());
        let first = registry.register("G".into(), "G".into());
        let replacement = registry.register("G".into(), "G".into());
        assert_eq!(registry.count(), 3);
        let (first_exit, last_exit) = if reverse {
            (replacement, first)
        } else {
            (first, replacement)
        };
        drop(first_exit);
        assert_eq!(registry.count(), 3);
        assert_eq!(registry.clients.read().get("G"), Some(&1));
        drop(last_exit);
        assert_eq!(registry.count(), 2);
        assert!(!registry.clients.read().contains_key("G"));
        drop((h, j));
        assert!(registry.clients.read().is_empty());
    }
}

#[tokio::test]
async fn test_wait_for_change_notifies() {
    let registry = Arc::new(ClientRegistry::new());
    let mut context = Context::from_waker(Waker::noop());
    let changed = registry.wait_for_change();
    tokio::pin!(changed);
    assert!(changed.as_mut().poll(&mut context).is_pending());
    let first = registry.register("G".into(), "G".into());
    assert_eq!(changed.as_mut().poll(&mut context), Poll::Ready(()));

    let changed = registry.wait_for_change();
    tokio::pin!(changed);
    assert!(changed.as_mut().poll(&mut context).is_pending());
    let replacement = registry.register("G".into(), "G".into());
    assert!(changed.as_mut().poll(&mut context).is_pending());
    drop(first);
    assert!(changed.as_mut().poll(&mut context).is_pending());
    drop(replacement);
    assert_eq!(changed.as_mut().poll(&mut context), Poll::Ready(()));
    assert!(registry.clients.read().is_empty());
}

#[tokio::test]
async fn registration_is_released_on_abort_before_and_after_first_poll() {
    for start_task in [false, true] {
        let registry = Arc::new(ClientRegistry::new());
        let registration = registry.register("G".into(), "G".into());
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _registration = registration;
            let _ = started.send(());
            std::future::pending::<()>().await;
        });
        if start_task {
            ready.await.unwrap();
        }
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(registry.clients.read().is_empty());
    }
}

// Exercise the actual RPC without opening a socket. Keeping these assertions in
// the registry's test module exposes live lease counts without production APIs.
use crate::conf_sync::conf_server::{ConfigSyncGrpcServer, ConfigSyncServer, ConfigSyncServerProvider};
use edgion_common::conf_sync::proto::{config_sync_server::ConfigSync, WatchServerMetaRequest};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

struct MetadataProvider(Arc<ConfigSyncServer>);

impl ConfigSyncServerProvider for MetadataProvider {
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        Some(self.0.clone())
    }
}

async fn wait_for_registrations(registry: &ClientRegistry, expected: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if registry.clients.read().get("G").copied().unwrap_or(0) == expected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("RPC must release its own registration");
}

fn metadata_request() -> tonic::Request<WatchServerMetaRequest> {
    tonic::Request::new(WatchServerMetaRequest {
        client_id: "G".into(),
        client_name: "G".into(),
    })
}

#[tokio::test]
async fn overlapping_rpc_receivers_release_only_their_own_registration() {
    for reverse in [false, true] {
        let server = Arc::new(ConfigSyncServer::new());
        let registry = server.client_registry();
        let _h = registry.register("H".into(), "H".into());
        let _j = registry.register("J".into(), "J".into());
        let grpc = ConfigSyncGrpcServer::new(Arc::new(MetadataProvider(server)), CancellationToken::new());
        let mut first = grpc.watch_server_meta(metadata_request()).await.unwrap().into_inner();
        assert_eq!(first.next().await.unwrap().unwrap().gateway_instance_count, 3);
        let mut replacement = grpc.watch_server_meta(metadata_request()).await.unwrap().into_inner();
        assert_eq!(replacement.next().await.unwrap().unwrap().gateway_instance_count, 3);
        assert_eq!(registry.clients.read().get("G"), Some(&2));
        let (first_exit, last_exit) = if reverse {
            (replacement, first)
        } else {
            (first, replacement)
        };
        drop(first_exit);
        wait_for_registrations(&registry, 1).await;
        assert_eq!(registry.count(), 3);
        drop(last_exit);
        wait_for_registrations(&registry, 0).await;
        assert_eq!(registry.count(), 2);
    }
}

#[tokio::test]
async fn initial_rpc_send_failure_releases_registration() {
    let server = Arc::new(ConfigSyncServer::new());
    let registry = server.client_registry();
    let grpc = ConfigSyncGrpcServer::new(Arc::new(MetadataProvider(server)), CancellationToken::new());
    // This RPC has no suspension before spawning/returning. On the current-thread
    // runtime the receiver closes before the stream task can send its first event.
    drop(grpc.watch_server_meta(metadata_request()).await.unwrap());
    assert_eq!(registry.clients.read().get("G"), Some(&1));
    wait_for_registrations(&registry, 0).await;
    assert!(registry.clients.read().is_empty());
}
