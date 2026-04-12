//! ControllerRegistry: manages per-controller sessions and stream handles.

use crate::core::common::fed_sync::proto::{CenterMessage, RegisterRequest};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Debug)]
pub struct ControllerSession {
    pub controller_id: String,
    pub session_id: String,
    pub info: RegisterRequest,
    pub stream_tx: mpsc::Sender<CenterMessage>,
    pub last_seen: Instant,
    pub offline_since: Option<Instant>,
}

impl ControllerSession {
    pub fn is_online(&self) -> bool {
        self.offline_since.is_none()
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
            stream_tx,
            last_seen: Instant::now(),
            offline_since: None,
        };
        self.inner.write().insert(controller_id, session);
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

    pub fn mark_offline(&self, controller_id: &str) {
        if let Some(session) = self.inner.write().get_mut(controller_id) {
            if session.offline_since.is_none() {
                session.offline_since = Some(Instant::now());
                tracing::info!(
                    component = "registry",
                    controller_id = %controller_id,
                    "Controller marked offline"
                );
            }
        }
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
            .filter(|s| s.is_online())
            .map(|s| (s.controller_id.clone(), s.stream_tx.clone()))
            .collect()
    }

    pub fn evict_stale(&self, hours: u64) {
        let threshold = Duration::from_secs(hours * 3600);
        self.inner.write().retain(|id, session| {
            if let Some(since) = session.offline_since {
                if since.elapsed() > threshold {
                    tracing::info!(
                        component = "registry",
                        controller_id = %id,
                        "Evicting stale offline controller"
                    );
                    return false;
                }
            }
            true
        });
    }
}

#[derive(Debug, Clone)]
pub struct SessionView {
    pub controller_id: String,
    pub session_id: String,
    pub info: RegisterRequest,
    pub stream_tx: mpsc::Sender<CenterMessage>,
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

    #[test]
    fn test_mark_offline_and_evict() {
        let registry = ControllerRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        registry.register("cid".to_string(), mock_info("cid"), tx, "s1".to_string());
        registry.mark_offline("cid");
        assert!(registry
            .get_session("cid")
            .map(|s| s.offline_since.is_some())
            .unwrap_or(false));
        registry.evict_stale(0);
        assert!(registry.get_session("cid").is_none());
    }

    #[test]
    fn test_online_senders_excludes_offline() {
        let registry = ControllerRegistry::new();
        let (tx1, _rx1) = mpsc::channel(8);
        let (tx2, _rx2) = mpsc::channel(8);
        registry.register("c1".to_string(), mock_info("c1"), tx1, "s1".to_string());
        registry.register("c2".to_string(), mock_info("c2"), tx2, "s2".to_string());
        registry.mark_offline("c2");
        let senders = registry.online_senders();
        assert_eq!(senders.len(), 1);
        assert_eq!(senders[0].0, "c1");
    }
}
