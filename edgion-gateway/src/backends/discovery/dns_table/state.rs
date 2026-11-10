//! Synchronous core of the Gateway DNS endpoint table.
//!
//! Entries are created, shared and deleted only by Service / configuration
//! events (never by requests), so the table is bounded by the configured
//! `ExternalName` Services. An entry lives exactly as long as some Service
//! references it. Every slice publication goes through [`SlicePublisher`].

use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use edgion_common::dns::endpoint_slices::{build_dns_endpoint_slices, normalize_addrs, DnsSliceOrigin};
use edgion_common::dns::resolver_select::{
    candidate_from_dns_owner, resolve_effective_spec, LinkSysKey, ResolverCandidate,
};
use edgion_common::dns::{DnsAnswer, DnsError, DnsLookupFamily, PreparedHttpDns, ResolverSpec};
use edgion_resources::constants::annotations::edgion::{DNS_LOOKUP_FAMILY, DNS_RESOLUTION_MODE};
use edgion_resources::resources::common::DnsResolutionMode;
use edgion_resources::resources::refs::dns_fields::DnsResolverOwner;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use serde::Serialize;
use tokio::sync::Notify;
use tokio::time::Instant;

use super::policy::DnsRefreshPolicy;

/// Receives the Gateway-local slices of one Service (empty = withdraw).
pub(crate) trait SlicePublisher: Send + Sync {
    fn publish(&self, service_key: &str, slices: Vec<EndpointSlice>);
    /// Service keys that currently hold Gateway-local slices.
    fn published_service_keys(&self) -> BTreeSet<String>;
}

/// Resolves an httpdns LinkSys reference to its prepared generation.
pub(crate) type HttpDnsLookup = dyn Fn(&LinkSysKey) -> Option<Arc<PreparedHttpDns>> + Send + Sync;

/// Configuration inputs shared by every target.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TableInputs {
    pub(crate) policy: DnsRefreshPolicy,
    pub(crate) family: DnsLookupFamily,
    pub(crate) egc_candidate: Option<ResolverCandidate>,
}

/// One DNS question: hostname, resolver identity and address family.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DnsTargetKey {
    pub(crate) hostname: String,
    pub(crate) resolver: String,
    pub(crate) family: DnsLookupFamily,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryStatus {
    /// Never answered yet; requests may wait for the first answer.
    Pending,
    /// Last resolution succeeded.
    Ready,
    /// Last resolution failed transiently; the last good set is kept until `maxStale`.
    Failing,
    /// The resolver answered authoritatively with no address.
    Gone,
}

struct Entry {
    spec: ResolverSpec,
    referrers: BTreeSet<String>,
    addrs: Arc<[IpAddr]>,
    status: EntryStatus,
    last_success: Option<Instant>,
    next_refresh: Instant,
    failures: u32,
    in_flight: bool,
    generation: u64,
    first_answer: Arc<Notify>,
}

struct Binding {
    service: Arc<Service>,
    /// The entry this Service reads, when the table owns it.
    target: Option<DnsTargetKey>,
    /// Owned but refused because the table is full.
    rejected: bool,
    /// Gateway-local slices are currently published for this Service.
    published: bool,
}

/// One resolution the worker must run.
#[derive(Clone, Debug)]
pub(crate) struct ResolveJob {
    pub(crate) target: DnsTargetKey,
    pub(crate) generation: u64,
    pub(crate) spec: ResolverSpec,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsEndpointTableStats {
    /// Distinct hostname/resolver/family targets. Zero once no Service uses the table.
    pub entries: usize,
    pub pending_entries: usize,
    /// `ExternalName` Services bound to an entry.
    pub bound_services: usize,
    /// Services refused because the table was full.
    pub rejected_services: usize,
    /// Services with published Gateway-local slices.
    pub published_services: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsEndpointTableEntryView {
    pub hostname: String,
    pub resolver: String,
    pub family: DnsLookupFamily,
    pub status: EntryStatus,
    pub addresses: Vec<String>,
    pub referrers: Vec<String>,
    pub failures: u32,
    pub last_success_secs_ago: Option<u64>,
    pub next_refresh_in_secs: u64,
}

pub(crate) struct TableState {
    inputs: TableInputs,
    entries: HashMap<DnsTargetKey, Entry>,
    services: HashMap<String, Binding>,
    next_generation: u64,
    pending_entries: usize,
    /// The set of Services bound to pending entries may have changed since the
    /// last `pending_waiters_if_dirty`.
    pending_dirty: bool,
    /// First-answer signals to fire once the caller has published this
    /// mutation's slices and pending map (see `take_released`).
    released: Vec<Arc<Notify>>,
}

fn is_external_name(service: &Service) -> bool {
    service.spec.as_ref().and_then(|spec| spec.type_.as_deref()) == Some("ExternalName")
}

fn annotation<'a>(service: &'a Service, key: &str) -> Option<&'a str> {
    service
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(key))
        .map(String::as_str)
}

fn normalized_hostname(service: &Service) -> Option<String> {
    let name = service.spec.as_ref()?.external_name.as_deref()?.trim();
    let name = name.strip_suffix('.').unwrap_or(name);
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

/// The entry a Service should read, or `None` when the table does not own it
/// (not `ExternalName`, another mode, an invalid annotation, no hostname).
pub(crate) fn desired_target(
    service: &Service,
    inputs: &TableInputs,
    lookup: &HttpDnsLookup,
) -> Option<(DnsTargetKey, ResolverSpec)> {
    if !is_external_name(service) {
        return None;
    }
    let mode = DnsResolutionMode::from_annotation(annotation(service, DNS_RESOLUTION_MODE))
        .ok()?
        .unwrap_or(DnsResolutionMode::global_default(inputs.policy.endpoint_table));
    if mode != DnsResolutionMode::Gateway {
        return None;
    }
    let hostname = normalized_hostname(service)?;
    let family = DnsLookupFamily::from_annotation(annotation(service, DNS_LOOKUP_FAMILY))
        .ok()?
        .unwrap_or(inputs.family);
    let service_candidate = candidate_from_dns_owner(DnsResolverOwner::Service(service));
    let spec = resolve_effective_spec(&[service_candidate.as_ref(), inputs.egc_candidate.as_ref()], |key| {
        lookup(key)
    });
    let key = DnsTargetKey {
        hostname,
        resolver: spec.fingerprint().into_owned(),
        family,
    };
    Some((key, spec))
}

fn same_ports(left: &Service, right: &Service) -> bool {
    left.spec.as_ref().map(|spec| &spec.ports) == right.spec.as_ref().map(|spec| &spec.ports)
}

fn resolver_label(spec: &ResolverSpec) -> String {
    match spec {
        ResolverSpec::System => "system".to_string(),
        ResolverSpec::StandardDns(_) => spec.fingerprint().into_owned(),
        ResolverSpec::HttpDns(_) => "httpdns".to_string(),
    }
}

impl TableState {
    pub(crate) fn new(inputs: TableInputs) -> Self {
        Self {
            inputs,
            entries: HashMap::new(),
            services: HashMap::new(),
            next_generation: 0,
            pending_entries: 0,
            pending_dirty: false,
            released: Vec::new(),
        }
    }

    pub(crate) fn pending_entries(&self) -> usize {
        self.pending_entries
    }

    /// Full Service relist: mark-and-sweep against the snapshot.
    pub(crate) fn replace_services<'a>(
        &mut self,
        data: impl IntoIterator<Item = (&'a String, &'a Arc<Service>)>,
        lookup: &HttpDnsLookup,
        publisher: &dyn SlicePublisher,
        now: Instant,
    ) {
        let mut seen = HashSet::new();
        for (key, service) in data {
            seen.insert(key.clone());
            self.sync_service(key, Some(Arc::clone(service)), lookup, publisher, now);
        }
        let stale: Vec<String> = self
            .services
            .keys()
            .filter(|key| !seen.contains(*key))
            .cloned()
            .collect();
        for key in stale {
            self.sync_service(&key, None, lookup, publisher, now);
        }
        self.retry_rejected(lookup, publisher, now);
    }

    /// Incremental Service events. Removal of a key that is also upserted is ignored.
    pub(crate) fn update_services(
        &mut self,
        upserts: Vec<(String, Arc<Service>)>,
        removes: Vec<String>,
        lookup: &HttpDnsLookup,
        publisher: &dyn SlicePublisher,
        now: Instant,
    ) {
        let upserted: HashSet<&str> = upserts.iter().map(|(key, _)| key.as_str()).collect();
        for key in removes.iter().filter(|key| !upserted.contains(key.as_str())) {
            self.sync_service(key, None, lookup, publisher, now);
        }
        for (key, service) in &upserts {
            self.sync_service(key, Some(Arc::clone(service)), lookup, publisher, now);
        }
        self.retry_rejected(lookup, publisher, now);
    }

    /// Configuration changed (EGC defaults or a resolver source): recompute
    /// every Service's target.
    pub(crate) fn set_inputs(
        &mut self,
        inputs: TableInputs,
        lookup: &HttpDnsLookup,
        publisher: &dyn SlicePublisher,
        now: Instant,
    ) {
        if self.inputs == inputs {
            return;
        }
        self.inputs = inputs;
        self.recompute_all(lookup, publisher, now);
    }

    pub(crate) fn recompute_all(&mut self, lookup: &HttpDnsLookup, publisher: &dyn SlicePublisher, now: Instant) {
        let services: Vec<(String, Arc<Service>)> = self
            .services
            .iter()
            .map(|(key, binding)| (key.clone(), Arc::clone(&binding.service)))
            .collect();
        for (key, service) in services {
            self.sync_service(&key, Some(service), lookup, publisher, now);
        }
        self.retry_rejected(lookup, publisher, now);
    }

    /// Bring one Service in line with its desired target. `None` = Service gone.
    fn sync_service(
        &mut self,
        service_key: &str,
        service: Option<Arc<Service>>,
        lookup: &HttpDnsLookup,
        publisher: &dyn SlicePublisher,
        now: Instant,
    ) {
        let service = service.filter(|service| is_external_name(service));
        let desired = service
            .as_ref()
            .and_then(|service| desired_target(service, &self.inputs, lookup));
        let previous = self.services.remove(service_key);

        let Some(service) = service else {
            if let Some(previous) = previous {
                self.release(service_key, previous, publisher);
            }
            return;
        };

        // Same target: keep the binding; republish only if the ports changed
        // or nothing was published yet.
        if let Some(previous) = previous {
            if previous.target.is_some() && previous.target.as_ref() == desired.as_ref().map(|(key, _)| key) {
                let ports_changed = !same_ports(&previous.service, &service);
                let (key, spec) = desired.expect("same non-empty target");
                if let Some(entry) = self.entries.get_mut(&key) {
                    // Same fingerprint: keep the newest resolver generation.
                    entry.spec = spec;
                }
                self.services.insert(
                    service_key.to_string(),
                    Binding {
                        service,
                        target: Some(key),
                        rejected: false,
                        published: previous.published,
                    },
                );
                if ports_changed || !previous.published {
                    self.publish_service(service_key, publisher);
                }
                return;
            }
            self.release(service_key, previous, publisher);
        }

        let mut binding = Binding {
            service,
            target: None,
            rejected: false,
            published: false,
        };
        if let Some((key, spec)) = desired {
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.referrers.insert(service_key.to_string());
                entry.spec = spec;
                self.pending_dirty |= entry.status == EntryStatus::Pending;
                binding.target = Some(key);
            } else if self.entries.len() >= self.inputs.policy.max_entries {
                if !binding.rejected {
                    tracing::warn!(
                        component = "dns_endpoint_table",
                        service = service_key,
                        max_entries = self.inputs.policy.max_entries,
                        "DNS endpoint table is full; the ExternalName Service gets no endpoints"
                    );
                }
                binding.rejected = true;
            } else {
                self.next_generation += 1;
                self.pending_entries += 1;
                self.pending_dirty = true;
                self.entries.insert(
                    key.clone(),
                    Entry {
                        spec,
                        referrers: BTreeSet::from([service_key.to_string()]),
                        addrs: Arc::from(Vec::new()),
                        status: EntryStatus::Pending,
                        last_success: None,
                        next_refresh: now,
                        failures: 0,
                        in_flight: false,
                        generation: self.next_generation,
                        first_answer: Arc::new(Notify::new()),
                    },
                );
                binding.target = Some(key);
            }
        }
        self.services.insert(service_key.to_string(), binding);
        self.publish_service(service_key, publisher);
    }

    /// Withdraw a Service's slices and drop its reference; delete the entry
    /// when no Service references it any more.
    fn release(&mut self, service_key: &str, binding: Binding, publisher: &dyn SlicePublisher) {
        if binding.published {
            publisher.publish(service_key, Vec::new());
        }
        let Some(target) = binding.target else {
            return;
        };
        let Some(entry) = self.entries.get_mut(&target) else {
            return;
        };
        entry.referrers.remove(service_key);
        let pending = entry.status == EntryStatus::Pending;
        self.pending_dirty |= pending;
        if pending {
            // This Service no longer waits on the entry; its waiters fall
            // through to normal selection instead of timing out.
            self.released.push(Arc::clone(&entry.first_answer));
        }
        if entry.referrers.is_empty() {
            let entry = self.entries.remove(&target).expect("entry present");
            if entry.status == EntryStatus::Pending {
                self.pending_entries -= 1;
            }
        }
    }

    /// Bind Services that were refused while the table was full. Every one is
    /// retried: a rejected Service whose target now exists shares it even when
    /// the table is at its cap (`sync_service` enforces the cap only when it
    /// must create an entry).
    fn retry_rejected(&mut self, lookup: &HttpDnsLookup, publisher: &dyn SlicePublisher, now: Instant) {
        let rejected: Vec<(String, Arc<Service>)> = self
            .services
            .iter()
            .filter(|(_, binding)| binding.rejected)
            .map(|(key, binding)| (key.clone(), Arc::clone(&binding.service)))
            .collect();
        for (key, service) in rejected {
            self.sync_service(&key, Some(service), lookup, publisher, now);
        }
    }

    /// Publish (or withdraw) the slices of one bound Service from its entry.
    fn publish_service(&mut self, service_key: &str, publisher: &dyn SlicePublisher) {
        let Some(binding) = self.services.get_mut(service_key) else {
            return;
        };
        let addrs = binding
            .target
            .as_ref()
            .and_then(|target| self.entries.get(target))
            .map(|entry| Arc::clone(&entry.addrs))
            .unwrap_or_else(|| Arc::from(Vec::new()));
        let slices = match service_key.split_once('/') {
            Some((namespace, name)) => build_dns_endpoint_slices(
                namespace,
                name,
                binding
                    .service
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.ports.as_deref())
                    .unwrap_or(&[]),
                &addrs,
                DnsSliceOrigin::Gateway,
            ),
            None => Vec::new(),
        };
        if slices.is_empty() && !binding.published {
            return;
        }
        binding.published = !slices.is_empty();
        publisher.publish(service_key, slices);
    }

    /// Take up to `limit` due targets and mark them in flight.
    pub(crate) fn take_due(&mut self, now: Instant, limit: usize) -> Vec<ResolveJob> {
        let mut due: Vec<(&DnsTargetKey, &Entry)> = self
            .entries
            .iter()
            .filter(|(_, entry)| !entry.in_flight && entry.next_refresh <= now)
            .collect();
        due.sort_by_key(|(_, entry)| entry.next_refresh);
        let jobs: Vec<ResolveJob> = due
            .into_iter()
            .take(limit)
            .map(|(key, entry)| ResolveJob {
                target: key.clone(),
                generation: entry.generation,
                spec: entry.spec.clone(),
            })
            .collect();
        for job in &jobs {
            if let Some(entry) = self.entries.get_mut(&job.target) {
                entry.in_flight = true;
            }
        }
        jobs
    }

    /// Return a taken job without running it.
    pub(crate) fn cancel_job(&mut self, job: &ResolveJob) {
        if let Some(entry) = self.entries.get_mut(&job.target) {
            if entry.generation == job.generation {
                entry.in_flight = false;
            }
        }
    }

    /// Earliest refresh among targets not in flight.
    pub(crate) fn next_due(&self) -> Option<Instant> {
        self.entries
            .values()
            .filter(|entry| !entry.in_flight)
            .map(|entry| entry.next_refresh)
            .min()
    }

    /// Apply one resolution outcome. A result for a deleted or recreated entry
    /// is discarded. An unchanged address set publishes nothing.
    pub(crate) fn apply_result(
        &mut self,
        job: &ResolveJob,
        result: Result<DnsAnswer, DnsError>,
        now: Instant,
        jitter_unit: f64,
        publisher: &dyn SlicePublisher,
    ) {
        let policy = &self.inputs.policy;
        let Some(entry) = self.entries.get_mut(&job.target) else {
            return;
        };
        if entry.generation != job.generation {
            return;
        }
        entry.in_flight = false;
        let was_pending = entry.status == EntryStatus::Pending;
        let empty: Arc<[IpAddr]> = Arc::from(Vec::new());
        let next_addrs = match result {
            Ok(answer) if !answer.addrs.is_empty() => {
                entry.status = EntryStatus::Ready;
                entry.last_success = Some(now);
                entry.failures = 0;
                entry.next_refresh = schedule(now, policy.success_delay(answer.ttl, jitter_unit));
                Arc::from(normalize_addrs(answer.addrs))
            }
            Ok(_) | Err(DnsError::NoAddresses) => {
                entry.status = EntryStatus::Gone;
                entry.failures = entry.failures.saturating_add(1);
                entry.next_refresh = schedule(now, policy.failure_delay(entry.failures, jitter_unit));
                Arc::clone(&empty)
            }
            Err(_) => {
                entry.status = EntryStatus::Failing;
                entry.failures = entry.failures.saturating_add(1);
                entry.next_refresh = schedule(now, policy.failure_delay(entry.failures, jitter_unit));
                let keep = entry
                    .last_success
                    .is_some_and(|last| now.saturating_duration_since(last) <= policy.max_stale);
                if keep {
                    Arc::clone(&entry.addrs)
                } else {
                    Arc::clone(&empty)
                }
            }
        };
        let changed = next_addrs[..] != entry.addrs[..];
        entry.addrs = next_addrs;
        let referrers: Vec<String> = if changed {
            entry.referrers.iter().cloned().collect()
        } else {
            Vec::new()
        };
        if was_pending {
            self.pending_entries -= 1;
            self.pending_dirty = true;
            // Fired by the caller after the slices below are published, so a
            // woken request observes the new endpoints.
            self.released.push(Arc::clone(&entry.first_answer));
        }
        for service_key in referrers {
            self.publish_service(&service_key, publisher);
        }
    }

    /// First-answer signals of every Service bound to a pending entry, when
    /// that set may have changed since the previous call.
    pub(crate) fn pending_waiters_if_dirty(&mut self) -> Option<HashMap<String, Arc<Notify>>> {
        if !std::mem::take(&mut self.pending_dirty) {
            return None;
        }
        Some(self.pending_waiters())
    }

    /// First-answer signals of every Service bound to a pending entry.
    pub(crate) fn pending_waiters(&self) -> HashMap<String, Arc<Notify>> {
        self.entries
            .values()
            .filter(|entry| entry.status == EntryStatus::Pending)
            .flat_map(|entry| {
                entry
                    .referrers
                    .iter()
                    .map(|service_key| (service_key.clone(), Arc::clone(&entry.first_answer)))
            })
            .collect()
    }

    /// Signals released by the last mutations, to fire after publication.
    pub(crate) fn take_released(&mut self) -> Vec<Arc<Notify>> {
        std::mem::take(&mut self.released)
    }

    /// Withdraw Gateway-local slices of Services this table does not publish
    /// (defense in depth against any divergence from the coordinator).
    pub(crate) fn reconcile_published(&self, publisher: &dyn SlicePublisher) -> usize {
        let mut withdrawn = 0;
        for service_key in publisher.published_service_keys() {
            let owned = self.services.get(&service_key).is_some_and(|binding| binding.published);
            if !owned {
                publisher.publish(&service_key, Vec::new());
                withdrawn += 1;
            }
        }
        withdrawn
    }

    pub(crate) fn stats(&self) -> DnsEndpointTableStats {
        DnsEndpointTableStats {
            entries: self.entries.len(),
            pending_entries: self.pending_entries,
            bound_services: self
                .services
                .values()
                .filter(|binding| binding.target.is_some())
                .count(),
            rejected_services: self.services.values().filter(|binding| binding.rejected).count(),
            published_services: self.services.values().filter(|binding| binding.published).count(),
        }
    }

    pub(crate) fn entries_view(&self, now: Instant, limit: usize) -> Vec<DnsEndpointTableEntryView> {
        let mut views: Vec<DnsEndpointTableEntryView> = self
            .entries
            .iter()
            .map(|(key, entry)| DnsEndpointTableEntryView {
                hostname: key.hostname.clone(),
                resolver: resolver_label(&entry.spec),
                family: key.family,
                status: entry.status,
                addresses: entry.addrs.iter().map(ToString::to_string).collect(),
                referrers: entry.referrers.iter().cloned().collect(),
                failures: entry.failures,
                last_success_secs_ago: entry
                    .last_success
                    .map(|last| now.saturating_duration_since(last).as_secs()),
                next_refresh_in_secs: entry.next_refresh.saturating_duration_since(now).as_secs(),
            })
            .collect();
        views.sort_by(|left, right| (&left.hostname, &left.resolver).cmp(&(&right.hostname, &right.resolver)));
        views.truncate(limit);
        views
    }
}

/// `now + delay`, saturating far in the future instead of overflowing.
fn schedule(now: Instant, delay: Duration) -> Instant {
    now.checked_add(delay)
        .unwrap_or_else(|| now + Duration::from_secs(u64::from(u32::MAX)))
}

/// Bound on one resolution, independent of the resolver's own timeouts.
pub(crate) const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
#[path = "../../../../tests/unit/backends/discovery/dns_table/state_test.rs"]
mod tests;
