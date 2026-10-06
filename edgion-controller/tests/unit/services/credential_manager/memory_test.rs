use super::*;

fn test_entry(key: &str) -> GeneratedSecretEntry {
    GeneratedSecretEntry {
        key: key.to_string(),
        value: Zeroizing::new(b"token-bytes".to_vec()),
        issued_at: "2026-08-27T00:00:00Z".to_string(),
        valid_until: "2026-08-27T01:00:00Z".to_string(),
        valid_until_mono: Instant::now(),
    }
}

#[test]
fn oauth_insert_keeps_only_active_and_releases_previous() {
    let store = CredentialMemoryStore::new();
    let mut previous = Vec::new();
    for id in ["p1", "p2", "p3"] {
        store.insert_oauth_publication("ns/src/uid", test_entry(id));
        previous.push(Arc::downgrade(&store.active("ns/src/uid").unwrap()));
    }
    assert_eq!(store.source_count(), 1);
    assert_eq!(store.key_count(), 1);
    assert_eq!(store.active("ns/src/uid").unwrap().key, "p3");
    assert!(previous[0].upgrade().is_none());
    assert!(previous[1].upgrade().is_none());
    assert!(previous[2].upgrade().is_some());
}

#[test]
fn replacement_and_clear_preserve_external_arc_until_last_holder_drops() {
    let store = CredentialMemoryStore::new();
    store.insert_oauth_publication("ns/src/uid", test_entry("a"));
    let previous = store.active("ns/src/uid").unwrap();
    let weak = Arc::downgrade(&previous);
    store.insert_oauth_publication("ns/src/uid", test_entry("b"));
    assert_eq!(store.key_count(), 1);
    assert_eq!(store.active("ns/src/uid").unwrap().key, "b");
    assert_eq!(previous.key, "a");
    assert_eq!(&*previous.value, b"token-bytes");

    store.clear_source("ns/src/uid");
    assert!(store.active("ns/src/uid").is_none());
    assert_eq!(store.key_count(), 0);
    assert_eq!(previous.key, "a");
    drop(previous);
    assert!(weak.upgrade().is_none());
}

#[test]
fn source_identities_are_isolated_across_namespaces_names_and_uids() {
    let store = CredentialMemoryStore::new();
    let identities = ["ns/src/uid", "other/src/uid", "ns/other/uid", "ns/src/other-uid"];
    for identity in identities {
        store.insert_oauth_publication(identity, test_entry(identity));
    }
    store.insert_oauth_publication(identities[0], test_entry("rotated"));
    assert_eq!(store.source_count(), 4);
    assert_eq!(store.key_count(), 4);
    for identity in &identities[1..] {
        assert_eq!(store.active(identity).unwrap().key, *identity);
    }

    store.clear_source(identities[0]);
    assert!(store.active(identities[0]).is_none());
    assert_eq!(store.key_count(), 3);
    for identity in &identities[1..] {
        assert_eq!(store.active(identity).unwrap().key, *identity);
    }
    store.clear();
    assert_eq!(store.source_count(), 0);
    assert_eq!(store.key_count(), 0);
    assert!(store.source_keys().is_empty());
}

#[test]
fn debug_redacts_value() {
    let entry = test_entry("p1");
    let rendered = format!("{entry:?}");
    assert!(!rendered.contains("token-bytes"));
    assert!(rendered.contains("[redacted]") || !rendered.contains("tok"));
}

#[test]
fn empty_store_reports_zero() {
    let store = CredentialMemoryStore::new();
    assert_eq!(store.source_count(), 0);
    assert_eq!(store.key_count(), 0);
    assert!(store.source_keys().is_empty());
}
