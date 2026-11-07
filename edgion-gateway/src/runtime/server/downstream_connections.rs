//! Exact downstream connection accounting and admission.
//!
//! Every counted connection holds a [`ConnectionSlot`] whose `Drop` releases
//! it. Listener admission stores its slots in a Pingora `ConnectionPermit`
//! owned by the accepted l4 stream, so the count covers each socket from
//! accept (before PROXY parsing and the TLS handshake) until the stream drops
//! on any exit path. TCPRoute connections and UDPRoute sessions hold their own
//! route-stage slot.
//!
//! The decision is always taken from the value returned by one atomic
//! read-modify-write, so a limit is never exceeded. This module is on the
//! accept hot path: no tracing, no locks.

use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use pingora_core::listeners::{ConnectionAdmission, ConnectionFilter, ConnectionPermit};

use crate::observe::metrics::{global_metrics, DownstreamConnectionRejectReason, GatewayMetrics};
use crate::plugins::stream::StreamPluginConnectionFilter;
use crate::runtime::server::proxy_protocol_policy::ProxyProtocolPolicyRuntime;

/// Live connection count for one scope (process, listener, or route stage).
///
/// Each scope owner (startup wiring, a listener filter, a TCP/UDP proxy) is
/// created once per process or listener and never rebuilt on config reload,
/// so counts survive hot reloads.
#[derive(Debug, Default)]
pub struct ConnectionCounter {
    active: AtomicU64,
    limit: Option<NonZeroU64>,
}

impl ConnectionCounter {
    /// Counter with an optional hard cap enforced by [`Self::try_acquire`].
    pub fn new(limit: Option<NonZeroU64>) -> Self {
        Self {
            active: AtomicU64::new(0),
            limit,
        }
    }

    /// Unlimited counter; the caller decides with the returned ordinal.
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// Count one connection and return its slot plus its admission ordinal:
    /// the number of live connections in this scope including this one. Ignores
    /// `limit`; use it only on unlimited counters (listener and route scopes).
    pub fn acquire(self: &Arc<Self>) -> (ConnectionSlot, u64) {
        debug_assert!(self.limit.is_none(), "acquire() bypasses the cap; use try_acquire()");
        let ordinal = self.active.fetch_add(1, Ordering::Relaxed) + 1;
        (
            ConnectionSlot {
                counter: Arc::clone(self),
            },
            ordinal,
        )
    }

    /// Count one connection unless that would exceed the configured cap. A
    /// rejected attempt never changes the count.
    pub fn try_acquire(self: &Arc<Self>) -> Option<ConnectionSlot> {
        let admitted = match self.limit {
            None => {
                self.active.fetch_add(1, Ordering::Relaxed);
                true
            }
            Some(limit) => self
                .active
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |active| {
                    (active < limit.get()).then_some(active + 1)
                })
                .is_ok(),
        };
        admitted.then(|| ConnectionSlot {
            counter: Arc::clone(self),
        })
    }

    /// Current number of live connections in this scope.
    pub fn active(&self) -> u64 {
        self.active.load(Ordering::Relaxed)
    }
}

/// One counted connection; dropping it releases the count.
#[derive(Debug)]
pub struct ConnectionSlot {
    counter: Arc<ConnectionCounter>,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.counter.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Slots owned by one admitted downstream socket. Also balances the process
/// active-connection gauge.
pub struct DownstreamConnectionPermit {
    metrics: &'static GatewayMetrics,
    _global: ConnectionSlot,
    _listener: ConnectionSlot,
}

impl DownstreamConnectionPermit {
    fn new(metrics: &'static GatewayMetrics, global: ConnectionSlot, listener: ConnectionSlot) -> Self {
        metrics.downstream_connection_opened();
        Self {
            metrics,
            _global: global,
            _listener: listener,
        }
    }
}

impl fmt::Debug for DownstreamConnectionPermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownstreamConnectionPermit").finish_non_exhaustive()
    }
}

impl Drop for DownstreamConnectionPermit {
    fn drop(&mut self) {
        self.metrics.downstream_connection_closed();
    }
}

/// Listener admission sequence: the fatal PROXY-policy guard (nothing is
/// counted and no plugin runs while it is set), then the process cap, the
/// listener slot, and the listener's stream-plugin chain with the listener
/// ordinal. Any rejection, and cancellation of the returned future, drops the
/// slots already taken. The global slot is taken before the listener slot so a
/// process-cap rejection never inflates the listener ordinal (covered by review;
/// the transient listener slot is not observable after a rejection).
pub async fn admit_downstream_connection<F, Fut>(
    metrics: &'static GatewayMetrics,
    fatal: bool,
    global: &Arc<ConnectionCounter>,
    listener: &Arc<ConnectionCounter>,
    run_chain: F,
) -> Option<DownstreamConnectionPermit>
where
    F: FnOnce(u64) -> Fut,
    Fut: Future<Output = bool>,
{
    if fatal {
        return None;
    }
    let Some(global_slot) = global.try_acquire() else {
        metrics.downstream_connection_rejected(DownstreamConnectionRejectReason::Global);
        return None;
    };
    let (listener_slot, ordinal) = listener.acquire();
    if !run_chain(ordinal).await {
        return None;
    }
    Some(DownstreamConnectionPermit::new(metrics, global_slot, listener_slot))
}

/// Always-installed Pingora listener filter; see [`admit_downstream_connection`].
pub struct ListenerAdmissionFilter {
    runtime: Arc<ProxyProtocolPolicyRuntime>,
    /// Process-wide counter shared by every listener (optional hard cap).
    global: Arc<ConnectionCounter>,
    /// This physical listener's live-socket counter.
    listener: Arc<ConnectionCounter>,
    stream_filter: StreamPluginConnectionFilter,
}

impl ListenerAdmissionFilter {
    pub fn new(
        runtime: Arc<ProxyProtocolPolicyRuntime>,
        global: Arc<ConnectionCounter>,
        listener: Arc<ConnectionCounter>,
        stream_filter: StreamPluginConnectionFilter,
    ) -> Self {
        Self {
            runtime,
            global,
            listener,
            stream_filter,
        }
    }

    /// Filter for one physical listener: shares the process counter and owns a
    /// fresh, unlimited listener counter. Build it once per listener service.
    pub fn for_listener(
        runtime: Arc<ProxyProtocolPolicyRuntime>,
        global: Arc<ConnectionCounter>,
        stream_filter: StreamPluginConnectionFilter,
    ) -> Self {
        Self::new(runtime, global, Arc::new(ConnectionCounter::unlimited()), stream_filter)
    }
}

impl fmt::Debug for ListenerAdmissionFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListenerAdmissionFilter")
            .field("fatal", &self.runtime.is_fatal())
            .field("global_active", &self.global.active())
            .field("listener_active", &self.listener.active())
            .finish()
    }
}

#[async_trait]
impl ConnectionFilter for ListenerAdmissionFilter {
    /// Listener admission must go through [`ConnectionFilter::admit`], which
    /// counts the socket for its whole lifetime. A decision-only query cannot
    /// hold a slot, so it fails closed instead of bypassing the guard or caps.
    async fn should_accept(&self, _addr: Option<&SocketAddr>) -> bool {
        false
    }

    async fn admit(&self, addr: Option<&SocketAddr>) -> ConnectionAdmission {
        self.admit_with(|ordinal| self.stream_filter.accept(addr, ordinal))
            .await
    }
}

impl ListenerAdmissionFilter {
    /// The admission body of [`ConnectionFilter::admit`] with the Stage-1 chain
    /// supplied by the caller, so tests can drive this filter's guard and
    /// counter wiring with an accepting chain.
    pub(crate) async fn admit_with<F, Fut>(&self, run_chain: F) -> ConnectionAdmission
    where
        F: FnOnce(u64) -> Fut,
        Fut: Future<Output = bool>,
    {
        let admitted = admit_downstream_connection(
            global_metrics(),
            self.runtime.is_fatal(),
            &self.global,
            &self.listener,
            run_chain,
        )
        .await;
        match admitted {
            Some(permit) => ConnectionAdmission::Accept(Some(ConnectionPermit::new(permit))),
            None => ConnectionAdmission::Reject,
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/runtime/server/downstream_connections_test.rs"]
mod tests;
