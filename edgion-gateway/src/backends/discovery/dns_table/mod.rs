//! Gateway DNS endpoint table for `ExternalName` Services in `gateway` mode.
//!
//! Envoy `STRICT_DNS` shape: a background worker resolves each distinct
//! hostname / resolver / family target, and the full normalized address set
//! is published as Gateway-local EndpointSlices into the discovery
//! coordinator, so ordinary LB, health checks and outlier ejection apply on
//! every protocol. Requests never resolve and never create entries; the only
//! request-path interaction is a bounded wait for a target's first answer.
//!
//! Lifecycle is reference-driven: Service / EGC / LinkSys events bind and
//! release Services, an entry is deleted with its last referrer, a Service
//! relist is a mark-and-sweep, and the worker periodically withdraws any
//! Gateway-local slice the table does not own.

mod policy;
mod state;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use arc_swap::ArcSwap;
use edgion_common::dns::{resolve_answer, DnsError, DnsQueryPolicy};
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use parking_lot::Mutex;
use tokio::sync::{Notify, Semaphore};
use tokio::time::Instant;

pub use policy::DnsRefreshPolicy;
pub use state::{DnsEndpointTableEntryView, DnsEndpointTableStats, EntryStatus};
use state::{HttpDnsLookup, ResolveJob, SlicePublisher, TableInputs, TableState, RESOLVE_TIMEOUT};

use super::coordinator::{get_service_discovery_coordinator, ServiceDiscoveryCoordinator};

/// Most resolutions running at once, so a relist cannot burst the resolver.
const MAX_CONCURRENT_RESOLUTIONS: usize = 16;
/// How long a request waits for a target's first answer before selecting
/// from whatever endpoints exist (none: 503).
pub const FIRST_ANSWER_WAIT: Duration = Duration::from_secs(2);
/// Period of the coordinator reconciliation pass.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);
/// Most entries listed by the admin view.
const MAX_LISTED_ENTRIES: usize = 256;

/// Destination of the table's publications.
pub(crate) trait SliceSink: Send + Sync {
    /// Apply every Service's replacement slices in one batch, in order.
    fn publish_batch(&self, batch: Vec<(String, Vec<EndpointSlice>)>);
    fn published_service_keys(&self) -> BTreeSet<String>;
}

struct CoordinatorSink(&'static Arc<ServiceDiscoveryCoordinator>);

impl SliceSink for CoordinatorSink {
    fn publish_batch(&self, batch: Vec<(String, Vec<EndpointSlice>)>) {
        self.0.replace_gateway_dns_slices_batch(batch);
    }

    fn published_service_keys(&self) -> BTreeSet<String> {
        self.0.gateway_dns_service_keys()
    }
}

/// Collects one mutation's publications so they reach the coordinator as a
/// single transaction instead of one full-state transaction per Service.
struct BatchingPublisher<'a> {
    sink: &'a dyn SliceSink,
    batch: Mutex<Vec<(String, Vec<EndpointSlice>)>>,
}

impl SlicePublisher for BatchingPublisher<'_> {
    fn publish(&self, service_key: &str, slices: Vec<EndpointSlice>) {
        self.batch.lock().push((service_key.to_string(), slices));
    }

    fn published_service_keys(&self) -> BTreeSet<String> {
        self.sink.published_service_keys()
    }
}

/// Process-wide Gateway DNS endpoint table.
pub struct DnsEndpointTable {
    state: Mutex<TableState>,
    /// Mirror of `TableState::pending_entries` for the lock-free request fast path.
    pending_entries: AtomicUsize,
    /// Services bound to a pending entry and their first-answer signals,
    /// republished after every mutation that changes the set. The request path
    /// reads only this, never the state lock.
    pending_waiters: ArcSwap<HashMap<String, Arc<Notify>>>,
    wake: Notify,
    sink: Box<dyn SliceSink>,
    lookup: Box<HttpDnsLookup>,
}

static TABLE: LazyLock<DnsEndpointTable> = LazyLock::new(|| {
    DnsEndpointTable::new(
        Box::new(CoordinatorSink(get_service_discovery_coordinator())),
        Box::new(|key| crate::link_sys::runtime::store::get_httpdns_client(&format!("{}/{}", key.namespace, key.name))),
    )
});

/// The process-wide table.
pub fn dns_endpoint_table() -> &'static DnsEndpointTable {
    &TABLE
}

fn current_inputs() -> TableInputs {
    match crate::config::get_egc_dns_resolver_default() {
        Some(default) => TableInputs {
            policy: default.refresh_policy().clone(),
            family: default.lookup_family(),
            egc_candidate: default.candidate().cloned(),
        },
        None => TableInputs::default(),
    }
}

impl DnsEndpointTable {
    fn new(sink: Box<dyn SliceSink>, lookup: Box<HttpDnsLookup>) -> Self {
        Self {
            state: Mutex::new(TableState::new(TableInputs::default())),
            pending_entries: AtomicUsize::new(0),
            pending_waiters: ArcSwap::from_pointee(HashMap::new()),
            wake: Notify::new(),
            sink,
            lookup,
        }
    }

    /// Run one mutation under the state lock. Its publications reach the
    /// coordinator as one batch, then the pending map is republished, and only
    /// then are released first-answer waiters woken, so a woken request sees
    /// the new endpoints and no stale pending entry.
    fn mutate(&self, apply: impl FnOnce(&mut TableState, &HttpDnsLookup, &dyn SlicePublisher, Instant)) {
        let mut state = self.state.lock();
        let publisher = BatchingPublisher {
            sink: self.sink.as_ref(),
            batch: Mutex::new(Vec::new()),
        };
        apply(&mut state, self.lookup.as_ref(), &publisher, Instant::now());
        let batch = publisher.batch.into_inner();
        if !batch.is_empty() {
            self.sink.publish_batch(batch);
        }
        if let Some(waiters) = state.pending_waiters_if_dirty() {
            self.pending_waiters.store(Arc::new(waiters));
        }
        self.pending_entries.store(state.pending_entries(), Ordering::Release);
        let released = state.take_released();
        drop(state);
        for notify in released {
            notify.notify_waiters();
        }
        self.wake.notify_one();
    }

    /// Full Service relist (mark-and-sweep).
    pub fn on_services_full(&self, data: &HashMap<String, Arc<Service>>) {
        self.mutate(|state, lookup, publisher, now| state.replace_services(data, lookup, publisher, now));
    }

    /// Incremental Service events.
    pub fn on_services_partial(
        &self,
        add: &HashMap<String, Arc<Service>>,
        update: &HashMap<String, Arc<Service>>,
        remove: &HashSet<String>,
    ) {
        let upserts: Vec<(String, Arc<Service>)> = add
            .iter()
            .chain(update.iter())
            .map(|(key, service)| (key.clone(), Arc::clone(service)))
            .collect();
        let removes: Vec<String> = remove.iter().cloned().collect();
        self.mutate(|state, lookup, publisher, now| state.update_services(upserts, removes, lookup, publisher, now));
    }

    /// The EGC DNS defaults changed: reload inputs and recompute every target.
    /// The snapshot is read under the table lock, so an older reader can never
    /// overwrite the inputs of a newer EGC publication. Unchanged inputs are a
    /// no-op.
    pub fn on_settings_changed(&self) {
        self.mutate(|state, lookup, publisher, now| state.set_inputs(current_inputs(), lookup, publisher, now));
    }

    /// A resolver source (httpdns LinkSys) changed: recompute every target.
    pub fn on_resolver_sources_changed(&self) {
        self.mutate(|state, lookup, publisher, now| state.recompute_all(lookup, publisher, now));
    }

    /// True while the Service's target has no first answer yet. Lock-free.
    pub fn is_pending(&self, service_key: &str) -> bool {
        self.pending_entries.load(Ordering::Acquire) != 0 && self.pending_waiters.load().contains_key(service_key)
    }

    /// Wait (bounded) for the first answer of a Service whose target is still
    /// pending. Lock-free: one atomic load in the steady state, one map lookup
    /// while any target is pending.
    pub async fn wait_first_answer(&self, service_key: &str) {
        if self.pending_entries.load(Ordering::Acquire) == 0 {
            return;
        }
        let Some(notify) = self.pending_waiters.load().get(service_key).cloned() else {
            return;
        };
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // `mutate` republishes the map before waking waiters: if the answer
        // landed before registration, the key is already gone here.
        if !self.pending_waiters.load().contains_key(service_key) {
            return;
        }
        let _ = tokio::time::timeout(FIRST_ANSWER_WAIT, notified).await;
    }

    pub fn stats(&self) -> DnsEndpointTableStats {
        self.state.lock().stats()
    }

    pub fn entries_view(&self) -> Vec<DnsEndpointTableEntryView> {
        self.state.lock().entries_view(Instant::now(), MAX_LISTED_ENTRIES)
    }

    fn apply_result(&self, job: &ResolveJob, result: Result<edgion_common::dns::DnsAnswer, DnsError>) {
        let jitter_unit: f64 = rand::random();
        self.mutate(|state, _, publisher, now| state.apply_result(job, result, now, jitter_unit, publisher));
    }

    /// Withdraw Gateway-local slices the table does not own.
    fn reconcile(&self) {
        let mut withdrawn = 0;
        self.mutate(|state, _, publisher, _| withdrawn = state.reconcile_published(publisher));
        if withdrawn > 0 {
            tracing::warn!(
                component = "dns_endpoint_table",
                withdrawn,
                "Withdrew Gateway-local DNS slices not owned by the endpoint table"
            );
        }
    }

    /// Background worker: runs due resolutions, at most
    /// `MAX_CONCURRENT_RESOLUTIONS` at a time, until shutdown.
    pub async fn run(&'static self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_RESOLUTIONS));
        let mut reconcile = tokio::time::interval(RECONCILE_INTERVAL);
        reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Pick up anything bound before the worker started.
        self.on_settings_changed();
        loop {
            let jobs = self.state.lock().take_due(Instant::now(), permits.available_permits());
            for job in jobs {
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    // Only this loop takes permits, so this cannot happen; never
                    // leave a target marked in flight without a task.
                    self.state.lock().cancel_job(&job);
                    continue;
                };
                tokio::spawn(async move {
                    // The lookup runs in its own task so a panic inside a
                    // resolver backend becomes an ordinary failure result and
                    // never leaves the target marked in flight.
                    let spec = job.spec.clone();
                    let hostname = job.target.hostname.clone();
                    let family = job.target.family;
                    let lookup = tokio::spawn(async move {
                        tokio::time::timeout(
                            RESOLVE_TIMEOUT,
                            resolve_answer(&spec, &hostname, DnsQueryPolicy::Family(family)),
                        )
                        .await
                    });
                    let result = match lookup.await {
                        Ok(Ok(result)) => result,
                        Ok(Err(_)) => Err(DnsError::Io("resolution timed out".to_string())),
                        Err(_) => Err(DnsError::Io("resolution task failed".to_string())),
                    };
                    // Release the permit before applying: applying wakes the loop,
                    // which must then see the freed permit.
                    drop(permit);
                    self.apply_result(&job, result);
                });
            }
            // With every permit busy, due targets cannot start; wait for a
            // completion's wake instead of spinning on a past due instant.
            let next_due = if permits.available_permits() == 0 {
                None
            } else {
                self.state.lock().next_due()
            };
            let sleep_until = next_due.unwrap_or_else(|| Instant::now() + RECONCILE_INTERVAL);
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep_until(sleep_until) => {}
                _ = reconcile.tick() => self.reconcile(),
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
        tracing::info!(component = "dns_endpoint_table", "DNS endpoint table worker stopped");
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/discovery/dns_table/mod_test.rs"]
mod tests;
