use super::cert_matcher::{AttachmentMatchSlot, TlsCertMatcher, TlsCertMatcherSnapshot, MAX_ATTACHMENT_CANDIDATES};
use super::compiled_tls::CompiledEdgionTls;
use crate::tls::validation::cert::{validate_cert, CertValidationResult};
use edgion_algo::matcher::rfc6125_hostname::canonical_key;
use edgion_resources::listener_key::ListenerKey;
use edgion_resources::resources::edgion_tls::ResolvedListenerAttachment;
use edgion_resources::EdgionTls;
use rpds::{HashTrieMapSync as PersistentHashMap, HashTrieSetSync as PersistentHashSet};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

pub(crate) const MAX_EDGION_TLS_COLLISION_LOG_PARTICIPANTS: usize = 64;

/// Internal entry for TLS certificate with validation status
#[derive(Clone)]
pub(super) struct TlsEntry {
    /// The accepted resource plus its cold-compiled handshake state. Exactly one
    /// `Arc<EdgionTls>` exists per entry (`compiled.tls`), so every `Arc::ptr_eq`
    /// identity check below keeps the meaning it had when this was a bare
    /// `Arc<EdgionTls>` field.
    compiled: Arc<CompiledEdgionTls>,
    validation: CertValidationResult,
    matchable: bool,
    /// true once a bad update was rejected for this key; used to log only on
    /// the valid->rejected transition (cleared when a valid update lands).
    rejected_active: bool,
}

/// Deterministic precedence for EdgionTls resources colliding on the same host.
///
/// Mirrors the Gateway API route-conflict convention and
/// `BackendTLSPolicyStore::sort_policies_by_precedence`: oldest `creationTimestamp`
/// wins (`Option<Time>` orders `None` first), then namespace, then name. The winner
/// is `tls_list.first()` after this sort.
fn edgion_tls_precedence(a: &Arc<CompiledEdgionTls>, b: &Arc<CompiledEdgionTls>) -> std::cmp::Ordering {
    let (a, b) = (&a.tls.metadata, &b.tls.metadata);
    a.creation_timestamp
        .cmp(&b.creation_timestamp)
        .then_with(|| a.namespace.cmp(&b.namespace))
        .then_with(|| a.name.cmp(&b.name))
}

#[cfg(test)]
include!("../../../tests/support/tls/store/tls_store_test/46_function_item.rs");

fn dimensions_are_bounded(tls: &EdgionTls) -> bool {
    edgion_resources::resources::edgion_tls::validate_operator_attachment_dimensions(
        &tls.spec,
        tls.metadata.namespace.as_deref().unwrap_or("default"),
    )
    .is_ok()
        && tls.spec.resolved_listener_attachments.as_ref().map_or(0, Vec::len)
            <= edgion_resources::resources::edgion_tls::MAX_EDGION_TLS_RESOLVED_ATTACHMENTS
        && tls.spec.resolved_listeners.as_ref().map_or(0, Vec::len)
            <= edgion_resources::resources::edgion_tls::MAX_EDGION_TLS_RESOLVED_ATTACHMENTS
}

/// Warn (config-sync path only — never the handshake path) when several EdgionTls
/// resources serve the same host on one listener. `tls_list` must already be sorted
/// by [`edgion_tls_precedence`], so `first()` is the selected certificate.
fn warn_edgion_tls_collision(
    host: &str,
    listener_key: &ListenerKey,
    tls_list: &[Arc<CompiledEdgionTls>],
    total: usize,
) {
    let fmt = |t: &Arc<CompiledEdgionTls>| {
        format!(
            "{}/{}",
            t.tls.metadata.namespace.as_deref().unwrap_or(""),
            t.tls.metadata.name.as_deref().unwrap_or("")
        )
    };
    let selected = tls_list.first().map(fmt).unwrap_or_default();
    tracing::warn!(
        component = "tls_store",
        host = %host,
        listener_key = %listener_key,
        candidate_count = total,
        omitted = total.saturating_sub(tls_list.len()),
        selected = %selected,
        "Multiple EdgionTls resources serve the same hostname on one listener; \
         selecting the certificate deterministically by (creationTimestamp, namespace, name)"
    );
}

/// Advisory warnings emitted once per config-apply on the gateway side (not on the
/// handshake hot path, and not inside validate_cert so the controller stays clean).
fn warn_cert_advisories(key: &str, tls: &EdgionTls) {
    use edgion_resources::resources::edgion_tls::TlsVersion;
    if let Some(v @ (TlsVersion::Tls10 | TlsVersion::Tls11)) = tls.spec.min_tls_version {
        tracing::warn!(component = "tls_store", key = %key, version = ?v,
            "EdgionTls min_tls_version is deprecated by RFC 8996; consider TLS 1.2+");
    }
}

/// Prepare SAN allowlist matchers after conf_sync deserialize. The handshake
/// only consumes prepared entries and never validates config or compiles a
/// regex. Resources without a SAN allowlist keep the incoming Arc so TlsStore
/// can preserve shared allocation.
fn compile_allowed_sans_for_runtime(tls: Arc<EdgionTls>) -> Arc<EdgionTls> {
    let needs_compile = tls
        .spec
        .client_auth
        .as_ref()
        .and_then(|auth| auth.allowed_sans.as_ref())
        .is_some_and(|entries| !entries.is_empty());
    if !needs_compile {
        return tls;
    }

    let mut owned = (*tls).clone();
    if let Some(client_auth) = owned.spec.client_auth.as_mut() {
        // A failure leaves the entry unprepared (fail closed). `validate_cert`
        // applies the same checks and reports the rejection, deduplicated by
        // last-known-good, so no separate log here.
        let _ = client_auth.compile_allowed_sans();
    }
    Arc::new(owned)
}

#[derive(serde::Serialize)]
struct CertRejectedLog<'a> {
    event: &'a str,
    namespace: &'a str,
    name: &'a str,
    reason: &'a str,
}

/// Emit a structured rejection record to tls.log (config-sync path; not the
/// handshake hot path). Fire-and-forget onto the current runtime since the
/// store update path is synchronous.
fn record_rejected(key: &str, validation: &CertValidationResult) {
    let (ns, name) = key.split_once('/').unwrap_or(("", key));
    let reason = validation
        .errors
        .first()
        .map(|e| e.status_reason())
        .unwrap_or("Invalid");
    tracing::error!(component = "tls_store", key = %key, reason = %reason,
        "Rejected invalid cert update; retaining last-known-good");
    let entry = CertRejectedLog {
        event: "cert_update_rejected",
        namespace: ns,
        name,
        reason,
    };
    if let Ok(payload) = serde_json::to_string(&entry) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Some(logger) = crate::observe::logs::tls_log::get_tls_logger() {
                    logger.send(payload).await;
                }
            });
        }
    }
}

type TlsEntries = PersistentHashMap<String, Arc<TlsEntry>>;
type HostCandidates = PersistentHashMap<String, Arc<CompiledEdgionTls>>;

pub(super) struct HostBucket {
    candidates: HostCandidates,
    pub(super) ordered: Arc<[Arc<CompiledEdgionTls>]>,
}

#[derive(Clone, Default)]
pub(super) struct ListenerBucket {
    pub(super) hosts: PersistentHashMap<String, Arc<HostBucket>>,
}

pub(super) struct AttachmentBucket {
    candidates: PersistentHashSet<String>,
    pub(super) slot: AttachmentMatchSlot,
}

/// The raw entries and both derived indexes have exactly one publication point.
#[derive(Clone, Default)]
pub(super) struct TlsSnapshotData {
    entries: TlsEntries,
    pub(super) listeners: PersistentHashMap<ListenerKey, Arc<ListenerBucket>>,
    pub(super) attachments: PersistentHashMap<ResolvedListenerAttachment, Arc<AttachmentBucket>>,
    invalid_count: usize,
}

impl TlsCertMatcherSnapshot {
    pub fn entry_count(&self) -> usize {
        self.data.entries.size()
    }

    /// Certificate validity, independently of listener attachment admission.
    pub fn get_cert_stats(&self) -> (usize, usize) {
        (self.entry_count() - self.data.invalid_count, self.data.invalid_count)
    }

    pub fn listener_count(&self) -> usize {
        self.data.listeners.size()
    }

    #[cfg(test)]
    fn get_validation_status(&self, key: &str) -> Option<CertValidationResult> {
        self.data.entries.get(key).map(|entry| entry.validation.clone())
    }
}

/// A store owns one matcher publication source. Independent stores are isolated.
pub struct TlsStore {
    matcher: Arc<TlsCertMatcher>,
}

impl TlsStore {
    pub fn new() -> Self {
        Self {
            matcher: Arc::new(TlsCertMatcher::new()),
        }
    }

    pub fn snapshot(&self) -> TlsCertMatcherSnapshot {
        self.matcher.snapshot()
    }

    #[cfg(test)]
    fn new_isolated() -> (Self, Arc<TlsCertMatcher>) {
        let store = Self::new();
        let matcher = store.matcher.clone();
        (store, matcher)
    }

    fn validated_entry(key: &str, tls: Arc<EdgionTls>, previous: Option<&Arc<TlsEntry>>) -> Arc<TlsEntry> {
        let dimensions_valid = dimensions_are_bounded(&tls);
        warn_cert_advisories(key, &tls);
        let tls = compile_allowed_sans_for_runtime(tls);
        // SAN preparation clones the spec and resets its derived attachment
        // index. Warm only the final resource that will reach the matcher.
        if dimensions_valid {
            tls.spec.warm_listener_attachment_index();
        }
        let validation = validate_cert(&tls);
        if !validation.is_valid {
            if let Some(existing) = previous.filter(|entry| dimensions_valid && entry.validation.is_valid) {
                if existing.rejected_active {
                    return existing.clone();
                }
                record_rejected(key, &validation);
                let mut kept = (**existing).clone();
                kept.rejected_active = true;
                return Arc::new(kept);
            }
            let reason = validation
                .errors
                .first()
                .map(|error| error.status_reason())
                .unwrap_or("Invalid");
            tracing::warn!(component = "tls_store", key = %key, reason = %reason,
                error_count = validation.errors.len(), "Certificate validation failed");
        }
        let matchable = validation.is_valid && dimensions_valid;
        if validation.is_valid && !dimensions_valid {
            tracing::warn!(component = "tls_store", key = %key,
                "Skipping EdgionTls whose attachment dimensions exceed runtime limits");
        }
        if matchable && tls.spec.resolved_listeners.as_ref().is_none_or(Vec::is_empty) {
            tracing::warn!(component = "tls_store", key = %key,
                host_count = tls.spec.hosts.len(),
                parent_ref_count = tls.spec.parent_refs.as_ref().map_or(0, Vec::len),
                "Skipping EdgionTls from matcher because no listener keys were resolved");
        }
        // Compile the handshake-ready state here, once per accepted generation.
        // A non-matchable resource never reaches any index, so its secret-bearing
        // state is deliberately not compiled.
        let compiled = Arc::new(if matchable {
            CompiledEdgionTls::compile(tls)
        } else {
            CompiledEdgionTls::not_compiled(tls)
        });
        Arc::new(TlsEntry {
            compiled,
            validation,
            matchable,
            rejected_active: false,
        })
    }

    /// Full List/reconnect replaces absent keys; only same-key invalid input can
    /// retain a valid predecessor. Full builds use the same slot builders as deltas.
    pub fn replace_all_tls(&self, data: HashMap<String, Arc<EdgionTls>>) {
        tracing::info!(
            component = "tls_store",
            count = data.len(),
            "Full set of TLS certificates"
        );
        let current = self.matcher.data.load_full();
        let mut next = TlsSnapshotData::default();
        let mut deltas = IndexDeltas::default();
        for (key, tls) in data {
            let entry = Self::validated_entry(&key, tls, current.entries.get(&key));
            next.invalid_count += usize::from(!entry.validation.is_valid);
            deltas.add(&key, &entry, true);
            next.entries.insert_mut(key, entry);
        }
        next.apply_indexes(deltas, &current);
        self.matcher.data.store(Arc::new(next));
    }

    /// Production writers are serialized by the conf-sync CacheData write lock.
    /// HAMT root/path copies share every unaffected entry, listener and host slot.
    pub fn apply_partial_update(
        &self,
        add: HashMap<String, Arc<EdgionTls>>,
        update: HashMap<String, Arc<EdgionTls>>,
        remove: &HashSet<String>,
    ) {
        if add.is_empty() && update.is_empty() && remove.is_empty() {
            return;
        }
        tracing::info!(
            component = "tls_store",
            add = add.len(),
            update = update.len(),
            remove = remove.len(),
            "Partial update of TLS certificates"
        );
        let current = self.matcher.data.load_full();
        let mut next = (*current).clone();
        let changed_keys: HashSet<String> = add.keys().chain(update.keys()).chain(remove.iter()).cloned().collect();

        // Preserve sequential overlap semantics: update's LKG predecessor may
        // be an add in this batch, never the deleted generation preceding that add.
        for (key, tls) in add {
            let entry = Self::validated_entry(&key, tls, None);
            next.entries.insert_mut(key, entry);
        }
        for (key, tls) in update {
            let entry = Self::validated_entry(&key, tls, next.entries.get(&key));
            next.entries.insert_mut(key, entry);
        }
        for key in remove {
            next.entries.remove_mut(key);
        }

        let mut deltas = IndexDeltas::default();
        let mut changed = false;
        for key in changed_keys {
            let previous = current.entries.get(&key);
            let incoming = next.entries.get(&key);
            if matches!((previous, incoming), (None, None))
                || matches!((previous, incoming), (Some(old), Some(new)) if Arc::ptr_eq(old, new))
            {
                continue;
            }
            changed = true;
            next.invalid_count -= usize::from(previous.is_some_and(|entry| !entry.validation.is_valid));
            next.invalid_count += usize::from(incoming.is_some_and(|entry| !entry.validation.is_valid));
            // Rejection state can change while the effective resource stays
            // identical. Do not touch any matcher for that bookkeeping change.
            if matches!((previous, incoming), (Some(old), Some(new)) if Arc::ptr_eq(&old.compiled.tls, &new.compiled.tls))
            {
                continue;
            }
            if let Some(previous) = previous {
                deltas.add(&key, previous, false);
            }
            if let Some(incoming) = incoming {
                deltas.add(&key, incoming, true);
            }
        }
        if changed {
            next.apply_indexes(deltas, &current);
            self.matcher.data.store(Arc::new(next));
        }
    }

    #[cfg(test)]
    pub fn contains(&self, key: &str) -> bool {
        self.matcher.data.load().entries.contains_key(key)
    }

    #[cfg(test)]
    pub fn get_validation_status(&self, key: &str) -> Option<CertValidationResult> {
        self.snapshot().get_validation_status(key)
    }

    pub fn get_cert_stats(&self) -> (usize, usize) {
        self.snapshot().get_cert_stats()
    }

    pub fn entry_count(&self) -> usize {
        self.snapshot().entry_count()
    }
}

type HostChanges = HashMap<String, Option<Arc<CompiledEdgionTls>>>;

#[derive(Default)]
struct IndexDeltas {
    listeners: HashMap<ListenerKey, HashMap<String, HostChanges>>,
    attachments: HashMap<ResolvedListenerAttachment, HashMap<String, bool>>,
}

impl IndexDeltas {
    fn add(&mut self, key: &str, entry: &TlsEntry, present: bool) {
        if !entry.matchable {
            return;
        }
        let tls = &entry.compiled.tls;
        let hosts: HashSet<String> = tls.spec.hosts.iter().filter_map(|host| canonical_key(host)).collect();
        if let Some(listeners) = &tls.spec.resolved_listeners {
            for listener in listeners {
                let changed_hosts = self.listeners.entry(listener.clone()).or_default();
                for host in &hosts {
                    changed_hosts
                        .entry(host.clone())
                        .or_default()
                        .insert(key.to_string(), present.then(|| entry.compiled.clone()));
                }
            }
        }
        if let Some(attachments) = &tls.spec.resolved_listener_attachments {
            for attachment in attachments
                .iter()
                .filter(|attachment| attachment.gateway_proof.is_resolved())
            {
                self.attachments
                    .entry(attachment.clone())
                    .or_default()
                    .insert(key.to_string(), present);
            }
        }
    }
}

impl TlsSnapshotData {
    fn apply_indexes(&mut self, deltas: IndexDeltas, warning_baseline: &Self) {
        for (listener, changed_hosts) in deltas.listeners {
            let mut bucket = self
                .listeners
                .get(&listener)
                .map_or_else(ListenerBucket::default, |bucket| (**bucket).clone());
            for (host, changes) in changed_hosts {
                let mut candidates = bucket
                    .hosts
                    .get(&host)
                    .map_or_else(HostCandidates::new_sync, |slot| slot.candidates.clone());
                for (key, compiled) in changes {
                    if let Some(compiled) = compiled {
                        candidates.insert_mut(key, compiled);
                    } else {
                        candidates.remove_mut(&key);
                    }
                }
                if candidates.is_empty() {
                    bucket.hosts.remove_mut(&host);
                    continue;
                }
                let mut ordered: Vec<_> = candidates.values().cloned().collect();
                ordered.sort_by(edgion_tls_precedence);
                let slot = HostBucket {
                    candidates,
                    ordered: Arc::from(ordered),
                };
                let previous = warning_baseline
                    .listeners
                    .get(&listener)
                    .and_then(|bucket| bucket.hosts.get(&host));
                if collision_changed(previous.map(Arc::as_ref), &slot) {
                    warn_edgion_tls_collision(
                        &host,
                        &listener,
                        &slot.ordered[..slot.ordered.len().min(MAX_EDGION_TLS_COLLISION_LOG_PARTICIPANTS)],
                        slot.ordered.len(),
                    );
                }
                bucket.hosts.insert_mut(host, Arc::new(slot));
            }
            if bucket.hosts.is_empty() {
                self.listeners.remove_mut(&listener);
            } else {
                self.listeners.insert_mut(listener, Arc::new(bucket));
            }
        }

        for (attachment, changes) in deltas.attachments {
            let mut candidates = self
                .attachments
                .get(&attachment)
                .map_or_else(PersistentHashSet::new_sync, |bucket| bucket.candidates.clone());
            for (key, present) in changes {
                if present {
                    candidates.insert_mut(key);
                } else {
                    candidates.remove_mut(&key);
                }
            }
            if candidates.is_empty() {
                self.attachments.remove_mut(&attachment);
                continue;
            }
            let slot = if candidates.size() > MAX_ATTACHMENT_CANDIDATES {
                if !warning_baseline
                    .attachments
                    .get(&attachment)
                    .is_some_and(|bucket| matches!(bucket.slot, AttachmentMatchSlot::Overflow))
                {
                    tracing::warn!(
                        component = "tls_store",
                        candidate_count = candidates.size(),
                        max = MAX_ATTACHMENT_CANDIDATES,
                        "Exact EdgionTls attachment candidate limit exceeded; omitting the complete overlay group"
                    );
                }
                AttachmentMatchSlot::Overflow
            } else {
                let mut ordered: Vec<_> = candidates
                    .iter()
                    .filter_map(|key| self.entries.get(key).map(|entry| entry.compiled.clone()))
                    .collect();
                ordered.sort_by(edgion_tls_precedence);
                AttachmentMatchSlot::Ready(Arc::from(ordered))
            };
            self.attachments
                .insert_mut(attachment, Arc::new(AttachmentBucket { candidates, slot }));
        }
    }
}

/// Renewal alone is not a new collision: compare members and winner identity,
/// not resource allocation or losing-candidate ordering.
fn collision_changed(previous: Option<&HostBucket>, next: &HostBucket) -> bool {
    if next.candidates.size() < 2 {
        return false;
    }
    let Some(previous) = previous else {
        return true;
    };
    previous.candidates.size() != next.candidates.size()
        || next.candidates.keys().any(|key| !previous.candidates.contains_key(key))
        || previous
            .ordered
            .first()
            .map(|c| (&c.tls.metadata.namespace, &c.tls.metadata.name))
            != next
                .ordered
                .first()
                .map(|c| (&c.tls.metadata.namespace, &c.tls.metadata.name))
}

impl Default for TlsStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
include!("../../../tests/support/tls/store/tls_store_test/577_impl_item.rs");

/// Global TlsStore instance
static GLOBAL_TLS_STORE: LazyLock<Arc<TlsStore>> = LazyLock::new(|| {
    Arc::new(TlsStore {
        matcher: super::cert_matcher::TLS_CERT_MATCHER.clone(),
    })
});

/// Get the global TlsStore instance
pub fn get_global_tls_store() -> Arc<TlsStore> {
    GLOBAL_TLS_STORE.clone()
}

#[cfg(test)]
#[path = "../../../tests/unit/tls/store/tls_store_test.rs"]
mod tests;
