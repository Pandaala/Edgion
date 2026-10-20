use super::*;

fn target(name: &str) -> PolicyTargetKey {
    PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "default".to_string(),
        name: name.to_string(),
    }
}

fn config(max_seconds: u64, max_percent: u32) -> OutlierDetectionResolved {
    OutlierDetectionResolved {
        consecutive_errors: 1,
        consecutive_gateway_errors: 1,
        consecutive_local_origin_failures: 1,
        ejection_seconds: 1,
        max_ejection_seconds: max_seconds,
        max_ejection_percent: max_percent,
    }
}

#[test]
fn count_and_keys_track_service_lifecycle() {
    let store = AiCredentialOutlierStore::new();
    assert_eq!(store.count(), 0);
    assert!(store.keys().is_empty());

    let target = target("tracked");
    store.configure_backend(target.clone(), None, ["primary".to_string()]);
    assert_eq!(store.count(), 1);
    assert_eq!(store.keys(), vec![target.to_string()]);

    store.remove_credentials(&target);
    assert_eq!(store.count(), 0);
    assert!(store.keys().is_empty());
}

#[test]
fn retry_after_is_clamped_and_isolated_by_service_and_alias() {
    let store = AiCredentialOutlierStore::new();
    let first = target("first");
    let second = target("second");
    for backend in [&first, &second] {
        assert!(store.configure_backend(
            backend.clone(),
            Some(config(10, 100)),
            ["primary".to_string(), "secondary".to_string()],
        ));
    }
    let applied = store
        .apply_retry_after(&first, "primary", "999", SystemTime::UNIX_EPOCH)
        .expect("valid cooldown");
    assert_eq!(applied, Duration::from_secs(10));
    assert!(matches!(store.admit(&first, "primary"), EjectionAdmission::Rejected));
    assert!(matches!(store.admit(&first, "secondary"), EjectionAdmission::Closed));
    assert!(matches!(store.admit(&second, "primary"), EjectionAdmission::Closed));
}

#[test]
fn parsed_reset_cooldown_uses_the_retry_after_floor_and_cap() {
    let store = AiCredentialOutlierStore::new();
    let target = target("svc");
    store.configure_backend(target.clone(), Some(config(10, 100)), ["primary".to_string()]);
    let credential = store
        .current_handle(&target)
        .and_then(|backend| backend.capture("primary"))
        .expect("credential");
    assert_eq!(
        credential.apply_cooldown(Duration::from_millis(1)),
        Some(Duration::from_secs(1))
    );
    assert!(matches!(credential.admit(), EjectionAdmission::Rejected));
    assert_eq!(
        credential.apply_cooldown(Duration::from_secs(999)),
        Some(Duration::from_secs(10))
    );
    assert!(matches!(credential.admit(), EjectionAdmission::Rejected));
}

#[test]
fn concurrent_success_does_not_cancel_retry_after_cooldown() {
    let store = AiCredentialOutlierStore::new();
    let target = target("svc");
    store.configure_backend(target.clone(), Some(config(60, 100)), ["primary".to_string()]);
    let backend = store.current_handle(&target).expect("backend handle");
    let rate_limited = backend.capture("primary").expect("rate-limited request handle");
    let in_flight_success = backend.capture("primary").expect("concurrent request handle");

    assert_eq!(
        rate_limited.apply_retry_after("60", SystemTime::UNIX_EPOCH),
        Some(Duration::from_secs(60))
    );
    in_flight_success.record_success();

    assert!(matches!(rate_limited.admit(), EjectionAdmission::Rejected));
}

#[test]
fn zero_past_invalid_and_unknown_alias_do_not_install_cooldown() {
    let store = AiCredentialOutlierStore::new();
    let target = target("svc");
    store.configure_backend(target.clone(), None, ["key".to_string()]);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    assert_eq!(store.apply_retry_after(&target, "key", "0", now), None);
    assert_eq!(store.apply_retry_after(&target, "key", "invalid", now), None);
    assert_eq!(store.apply_retry_after(&target, "missing", "10", now), None);
    assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Closed));
}

#[test]
fn max_ejection_percent_is_scoped_to_credentials_of_one_service() {
    for percent in [1, 50] {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        let mut policy = config(60, percent);
        policy.ejection_seconds = 60;
        store.configure_backend(target.clone(), Some(policy), ["one".to_string(), "two".to_string()]);
        assert_eq!(
            store.record_failure(&target, "one", FailureClass::Gateway),
            FailureTransition::Opened
        );
        assert_eq!(
            store.record_failure(&target, "two", FailureClass::Gateway),
            FailureTransition::None
        );
        assert!(matches!(store.admit(&target, "one"), EjectionAdmission::Rejected));
        assert!(matches!(store.admit(&target, "two"), EjectionAdmission::Closed));
    }
}

#[test]
fn concurrent_credential_failures_respect_the_percent_cap() {
    let store = AiCredentialOutlierStore::new();
    let target = target("svc");
    let aliases = ["one", "two", "three", "four"];
    let mut policy = config(60, 50);
    policy.ejection_seconds = 60;
    store.configure_backend(target.clone(), Some(policy), aliases.map(str::to_owned));
    let backend = store.current_handle(&target).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(aliases.len()));
    let workers: Vec<_> = aliases
        .iter()
        .map(|alias| {
            let credential = backend.capture(alias).unwrap();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                credential.record_failure(FailureClass::Gateway)
            })
        })
        .collect();
    let opened = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .filter(|transition| *transition == FailureTransition::Opened)
        .count();
    assert_eq!(opened, 2);
    assert_eq!(
        aliases
            .iter()
            .filter(|alias| matches!(store.admit(&target, alias), EjectionAdmission::Rejected))
            .count(),
        2
    );
}

#[test]
fn credential_success_resets_failures_and_releases_ejection_capacity() {
    let store = AiCredentialOutlierStore::new();
    let target = target("svc");
    let mut threshold_three = config(10, 50);
    threshold_three.consecutive_gateway_errors = 3;
    store.configure_backend(
        target.clone(),
        Some(threshold_three),
        ["one".to_string(), "two".to_string()],
    );

    assert_eq!(
        store.record_failure(&target, "one", FailureClass::Gateway),
        FailureTransition::None
    );
    store.record_success(&target, "one");
    for _ in 0..2 {
        assert_eq!(
            store.record_failure(&target, "one", FailureClass::Gateway),
            FailureTransition::None
        );
    }
    assert_eq!(
        store.record_failure(&target, "one", FailureClass::Gateway),
        FailureTransition::Opened
    );
    for _ in 0..2 {
        assert_eq!(
            store.record_failure(&target, "two", FailureClass::Gateway),
            FailureTransition::None
        );
    }
    assert_eq!(
        store.record_failure(&target, "two", FailureClass::Gateway),
        FailureTransition::None,
        "the first open credential consumes the 50% ejection capacity"
    );

    store.record_success(&target, "one");
    assert_eq!(
        store.record_failure(&target, "two", FailureClass::Gateway),
        FailureTransition::Opened,
        "recovery must release ejection capacity for another alias"
    );
}

#[test]
fn equivalent_config_preserves_state_and_changed_config_resets() {
    let store = AiCredentialOutlierStore::new();
    let target = target("svc");
    store.configure_backend(target.clone(), Some(config(10, 100)), ["key".to_string()]);
    store.apply_retry_after(&target, "key", "5", SystemTime::UNIX_EPOCH);
    store.configure_backend(target.clone(), Some(config(10, 100)), ["key".to_string()]);
    assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Rejected));
    store.configure_backend(target.clone(), Some(config(20, 100)), ["key".to_string()]);
    assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Closed));
}

#[test]
fn expired_cooldown_admits_one_probe_and_drop_releases_it() {
    let store = AiCredentialOutlierStore::new();
    let target = target("svc");
    store.configure_backend(target.clone(), Some(config(10, 100)), ["key".to_string()]);
    store.state(&target, "key").open_for(Duration::ZERO);
    let EjectionAdmission::Probe(probe) = store.admit(&target, "key") else {
        panic!("probe must be admitted");
    };
    assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Rejected));
    drop(probe);
    assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Probe(_)));
}

#[test]
fn service_kind_is_part_of_identity() {
    let store = AiCredentialOutlierStore::new();
    let mut backend = target("same-name");
    backend.target_kind = PolicyTargetKind::Service;
    assert!(!store.configure_backend(backend, None, ["key".to_string()]));
}

#[test]
fn independent_policy_and_credential_sync_preserve_the_other_half() {
    let store = AiCredentialOutlierStore::new();
    let target = target("svc");
    store.set_credentials(target.clone(), ["key".to_string()]);
    store.publish_effective_policy(target.clone(), Some(config(10, 100)));
    assert_eq!(
        store.record_failure(&target, "key", FailureClass::Gateway),
        FailureTransition::Opened
    );

    // Equivalent updates from either independently-ordered conf-sync stream
    // retain live state.
    store.publish_effective_policy(target.clone(), Some(config(10, 100)));
    store.set_credentials(target.clone(), ["key".to_string()]);
    assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Rejected));
}

#[test]
fn policy_generations_are_request_stable_across_change_and_remove() {
    let store = AiCredentialOutlierStore::new();
    let target = target("svc");
    store.set_credentials(target.clone(), ["key".to_string()]);

    let first = store
        .publish_effective_policy(target.clone(), Some(config(10, 100)))
        .expect("AI target");
    let first_credential = first.capture("key").expect("first credential");
    first_credential.apply_retry_after("5", SystemTime::UNIX_EPOCH);

    let equivalent = store
        .publish_effective_policy(target.clone(), Some(config(10, 100)))
        .expect("AI target");
    assert!(Arc::ptr_eq(&first, &equivalent));

    let second = store
        .publish_effective_policy(target.clone(), Some(config(20, 100)))
        .expect("AI target");
    assert!(!Arc::ptr_eq(&first, &second));
    let second_credential = second.capture("key").expect("second credential");
    assert!(matches!(second_credential.admit(), EjectionAdmission::Closed));
    assert!(matches!(first_credential.admit(), EjectionAdmission::Rejected));

    store.set_credentials(target.clone(), std::iter::empty());
    assert!(store
        .current_handle(&target)
        .is_some_and(|handle| handle.capture("key").is_none()));
    // The request-owned handle does not consult the mutable alias index.
    second_credential.apply_retry_after("5", SystemTime::UNIX_EPOCH);
    assert!(matches!(second_credential.admit(), EjectionAdmission::Rejected));

    store.remove_credentials(&target);
    assert!(store.current_handle(&target).is_some());
    store.clear_effective_policy(&target);
    assert!(store.current_handle(&target).is_none());
    assert!(matches!(first_credential.admit(), EjectionAdmission::Rejected));
}

#[test]
fn no_policy_generation_still_supports_retry_after() {
    let store = AiCredentialOutlierStore::new();
    let target = target("svc");
    store.set_credentials(target.clone(), ["key".to_string()]);
    let default_generation = store.fallback_handle_for_test(&target).expect("default generation");
    let credential = default_generation.capture("key").expect("credential");
    assert_eq!(
        credential.apply_retry_after("999", SystemTime::UNIX_EPOCH),
        Some(DEFAULT_COOLDOWN_CAP)
    );
    assert!(matches!(credential.admit(), EjectionAdmission::Rejected));
}

#[test]
fn clearing_policy_for_an_unknown_target_creates_nothing() {
    let store = AiCredentialOutlierStore::new();
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "default".to_string(),
        name: "ghost".to_string(),
    };
    store.clear_effective_policy(&target);
    assert_eq!(store.count(), 0, "leaked: {:?}", store.keys());
    assert!(store.current_handle(&target).is_none());
}

#[test]
fn effective_policy_without_outlier_config_is_distinct_from_fallback() {
    let store = AiCredentialOutlierStore::new();
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "default".to_string(),
        name: "live".to_string(),
    };
    let effective = store
        .publish_effective_policy(target.clone(), None)
        .expect("effective generation");
    assert!(effective.capture("primary").is_none());
    let fallback = store
        .set_credentials(target.clone(), ["primary".to_string()])
        .expect("fallback generation");
    assert_eq!(store.count(), 1);
    assert!(!Arc::ptr_eq(&fallback, &effective));
    assert!(fallback.capture("primary").is_some());
    assert!(effective.capture("primary").is_some());
}

#[test]
fn fallback_alias_changes_are_copy_on_write_and_share_retained_state() {
    let store = AiCredentialOutlierStore::new();
    let target = target("copy-on-write");
    let first = store
        .set_credentials(target.clone(), ["primary".to_string()])
        .expect("first fallback");
    let equivalent = store
        .set_credentials(target.clone(), ["primary".to_string()])
        .expect("equivalent fallback");
    assert!(Arc::ptr_eq(&first, &equivalent));
    let primary = first.capture("primary").expect("primary credential");
    primary.apply_retry_after("30", SystemTime::UNIX_EPOCH);
    let effective = store
        .publish_effective_policy(target.clone(), Some(config(10, 100)))
        .expect("effective generation");

    let second = store
        .set_credentials(target.clone(), ["primary".to_string(), "secondary".to_string()])
        .expect("second fallback");
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(first.capture("secondary").is_none());
    assert!(second.capture("secondary").is_some());
    assert!(effective.capture("secondary").is_some());
    assert!(matches!(
        second.capture("primary").expect("retained primary").admit(),
        EjectionAdmission::Rejected
    ));

    store.remove_credentials(&target);
    assert!(store.fallback_handle_for_test(&target).is_none());
    assert!(store
        .effective_handle_for_test(&target)
        .is_some_and(|handle| handle.capture("primary").is_none()));
    assert!(first.capture("primary").is_some());
    assert!(second.capture("secondary").is_some());
    store.clear_effective_policy(&target);
    assert_eq!(store.count(), 0);
}
