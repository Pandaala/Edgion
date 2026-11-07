//! Per-destination health for managed outbound clients.
//!
//! A destination is one ready endpoint of a Service-pool target, or the single
//! URL / ClusterIP destination of a static target. Each destination owns the
//! canonical [`PassiveEjectionState`] (time-bounded ejection, one half-open
//! probe lease) plus an independent active-probe verdict.
//!
//! Request selection reads an `ArcSwap` snapshot of destinations and lock-free
//! flags. The snapshot is rebuilt only when the endpoint section published for
//! the Service port changes, which also prunes state of departed endpoints.
//! Mutexes are taken only on the failure path, on a destination that may be
//! ejected, and on a success that follows recorded failures.
//!
//! Upstream-authority calls dial through the pinned resolver and the client
//! connection pool, so their destination is not chosen here. They share one
//! pool-level destination with the same bounded-ejection semantics.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use parking_lot::Mutex;

use edgion_resources::resources::common::external_http::{ActiveHealthCheck, PassiveHealthCheck};
use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;

use crate::backends::discovery::endpoint_slice::ready_section_sockets;
use crate::backends::discovery::{get_roundrobin_store, ResolvedServiceSection};
use crate::backends::outlier_detection::PassiveEjectionState;
use crate::backends::outlier_detection::{EjectionAdmission, FailureClass, FailureTransition, HalfOpenProbe};
use crate::backends::validate_endpoint_backend_addr;

/// Every passive failure uses one class so one consecutive counter decides
/// ejection, whatever mix of statuses, transport errors and timeouts occurs.
const PASSIVE_FAILURE_CLASS: FailureClass = FailureClass::Gateway;

/// Destination source for an [`OutboundHealth`].
pub(crate) enum HealthTarget {
    /// One fixed destination (URL target or ClusterIP DNS name).
    Static(String),
    /// Ready endpoints of one Service port, read from the endpoint store.
    Service {
        service_key: String,
        port: Option<i32>,
        scheme: &'static str,
    },
}

/// Health of one outbound destination.
pub struct DestHealth {
    addr: Option<SocketAddr>,
    url: String,
    dial_allowed: bool,
    state: Arc<PassiveEjectionState>,
    /// Lock-free mirror of "the passive state may be open". A stale `true`
    /// only costs a locked admission check; it is cleared after a success.
    maybe_open: AtomicBool,
    /// Set after a failure so the next success resets the passive counter.
    failures_pending: AtomicBool,
    active_down: AtomicBool,
    active_failures: AtomicU32,
    active_successes: AtomicU32,
}

impl DestHealth {
    fn new(
        addr: Option<SocketAddr>,
        url: String,
        dial_allowed: bool,
        config: Option<OutlierDetectionResolved>,
    ) -> Self {
        Self {
            addr,
            url,
            dial_allowed,
            state: PassiveEjectionState::new(config),
            maybe_open: AtomicBool::new(false),
            failures_pending: AtomicBool::new(false),
            active_down: AtomicBool::new(false),
            active_failures: AtomicU32::new(0),
            active_successes: AtomicU32::new(0),
        }
    }

    /// Endpoint socket for Service-pool destinations; `None` for static ones.
    pub fn addr(&self) -> Option<SocketAddr> {
        self.addr
    }

    /// Base URL (`scheme://host[:port]`) of this destination.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Clear the open mirror after a success, restoring it if a concurrent
    /// failure reopened the destination in between.
    fn clear_open(&self) {
        self.maybe_open.store(false, Ordering::SeqCst);
        if self.state.is_open() {
            self.maybe_open.store(true, Ordering::SeqCst);
        }
    }
}

struct DestSnapshot {
    section: Option<Arc<ResolvedServiceSection>>,
    dests: Arc<[Arc<DestHealth>]>,
}

struct PassivePolicy {
    resolved: OutlierDetectionResolved,
    count_timeout: bool,
    failure_status_codes: Vec<u16>,
}

/// Why no destination could be selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickError {
    /// No ready destination exists, or every destination is unsafe to dial.
    NoEndpoint,
    /// Destinations exist but all are ejected or actively unhealthy.
    Unhealthy,
}

/// One admitted destination for one attempt. Holding a half-open probe lease
/// makes this attempt the sole recovery probe of an ejected destination;
/// dropping it without a verdict releases the lease.
pub(crate) struct Pick {
    dest: Arc<DestHealth>,
    probe: Option<HalfOpenProbe>,
}

impl Pick {
    pub(crate) fn dest(&self) -> &DestHealth {
        &self.dest
    }

    /// Admit this destination again for a follow-up attempt. Returns `None`
    /// when it was ejected or marked actively unhealthy meanwhile.
    pub(crate) fn readmit(&self) -> Option<Pick> {
        admit(&self.dest)
    }

    /// The same destination without a probe lease, for a follow-up attempt.
    pub(crate) fn without_probe(&self) -> Pick {
        Pick {
            dest: Arc::clone(&self.dest),
            probe: None,
        }
    }
}

/// Passive classification of one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptOutcome {
    Success,
    Failure,
    /// Not evidence about the destination (for example a local size cap).
    Ignore,
}

/// Per-destination health shared by one managed client and its active task.
pub struct OutboundHealth {
    target: HealthTarget,
    passive: Option<PassivePolicy>,
    snapshot: ArcSwap<DestSnapshot>,
    rebuild_lock: Mutex<()>,
    eject_lock: Mutex<()>,
    cursor: AtomicUsize,
    /// Pool-level destination for upstream-authority calls. For a static
    /// target it is the single destination itself.
    pool: Arc<DestHealth>,
}

impl OutboundHealth {
    pub(crate) fn new(target: HealthTarget, passive: Option<&PassiveHealthCheck>) -> Arc<Self> {
        let passive = passive.map(|p| PassivePolicy {
            resolved: p.resolve(),
            count_timeout: p.count_timeout,
            failure_status_codes: p.failure_status_codes.clone(),
        });
        let config = passive.as_ref().map(|p| p.resolved.clone());
        let (pool, dests): (Arc<DestHealth>, Arc<[Arc<DestHealth>]>) = match &target {
            HealthTarget::Static(url) => {
                // Static URLs keep their literal-address guard at dial time.
                let dest = Arc::new(DestHealth::new(None, url.clone(), true, config));
                (Arc::clone(&dest), Arc::from(vec![dest]))
            }
            HealthTarget::Service { .. } => (
                Arc::new(DestHealth::new(None, String::new(), true, config)),
                Arc::from(Vec::new()),
            ),
        };
        Arc::new(Self {
            target,
            passive,
            snapshot: ArcSwap::from_pointee(DestSnapshot { section: None, dests }),
            rebuild_lock: Mutex::new(()),
            eject_lock: Mutex::new(()),
            cursor: AtomicUsize::new(0),
            pool,
        })
    }

    /// Whether a timeout counts as a passive failure.
    pub(crate) fn count_timeout(&self) -> bool {
        self.passive.as_ref().is_none_or(|p| p.count_timeout)
    }

    /// Whether a terminal HTTP status counts as a passive failure.
    pub(crate) fn is_failure_status(&self, status: u16) -> bool {
        self.passive
            .as_ref()
            .is_some_and(|p| p.failure_status_codes.contains(&status))
    }

    /// Current destinations, rebuilt when the published endpoint section changes.
    fn current(&self) -> Arc<DestSnapshot> {
        let HealthTarget::Service {
            service_key,
            port,
            scheme,
        } = &self.target
        else {
            return self.snapshot.load_full();
        };
        let section = get_roundrobin_store().ready_section_for_port(service_key, *port);
        let snapshot = self.snapshot.load_full();
        if same_section(&section, &snapshot.section) {
            return snapshot;
        }
        self.rebuild(section, scheme)
    }

    /// Cold path: rebuild destinations for a newly published section,
    /// carrying over state of endpoints that are still present.
    fn rebuild(&self, section: Option<Arc<ResolvedServiceSection>>, scheme: &str) -> Arc<DestSnapshot> {
        let _guard = self.rebuild_lock.lock();
        let previous = self.snapshot.load_full();
        if same_section(&section, &previous.section) {
            return previous;
        }
        let config = self.passive.as_ref().map(|p| p.resolved.clone());
        let addrs = section.as_deref().map(ready_section_sockets).unwrap_or_default();
        let dests = carry_over(&previous.dests, addrs, scheme, config.as_ref());
        let next = Arc::new(DestSnapshot {
            section,
            dests: Arc::from(dests),
        });
        self.snapshot.store(Arc::clone(&next));
        next
    }

    /// Select a usable destination, round-robin, avoiding `tried` endpoints
    /// when another usable endpoint exists.
    pub(crate) fn pick(&self, tried: &[SocketAddr]) -> Result<Pick, PickError> {
        let snapshot = self.current();
        let dests = &snapshot.dests;
        if dests.is_empty() {
            return Err(PickError::NoEndpoint);
        }
        let start = self.cursor.fetch_add(1, Ordering::Relaxed);
        let mut saw_allowed = false;
        for allow_tried in [false, true] {
            if allow_tried && tried.is_empty() {
                break;
            }
            for offset in 0..dests.len() {
                let dest = &dests[start.wrapping_add(offset) % dests.len()];
                if !dest.dial_allowed {
                    continue;
                }
                saw_allowed = true;
                let was_tried = dest.addr.is_some_and(|addr| tried.contains(&addr));
                if was_tried != allow_tried {
                    continue;
                }
                if let Some(pick) = admit(dest) {
                    return Ok(pick);
                }
            }
        }
        Err(if saw_allowed {
            PickError::Unhealthy
        } else {
            PickError::NoEndpoint
        })
    }

    /// Admit an upstream-authority call through the pool-level destination.
    pub(crate) fn pick_pool(&self) -> Result<Pick, PickError> {
        admit(&self.pool).ok_or(PickError::Unhealthy)
    }

    /// Record the passive verdict of one attempt against its destination.
    pub(crate) fn record(&self, pick: Pick, outcome: AttemptOutcome) {
        let Pick { dest, probe } = pick;
        if self.passive.is_none() {
            return;
        }
        match outcome {
            AttemptOutcome::Ignore => drop(probe),
            AttemptOutcome::Success => {
                if let Some(probe) = probe {
                    probe.success();
                    dest.failures_pending.store(false, Ordering::Release);
                    dest.clear_open();
                } else if dest.failures_pending.swap(false, Ordering::AcqRel) {
                    dest.state.record_success();
                    dest.clear_open();
                }
            }
            AttemptOutcome::Failure => {
                if let Some(probe) = probe {
                    probe.failure(PASSIVE_FAILURE_CLASS);
                    dest.failures_pending.store(true, Ordering::Release);
                    return;
                }
                let candidate = dest.state.record_failure_candidate(PASSIVE_FAILURE_CLASS);
                dest.failures_pending.store(true, Ordering::Release);
                let Some(candidate) = candidate else {
                    return;
                };
                if self.finish_failure(&dest, candidate) == FailureTransition::Opened {
                    dest.maybe_open.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    /// Open a threshold-reaching destination unless the max-ejection-percent
    /// cap of its group is already reached. Failure path only.
    fn finish_failure(
        &self,
        dest: &Arc<DestHealth>,
        candidate: crate::backends::outlier_detection::state::FailureCandidate,
    ) -> FailureTransition {
        let percent = self.passive.as_ref().map_or(0, |p| p.resolved.max_ejection_percent);
        if percent == 0 {
            return dest.state.finish_failure(candidate, true);
        }
        let _guard = self.eject_lock.lock();
        if Arc::ptr_eq(dest, &self.pool) {
            return dest.state.finish_failure(candidate, true);
        }
        let snapshot = self.snapshot.load_full();
        let total = snapshot.dests.len().max(1);
        let maximum = ((total as u64 * u64::from(percent) / 100) as usize).max(1);
        let open = snapshot
            .dests
            .iter()
            .filter(|other| other.state.is_open())
            .take(maximum)
            .count();
        dest.state.finish_failure(candidate, open < maximum)
    }

    /// Current destinations eligible for an active probe (safe to dial).
    pub(crate) fn probe_targets(&self) -> Vec<Arc<DestHealth>> {
        self.current()
            .dests
            .iter()
            .filter(|dest| dest.dial_allowed)
            .cloned()
            .collect()
    }

    /// Pool-level destination used by upstream-authority calls.
    pub(crate) fn pool(&self) -> &Arc<DestHealth> {
        &self.pool
    }

    /// Apply one active-probe verdict to a destination. Active success at the
    /// healthy threshold also lifts a passive ejection.
    pub(crate) fn record_active(&self, dest: &DestHealth, active: &ActiveHealthCheck, success: bool) {
        if success {
            dest.active_failures.store(0, Ordering::Relaxed);
            let successes = dest.active_successes.fetch_add(1, Ordering::Relaxed).saturating_add(1);
            if successes >= active.healthy_threshold {
                dest.active_down.store(false, Ordering::Release);
                if self.passive.is_some() && dest.maybe_open.load(Ordering::Acquire) {
                    dest.failures_pending.store(false, Ordering::Release);
                    dest.state.record_success();
                    dest.clear_open();
                }
            }
        } else {
            dest.active_successes.store(0, Ordering::Relaxed);
            let failures = dest.active_failures.fetch_add(1, Ordering::Relaxed).saturating_add(1);
            if failures >= active.unhealthy_threshold {
                dest.active_down.store(true, Ordering::Release);
            }
        }
    }
}

#[cfg(test)]
include!("../../../../../tests/support/plugins/http/common/outbound/health/test_helpers.rs");

/// Destinations for `addrs`, reusing the state of endpoints still present so
/// churn neither resets nor leaks per-endpoint health.
fn carry_over(
    previous: &[Arc<DestHealth>],
    addrs: Vec<SocketAddr>,
    scheme: &str,
    config: Option<&OutlierDetectionResolved>,
) -> Vec<Arc<DestHealth>> {
    addrs
        .into_iter()
        .map(|addr| {
            previous
                .iter()
                .find(|dest| dest.addr == Some(addr))
                .cloned()
                .unwrap_or_else(|| {
                    let allowed = validate_endpoint_backend_addr(
                        &pingora_core::protocols::l4::socket::SocketAddr::Inet(addr),
                        false,
                    )
                    .is_ok();
                    Arc::new(DestHealth::new(
                        Some(addr),
                        format!("{scheme}://{addr}"),
                        allowed,
                        config.cloned(),
                    ))
                })
        })
        .collect()
}

fn same_section(left: &Option<Arc<ResolvedServiceSection>>, right: &Option<Arc<ResolvedServiceSection>>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        _ => false,
    }
}

/// Lock-free admission for closed destinations; the canonical state machine
/// decides only when the destination may be ejected.
fn admit(dest: &Arc<DestHealth>) -> Option<Pick> {
    if dest.active_down.load(Ordering::Acquire) {
        return None;
    }
    if !dest.maybe_open.load(Ordering::SeqCst) {
        return Some(Pick {
            dest: Arc::clone(dest),
            probe: None,
        });
    }
    match dest.state.admit() {
        EjectionAdmission::Closed => Some(Pick {
            dest: Arc::clone(dest),
            probe: None,
        }),
        EjectionAdmission::Rejected => None,
        EjectionAdmission::Probe(probe) => Some(Pick {
            dest: Arc::clone(dest),
            probe: Some(probe),
        }),
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/common/outbound/health_test.rs"]
mod tests;
