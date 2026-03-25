//! Periodic scheduler: sends ListRequest to all online controllers every list_interval_secs.

use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::core::center::config::CenterSyncConfig;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::fed_sync::server::{PendingListEntry, PendingListMap};
use crate::core::common::fed_sync::proto::{center_message::Payload as CenterPayload, CenterMessage, ListRequest};

/// Send ListRequests to all currently online controllers, registering each in pending_lists.
pub async fn send_list_requests(registry: &ControllerRegistry, pending: &PendingListMap) {
    let senders = registry.online_senders();
    for (controller_id, tx) in senders {
        let request_id = Uuid::new_v4().to_string();
        let (resp_tx, _resp_rx) = oneshot::channel();

        pending.lock().unwrap().insert(
            request_id.clone(),
            PendingListEntry {
                sender: resp_tx,
                created_at: std::time::Instant::now(),
            },
        );

        let msg = CenterMessage {
            payload: Some(CenterPayload::ListRequest(ListRequest {
                request_id: request_id.clone(),
                kinds: vec![],
            })),
        };

        if tx.send(msg).await.is_err() {
            tracing::warn!(
                component = "scheduler",
                controller_id = %controller_id,
                "Failed to send ListRequest (stream closed)"
            );
            pending.lock().unwrap().remove(&request_id);
        } else {
            tracing::debug!(
                component = "scheduler",
                controller_id = %controller_id,
                request_id = %request_id,
                "ListRequest sent"
            );
        }
    }
}

/// Run the scheduler loop. Exits when shutdown fires.
pub async fn run(
    registry: ControllerRegistry,
    pending: PendingListMap,
    config: CenterSyncConfig,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let list_interval = Duration::from_secs(config.list_interval_secs);
    let list_timeout = Duration::from_secs(config.list_timeout_secs);
    let mut interval = tokio::time::interval(list_interval);
    interval.tick().await; // skip immediate first tick

    // Background GC task: evict pending ListRequest entries that exceeded list_timeout.
    {
        let pending = pending.clone();
        tokio::spawn(async move {
            let mut gc_interval = tokio::time::interval(list_timeout);
            gc_interval.tick().await; // skip first
            loop {
                gc_interval.tick().await;
                pending.lock().unwrap().retain(|request_id, entry| {
                    let timed_out = entry.created_at.elapsed() >= list_timeout;
                    if timed_out {
                        tracing::warn!(
                            component = "scheduler",
                            request_id = %request_id,
                            "ListRequest timed out, dropping pending entry"
                        );
                    }
                    !timed_out
                });
            }
        });
    }

    loop {
        tokio::select! {
            _ = interval.tick() => {
                tracing::debug!(component = "scheduler", "Sending ListRequests to all online controllers");
                send_list_requests(&registry, &pending).await;
            }
            _ = shutdown.changed() => {
                tracing::info!(component = "scheduler", "Scheduler shutdown");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::common::fed_sync::proto::RegisterRequest;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn test_send_list_request_registers_pending() {
        let registry = ControllerRegistry::new();
        let pending: PendingListMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        registry.register("c1".to_string(), RegisterRequest::default(), tx, "s1".to_string());

        send_list_requests(&registry, &pending).await;

        let msg = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();

        let request_id = match msg.payload {
            Some(CenterPayload::ListRequest(r)) => r.request_id,
            _ => panic!("Expected ListRequest"),
        };

        // Verify entry is in pending map with a recent timestamp
        let map = pending.lock().unwrap();
        let entry = map.get(&request_id).expect("request_id should be in pending map");
        assert!(entry.created_at.elapsed().as_secs() < 5);
    }
}
