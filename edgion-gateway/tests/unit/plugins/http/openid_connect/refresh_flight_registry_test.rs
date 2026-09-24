use std::sync::mpsc;
use std::time::Duration;

use super::RefreshFlightRegistry;

#[test]
fn same_key_reuses_flight_and_last_owner_cleans_entry() {
    let registry = RefreshFlightRegistry::new(2).unwrap();
    let first = registry.get_or_create("digest-a");
    let second = registry.get_or_create("digest-a");

    assert!(std::sync::Arc::ptr_eq(&first, &second));
    assert_eq!(registry.registered_len(), 1);
    drop(first);
    assert_eq!(registry.registered_len(), 1);
    drop(second);
    assert_eq!(registry.registered_len(), 0);
}

#[test]
fn different_keys_receive_independent_flights() {
    let registry = RefreshFlightRegistry::new(2).unwrap();
    let first = registry.get_or_create("digest-a");
    let second = registry.get_or_create("digest-b");

    assert!(!std::sync::Arc::ptr_eq(&first, &second));
    assert_eq!(registry.registered_len(), 2);
}

#[test]
fn saturation_uses_unregistered_flights_without_growing_map() {
    let registry = RefreshFlightRegistry::new(1).unwrap();
    let registered = registry.get_or_create("digest-a");
    let first_ephemeral = registry.get_or_create("digest-b");
    let second_ephemeral = registry.get_or_create("digest-b");

    assert!(registered.is_registered());
    assert!(!first_ephemeral.is_registered());
    assert!(!second_ephemeral.is_registered());
    assert!(!std::sync::Arc::ptr_eq(&first_ephemeral, &second_ephemeral));
    assert_eq!(registry.registered_len(), 1);
}

#[test]
fn final_drop_does_not_deadlock_on_registry_mutex() {
    let registry = RefreshFlightRegistry::new(1).unwrap();
    let flight = registry.get_or_create("digest-a");
    let (sent, received) = mpsc::channel();

    std::thread::spawn(move || {
        drop(flight);
        sent.send(()).unwrap();
    });

    received
        .recv_timeout(Duration::from_secs(1))
        .expect("last-owner cleanup must not deadlock");
    assert_eq!(registry.registered_len(), 0);
}

#[test]
fn zero_limit_is_rejected() {
    assert!(RefreshFlightRegistry::new(0).is_err());
}

#[test]
fn absolute_limit_is_validated_before_registration() {
    assert!(RefreshFlightRegistry::new(super::MAX_REFRESH_SINGLEFLIGHT_REGISTERED_KEYS).is_ok());
    assert!(RefreshFlightRegistry::new(super::MAX_REFRESH_SINGLEFLIGHT_REGISTERED_KEYS + 1).is_err());
}
