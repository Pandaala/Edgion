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

/// Add one resource's hosts to a listener bucket using the same canonical
/// ASCII case as SNI lookup. Duplicate case variants within one resource are
/// collapsed so they cannot create a false self-collision.
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
    let selected = tls_list.first().map(fmt).unwrap_or_default();
    tracing::warn!(
        component = "tls_store",
        host = %host,
        listener_key = %listener_key,
        candidate_count = tls_list.len(),
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

    /// Insert `tls` under `key`, optionally retaining the existing valid entry
    /// when the new cert fails validation (last-known-good / Envoy SDS NACK
    /// semantics for updates, disabled for a new resource generation).
    fn insert_validated(
        &self,
        new_map: &mut HashMap<String, TlsEntry>,
        key: String,
        tls: Arc<EdgionTls>,
        retain_last_known_good: bool,
    ) {
        warn_cert_advisories(&key, &tls);
        let tls = compile_allowed_sans_for_runtime(tls);
        let validation = validate_cert(&tls);
        if !validation.is_valid {
            if retain_last_known_good {
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
            }
            let reason = validation
                .errors
                .first()
                .map(|error| error.status_reason())
                .unwrap_or("Invalid");
            tracing::warn!(component = "tls_store", key = %key, reason = %reason,
                error_count = validation.errors.len(),
                "Certificate validation failed");
        }
        new_map.insert(
            key,
            TlsEntry {
                tls,
                validation,
                rejected_active: false,
            },
        );
    }

    /// Replace all TLS certificates with the provided data.
    ///
    /// Last-known-good semantics (mirrors `insert_validated`, but "existing" is the
    /// current store snapshot rather than `new_map`, because a full snapshot has no
    /// in-flight predecessor for the same key):
    /// - Invalid incoming cert + current store has a VALID entry for that key
    ///   → carry the old valid entry forward, log the rejection, skip inserting bad data.
    /// - Keys absent from `data` are intentionally removed (never carried forward).
    pub fn replace_all_tls(&self, data: HashMap<String, Arc<EdgionTls>>) {
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
            let tls = compile_allowed_sans_for_runtime(tls);
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
                let reason = validation
                    .errors
                    .first()
                    .map(|error| error.status_reason())
                    .unwrap_or("Invalid");
                tracing::warn!(component = "tls_store", key = %key, reason = %reason,
                    error_count = validation.errors.len(),
                    "Certificate validation failed");
            }
            new_map.insert(
                key,
                TlsEntry {
                    tls,
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
        add: HashMap<String, Arc<EdgionTls>>,
        update: HashMap<String, Arc<EdgionTls>>,
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

        // TODO(tls-store-true-incremental-atomic-publication): replace this
        // whole-map clone and matcher rebuild with a genuine per-key update and
        // publish data plus matcher as one immutable generation.
        let current = self.tls_data.load_full();
        let mut new_map = (*current).clone();

        // Add new certificates
        for (key, tls) in add {
            // A compressed add can represent delete→create for the same key.
            // Never inherit the deleted generation's last-known-good entry.
            self.insert_validated(&mut new_map, key, tls, false);
        }

        // Update existing certificates
        for (key, tls) in update {
            self.insert_validated(&mut new_map, key, tls, true);
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
                        total_hosts += add_canonical_hosts(host_map, tls);
                    }
                    valid_certs += 1;
                }
                _ => {
                    skipped_unbound_certs += 1;
                    tracing::warn!(
                        component = "tls_store",
                        key = %key,
                        host_count = tls.spec.hosts.len(),
                        parent_ref_count = tls.spec.parent_refs.as_ref().map_or(0, Vec::len),
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

        let snapshot = store.tls_data.load();
        let entry = snapshot
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

    #[test]
    fn conf_handler_preserves_shared_resource_allocation() {
        use edgion_common::conf_sync::traits::ConfHandler;

        let store = TlsStore::new();
        let initial = make_valid_tls("ns", "shared", &["shared.example.com"]);
        let full = HashMap::from([("ns/shared".to_string(), initial.clone())]);
        ConfHandler::<EdgionTls>::full_set(&store, &full);

        let snapshot = store.tls_data.load();
        assert!(Arc::ptr_eq(&snapshot["ns/shared"].tls, &initial));
        drop(snapshot);

        let updated = make_valid_tls("ns", "shared", &["updated.example.com"]);
        ConfHandler::<EdgionTls>::partial_update(
            &store,
            HashMap::new(),
            HashMap::from([("ns/shared".to_string(), updated.clone())]),
            std::collections::HashSet::new(),
        );

        let snapshot = store.tls_data.load();
        assert!(Arc::ptr_eq(&snapshot["ns/shared"].tls, &updated));
    }
}
