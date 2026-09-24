use super::*;
use std::collections::HashMap;

fn make_credential(secret: &str, group: &str) -> HmacCredential {
    HmacCredential {
        secret: secret.as_bytes().to_vec(),
        headers: HashMap::new(),
        group: group.to_string(),
    }
}

#[test]
fn first_wins_second_group_rejected() {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut all_credentials: HashMap<String, HmacCredential> = HashMap::new();

    // partner-a inserts alice — should succeed.
    let inserted = first_wins_insert(
        &mut seen,
        &mut all_credentials,
        "alice".to_string(),
        make_credential("partner-a-secret", "partner-a"),
        "partner-a".to_string(),
    );
    assert!(inserted, "first insert must succeed");

    // partner-b tries alice — must be rejected.
    let inserted_again = first_wins_insert(
        &mut seen,
        &mut all_credentials,
        "alice".to_string(),
        make_credential("partner-b-secret", "partner-b"),
        "partner-b".to_string(),
    );
    assert!(!inserted_again, "second insert must be rejected (first-wins)");

    // Verify partner-a's credential is the one kept.
    let stored = all_credentials.get("alice").unwrap();
    assert_eq!(stored.group.as_str(), "partner-a", "first group must win");
    assert_eq!(stored.secret, b"partner-a-secret", "partner-a secret must be kept");

    assert_eq!(seen.len(), 1);
    assert_eq!(seen.get("alice").unwrap().as_str(), "partner-a");
}

#[test]
fn first_wins_distinct_usernames_both_inserted() {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut all_credentials: HashMap<String, HmacCredential> = HashMap::new();

    assert!(first_wins_insert(
        &mut seen,
        &mut all_credentials,
        "alice".to_string(),
        make_credential("alice-secret", "partner-a"),
        "partner-a".to_string(),
    ));
    assert!(first_wins_insert(
        &mut seen,
        &mut all_credentials,
        "bob".to_string(),
        make_credential("bob-secret", "partner-b"),
        "partner-b".to_string(),
    ));

    assert_eq!(all_credentials.len(), 2);
    assert_eq!(all_credentials.get("alice").unwrap().group.as_str(), "partner-a");
    assert_eq!(all_credentials.get("bob").unwrap().group.as_str(), "partner-b");
}
