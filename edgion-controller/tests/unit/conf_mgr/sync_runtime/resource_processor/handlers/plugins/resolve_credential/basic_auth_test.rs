use super::*;
use std::collections::HashMap;

fn make_user(password: &str, group: &str) -> ResolvedBasicAuthUser {
    ResolvedBasicAuthUser {
        password: password.to_string(),
        group: group.to_string(),
    }
}

#[test]
fn first_wins_second_group_rejected() {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut all_users: HashMap<String, ResolvedBasicAuthUser> = HashMap::new();

    let inserted = first_wins_insert(
        &mut seen,
        &mut all_users,
        "alice".to_string(),
        make_user("pass-a", "partner-a"),
        "partner-a".to_string(),
    );
    assert!(inserted, "first insert must succeed");

    let inserted_again = first_wins_insert(
        &mut seen,
        &mut all_users,
        "alice".to_string(),
        make_user("pass-b", "partner-b"),
        "partner-b".to_string(),
    );
    assert!(!inserted_again, "second insert must be rejected (first-wins)");

    let stored = all_users.get("alice").unwrap();
    assert_eq!(stored.group.as_str(), "partner-a", "first group must win");
    assert_eq!(stored.password.as_str(), "pass-a", "partner-a password must be kept");

    assert_eq!(seen.len(), 1);
    assert_eq!(seen.get("alice").unwrap().as_str(), "partner-a");
}

#[test]
fn first_wins_distinct_usernames_both_inserted() {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut all_users: HashMap<String, ResolvedBasicAuthUser> = HashMap::new();

    assert!(first_wins_insert(
        &mut seen,
        &mut all_users,
        "alice".to_string(),
        make_user("pass-a", "partner-a"),
        "partner-a".to_string(),
    ));
    assert!(first_wins_insert(
        &mut seen,
        &mut all_users,
        "bob".to_string(),
        make_user("pass-b", "partner-b"),
        "partner-b".to_string(),
    ));

    assert_eq!(all_users.len(), 2);
    assert_eq!(all_users.get("alice").unwrap().group.as_str(), "partner-a");
    assert_eq!(all_users.get("bob").unwrap().group.as_str(), "partner-b");
}
