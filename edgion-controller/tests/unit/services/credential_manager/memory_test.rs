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
fn oauth_insert_keeps_only_active_and_previous() {
    let store = CredentialMemoryStore::new();
    for id in ["p1", "p2", "p3"] {
        store
            .insert_oauth_publication("ns/src/uid", test_entry(id), 10_000)
            .unwrap();
    }
    assert_eq!(store.key_count(), 2);
    assert_eq!(store.active("ns/src/uid").unwrap().key, "p3");
}

#[test]
fn never_drops_active_when_cap_is_one() {
    let store = CredentialMemoryStore::new();
    store
        .insert_oauth_publication("ns/src/uid", test_entry("a"), 1)
        .unwrap();
    store
        .insert_oauth_publication("ns/src/uid", test_entry("b"), 1)
        .unwrap();
    assert_eq!(store.key_count(), 1);
    assert_eq!(store.active("ns/src/uid").unwrap().key, "b");
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
