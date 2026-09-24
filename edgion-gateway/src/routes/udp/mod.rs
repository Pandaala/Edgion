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
#[path = "../../../tests/unit/routes/udp/mod_test.rs"]
mod tests;
