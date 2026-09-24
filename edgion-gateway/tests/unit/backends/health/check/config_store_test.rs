use super::*;
use edgion_resources::resources::health_check::{ActiveHealthCheckConfig, HealthCheckType};

fn test_config(tp: HealthCheckType) -> ActiveHealthCheckConfig {
    ActiveHealthCheckConfig {
        r#type: tp,
        ..Default::default()
    }
}

#[test]
fn test_resolve_none() {
    let store = HealthCheckConfigStore::new();
    assert!(store.resolve("default/svc").is_none());
}

#[test]
fn test_set_then_resolve_returns_config() {
    let store = HealthCheckConfigStore::new();
    store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));

    let resolved = store.resolve("default/svc").expect("resolved config");
    assert_eq!(resolved.r#type, HealthCheckType::Http);
}

#[test]
fn test_set_none_removes() {
    let store = HealthCheckConfigStore::new();
    store.set_service_config("default/svc", Some(test_config(HealthCheckType::Tcp)));
    assert!(store.resolve("default/svc").is_some());

    store.set_service_config("default/svc", None);
    assert!(store.resolve("default/svc").is_none());
}

#[test]
fn test_remove_service_is_set_none() {
    let store = HealthCheckConfigStore::new();
    store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));
    store.remove_service("default/svc");
    assert!(store.resolve("default/svc").is_none());
}

#[test]
fn test_overwrite_replaces_config() {
    let store = HealthCheckConfigStore::new();
    store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));
    store.set_service_config("default/svc", Some(test_config(HealthCheckType::Tcp)));

    let resolved = store.resolve("default/svc").expect("resolved config");
    assert_eq!(resolved.r#type, HealthCheckType::Tcp);
}

#[test]
fn identical_config_reuses_policy_identity_but_change_replaces_it() {
    let store = HealthCheckConfigStore::new();
    let config = test_config(HealthCheckType::Http);
    store.set_service_config("default/svc", Some(config.clone()));
    let first = store.resolve_policy("default/svc").unwrap();
    store.set_service_config("default/svc", Some(config));
    let same = store.resolve_policy("default/svc").unwrap();
    assert!(Arc::ptr_eq(&first.identity, &same.identity));

    store.set_service_config("default/svc", Some(test_config(HealthCheckType::Tcp)));
    let changed = store.resolve_policy("default/svc").unwrap();
    assert!(!Arc::ptr_eq(&first.identity, &changed.identity));
    assert!(changed.identity.generation > first.identity.generation);
}

#[test]
fn test_keys_and_count_track_entries() {
    let store = HealthCheckConfigStore::new();
    store.set_service_config("default/a", Some(test_config(HealthCheckType::Http)));
    store.set_service_config("default/b", Some(test_config(HealthCheckType::Tcp)));
    assert_eq!(store.count(), 2);
    let mut keys = store.keys();
    keys.sort();
    assert_eq!(keys, vec!["default/a".to_string(), "default/b".to_string()]);
}

#[test]
fn publication_revokes_an_unpolled_encrypted_probe() {
    use futures::task::noop_waker;
    use std::future::Future;
    use std::task::{Context, Poll};

    let store = HealthCheckConfigStore::new();
    store.set_service_config("default/svc", Some(test_config(HealthCheckType::Https)));
    let identity = store.resolve_policy("default/svc").unwrap().identity.clone();
    let probe = async { panic!("an old queued probe must never start") };
    let guarded = store.with_current_generation::<_, ()>("default/svc", &identity, probe);
    let mut guarded = std::pin::pin!(guarded);
    store.set_service_config("default/svc", Some(test_config(HealthCheckType::Grpcs)));
    let waker = noop_waker();
    assert!(matches!(
        guarded.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Ready(Err(super::super::probes::ProbeError::Superseded))
    ));
}

#[test]
fn publication_waits_for_poll_and_revokes_before_repoll() {
    use futures::task::noop_waker;
    use std::future::Future;
    use std::sync::atomic::AtomicUsize;
    use std::task::{Context, Poll};

    let store = Arc::new(HealthCheckConfigStore::new());
    store.set_service_config("default/svc", Some(test_config(HealthCheckType::Https)));
    let identity = store.resolve_policy("default/svc").unwrap().identity.clone();
    let polls = AtomicUsize::new(0);
    let (poll_entered, publisher_start) = std::sync::mpsc::sync_channel(0);
    let (gate_checked, checked) = std::sync::mpsc::sync_channel(0);
    let writer_store = Arc::clone(&store);
    let writer = std::thread::spawn(move || {
        publisher_start.recv().unwrap();
        assert!(writer_store.publication.try_write().is_none());
        gate_checked.send(()).unwrap();
        writer_store.set_service_config("default/svc", Some(test_config(HealthCheckType::Grpcs)));
    });
    let probe = futures::future::poll_fn(|_| {
        polls.fetch_add(1, Ordering::Relaxed);
        poll_entered.send(()).unwrap();
        checked.recv().unwrap();
        Poll::<Result<(), super::super::probes::ProbeError>>::Pending
    });
    let guarded = store.with_current_generation("default/svc", &identity, probe);
    let mut guarded = std::pin::pin!(guarded);
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    assert!(guarded.as_mut().poll(&mut context).is_pending());
    writer.join().unwrap();
    assert!(matches!(
        guarded.as_mut().poll(&mut context),
        Poll::Ready(Err(super::super::probes::ProbeError::Superseded))
    ));
    assert_eq!(polls.load(Ordering::Relaxed), 1);
}

#[test]
fn resolved_secret_rotation_replaces_health_generation() {
    let store = HealthCheckConfigStore::new();
    let mut config = test_config(HealthCheckType::Https);
    let secret = |bytes: &[u8]| k8s_openapi::api::core::v1::Secret {
        data: Some(std::collections::BTreeMap::from([(
            "ca.crt".to_owned(),
            k8s_openapi::ByteString(bytes.to_vec()),
        )])),
        ..Default::default()
    };
    config.resolved_ca_certificates = Some(vec![secret(b"first")]);
    store.set_service_config("default/svc", Some(config.clone()));
    let first = store.resolve_policy("default/svc").unwrap();
    config.resolved_ca_certificates = Some(vec![secret(b"second")]);
    store.set_service_config("default/svc", Some(config.clone()));
    let second = store.resolve_policy("default/svc").unwrap();
    assert!(!Arc::ptr_eq(&first.identity, &second.identity));
    config.resolved_client_certificate = Some(secret(b"client"));
    store.set_service_config("default/svc", Some(config));
    let third = store.resolve_policy("default/svc").unwrap();
    assert!(!Arc::ptr_eq(&second.identity, &third.identity));
}
