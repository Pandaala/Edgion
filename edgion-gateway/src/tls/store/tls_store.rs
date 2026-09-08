use super::cert_matcher::{AttachmentMatchSlot, TlsCertMatcher, TlsCertMatcherSnapshot, MAX_ATTACHMENT_CANDIDATES};
use crate::tls::validation::cert::{validate_cert, CertValidationResult};
use edgion_resources::listener_key::ListenerKey;
use edgion_resources::matcher::canonical_host_key;
use edgion_resources::resources::edgion_tls::ResolvedListenerAttachment;
use edgion_resources::EdgionTls;
use rpds::{HashTrieMapSync as PersistentHashMap, HashTrieSetSync as PersistentHashSet};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

pub(crate) const MAX_EDGION_TLS_COLLISION_LOG_PARTICIPANTS: usize = 64;

/// Internal entry for TLS certificate with validation status
#[derive(Clone)]
pub(super) struct TlsEntry {
    tls: Arc<EdgionTls>,
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
fn edgion_tls_precedence(a: &Arc<EdgionTls>, b: &Arc<EdgionTls>) -> std::cmp::Ordering {
    a.metadata
        .creation_timestamp
        .cmp(&b.metadata.creation_timestamp)
        .then_with(|| a.metadata.namespace.cmp(&b.metadata.namespace))
        .then_with(|| a.metadata.name.cmp(&b.metadata.name))
}

/// Add one resource's hosts to a listener bucket using the same canonical
/// ASCII case as SNI lookup. Duplicate case variants within one resource are
/// collapsed so they cannot create a false self-collision.
#[cfg(test)]
fn add_canonical_hosts(host_map: &mut HashMap<String, Vec<Arc<EdgionTls>>>, tls: &Arc<EdgionTls>) -> usize {
    let mut added = 0;
    for host in &tls.spec.hosts {
        let tls_list = host_map.entry(host.to_ascii_lowercase()).or_default();
        if tls_list.last().is_none_or(|existing| !Arc::ptr_eq(existing, tls)) {
            tls_list.push(tls.clone());
            added += 1;
        }
    }
    added
}

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
fn warn_edgion_tls_collision(host: &str, listener_key: &ListenerKey, tls_list: &[Arc<EdgionTls>], total: usize) {
    let fmt = |t: &Arc<EdgionTls>| {
        format!(
            "{}/{}",
            t.metadata.namespace.as_deref().unwrap_or(""),
            t.metadata.name.as_deref().unwrap_or("")
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

/// Compile RegularExpression SAN allowlist entries after conf_sync deserialize.
/// Handshake matching never compiles a regex. Resources without regex keep the
/// incoming Arc so TlsStore can preserve shared allocation.
fn compile_allowed_sans_for_runtime(tls: Arc<EdgionTls>) -> Arc<EdgionTls> {
    use edgion_resources::resources::edgion_tls::AllowedSanMatch;

    let needs_compile = tls
        .spec
        .client_auth
        .as_ref()
        .and_then(|auth| auth.allowed_sans.as_ref())
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.match_kind == AllowedSanMatch::RegularExpression)
        });
    if !needs_compile {
        return tls;
    }

    let mut owned = (*tls).clone();
    if let Some(client_auth) = owned.spec.client_auth.as_mut() {
        if let Err(error) = client_auth.compile_allowed_sans() {
            tracing::warn!(
                component = "tls_store",
                error = %error,
                "Failed to compile EdgionTls allowedSans regular expressions"
            );
        }
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
type HostCandidates = PersistentHashMap<String, Arc<EdgionTls>>;

pub(super) struct HostBucket {
    candidates: HostCandidates,
    pub(super) ordered: Arc<Vec<Arc<EdgionTls>>>,
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
        // Regex compilation clones the spec and resets its derived attachment
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
        Arc::new(TlsEntry {
            tls,
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
            if matches!((previous, incoming), (Some(old), Some(new)) if Arc::ptr_eq(&old.tls, &new.tls)) {
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

type HostChanges = HashMap<String, Option<Arc<EdgionTls>>>;

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
        let tls = &entry.tls;
        let hosts: HashSet<String> = tls
            .spec
            .hosts
            .iter()
            .filter_map(|host| canonical_host_key(host))
            .collect();
        if let Some(listeners) = &tls.spec.resolved_listeners {
            for listener in listeners {
                let changed_hosts = self.listeners.entry(listener.clone()).or_default();
                for host in &hosts {
                    changed_hosts
                        .entry(host.clone())
                        .or_default()
                        .insert(key.to_string(), present.then(|| tls.clone()));
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
                for (key, tls) in changes {
                    if let Some(tls) = tls {
                        candidates.insert_mut(key, tls);
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
                    ordered: Arc::new(ordered),
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
                    .filter_map(|key| self.entries.get(key).map(|entry| entry.tls.clone()))
                    .collect();
                ordered.sort_by(edgion_tls_precedence);
                AttachmentMatchSlot::Ready(Arc::new(ordered))
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
            .map(|tls| (&tls.metadata.namespace, &tls.metadata.name))
            != next
                .ordered
                .first()
                .map(|tls| (&tls.metadata.namespace, &tls.metadata.name))
}

impl Default for TlsStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl TlsStore {
    fn is_rejected_active(&self, key: &str) -> bool {
        self.matcher
            .data
            .load()
            .entries
            .get(key)
            .map(|e| e.rejected_active)
            .unwrap_or(false)
    }
}

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
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use edgion_resources::resources::edgion_tls::EdgionTlsSpec;
    use edgion_resources::resources::gateway::SecretObjectReference;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
    use kube::api::ObjectMeta;

    fn make_etls(namespace: &str, name: &str, ts: Option<Time>) -> Arc<EdgionTls> {
        Arc::new(EdgionTls {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                creation_timestamp: ts,
                ..Default::default()
            },
            spec: EdgionTlsSpec {
                parent_refs: None,
                hosts: vec!["shared.example.com".to_string()],
                secret_ref: SecretObjectReference {
                    name: format!("{name}-cert"),
                    namespace: Some(namespace.to_string()),
                    group: None,
                    kind: None,
                },
                client_auth: None,
                min_tls_version: None,
                ciphers: None,
                secret: None,
                resolved_listeners: None,
                resolved_listener_attachments: None,
                resolved_listener_attachment_index: Default::default(),
                resolved_log_labels: None,
            },
            status: None,
        })
    }

    /// Build an EdgionTls whose secret contains a real self-signed cert+key so
    /// that `validate_cert` returns `is_valid = true`.
    fn make_valid_tls(ns: &str, name: &str, hosts: &[&str]) -> Arc<EdgionTls> {
        use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use rcgen::{CertificateParams, KeyPair, SanType};
        use std::collections::BTreeMap;
        use std::net::SocketAddr;

        let host_strings: Vec<String> = hosts.iter().map(|s| s.to_string()).collect();
        let san_types: Vec<SanType> = hosts
            .iter()
            .map(|h| SanType::DnsName((*h).try_into().expect("valid DNS SAN")))
            .collect();

        let key_pair = KeyPair::generate().expect("rcgen key generation failed");
        let mut params = CertificateParams::new(vec![]).expect("rcgen params failed");
        params.subject_alt_names = san_types;
        let cert = params.self_signed(&key_pair).expect("rcgen self-sign failed");

        let mut data = BTreeMap::new();
        data.insert(CERT.to_string(), ByteString(cert.pem().into_bytes()));
        data.insert(KEY.to_string(), ByteString(key_pair.serialize_pem().into_bytes()));

        let addr: SocketAddr = "0.0.0.0:443".parse().unwrap();
        let listener_key = edgion_resources::ListenerKey::new(addr, "HTTPS");

        Arc::new(EdgionTls {
            metadata: ObjectMeta {
                namespace: Some(ns.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: edgion_resources::EdgionTlsSpec {
                parent_refs: None,
                hosts: host_strings,
                secret_ref: edgion_resources::resources::gateway::SecretObjectReference {
                    name: format!("{name}-cert"),
                    namespace: Some(ns.to_string()),
                    group: None,
                    kind: None,
                },
                client_auth: None,
                min_tls_version: None,
                ciphers: None,
                secret: Some(Secret {
                    data: Some(data),
                    ..Default::default()
                }),
                resolved_listeners: Some(vec![listener_key]),
                resolved_listener_attachments: None,
                resolved_listener_attachment_index: Default::default(),
                resolved_log_labels: None,
            },
            status: None,
        })
    }

    /// Build an EdgionTls whose secret has non-PEM bytes so that
    /// `validate_cert` returns `is_valid = false`.
    fn make_invalid_tls(ns: &str, name: &str, hosts: &[&str]) -> Arc<EdgionTls> {
        use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;
        use std::net::SocketAddr;

        let host_strings: Vec<String> = hosts.iter().map(|s| s.to_string()).collect();
        let mut data = BTreeMap::new();
        data.insert(CERT.to_string(), ByteString(b"not a pem".to_vec()));
        data.insert(KEY.to_string(), ByteString(b"not a pem".to_vec()));

        let addr: SocketAddr = "0.0.0.0:443".parse().unwrap();
        let listener_key = edgion_resources::ListenerKey::new(addr, "HTTPS");

        Arc::new(EdgionTls {
            metadata: ObjectMeta {
                namespace: Some(ns.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: edgion_resources::EdgionTlsSpec {
                parent_refs: None,
                hosts: host_strings,
                secret_ref: edgion_resources::resources::gateway::SecretObjectReference {
                    name: format!("{name}-cert"),
                    namespace: Some(ns.to_string()),
                    group: None,
                    kind: None,
                },
                client_auth: None,
                min_tls_version: None,
                ciphers: None,
                secret: Some(Secret {
                    data: Some(data),
                    ..Default::default()
                }),
                resolved_listeners: Some(vec![listener_key]),
                resolved_listener_attachments: None,
                resolved_listener_attachment_index: Default::default(),
                resolved_log_labels: None,
            },
            status: None,
        })
    }

    #[test]
    fn invalid_update_retains_last_known_good() {
        let store = TlsStore::new();
        let good = make_valid_tls("ns", "foo", &["foo.example.com"]);
        store.apply_partial_update(
            std::collections::HashMap::from([("ns/foo".to_string(), good)]),
            std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        );
        assert!(store.get_validation_status("ns/foo").unwrap().is_valid);

        let bad = make_invalid_tls("ns", "foo", &["foo.example.com"]);
        store.apply_partial_update(
            std::collections::HashMap::new(),
            std::collections::HashMap::from([("ns/foo".to_string(), bad)]),
            &std::collections::HashSet::new(),
        );

        let v = store.get_validation_status("ns/foo").expect("entry must remain");
        assert!(v.is_valid, "bad update must not overwrite the last-known-good cert");
    }

    #[test]
    fn invalid_attachment_dimensions_revoke_old_matcher_generation_then_repair() {
        let (store, matcher) = TlsStore::new_isolated();
        let listener = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let good = make_valid_tls("ns", "bounded", &["bounded.example.com"]);
        store.apply_partial_update(
            std::collections::HashMap::from([("ns/bounded".to_string(), good)]),
            std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        );
        assert!(matcher.match_sni(&listener, "bounded.example.com").is_some());

        let forged = make_valid_tls("ns", "bounded", &["bounded.example.com", "BOUNDED.EXAMPLE.COM"]);
        store.apply_partial_update(
            std::collections::HashMap::new(),
            std::collections::HashMap::from([("ns/bounded".to_string(), forged)]),
            &std::collections::HashSet::new(),
        );
        let snapshot = store.matcher.data.load();
        assert_eq!(snapshot.entries["ns/bounded"].tls.spec.hosts.len(), 2);
        assert!(
            !dimensions_are_bounded(&snapshot.entries["ns/bounded"].tls),
            "the forged generation must replace, rather than retain, the old valid object and is excluded on rebuild"
        );
        drop(snapshot);
        assert!(matcher.match_sni(&listener, "bounded.example.com").is_none());

        let repaired = make_valid_tls("ns", "bounded", &["bounded.example.com"]);
        store.apply_partial_update(
            std::collections::HashMap::new(),
            std::collections::HashMap::from([("ns/bounded".to_string(), repaired)]),
            &std::collections::HashSet::new(),
        );
        assert!(matcher.match_sni(&listener, "bounded.example.com").is_some());
    }

    #[test]
    fn overflowing_exact_attachment_omits_entire_group_independent_of_input_order() {
        use edgion_resources::resources::edgion_tls::{GatewayAttachmentProof, ResolvedListenerAttachment};

        let listener = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let attachment = ResolvedListenerAttachment {
            gateway_namespace: "ns".to_string(),
            gateway_name: "gateway".to_string(),
            listener_name: "https".to_string(),
            listener_key: listener.clone(),
            gateway_proof: GatewayAttachmentProof {
                instance: "uid-1".to_string(),
                generation: 1,
                epoch: "boot:1".to_string(),
                attachment_digest: "a".repeat(64),
            },
        };
        let shared = make_valid_tls("ns", "template", &["shared.example.com"]);
        let unique = make_valid_tls("ns", "winner", &["unique.example.com"]);
        let mut resources = (0..super::super::cert_matcher::MAX_ATTACHMENT_CANDIDATES)
            .map(|index| {
                let mut tls = (*shared).clone();
                tls.metadata.name = Some(format!("shared-{index}"));
                tls.spec.resolved_listener_attachments = Some(vec![attachment.clone()]);
                (format!("ns/shared-{index}"), Arc::new(tls))
            })
            .collect::<Vec<_>>();
        let mut highest_precedence_unique = (*unique).clone();
        highest_precedence_unique.metadata.name = Some("aaa-oldest".to_string());
        highest_precedence_unique.metadata.creation_timestamp =
            Some(Time(Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap()));
        highest_precedence_unique.spec.resolved_listener_attachments = Some(vec![attachment.clone()]);
        resources.push(("ns/aaa-oldest".to_string(), Arc::new(highest_precedence_unique)));

        for ordered in [resources.clone(), resources.into_iter().rev().collect()] {
            let (store, matcher) = TlsStore::new_isolated();
            store.replace_all_tls(ordered.into_iter().collect());
            assert!(matcher
                .match_attachment_sni(&attachment, "shared.example.com")
                .is_none());
            assert!(matcher
                .match_attachment_sni(&attachment, "unique.example.com")
                .is_none());
        }
    }

    #[test]
    fn intentional_removal_still_removes() {
        let store = TlsStore::new();
        let good = make_valid_tls("ns", "bar", &["bar.example.com"]);
        store.apply_partial_update(
            std::collections::HashMap::from([("ns/bar".to_string(), good)]),
            std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        );
        store.apply_partial_update(
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            &std::collections::HashSet::from(["ns/bar".to_string()]),
        );
        assert!(!store.contains("ns/bar"), "removal must still delete");
    }

    #[test]
    fn full_snapshot_retains_last_known_good_on_reconnect() {
        let store = TlsStore::new();
        let good = make_valid_tls("ns", "baz", &["baz.example.com"]);
        store.replace_all_tls(std::collections::HashMap::from([("ns/baz".to_string(), good)]));
        assert!(store.get_validation_status("ns/baz").unwrap().is_valid);

        // A reconnect full snapshot carries an invalid cert for the same key
        // (version-skew: an old controller without Part B). Must NOT drop it.
        let bad = make_invalid_tls("ns", "baz", &["baz.example.com"]);
        store.replace_all_tls(std::collections::HashMap::from([("ns/baz".to_string(), bad)]));

        let v = store.get_validation_status("ns/baz").expect("entry must remain");
        assert!(
            v.is_valid,
            "full snapshot must retain last-known-good for an invalid key"
        );
    }

    #[test]
    fn full_snapshot_drops_absent_keys() {
        let store = TlsStore::new();
        let good = make_valid_tls("ns", "gone", &["gone.example.com"]);
        store.replace_all_tls(std::collections::HashMap::from([("ns/gone".to_string(), good)]));
        // New snapshot omits the key entirely -> intentional removal.
        store.replace_all_tls(std::collections::HashMap::new());
        assert!(
            !store.contains("ns/gone"),
            "absent key in full snapshot must be removed"
        );
    }

    #[test]
    fn test_edgion_tls_precedence_prefers_oldest_over_name() {
        let older = Time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
        let newer = Time(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());

        // incumbent is lexically LARGER (team-z/zzz) but OLDER; newcomer smaller but newer.
        let incumbent = make_etls("team-z", "zzz", Some(older));
        let newcomer = make_etls("team-a", "aaa", Some(newer));

        for mut list in [
            vec![newcomer.clone(), incumbent.clone()],
            vec![incumbent.clone(), newcomer.clone()],
        ] {
            list.sort_by(edgion_tls_precedence);
            assert_eq!(
                list.first().unwrap().metadata.name.as_deref(),
                Some("zzz"),
                "older EdgionTls must win regardless of name or input order"
            );
        }
    }

    #[test]
    fn test_edgion_tls_precedence_name_tiebreak_when_no_timestamp() {
        // No timestamps (both None) → fall back to (namespace, name) lexicographic.
        let a = make_etls("team-a", "aaa", None);
        let b = make_etls("team-b", "bbb", None);

        let mut list = [b, a];
        list.sort_by(edgion_tls_precedence);
        assert_eq!(list.first().unwrap().metadata.namespace.as_deref(), Some("team-a"));
    }

    #[test]
    fn canonical_host_collision_always_selects_precedence_winner() {
        let older = Time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
        let newer = Time(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        let mut incumbent = make_etls("team-z", "older", Some(older));
        Arc::get_mut(&mut incumbent)
            .expect("test Arc must be unique")
            .spec
            .hosts = vec!["Shared.Example.com".to_string(), "shared.example.com".to_string()];
        let newcomer = make_etls("team-a", "newer", Some(newer));

        for resources in [
            [newcomer.clone(), incumbent.clone()],
            [incumbent.clone(), newcomer.clone()],
        ] {
            let mut host_map = HashMap::new();
            for tls in resources {
                add_canonical_hosts(&mut host_map, &tls);
            }
            assert_eq!(host_map.len(), 1, "ASCII case variants must share one bucket");
            let list = host_map.get_mut("shared.example.com").expect("canonical host bucket");
            assert_eq!(list.len(), 2, "one resource must not collide with itself");
            list.sort_by(edgion_tls_precedence);
            assert_eq!(list[0].metadata.name.as_deref(), Some("older"));
        }
    }

    #[test]
    fn invalid_add_replaces_deleted_generation_instead_of_retaining_it() {
        let store = TlsStore::new();
        let old = make_valid_tls("ns", "recreated", &["old.example.com"]);
        store.apply_partial_update(
            HashMap::from([("ns/recreated".to_string(), old)]),
            HashMap::new(),
            &std::collections::HashSet::new(),
        );

        let recreated = make_invalid_tls("ns", "recreated", &["new.example.com"]);
        store.apply_partial_update(
            HashMap::from([("ns/recreated".to_string(), recreated.clone())]),
            HashMap::new(),
            &std::collections::HashSet::new(),
        );

        let snapshot = store.matcher.data.load();
        let entry = snapshot
            .entries
            .get("ns/recreated")
            .expect("recreated entry must remain visible as invalid");
        assert!(Arc::ptr_eq(&entry.tls, &recreated));
        assert!(!entry.validation.is_valid);
        assert_eq!(entry.tls.spec.hosts, ["new.example.com"]);
    }

    #[test]
    fn repeated_bad_update_marks_rejected_active_once() {
        let store = TlsStore::new();
        let good = make_valid_tls("ns", "q", &["q.example.com"]);
        store.apply_partial_update(
            std::collections::HashMap::from([("ns/q".to_string(), good)]),
            std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        );
        assert!(!store.is_rejected_active("ns/q"));

        let bad = make_invalid_tls("ns", "q", &["q.example.com"]);
        for _ in 0..2 {
            store.apply_partial_update(
                std::collections::HashMap::new(),
                std::collections::HashMap::from([("ns/q".to_string(), bad.clone())]),
                &std::collections::HashSet::new(),
            );
        }
        assert!(store.is_rejected_active("ns/q"));
        assert!(store.get_validation_status("ns/q").unwrap().is_valid);
    }

    #[test]
    fn valid_update_after_rejection_rearms_transition() {
        let store = TlsStore::new();
        let good = make_valid_tls("ns", "r", &["r.example.com"]);
        store.apply_partial_update(
            std::collections::HashMap::from([("ns/r".to_string(), good.clone())]),
            std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        );

        let bad = make_invalid_tls("ns", "r", &["r.example.com"]);
        store.apply_partial_update(
            std::collections::HashMap::new(),
            std::collections::HashMap::from([("ns/r".to_string(), bad)]),
            &std::collections::HashSet::new(),
        );
        assert!(store.is_rejected_active("ns/r"), "rejection should set the flag");

        // A valid update lands again -> fresh entry, flag cleared (transition re-armed).
        let good2 = make_valid_tls("ns", "r", &["r.example.com"]);
        store.apply_partial_update(
            std::collections::HashMap::new(),
            std::collections::HashMap::from([("ns/r".to_string(), good2)]),
            &std::collections::HashSet::new(),
        );
        assert!(
            !store.is_rejected_active("ns/r"),
            "valid update must clear rejected_active"
        );
        assert!(store.get_validation_status("ns/r").unwrap().is_valid);
    }

    fn listener(port: u16) -> ListenerKey {
        ListenerKey::new(format!("0.0.0.0:{port}").parse().unwrap(), "HTTPS")
    }

    fn attachment(port: u16) -> ResolvedListenerAttachment {
        use edgion_resources::resources::edgion_tls::GatewayAttachmentProof;
        ResolvedListenerAttachment {
            gateway_namespace: "ns".to_string(),
            gateway_name: "gateway".to_string(),
            listener_name: "https".to_string(),
            listener_key: listener(port),
            gateway_proof: GatewayAttachmentProof {
                instance: "uid-1".to_string(),
                generation: 1,
                epoch: "boot:1".to_string(),
                attachment_digest: "a".repeat(64),
            },
        }
    }

    fn publish_update(store: &TlsStore, name: &str, tls: Arc<EdgionTls>) {
        store.apply_partial_update(
            HashMap::new(),
            HashMap::from([(format!("ns/{name}"), tls)]),
            &HashSet::new(),
        );
    }

    #[test]
    fn delta_shares_unaffected_entries_hosts_and_listeners() {
        let store = TlsStore::new();
        let changed = make_valid_tls("ns", "changed", &["changed.example.com"]);
        let same_listener = make_valid_tls("ns", "same", &["same.example.com"]);
        let mut other_listener = make_valid_tls("ns", "other", &["other.example.com"]);
        Arc::get_mut(&mut other_listener).unwrap().spec.resolved_listeners = Some(vec![listener(8443)]);
        store.replace_all_tls(HashMap::from([
            ("ns/changed".to_string(), changed),
            ("ns/same".to_string(), same_listener.clone()),
            ("ns/other".to_string(), other_listener.clone()),
        ]));
        let before = store.snapshot();
        publish_update(&store, "changed", make_valid_tls("ns", "changed", &["new.example.com"]));
        let after = store.snapshot();
        assert!(Arc::ptr_eq(
            &before.data.entries["ns/same"],
            &after.data.entries["ns/same"]
        ));
        assert!(Arc::ptr_eq(&after.data.entries["ns/same"].tls, &same_listener));
        assert!(Arc::ptr_eq(&after.data.entries["ns/other"].tls, &other_listener));
        assert!(Arc::ptr_eq(
            &before.data.listeners[&listener(8443)],
            &after.data.listeners[&listener(8443)]
        ));
        assert!(Arc::ptr_eq(
            &before.data.listeners[&listener(443)].hosts["same.example.com"],
            &after.data.listeners[&listener(443)].hosts["same.example.com"],
        ));
        assert!(after
            .match_sni_candidates(&listener(443), "changed.example.com")
            .is_empty());
        assert_eq!(after.match_sni_candidates(&listener(443), "new.example.com").len(), 1);
        assert_eq!(
            before.match_sni_candidates(&listener(443), "changed.example.com").len(),
            1
        );
    }

    #[test]
    fn wildcard_delta_removes_canonical_slot_and_preserves_exact_priority() {
        let store = TlsStore::new();
        let wildcard = make_valid_tls("ns", "wildcard", &["*.example.com"]);
        let exact = make_valid_tls("ns", "exact", &["one.example.com"]);
        store.replace_all_tls(HashMap::from([
            ("ns/wildcard".to_string(), wildcard.clone()),
            ("ns/exact".to_string(), exact.clone()),
        ]));
        let snapshot = store.snapshot();
        assert!(Arc::ptr_eq(
            &snapshot.match_sni_candidates(&listener(443), "ONE.EXAMPLE.COM")[0],
            &exact
        ));
        assert!(Arc::ptr_eq(
            &snapshot.match_sni_candidates(&listener(443), "two.example.com")[0],
            &wildcard
        ));
        assert!(snapshot
            .match_sni_candidates(&listener(443), "a.b.example.com")
            .is_empty());
        store.apply_partial_update(HashMap::new(), HashMap::new(), &HashSet::from(["ns/exact".to_string()]));
        assert!(Arc::ptr_eq(
            &store.snapshot().match_sni_candidates(&listener(443), "one.example.com")[0],
            &wildcard
        ));
        publish_update(
            &store,
            "wildcard",
            make_valid_tls("ns", "wildcard", &["*.new.example.com"]),
        );
        let next = store.snapshot();
        assert!(next.match_sni_candidates(&listener(443), "one.example.com").is_empty());
        assert_eq!(
            next.match_sni_candidates(&listener(443), "one.new.example.com").len(),
            1
        );
        assert!(!next.data.listeners[&listener(443)].hosts.contains_key(".example.com"));
    }

    #[test]
    fn one_snapshot_never_mixes_entries_listener_and_attachment_generations() {
        let store = TlsStore::new();
        let old_attachment = attachment(443);
        let new_attachment = attachment(8443);
        let mut old = make_valid_tls("ns", "moving", &["old.example.com"]);
        Arc::get_mut(&mut old).unwrap().spec.resolved_listener_attachments = Some(vec![old_attachment.clone()]);
        store.replace_all_tls(HashMap::from([("ns/moving".to_string(), old.clone())]));
        let before = store.snapshot();
        let mut new = make_valid_tls("ns", "moving", &["new.example.com"]);
        let spec = &mut Arc::get_mut(&mut new).unwrap().spec;
        spec.resolved_listeners = Some(vec![listener(8443)]);
        spec.resolved_listener_attachments = Some(vec![new_attachment.clone()]);
        publish_update(&store, "moving", new.clone());
        let after = store.snapshot();
        assert!(Arc::ptr_eq(&before.data.entries["ns/moving"].tls, &old));
        assert!(before.get_validation_status("ns/moving").unwrap().is_valid);
        assert_eq!(before.get_cert_stats(), (1, 0));
        assert!(Arc::ptr_eq(
            &before.match_sni_candidates(&listener(443), "old.example.com")[0],
            &old
        ));
        assert!(Arc::ptr_eq(
            &before.match_attachment_sni(&old_attachment, "old.example.com").unwrap(),
            &old
        ));
        assert!(after.match_sni_candidates(&listener(443), "old.example.com").is_empty());
        assert!(after.match_attachment_sni(&old_attachment, "old.example.com").is_none());
        assert!(Arc::ptr_eq(&after.data.entries["ns/moving"].tls, &new));
        assert!(Arc::ptr_eq(
            &after.match_attachment_sni(&new_attachment, "new.example.com").unwrap(),
            &new
        ));
        assert_eq!(after.listener_count(), 1);

        // Full empty publication cannot mutate a retained generation.
        store.replace_all_tls(HashMap::new());
        assert_eq!(store.snapshot().get_cert_stats(), (0, 0));
        assert_eq!(store.snapshot().listener_count(), 0);
        assert!(store.snapshot().data.attachments.is_empty());
        assert_eq!(after.entry_count(), 1);
        assert!(after.match_attachment_sni(&new_attachment, "new.example.com").is_some());
    }

    #[test]
    fn rejected_replacement_keeps_all_effective_indexes_and_rearms() {
        let store = TlsStore::new();
        let original = make_valid_tls("ns", "kept", &["old.example.com"]);
        store.replace_all_tls(HashMap::from([("ns/kept".to_string(), original.clone())]));
        let before = store.snapshot();
        let mut bad = make_invalid_tls("ns", "kept", &["new.example.com"]);
        Arc::get_mut(&mut bad).unwrap().spec.resolved_listeners = Some(vec![listener(8443)]);
        publish_update(&store, "kept", bad.clone());
        let rejected = store.snapshot();
        assert!(store.is_rejected_active("ns/kept"));
        assert!(Arc::ptr_eq(
            &before.data.listeners[&listener(443)],
            &rejected.data.listeners[&listener(443)]
        ));
        assert!(!rejected.data.listeners.contains_key(&listener(8443)));
        assert!(Arc::ptr_eq(&rejected.data.entries["ns/kept"].tls, &original));
        publish_update(&store, "kept", bad.clone());
        assert!(Arc::ptr_eq(&rejected.data, &store.snapshot().data));

        let repaired = make_valid_tls("ns", "kept", &["new.example.com"]);
        publish_update(&store, "kept", repaired.clone());
        assert!(!store.is_rejected_active("ns/kept"));
        publish_update(&store, "kept", bad);
        assert!(store.is_rejected_active("ns/kept"));
        assert!(Arc::ptr_eq(&store.snapshot().data.entries["ns/kept"].tls, &repaired));
    }

    #[test]
    fn overlapping_add_update_uses_added_predecessor_and_remove_wins() {
        let store = TlsStore::new();
        store.replace_all_tls(HashMap::from([(
            "ns/recreated".to_string(),
            make_valid_tls("ns", "recreated", &["old.example.com"]),
        )]));
        let added = make_valid_tls("ns", "recreated", &["new.example.com"]);
        let invalid = make_invalid_tls("ns", "recreated", &["bad.example.com"]);
        store.apply_partial_update(
            HashMap::from([("ns/recreated".to_string(), added.clone())]),
            HashMap::from([("ns/recreated".to_string(), invalid.clone())]),
            &HashSet::new(),
        );
        assert!(Arc::ptr_eq(&store.snapshot().data.entries["ns/recreated"].tls, &added));
        assert!(store
            .snapshot()
            .match_sni_candidates(&listener(443), "old.example.com")
            .is_empty());
        store.apply_partial_update(
            HashMap::from([("ns/recreated".to_string(), invalid.clone())]),
            HashMap::from([("ns/recreated".to_string(), invalid.clone())]),
            &HashSet::new(),
        );
        assert_eq!(store.snapshot().get_cert_stats(), (0, 1));
        assert_eq!(store.snapshot().listener_count(), 0);
        store.apply_partial_update(
            HashMap::from([("ns/recreated".to_string(), added)]),
            HashMap::from([("ns/recreated".to_string(), invalid)]),
            &HashSet::from(["ns/recreated".to_string()]),
        );
        assert_eq!(store.snapshot().entry_count(), 0);
        assert_eq!(store.snapshot().get_cert_stats(), (0, 0));
    }

    #[test]
    fn collision_membership_and_winner_transitions_are_bounded() {
        let store = TlsStore::new();
        let old_time = Time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
        let new_time = Time(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        let mut older = make_valid_tls("ns", "z-old", &["shared.example.com"]);
        Arc::get_mut(&mut older).unwrap().metadata.creation_timestamp = Some(old_time.clone());
        let mut newer = make_valid_tls("ns", "a-new", &["shared.example.com"]);
        Arc::get_mut(&mut newer).unwrap().metadata.creation_timestamp = Some(new_time.clone());
        store.replace_all_tls(HashMap::from([
            ("ns/z-old".to_string(), older.clone()),
            ("ns/a-new".to_string(), newer.clone()),
        ]));
        let first = store.snapshot();
        let first_slot = &first.data.listeners[&listener(443)].hosts["shared.example.com"];
        assert!(Arc::ptr_eq(&first_slot.ordered[0], &older));
        assert!(collision_changed(None, first_slot));

        let mut renewal = make_valid_tls("ns", "z-old", &["shared.example.com"]);
        Arc::get_mut(&mut renewal).unwrap().metadata.creation_timestamp = Some(old_time);
        publish_update(&store, "z-old", renewal);
        let renewed = store.snapshot();
        let renewed_slot = &renewed.data.listeners[&listener(443)].hosts["shared.example.com"];
        assert!(
            !collision_changed(Some(first_slot), renewed_slot),
            "renewal must not re-warn an unchanged collision"
        );

        let mut promoted = (*newer).clone();
        promoted.metadata.creation_timestamp = None;
        publish_update(&store, "a-new", Arc::new(promoted));
        let promoted = store.snapshot();
        let promoted_slot = &promoted.data.listeners[&listener(443)].hosts["shared.example.com"];
        assert!(collision_changed(Some(renewed_slot), promoted_slot));
        assert_eq!(promoted_slot.ordered[0].metadata.name.as_deref(), Some("a-new"));

        store.apply_partial_update(HashMap::new(), HashMap::new(), &HashSet::from(["ns/a-new".to_string()]));
        let single = store.snapshot();
        let single_slot = &single.data.listeners[&listener(443)].hosts["shared.example.com"];
        assert!(!collision_changed(Some(promoted_slot), single_slot));
        assert_eq!(single_slot.ordered[0].metadata.name.as_deref(), Some("z-old"));

        publish_update(&store, "a-new", newer);
        let collided_again = store.snapshot();
        assert!(collision_changed(
            Some(single_slot),
            &collided_again.data.listeners[&listener(443)].hosts["shared.example.com"]
        ));
    }

    #[test]
    fn exact_attachment_overflow_recovers_without_scanning_unrelated_candidates() {
        let store = TlsStore::new();
        let exact = attachment(443);
        let template = make_valid_tls("ns", "template", &["shared.example.com"]);
        let resources: HashMap<_, _> = (0..MAX_ATTACHMENT_CANDIDATES)
            .map(|index| {
                let mut tls = (*template).clone();
                tls.metadata.name = Some(format!("cert-{index:02}"));
                tls.spec.resolved_listener_attachments = Some(vec![exact.clone(), exact.clone()]);
                // Duplicate resolved listeners must not create self-collisions.
                tls.spec.resolved_listeners = Some(vec![listener(443), listener(443)]);
                (format!("ns/cert-{index:02}"), Arc::new(tls))
            })
            .collect();
        store.replace_all_tls(resources);
        let ready = store.snapshot();
        assert_eq!(
            ready.data.attachments[&exact].candidates.size(),
            MAX_ATTACHMENT_CANDIDATES
        );
        assert_eq!(
            ready
                .match_attachment_sni(&exact, "shared.example.com")
                .unwrap()
                .metadata
                .name
                .as_deref(),
            Some("cert-00")
        );

        let mut extra = (*template).clone();
        extra.metadata.name = Some("overflow".to_string());
        extra.spec.resolved_listener_attachments = Some(vec![exact.clone()]);
        publish_update(&store, "overflow", Arc::new(extra));
        let overflow = store.snapshot();
        assert_eq!(
            overflow.data.attachments[&exact].candidates.size(),
            MAX_ATTACHMENT_CANDIDATES + 1
        );
        assert!(overflow.match_attachment_sni(&exact, "shared.example.com").is_none());
        assert!(ready.match_attachment_sni(&exact, "shared.example.com").is_some());

        store.apply_partial_update(
            HashMap::new(),
            HashMap::new(),
            &HashSet::from(["ns/cert-00".to_string()]),
        );
        let recovered = store.snapshot();
        assert_eq!(
            recovered.data.attachments[&exact].candidates.size(),
            MAX_ATTACHMENT_CANDIDATES
        );
        assert_eq!(
            recovered
                .match_attachment_sni(&exact, "shared.example.com")
                .unwrap()
                .metadata
                .name
                .as_deref(),
            Some("cert-01")
        );
        assert!(overflow.match_attachment_sni(&exact, "shared.example.com").is_none());
    }

    #[test]
    fn exact_attachment_lookup_is_not_shadowed_by_more_than_64_unrelated_candidates() {
        let store = TlsStore::new();
        let template = make_valid_tls("ns", "template", &["shared.example.com"]);
        let exact = attachment(443);
        let mut resources: HashMap<_, _> = (0..65)
            .map(|index| {
                let mut tls = (*template).clone();
                tls.metadata.name = Some(format!("unrelated-{index}"));
                (format!("ns/unrelated-{index}"), Arc::new(tls))
            })
            .collect();
        let mut overlay = (*template).clone();
        overlay.metadata.name = Some("overlay".to_string());
        overlay.spec.resolved_listener_attachments = Some(vec![exact.clone()]);
        let overlay = Arc::new(overlay);
        resources.insert("ns/overlay".to_string(), overlay.clone());
        store.replace_all_tls(resources);
        assert!(Arc::ptr_eq(
            &store
                .snapshot()
                .match_attachment_sni(&exact, "shared.example.com")
                .unwrap(),
            &overlay
        ));
    }

    #[test]
    fn full_and_partial_publication_agree_and_empty_deltas_do_not_publish() {
        let full = TlsStore::new();
        let partial = TlsStore::new();
        let valid = make_valid_tls("ns", "valid", &["shared.example.com"]);
        let invalid = make_invalid_tls("ns", "invalid", &["bad.example.com"]);
        let data = HashMap::from([("ns/valid".to_string(), valid), ("ns/invalid".to_string(), invalid)]);
        full.replace_all_tls(data.clone());
        partial.apply_partial_update(data, HashMap::new(), &HashSet::new());
        let snapshot = partial.snapshot();
        assert_eq!(snapshot.get_cert_stats(), full.snapshot().get_cert_stats());
        assert_eq!(snapshot.listener_count(), full.snapshot().listener_count());
        assert!(Arc::ptr_eq(
            &snapshot.match_sni_candidates(&listener(443), "shared.example.com")[0],
            &full
                .snapshot()
                .match_sni_candidates(&listener(443), "shared.example.com")[0],
        ));
        partial.apply_partial_update(HashMap::new(), HashMap::new(), &HashSet::new());
        partial.apply_partial_update(
            HashMap::new(),
            HashMap::new(),
            &HashSet::from(["ns/absent".to_string()]),
        );
        assert!(Arc::ptr_eq(&snapshot.data, &partial.snapshot().data));
    }
    #[test]
    fn conf_handler_preserves_shared_resource_allocation() {
        use edgion_common::conf_sync::traits::ConfHandler;

        let store = TlsStore::new();
        let initial = make_valid_tls("ns", "shared", &["shared.example.com"]);
        let full = HashMap::from([("ns/shared".to_string(), initial.clone())]);
        ConfHandler::<EdgionTls>::full_set(&store, &full);

        let snapshot = store.matcher.data.load();
        assert!(Arc::ptr_eq(&snapshot.entries["ns/shared"].tls, &initial));
        drop(snapshot);

        let updated = make_valid_tls("ns", "shared", &["updated.example.com"]);
        ConfHandler::<EdgionTls>::partial_update(
            &store,
            HashMap::new(),
            HashMap::from([("ns/shared".to_string(), updated.clone())]),
            std::collections::HashSet::new(),
        );

        let snapshot = store.matcher.data.load();
        assert!(Arc::ptr_eq(&snapshot.entries["ns/shared"].tls, &updated));
    }
}
