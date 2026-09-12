//! UDP Routes and Proxy Implementation
//!
//! This module contains all UDP-related functionality:
//! - Per-port route management ([`routes_mgr`])
//! - UDP proxy implementation ([`edgion_udp`])
//! - Per-port route table ([`udp_route_table`])

mod conf_handler_impl;
mod route_unit;
mod routes_mgr;
pub(crate) mod udp_route_table;

pub mod edgion_udp;

pub use route_unit::UdpRouteUnit;

pub use routes_mgr::{
    get_global_udp_route_managers, GlobalUdpRouteManagers, UdpPortRouteManager, UdpRouteManagerStats,
};

pub use conf_handler_impl::create_udp_route_handler;

pub use udp_route_table::UdpRouteTable;

pub use edgion_udp::EdgionUdpProxy;

use anyhow::{bail, Result};
use edgion_resources::ListenerKey;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::future::Future;
use std::sync::OnceLock;
use tokio::task::JoinHandle;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Process-owned UDP listeners. The port set is frozen after startup.
pub(crate) struct UdpListenerRegistry {
    state: Mutex<RegistryState>,
    shutdown_gate: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct RegistryState {
    stopping: bool,
    listeners: HashMap<ListenerKey, UdpListenerTask>,
    outcome: Option<UdpShutdownOutcome>,
}

struct UdpListenerTask {
    cancel_token: CancellationToken,
    handle: JoinHandle<Result<()>>,
    children: TaskTracker,
    joined: bool,
}

impl Drop for UdpListenerTask {
    fn drop(&mut self) {
        self.cancel_token.cancel();
        self.handle.abort();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct UdpShutdownOutcome {
    pub timed_out: bool,
    pub failed_listeners: usize,
}

impl UdpListenerRegistry {
    fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState::default()),
            shutdown_gate: tokio::sync::Mutex::new(()),
        }
    }

    /// Atomically register and start a listener; never detach an unowned task.
    pub(crate) fn spawn(
        &self,
        key: &ListenerKey,
        cancel_token: CancellationToken,
        children: TaskTracker,
        serve: impl Future<Output = Result<()>> + Send + 'static,
    ) -> Result<()> {
        let mut state = self.state.lock();
        if state.stopping || state.listeners.contains_key(key) {
            cancel_token.cancel();
            bail!("UDP listener {key} cannot start: shutdown started or listener already registered");
        }
        state.listeners.insert(
            key.clone(),
            UdpListenerTask {
                cancel_token,
                handle: tokio::spawn(serve),
                children,
                joined: false,
            },
        );
        Ok(())
    }

    /// Stop all listeners and permanently close registration, without losing handles.
    pub(crate) fn cancel_all(&self) {
        let mut state = self.state.lock();
        state.stopping = true;
        for listener in state.listeners.values() {
            listener.cancel_token.cancel();
        }
    }

    /// Drain under one shared grace deadline, then abort and acknowledge all tasks.
    /// Abort acknowledgement is cooperative and may extend past the grace deadline.
    pub(crate) async fn shutdown(&self, deadline: Instant) -> UdpShutdownOutcome {
        let _gate = self.shutdown_gate.lock().await;
        self.cancel_all();
        let mut listeners = {
            let mut state = self.state.lock();
            if let Some(outcome) = state.outcome {
                return outcome;
            }
            std::mem::take(&mut state.listeners)
        };
        let mut outcome = UdpShutdownOutcome::default();
        let drain = async {
            for listener in listeners.values_mut() {
                if !matches!((&mut listener.handle).await, Ok(Ok(()))) {
                    outcome.failed_listeners += 1;
                }
                listener.joined = true;
                // A completed/aborted parent can no longer spawn children. Its
                // local JoinSet aborts remaining children on drop; track their
                // destructors separately before allowing runtime teardown.
                listener.children.close();
                listener.children.wait().await;
            }
        };
        if timeout_at(deadline, drain).await.is_err() {
            outcome.timed_out = true;
            for listener in listeners.values() {
                if !listener.joined {
                    listener.handle.abort();
                }
            }
            for listener in listeners.values_mut() {
                if !listener.joined {
                    if !matches!((&mut listener.handle).await, Ok(Ok(()))) {
                        outcome.failed_listeners += 1;
                    }
                    listener.joined = true;
                }
                listener.children.close();
                listener.children.wait().await;
            }
        }
        self.state.lock().outcome = Some(outcome);
        outcome
    }
}

static UDP_LISTENER_REGISTRY: OnceLock<UdpListenerRegistry> = OnceLock::new();

pub(crate) fn get_udp_listener_registry() -> &'static UdpListenerRegistry {
    UDP_LISTENER_REGISTRY.get_or_init(UdpListenerRegistry::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::oneshot;
    use tokio::task::JoinSet;

    fn key(port: u16) -> ListenerKey {
        ListenerKey::new(([127, 0, 0, 1], port).into(), "UDP")
    }

    struct DropCount(Arc<AtomicUsize>);

    impl Drop for DropCount {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn shutdown_cancels_every_listener_and_waits_for_cleanup() {
        let registry = UdpListenerRegistry::new();
        let mut tokens = Vec::new();
        let completed = Arc::new(AtomicUsize::new(0));
        for port in [9000, 9001, 9002] {
            let cancel = CancellationToken::new();
            tokens.push(cancel.clone());
            let done = DropCount(completed.clone());
            registry
                .spawn(&key(port), cancel.clone(), TaskTracker::new(), async move {
                    let _done = done;
                    cancel.cancelled().await;
                    tokio::task::yield_now().await;
                    Ok(())
                })
                .unwrap();
        }
        let outcome = registry.shutdown(Instant::now() + Duration::from_secs(1)).await;
        assert_eq!(outcome, UdpShutdownOutcome::default());
        assert!(tokens.iter().all(CancellationToken::is_cancelled));
        assert_eq!(completed.load(Ordering::SeqCst), 3);
        assert!(registry.state.lock().listeners.is_empty());
        assert_eq!(registry.shutdown(Instant::now()).await, outcome);
    }

    #[tokio::test]
    async fn shutdown_waits_for_descendants_after_parent_abort() {
        let registry = UdpListenerRegistry::new();
        let tracker = TaskTracker::new();
        let children = tracker.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        let dropped = Arc::new(AtomicUsize::new(0));
        let child_drop = DropCount(dropped.clone());
        let parent_drop = DropCount(dropped.clone());
        registry
            .spawn(&key(9000), CancellationToken::new(), tracker.clone(), async move {
                let _parent_drop = parent_drop;
                let mut owned_children = JoinSet::new();
                owned_children.spawn(children.track_future(async move {
                    let _child_drop = child_drop;
                    let _ = ready_tx.send(());
                    pending::<()>().await;
                }));
                pending::<()>().await;
                drop(owned_children);
                Ok(())
            })
            .unwrap();
        ready_rx.await.unwrap();

        let outcome = registry.shutdown(Instant::now()).await;
        assert!(outcome.timed_out);
        assert_eq!(outcome.failed_listeners, 1);
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
        assert!(tracker.is_closed());
        assert!(tracker.is_empty());
        assert_eq!(registry.shutdown(Instant::now()).await, outcome);
    }

    #[tokio::test]
    async fn duplicate_and_late_registration_never_spawn() {
        let registry = UdpListenerRegistry::new();
        let original = CancellationToken::new();
        let cancel = original.clone();
        registry
            .spawn(&key(9000), original.clone(), TaskTracker::new(), async move {
                cancel.cancelled().await;
                Ok(())
            })
            .unwrap();
        for stopping in [false, true] {
            if stopping {
                registry.cancel_all();
            }
            let token = CancellationToken::new();
            let (tx, mut rx) = oneshot::channel();
            let rejected = registry.spawn(
                &key(if stopping { 9001 } else { 9000 }),
                token.clone(),
                TaskTracker::new(),
                async move {
                    let _ = tx.send(());
                    Ok(())
                },
            );
            assert!(rejected.is_err());
            assert!(token.is_cancelled());
            assert_eq!(rx.try_recv(), Err(oneshot::error::TryRecvError::Closed));
            assert_eq!(original.is_cancelled(), stopping);
        }
        assert_eq!(
            registry.shutdown(Instant::now() + Duration::from_secs(1)).await,
            UdpShutdownOutcome::default()
        );
    }

    #[tokio::test]
    async fn concurrent_shutdown_callers_observe_the_same_completed_result() {
        let registry = Arc::new(UdpListenerRegistry::new());
        let cancel = CancellationToken::new();
        let (release_tx, release_rx) = oneshot::channel();
        registry
            .spawn(&key(9000), cancel.clone(), TaskTracker::new(), async move {
                release_rx.await.unwrap();
                bail!("simulated listener failure")
            })
            .unwrap();
        let first_registry = registry.clone();
        let first = tokio::spawn(async move { first_registry.shutdown(Instant::now() + Duration::from_secs(1)).await });
        cancel.cancelled().await;
        let second_registry = registry.clone();
        let second =
            tokio::spawn(async move { second_registry.shutdown(Instant::now() + Duration::from_secs(1)).await });
        release_tx.send(()).unwrap();
        let outcome = first.await.unwrap();
        assert!(!outcome.timed_out);
        assert_eq!(outcome.failed_listeners, 1);
        assert_eq!(second.await.unwrap(), outcome);
    }
}
