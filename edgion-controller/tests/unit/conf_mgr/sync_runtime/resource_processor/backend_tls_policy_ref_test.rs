use super::*;
use edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKind;
use edgion_resources::ResourceKind;

/// Parse an RFC3339 literal into the `DateTime<Utc>` the store now holds.
fn ts(s: &str) -> DateTime<Utc> {
    s.parse().expect("valid RFC3339")
}

fn make_policy_ref(ns: &str, name: &str) -> ResourceRef {
    ResourceRef::new(ResourceKind::BackendTLSPolicy, Some(ns.to_string()), name.to_string())
}

fn make_state() -> BackendTLSPolicyState {
    BackendTLSPolicyState::new()
}

fn service_target(ns: &str, name: &str) -> PolicyTargetKey {
    PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: ns.to_string(),
        name: name.to_string(),
    }
}

/// Set up two conflicting policies for testing on a given state instance.
fn setup_two_policies(state: &BackendTLSPolicyState, ns: &str) -> (ResourceRef, ResourceRef) {
    let policy_a = make_policy_ref(ns, "policy-a");
    let policy_b = make_policy_ref(ns, "policy-b");
    let target_key = make_target_conflict_key(&service_target(ns, "my-service"), None);

    state.target_mgr().add_ref(target_key.clone(), policy_a.clone());
    state.target_mgr().add_ref(target_key, policy_b.clone());

    state.set_timestamp(&format!("{}/policy-a", ns), Some(ts("2024-01-01T00:00:00Z")));
    state.set_timestamp(&format!("{}/policy-b", ns), Some(ts("2024-01-02T00:00:00Z")));

    (policy_a, policy_b)
}

#[test]
fn test_make_target_conflict_key_no_section() {
    let key = make_target_conflict_key(&service_target("ns-key1", "my-service"), None);
    assert_eq!(key, "/Service/ns-key1/my-service");
}

#[test]
fn test_make_target_conflict_key_with_section() {
    let key = make_target_conflict_key(&service_target("ns-key2", "my-service"), Some("port-80"));
    assert_eq!(key, "/Service/ns-key2/my-service::port-80");
}

#[test]
fn test_no_conflict_single_policy() {
    let ns = "ns-single";
    let state = make_state();

    let policy = make_policy_ref(ns, "only-policy");
    let target_key = make_target_conflict_key(&service_target(ns, "svc"), None);
    state.target_mgr().add_ref(target_key.clone(), policy);

    assert!(state.is_winner(&format!("{}/only-policy", ns), &target_key));
}

#[test]
fn test_older_policy_wins() {
    let ns = "ns-older";
    let state = make_state();
    let (_policy_a, _policy_b) = setup_two_policies(&state, ns);
    let target_key = make_target_conflict_key(&service_target(ns, "my-service"), None);

    assert!(state.is_winner(&format!("{}/policy-a", ns), &target_key));
    assert!(!state.is_winner(&format!("{}/policy-b", ns), &target_key));
}

#[test]
fn test_same_timestamp_name_tiebreak() {
    let ns = "ns-tiebreak";
    let state = make_state();

    let policy_a = make_policy_ref(ns, "alpha");
    let policy_b = make_policy_ref(ns, "beta");
    let target_key = make_target_conflict_key(&service_target(ns, "svc"), None);

    state.target_mgr().add_ref(target_key.clone(), policy_a.clone());
    state.target_mgr().add_ref(target_key.clone(), policy_b.clone());

    // Same timestamp — alphabetically earlier name wins
    state.set_timestamp(&format!("{}/alpha", ns), Some(ts("2024-01-01T00:00:00Z")));
    state.set_timestamp(&format!("{}/beta", ns), Some(ts("2024-01-01T00:00:00Z")));

    assert!(state.is_winner(&format!("{}/alpha", ns), &target_key));
    assert!(!state.is_winner(&format!("{}/beta", ns), &target_key));
}

#[test]
fn test_different_section_names_no_conflict() {
    let ns = "ns-sections";
    let state = make_state();

    let policy_a = make_policy_ref(ns, "policy-a");
    let policy_b = make_policy_ref(ns, "policy-b");

    let key_80 = make_target_conflict_key(&service_target(ns, "svc"), Some("port-80"));
    let key_443 = make_target_conflict_key(&service_target(ns, "svc"), Some("port-443"));

    state.target_mgr().add_ref(key_80.clone(), policy_a);
    state.target_mgr().add_ref(key_443.clone(), policy_b);

    // Each target has only one policy — no conflict
    assert!(state.is_winner(&format!("{}/policy-a", ns), &key_80));
    assert!(state.is_winner(&format!("{}/policy-b", ns), &key_443));
}

#[test]
fn test_get_sibling_policies() {
    let ns = "ns-siblings";
    let state = make_state();
    let (policy_a, policy_b) = setup_two_policies(&state, ns);

    let siblings_a = state.get_siblings(&policy_a);
    assert_eq!(siblings_a.len(), 1);
    assert_eq!(siblings_a[0], policy_b);

    let siblings_b = state.get_siblings(&policy_b);
    assert_eq!(siblings_b.len(), 1);
    assert_eq!(siblings_b[0], policy_a);
}

#[test]
fn test_no_siblings_single_policy() {
    let ns = "ns-lonely";
    let state = make_state();

    let policy = make_policy_ref(ns, "lonely");
    let target_key = make_target_conflict_key(&service_target(ns, "svc"), None);
    state.target_mgr().add_ref(target_key, policy.clone());

    assert!(state.get_siblings(&policy).is_empty());
}
