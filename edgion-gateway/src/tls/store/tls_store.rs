use crate::tls::validation::cert::{validate_cert, CertValidationResult};
use arc_swap::ArcSwap;
use edgion_resources::listener_key::ListenerKey;
use edgion_resources::matcher::HashHost;
use edgion_resources::EdgionTls;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// Internal entry for TLS certificate with validation status
#[derive(Clone)]
struct TlsEntry {
    tls: Arc<EdgionTls>,
    validation: CertValidationResult,
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

/// Warn (config-sync path only — never the handshake path) when several EdgionTls
/// resources serve the same host on one listener. `tls_list` must already be sorted
/// by [`edgion_tls_precedence`], so `first()` is the selected certificate.
fn warn_edgion_tls_collision(host: &str, listener_key: &ListenerKey, tls_list: &[Arc<EdgionTls>]) {
    let fmt = |t: &Arc<EdgionTls>| {
        format!(
            "{}/{}",
            t.metadata.namespace.as_deref().unwrap_or(""),
            t.metadata.name.as_deref().unwrap_or("")
        )
    };
    let participants = tls_list.iter().map(fmt).collect::<Vec<_>>().join(", ");
    let selected = tls_list.first().map(fmt).unwrap_or_default();
    tracing::warn!(
        component = "tls_store",
        host = %host,
        listener_key = %listener_key,
        participants = %participants,
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

/// TlsStore manages all TLS certificates and builds the TLS matcher
pub struct TlsStore {
    // Key: namespace/name (or name for cluster-scoped).
    // Single writer (config-sync). Readers are lock-free.
    tls_data: ArcSwap<HashMap<String, TlsEntry>>,
}

impl TlsStore {
    pub fn new() -> Self {
        Self {
            tls_data: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Insert `tls` under `key`, or RETAIN the existing valid entry when the new
    /// cert fails validation (last-known-good / Envoy SDS NACK semantics).
    fn insert_or_retain(&self, new_map: &mut HashMap<String, TlsEntry>, key: String, tls: EdgionTls) {
        warn_cert_advisories(&key, &tls);
        let validation = validate_cert(&tls);
        if !validation.is_valid {
            if let Some(existing) = new_map.get(&key) {
                if existing.validation.is_valid {
                    if !existing.rejected_active {
                        record_rejected(&key, &validation);
                    }
                    let mut kept = existing.clone();
                    kept.rejected_active = true;
                    new_map.insert(key, kept);
                    return;
                }
            }
            tracing::warn!(component = "tls_store", key = %key, errors = ?validation.errors,
                "Certificate validation failed");
        }
        new_map.insert(
            key,
            TlsEntry {
                tls: Arc::new(tls),
                validation,
                rejected_active: false,
            },
        );
    }

    /// Replace all TLS certificates with the provided data.
    ///
    /// Last-known-good semantics (mirrors `insert_or_retain`, but "existing" is the
    /// current store snapshot rather than `new_map`, because a full snapshot has no
    /// in-flight predecessor for the same key):
    /// - Invalid incoming cert + current store has a VALID entry for that key
    ///   → carry the old valid entry forward, log the rejection, skip inserting bad data.
    /// - Keys absent from `data` are intentionally removed (never carried forward).
    pub fn replace_all_tls(&self, data: HashMap<String, EdgionTls>) {
        let cert_count = data.len();
        tracing::info!(
            component = "tls_store",
            count = cert_count,
            "Full set of TLS certificates"
        );

        // Read current snapshot ONCE before the loop so we can fall back to the
        // last-known-good entry per key when an incoming cert is invalid.
        let current = self.tls_data.load_full();
        let mut new_map: HashMap<String, TlsEntry> = HashMap::with_capacity(cert_count);

        for (key, tls) in data {
            warn_cert_advisories(&key, &tls);
            let validation = validate_cert(&tls);
            if !validation.is_valid {
                if let Some(existing) = current.get(&key) {
                    if existing.validation.is_valid {
                        if !existing.rejected_active {
                            record_rejected(&key, &validation);
                        }
                        let mut kept = existing.clone();
                        kept.rejected_active = true;
                        new_map.insert(key, kept);
                        continue;
                    }
                }
                tracing::warn!(component = "tls_store", key = %key, errors = ?validation.errors,
                    "Certificate validation failed");
            }
            new_map.insert(
                key,
                TlsEntry {
                    tls: Arc::new(tls),
                    validation,
                    rejected_active: false,
                },
            );
        }

        self.rebuild_matcher_from_data(&new_map);
        self.tls_data.store(Arc::new(new_map));
    }

    /// Partially update TLS certificates
    pub fn apply_partial_update(
        &self,
        add: HashMap<String, EdgionTls>,
        update: HashMap<String, EdgionTls>,
        remove: &std::collections::HashSet<String>,
    ) {
        // Capture counts before moving
        let add_count = add.len();
        let update_count = update.len();
        let remove_count = remove.len();

        tracing::info!(
            component = "tls_store",
            add = add_count,
            update = update_count,
            remove = remove_count,
            "Partial update of TLS certificates"
        );

        let current = self.tls_data.load_full();
        let mut new_map = (*current).clone();

        // Add new certificates
        for (key, tls) in add {
            self.insert_or_retain(&mut new_map, key, tls);
        }

        // Update existing certificates
        for (key, tls) in update {
            self.insert_or_retain(&mut new_map, key, tls);
        }

        // Remove certificates
        for key in remove {
            new_map.remove(key);
        }

        self.rebuild_matcher_from_data(&new_map);

        self.tls_data.store(Arc::new(new_map));
    }

    /// Rebuild the TLS certificate matcher from provided data.
    /// Only listener-resolved certs are admitted into the matcher.
    ///
    /// # Arguments
    /// * `tls_data` - Reference to the TLS data map
    fn rebuild_matcher_from_data(&self, tls_data: &HashMap<String, TlsEntry>) {
        let mut listener_host_map: HashMap<ListenerKey, HashMap<String, Vec<Arc<EdgionTls>>>> = HashMap::new();
        let mut total_hosts = 0;
        let mut valid_certs = 0;
        let mut invalid_certs = 0;
        let mut skipped_unbound_certs = 0;

        for (key, entry) in tls_data.iter() {
            if !entry.validation.is_valid {
                tracing::debug!(
                    component = "tls_store",
                    key = %key,
                    "Skipping invalid certificate from matcher"
                );
                invalid_certs += 1;
                continue;
            }

            let tls = &entry.tls;

            match &tls.spec.resolved_listeners {
                Some(listeners) if !listeners.is_empty() => {
                    for listener_key in listeners {
                        let host_map = listener_host_map.entry(listener_key.clone()).or_default();
                        for host in &tls.spec.hosts {
                            host_map.entry(host.clone()).or_default().push(tls.clone());
                            total_hosts += 1;
                        }
                    }
                    valid_certs += 1;
                }
                _ => {
                    skipped_unbound_certs += 1;
                    tracing::warn!(
                        component = "tls_store",
                        key = %key,
                        hosts = ?tls.spec.hosts,
                        parent_refs = ?tls.spec.parent_refs,
                        "Skipping EdgionTls from matcher because no listener keys were resolved"
                    );
                }
            }
        }

        let mut listener_matcher = HashMap::new();
        for (listener_key, host_map) in listener_host_map {
            let mut matcher = HashHost::new();
            for (host, mut tls_list) in host_map {
                if tls_list.len() > 1 {
                    // Two or more EdgionTls resources resolve to the same host on the same
                    // listener. Nothing admission-blocks this, and `match_sni` returns the
                    // first entry — so without a stable sort the served certificate would
                    // flip with HashMap iteration order across rebuilds. Sort by the Gateway
                    // API precedence (oldest creationTimestamp wins, then namespace/name) so
                    // the selection is deterministic, and warn the operator.
                    tls_list.sort_by(edgion_tls_precedence);
                    warn_edgion_tls_collision(&host, &listener_key, &tls_list);
                }
                matcher.insert(&host, tls_list);
            }
            listener_matcher.insert(listener_key, matcher);
        }

        super::cert_matcher::get_tls_cert_matcher().set(listener_matcher);

        tracing::info!(
            component = "tls_store",
            valid_certs = valid_certs,
            invalid_certs = invalid_certs,
            skipped_unbound_certs = skipped_unbound_certs,
            total_hosts = total_hosts,
            "TLS matcher rebuilt successfully"
        );
    }

    /// Returns true if a certificate with the given key ("namespace/name") exists,
    /// regardless of validity.
    ///
    /// Lock-free: O(1) hash lookup on the current snapshot.
    #[cfg(test)]
    pub fn contains(&self, key: &str) -> bool {
        let snapshot = self.tls_data.load();
        snapshot.contains_key(key)
    }

    /// Returns the validation status for the given key ("namespace/name"),
    /// or `None` if no such entry exists.
    ///
    /// Lock-free: O(1) hash lookup on the current snapshot. Result is cloned.
    #[cfg(test)]
    pub fn get_validation_status(&self, key: &str) -> Option<CertValidationResult> {
        let snapshot = self.tls_data.load();
        snapshot.get(key).map(|entry| entry.validation.clone())
    }

    /// Returns `(valid_count, invalid_count)` for all stored certificates.
    ///
    /// Lock-free: O(n) iteration over the current snapshot.
    ///
    /// ```ignore
    /// let (valid, invalid) = store.get_cert_stats();
    /// println!("Valid: {}, Invalid: {}", valid, invalid);
    /// ```
    pub fn get_cert_stats(&self) -> (usize, usize) {
        let snapshot = self.tls_data.load();
        let total = snapshot.len();
        let invalid = snapshot.values().filter(|entry| !entry.validation.is_valid).count();
        (total - invalid, invalid)
    }
}

impl TlsStore {
    /// Total number of TLS entries (valid + invalid).
    pub fn entry_count(&self) -> usize {
        self.tls_data.load().len()
    }
}

impl Default for TlsStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl TlsStore {
    fn is_rejected_active(&self, key: &str) -> bool {
        self.tls_data
            .load()
            .get(key)
            .map(|e| e.rejected_active)
            .unwrap_or(false)
    }
}

/// Global TlsStore instance
static GLOBAL_TLS_STORE: LazyLock<Arc<TlsStore>> = LazyLock::new(|| Arc::new(TlsStore::new()));

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
                resolved_log_labels: None,
            },
            status: None,
        })
    }

    /// Build an EdgionTls whose secret contains a real self-signed cert+key so
    /// that `validate_cert` returns `is_valid = true`.
    fn make_valid_tls(ns: &str, name: &str, hosts: &[&str]) -> EdgionTls {
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

        EdgionTls {
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
                resolved_log_labels: None,
            },
            status: None,
        }
    }

    /// Build an EdgionTls whose secret has non-PEM bytes so that
    /// `validate_cert` returns `is_valid = false`.
    fn make_invalid_tls(ns: &str, name: &str, hosts: &[&str]) -> EdgionTls {
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

        EdgionTls {
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
                resolved_log_labels: None,
            },
            status: None,
        }
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
}
