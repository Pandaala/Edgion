use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use super::*;

fn observed_health(initial: bool) -> (Arc<PublishedHealth>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let recoveries = Arc::new(AtomicUsize::new(0));
    let degradations = Arc::new(AtomicUsize::new(0));
    let recovered = Arc::clone(&recoveries);
    let degraded = Arc::clone(&degradations);
    let reporter: HealthTransitionReporter = Arc::new(move |healthy| {
        if healthy {
            recovered.fetch_add(1, Ordering::Relaxed);
        } else {
            degraded.fetch_add(1, Ordering::Relaxed);
        }
    });
    (
        Arc::new(PublishedHealth::with_reporter(initial, reporter)),
        recoveries,
        degradations,
    )
}

#[test]
fn unpublished_updates_set_the_baseline_without_reporting() {
    let (health, recoveries, degradations) = observed_health(false);
    assert!(health.update(true, false));
    assert!(health.load());
    assert_eq!(recoveries.load(Ordering::Relaxed), 0);
    assert_eq!(degradations.load(Ordering::Relaxed), 0);

    PublishedHealth::transition_publication(None, Some(&health), || {});
    assert_eq!(recoveries.load(Ordering::Relaxed), 0);
    assert!(!health.update(true, false));
    assert_eq!(recoveries.load(Ordering::Relaxed), 0);
}

#[test]
fn published_transitions_are_deduplicated_by_direction() {
    let (health, recoveries, degradations) = observed_health(true);
    PublishedHealth::transition_publication(None, Some(&health), || {});

    assert!(health.update(false, false));
    assert!(!health.update(false, false));
    assert!(health.update(true, false));
    assert!(!health.update(true, false));
    assert_eq!(degradations.load(Ordering::Relaxed), 1);
    assert_eq!(recoveries.load(Ordering::Relaxed), 1);
}

#[test]
fn publication_handover_disables_old_and_baselines_new() {
    let (old, old_recoveries, old_degradations) = observed_health(true);
    let (new, new_recoveries, new_degradations) = observed_health(false);
    PublishedHealth::transition_publication(None, Some(&old), || {});

    PublishedHealth::transition_publication(Some(&old), Some(&new), || {});
    assert!(old.update(false, false));
    assert!(new.update(true, false));
    assert_eq!(old_recoveries.load(Ordering::Relaxed), 0);
    assert_eq!(old_degradations.load(Ordering::Relaxed), 0);
    assert_eq!(new_degradations.load(Ordering::Relaxed), 0);
    assert_eq!(new_recoveries.load(Ordering::Relaxed), 1);

    PublishedHealth::transition_publication(Some(&new), None, || {});
    assert!(new.update(false, false));
    assert_eq!(new_degradations.load(Ordering::Relaxed), 0);
}

#[test]
fn concurrent_writers_emit_one_transition() {
    let (health, recoveries, _degradations) = observed_health(false);
    PublishedHealth::transition_publication(None, Some(&health), || {});
    let barrier = Arc::new(Barrier::new(17));
    let mut writers = Vec::new();
    for _ in 0..16 {
        let health = Arc::clone(&health);
        let barrier = Arc::clone(&barrier);
        writers.push(std::thread::spawn(move || {
            barrier.wait();
            health.update(true, false)
        }));
    }
    barrier.wait();
    let winners = writers
        .into_iter()
        .map(|writer| writer.join().expect("health writer"))
        .filter(|changed| *changed)
        .count();
    assert_eq!(winners, 1);
    assert_eq!(recoveries.load(Ordering::Relaxed), 1);
}

#[test]
fn controller_style_health_without_reporter_only_tracks_level() {
    let health = PublishedHealth::new(false);
    PublishedHealth::transition_publication(None, Some(&health), || {});
    assert!(health.update(true, false));
    assert!(health.load());
    health.deactivate();
    assert!(health.update(false, false));
    assert!(!health.load());
}
