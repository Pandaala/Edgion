use super::*;
use chrono::{TimeZone, Utc};
use edgion_resources::resources::edgion_tls::EdgionTlsSpec;
use edgion_resources::resources::gateway::SecretObjectReference;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::api::ObjectMeta;

/// Wrap a fixture resource for the index-shaped assertions. These exercise
/// precedence and bucket placement only, so the secret-bearing state the
/// publication path would compile is deliberately left uncompiled.
fn uncompiled(tls: Arc<EdgionTls>) -> Arc<CompiledEdgionTls> {
    Arc::new(CompiledEdgionTls::not_compiled(tls))
}

fn make_etls(namespace: &str, name: &str, ts: Option<Time>) -> Arc<EdgionTls> {
    Arc::new(EdgionTls {
        metadata: ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            creation_timestamp: ts,
            ..Default::default()
        },
        spec: EdgionTlsSpec {
            current_status: None,
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
            current_status: None,
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
            current_status: None,
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
    let installed = store.snapshot().data.entries["ns/foo"].compiled.clone();
    let validity = installed.identity.as_ref().unwrap().validity;

    let bad = make_invalid_tls("ns", "foo", &["foo.example.com"]);
    store.apply_partial_update(
        std::collections::HashMap::new(),
        std::collections::HashMap::from([("ns/foo".to_string(), bad)]),
        &std::collections::HashSet::new(),
    );

    let v = store.get_validation_status("ns/foo").expect("entry must remain");
    assert!(v.is_valid, "bad update must not overwrite the last-known-good cert");
    let retained = store.snapshot().data.entries["ns/foo"].compiled.clone();
    assert!(Arc::ptr_eq(&installed, &retained));
    assert_eq!(retained.identity.as_ref().unwrap().validity, validity);
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
    assert_eq!(snapshot.entries["ns/bounded"].compiled.tls.spec.hosts.len(), 2);
    assert!(
        !dimensions_are_bounded(&snapshot.entries["ns/bounded"].compiled.tls),
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
        vec![uncompiled(newcomer.clone()), uncompiled(incumbent.clone())],
        vec![uncompiled(incumbent.clone()), uncompiled(newcomer.clone())],
    ] {
        list.sort_by(edgion_tls_precedence);
        assert_eq!(
            list.first().unwrap().tls.metadata.name.as_deref(),
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

    let mut list = [uncompiled(b), uncompiled(a)];
    list.sort_by(edgion_tls_precedence);
    assert_eq!(list.first().unwrap().tls.metadata.namespace.as_deref(), Some("team-a"));
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
            add_canonical_hosts(&mut host_map, &uncompiled(tls));
        }
        assert_eq!(host_map.len(), 1, "ASCII case variants must share one bucket");
        let list = host_map.get_mut("shared.example.com").expect("canonical host bucket");
        assert_eq!(list.len(), 2, "one resource must not collide with itself");
        list.sort_by(edgion_tls_precedence);
        assert_eq!(list[0].tls.metadata.name.as_deref(), Some("older"));
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
    assert!(Arc::ptr_eq(&entry.compiled.tls, &recreated));
    assert!(!entry.validation.is_valid);
    assert_eq!(entry.compiled.tls.spec.hosts, ["new.example.com"]);
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
    assert!(Arc::ptr_eq(&after.data.entries["ns/same"].compiled.tls, &same_listener));
    assert!(Arc::ptr_eq(
        &after.data.entries["ns/other"].compiled.tls,
        &other_listener
    ));
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
        &snapshot.match_sni_candidates(&listener(443), "ONE.EXAMPLE.COM")[0].tls,
        &exact
    ));
    assert!(Arc::ptr_eq(
        &snapshot.match_sni_candidates(&listener(443), "two.example.com")[0].tls,
        &wildcard
    ));
    assert!(snapshot
        .match_sni_candidates(&listener(443), "a.b.example.com")
        .is_empty());
    store.apply_partial_update(HashMap::new(), HashMap::new(), &HashSet::from(["ns/exact".to_string()]));
    assert!(Arc::ptr_eq(
        &store.snapshot().match_sni_candidates(&listener(443), "one.example.com")[0].tls,
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
    assert!(Arc::ptr_eq(&before.data.entries["ns/moving"].compiled.tls, &old));
    assert!(before.get_validation_status("ns/moving").unwrap().is_valid);
    assert_eq!(before.get_cert_stats(), (1, 0));
    assert!(Arc::ptr_eq(
        &before.match_sni_candidates(&listener(443), "old.example.com")[0].tls,
        &old
    ));
    assert!(Arc::ptr_eq(
        &before
            .match_attachment_sni(&old_attachment, "old.example.com")
            .unwrap()
            .tls,
        &old
    ));
    assert!(after.match_sni_candidates(&listener(443), "old.example.com").is_empty());
    assert!(after.match_attachment_sni(&old_attachment, "old.example.com").is_none());
    assert!(Arc::ptr_eq(&after.data.entries["ns/moving"].compiled.tls, &new));
    assert!(Arc::ptr_eq(
        &after
            .match_attachment_sni(&new_attachment, "new.example.com")
            .unwrap()
            .tls,
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
    assert!(Arc::ptr_eq(&rejected.data.entries["ns/kept"].compiled.tls, &original));
    publish_update(&store, "kept", bad.clone());
    assert!(Arc::ptr_eq(&rejected.data, &store.snapshot().data));

    let repaired = make_valid_tls("ns", "kept", &["new.example.com"]);
    publish_update(&store, "kept", repaired.clone());
    assert!(!store.is_rejected_active("ns/kept"));
    publish_update(&store, "kept", bad);
    assert!(store.is_rejected_active("ns/kept"));
    assert!(Arc::ptr_eq(
        &store.snapshot().data.entries["ns/kept"].compiled.tls,
        &repaired
    ));
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
    assert!(Arc::ptr_eq(
        &store.snapshot().data.entries["ns/recreated"].compiled.tls,
        &added
    ));
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
    assert!(Arc::ptr_eq(&first_slot.ordered[0].tls, &older));
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
    assert_eq!(promoted_slot.ordered[0].tls.metadata.name.as_deref(), Some("a-new"));

    store.apply_partial_update(HashMap::new(), HashMap::new(), &HashSet::from(["ns/a-new".to_string()]));
    let single = store.snapshot();
    let single_slot = &single.data.listeners[&listener(443)].hosts["shared.example.com"];
    assert!(!collision_changed(Some(promoted_slot), single_slot));
    assert_eq!(single_slot.ordered[0].tls.metadata.name.as_deref(), Some("z-old"));

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
            .tls
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
            .tls
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
            .unwrap()
            .tls,
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
        &snapshot.match_sni_candidates(&listener(443), "shared.example.com")[0].tls,
        &full
            .snapshot()
            .match_sni_candidates(&listener(443), "shared.example.com")[0]
            .tls,
    ));
    partial.apply_partial_update(HashMap::new(), HashMap::new(), &HashSet::new());
    partial.apply_partial_update(
        HashMap::new(),
        HashMap::new(),
        &HashSet::from(["ns/absent".to_string()]),
    );
    assert!(Arc::ptr_eq(&snapshot.data, &partial.snapshot().data));
}
/// A resource whose mTLS CA content cannot be compiled must keep its
/// hostname slot and its collision precedence. `validate_cert` only checks
/// that a CA reference is present (`validate_mtls_config`), never the
/// controller-resolved `ca.crt` bytes, so this state is reachable in
/// production. Removing such an entry from the index would silently promote
/// the next candidate for that hostname.
#[test]
fn uncompilable_ca_keeps_index_position_and_fails_closed_at_handshake() {
    use edgion_resources::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let mut resource = (*make_valid_tls("ns", "broken-ca", &["shared.example.com"])).clone();
    resource.spec.resolved_listeners = Some(vec![listener(443)]);
    resource.spec.client_auth = Some(ClientAuthConfig {
        mode: ClientAuthMode::Mutual,
        ca_secret_ref: Some(SecretObjectReference {
            name: "ca".to_string(),
            namespace: Some("ns".to_string()),
            group: None,
            kind: None,
        }),
        verify_depth: 1,
        ca_secret: Some(Secret {
            data: Some(BTreeMap::from([(
                "ca.crt".to_string(),
                ByteString(b"not a pem at all".to_vec()),
            )])),
            ..Default::default()
        }),
        allowed_sans: None,
        allowed_cns: None,
    });
    let resource = Arc::new(resource);

    let store = TlsStore::new();
    store.replace_all_tls(HashMap::from([("ns/broken-ca".to_string(), resource.clone())]));

    // The store accepted it: a garbage CA body is not a validation failure.
    assert!(store.get_validation_status("ns/broken-ca").unwrap().is_valid);

    // It still owns its hostname slot rather than yielding it.
    let candidates = store
        .snapshot()
        .match_sni_candidates(&listener(443), "shared.example.com");
    assert_eq!(candidates.len(), 1, "an uncompilable CA must not drop the resource");
    assert!(Arc::ptr_eq(&candidates[0].tls, &resource));

    // And the handshake has a precompiled failure to fail closed on.
    assert!(
        candidates[0].client_ca_store.is_err(),
        "a garbage CA bundle must be carried as a compile failure"
    );
    // The serving identity itself is unaffected and still usable.
    assert!(candidates[0].identity.is_ok());
}

/// Cold-compiled state participates in the same structural sharing as the
/// resource: a partial update that does not touch a key must leave that
/// key's compiled generation allocation untouched, not recompile it.
#[test]
fn partial_update_shares_compiled_state_for_untouched_keys() {
    let store = TlsStore::new();
    let untouched = make_valid_tls("ns", "untouched", &["untouched.example.com"]);
    let mut initial = (*untouched).clone();
    initial.spec.resolved_listeners = Some(vec![listener(443)]);
    let untouched = Arc::new(initial);
    let moving = make_valid_tls("ns", "moving", &["moving.example.com"]);
    store.replace_all_tls(HashMap::from([
        ("ns/untouched".to_string(), untouched.clone()),
        ("ns/moving".to_string(), moving.clone()),
    ]));
    let before = store.matcher.data.load_full();

    store.apply_partial_update(
        HashMap::new(),
        HashMap::from([(
            "ns/moving".to_string(),
            make_valid_tls("ns", "moving", &["moved.example.com"]),
        )]),
        &HashSet::new(),
    );
    let after = store.matcher.data.load_full();

    assert!(
        Arc::ptr_eq(
            &before.entries["ns/untouched"].compiled,
            &after.entries["ns/untouched"].compiled
        ),
        "an untouched key must keep its compiled generation allocation"
    );
    assert!(
        !Arc::ptr_eq(
            &before.entries["ns/moving"].compiled,
            &after.entries["ns/moving"].compiled
        ),
        "an updated key must publish freshly compiled state"
    );
}

#[test]
fn conf_handler_preserves_shared_resource_allocation() {
    use edgion_common::conf_sync::traits::ConfHandler;

    let store = TlsStore::new();
    let initial = make_valid_tls("ns", "shared", &["shared.example.com"]);
    let full = HashMap::from([("ns/shared".to_string(), initial.clone())]);
    ConfHandler::<EdgionTls>::full_set(&store, &full);

    let snapshot = store.matcher.data.load();
    assert!(Arc::ptr_eq(&snapshot.entries["ns/shared"].compiled.tls, &initial));
    drop(snapshot);

    let updated = make_valid_tls("ns", "shared", &["updated.example.com"]);
    ConfHandler::<EdgionTls>::partial_update(
        &store,
        HashMap::new(),
        HashMap::from([("ns/shared".to_string(), updated.clone())]),
        std::collections::HashSet::new(),
    );

    let snapshot = store.matcher.data.load();
    assert!(Arc::ptr_eq(&snapshot.entries["ns/shared"].compiled.tls, &updated));
}

#[test]
fn regex_ignore_case_reconstructs_after_serde_and_updates_published_generations() {
    use edgion_common::allowed_san::{client_sans_match_allowed, PresentedSan};
    use edgion_resources::resources::edgion_tls::{
        AllowedSan, AllowedSanMatch, AllowedSanType, ClientAuthConfig, ClientAuthMode,
    };

    let store = TlsStore::new();
    let mut resource = (*make_valid_tls("ns", "regex", &["regex.example.com"])).clone();
    let mut san = AllowedSan::exact(AllowedSanType::Uri, r"^spiffe://cluster$");
    san.match_kind = AllowedSanMatch::RegularExpression;
    resource.spec.client_auth = Some(ClientAuthConfig {
        mode: ClientAuthMode::Terminate,
        ca_secret_ref: None,
        verify_depth: 1,
        ca_secret: None,
        allowed_sans: Some(vec![san]),
        allowed_cns: None,
    });
    let presented = [PresentedSan::uri("SPIFFE://CLUSTER")];
    let mut previous = None;
    for (index, ignore_case) in [false, true, false].into_iter().enumerate() {
        let auth = resource.spec.client_auth.as_mut().unwrap();
        let san = &mut auth.allowed_sans.as_mut().unwrap()[0];
        san.ignore_case = ignore_case;
        san.compile().unwrap();
        let wire = serde_json::to_vec(&resource).unwrap();
        let decoded: EdgionTls = serde_json::from_slice(&wire).unwrap();
        let allowed = decoded
            .spec
            .client_auth
            .as_ref()
            .unwrap()
            .allowed_sans
            .as_ref()
            .unwrap();
        assert_eq!(allowed[0].ignore_case, ignore_case);
        assert!(allowed[0].compiled_regex.is_none());
        assert!(!client_sans_match_allowed(&presented, allowed));
        let data = HashMap::from([("ns/regex".to_string(), Arc::new(decoded))]);
        if index == 0 {
            store.replace_all_tls(data);
        } else {
            store.apply_partial_update(HashMap::new(), data, &HashSet::new());
        }
        assert!(store.get_validation_status("ns/regex").unwrap().is_valid);
        let snapshot = store.snapshot();
        let compiled = snapshot.match_sni_candidates(&listener(443), "regex.example.com")[0].clone();
        let allowed = compiled
            .tls
            .spec
            .client_auth
            .as_ref()
            .unwrap()
            .allowed_sans
            .as_ref()
            .unwrap();
        assert!(allowed[0].compiled_regex.is_some());
        assert_eq!(client_sans_match_allowed(&presented, allowed), ignore_case);
        if let Some((old, old_ignore_case)) = previous.take() {
            let old: Arc<CompiledEdgionTls> = old;
            let allowed = old
                .tls
                .spec
                .client_auth
                .as_ref()
                .unwrap()
                .allowed_sans
                .as_ref()
                .unwrap();
            assert_eq!(client_sans_match_allowed(&presented, allowed), old_ignore_case);
        }
        previous = Some((compiled, ignore_case));
    }
}
