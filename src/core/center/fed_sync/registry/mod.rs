//! ControllerRegistry: manages per-controller sessions and stream handles.

use crate::core::common::fed_sync::proto::{CenterMessage, RegisterRequest};
use crate::core::common::observe::fed_metrics;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

#[derive(Debug)]
pub struct ControllerSession {
    pub controller_id: String,
    pub session_id: String,
    pub info: RegisterRequest,
    pub stream_tx: Option<mpsc::Sender<CenterMessage>>,
    pub last_seen: Instant,
    pub offline_since: Option<Instant>,
}

impl ControllerSession {
    pub fn is_online(&self) -> bool {
        self.stream_tx.is_some()
    }
}

#[derive(Clone)]
pub struct ControllerRegistry {
    inner: Arc<RwLock<HashMap<String, ControllerSession>>>,
}

impl ControllerRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register(
        &self,
        controller_id: String,
        info: RegisterRequest,
        stream_tx: mpsc::Sender<CenterMessage>,
        session_id: String,
    ) {
        let session = ControllerSession {
            controller_id: controller_id.clone(),
            session_id,
            info,
            stream_tx: Some(stream_tx),
            last_seen: Instant::now(),
            offline_since: None,
        };
        let mut guard = self.inner.write();
        // Detect same controller_id re-registering while a session entry still
        // exists — a signal for reconnect storms / session races. Covers both
        // the online→online case and the offline→online (recovered) case.
        if guard.contains_key(&controller_id) {
            fed_metrics::record_session_reentry();
        }
        guard.insert(controller_id, session);
    }

    pub fn get_session(&self, controller_id: &str) -> Option<SessionView> {
        let map = self.inner.read();
        map.get(controller_id).map(|s| SessionView {
            controller_id: s.controller_id.clone(),
            session_id: s.session_id.clone(),
            info: s.info.clone(),
            stream_tx: s.stream_tx.clone(),
            last_seen: s.last_seen,
            offline_since: s.offline_since,
        })
    }

    pub fn update_last_seen(&self, controller_id: &str) {
        if let Some(session) = self.inner.write().get_mut(controller_id) {
            session.last_seen = Instant::now();
        }
    }

    /// Mark the session offline and release its stream sender.
    ///
    /// Returns true if the transition actually happened. Returns false when
    /// the session is already offline, the controller is not known, or the
    /// `expected_session_id` does not match the currently registered session
    /// (a stale task raced with a reconnect — it must not touch the new
    /// session).
    pub fn mark_offline(&self, controller_id: &str, expected_session_id: &str) -> bool {
        let mut guard = self.inner.write();
        let Some(session) = guard.get_mut(controller_id) else {
            return false;
        };
        if session.session_id != expected_session_id {
            tracing::debug!(
                component = "registry",
                controller_id = %controller_id,
                expected_session_id = %expected_session_id,
                actual_session_id = %session.session_id,
                "mark_offline called by stale task; ignoring"
            );
            return false;
        }
        if session.offline_since.is_some() {
            return false;
        }
        session.offline_since = Some(Instant::now());
        session.stream_tx.take();
        tracing::info!(
            component = "registry",
            controller_id = %controller_id,
            session_id = %expected_session_id,
            "Controller marked offline, stream sender released"
        );
        true
    }

    pub fn online_controller_ids(&self) -> Vec<String> {
        self.inner
            .read()
            .values()
            .filter(|s| s.is_online())
            .map(|s| s.controller_id.clone())
            .collect()
    }

    pub fn online_senders(&self) -> Vec<(String, mpsc::Sender<CenterMessage>)> {
        self.inner
            .read()
            .values()
            .filter_map(|s| s.stream_tx.as_ref().map(|tx| (s.controller_id.clone(), tx.clone())))
            .collect()
    }

    /// Remove a controller entry from the registry entirely.
    /// Returns true if an entry was removed. Unlike `mark_offline`, this
    /// drops the session record itself (used by Admin DELETE cascade).
    pub fn remove(&self, controller_id: &str) -> bool {
        let removed = self.inner.write().remove(controller_id).is_some();
        if removed {
            fed_metrics::record_evict_stale(fed_metrics::labels::evict_source::REGISTRY);
        }
        removed
    }

    /// Current number of registered controller sessions (online + offline).
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// True when no controller sessions are registered.
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Number of currently online controller sessions.
    pub fn online_len(&self) -> usize {
        self.inner.read().values().filter(|s| s.is_online()).count()
    }
}

#[derive(Debug, Clone)]
pub struct SessionView {
    pub controller_id: String,
    pub session_id: String,
    pub info: RegisterRequest,
    pub stream_tx: Option<mpsc::Sender<CenterMessage>>,
    pub last_seen: Instant,
    pub offline_since: Option<Instant>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_info(cid: &str) -> RegisterRequest {
        RegisterRequest {
            controller_id: cid.to_string(),
            cluster: "cluster".to_string(),
            env: vec![],
            tag: vec![],
            supported_kinds: vec![],
        }
    }

    #[test]
    fn test_register_and_lookup() {
        let registry = ControllerRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        registry.register(
            "cluster/ctrl-01".to_string(),
            mock_info("cluster/ctrl-01"),
            tx,
            "sess-1".to_string(),
        );
        assert!(registry.get_session("cluster/ctrl-01").is_some());
        assert_eq!(registry.online_controller_ids().len(), 1);
    }

    #[tokio::test]
    async fn test_mark_offline_releases_sender_and_preserves_entry() {
        let registry = ControllerRegistry::new();
        let (tx, mut rx) = mpsc::channel(8);
        registry.register("cid".to_string(), mock_info("cid"), tx, "s1".to_string());

        assert_eq!(registry.online_senders().len(), 1);

        let marked = registry.mark_offline("cid", "s1");
        assert!(marked, "first mark_offline should transition");

        let view = registry.get_session("cid").expect("session should still exist");
        assert!(view.offline_since.is_some());
        assert!(view.stream_tx.is_none());
        assert_eq!(registry.online_senders().len(), 0);

        // Only the registry held a Sender clone; after take() the receiver drains.
        assert!(rx.recv().await.is_none(), "receiver should see channel close");
    }

    #[test]
    fn test_mark_offline_wrong_session_id_is_noop() {
        let registry = ControllerRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        registry.register("cid".to_string(), mock_info("cid"), tx, "s1".to_string());

        let marked = registry.mark_offline("cid", "s2");
        assert!(!marked, "mismatched session_id must not transition");

        let view = registry.get_session("cid").expect("session should still exist");
        assert!(view.offline_since.is_none());
        assert!(view.stream_tx.is_some());
    }

    #[test]
    fn test_mark_offline_idempotent() {
        let registry = ControllerRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        registry.register("cid".to_string(), mock_info("cid"), tx, "s1".to_string());

        assert!(registry.mark_offline("cid", "s1"));
        assert!(
            !registry.mark_offline("cid", "s1"),
            "second call on already-offline session must be noop"
        );

        let view = registry.get_session("cid").expect("still exists");
        assert!(view.offline_since.is_some());
        assert!(view.stream_tx.is_none());
    }

    #[test]
    fn test_online_senders_excludes_offline() {
        let registry = ControllerRegistry::new();
        let (tx1, _rx1) = mpsc::channel(8);
        let (tx2, _rx2) = mpsc::channel(8);
        registry.register("c1".to_string(), mock_info("c1"), tx1, "s1".to_string());
        registry.register("c2".to_string(), mock_info("c2"), tx2, "s2".to_string());
        assert!(registry.mark_offline("c2", "s2"));
        let senders = registry.online_senders();
        assert_eq!(senders.len(), 1);
        assert_eq!(senders[0].0, "c1");
    }

    #[test]
    fn test_remove_drops_entry_and_is_idempotent() {
        let registry = ControllerRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        registry.register("cid".to_string(), mock_info("cid"), tx, "s1".to_string());
        assert!(registry.remove("cid"));
        assert!(registry.get_session("cid").is_none());
        assert!(!registry.remove("cid"));
    }
}
