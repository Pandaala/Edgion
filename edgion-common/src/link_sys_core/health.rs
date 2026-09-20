//! Health primitives shared by all LinkSys providers.
//!
//! These are the canonical types — providers must use this module rather than
//! re-defining health DTOs or publication-aware transition state locally.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;

/// Binary-owned callback invoked for one published health transition.
///
/// The shared LinkSys core never knows which observability backend consumes the
/// event. Gateway clients install a metrics callback; Controller-owned clients
/// leave it absent.
pub type HealthTransitionReporter = Arc<dyn Fn(bool) + Send + Sync + 'static>;

struct TransitionState {
    published: bool,
    reporter: Option<HealthTransitionReporter>,
}

/// Lock-free health reads plus publication-aware transition reporting.
///
/// State writers and publication changes share one short mutex. This makes
/// `deactivate` a barrier: after it returns, no late transition can invoke the
/// reporter. The callback runs inside that critical section and therefore must
/// stay short and must not call back into this health cell.
pub struct PublishedHealth {
    value: AtomicBool,
    transition: Mutex<TransitionState>,
}

impl PublishedHealth {
    pub fn new(initial: bool) -> Self {
        Self {
            value: AtomicBool::new(initial),
            transition: Mutex::new(TransitionState {
                published: false,
                reporter: None,
            }),
        }
    }

    pub fn with_reporter(initial: bool, reporter: HealthTransitionReporter) -> Self {
        Self {
            value: AtomicBool::new(initial),
            transition: Mutex::new(TransitionState {
                published: false,
                reporter: Some(reporter),
            }),
        }
    }

    #[inline]
    pub fn load(&self) -> bool {
        self.value.load(Ordering::Relaxed)
    }

    /// Apply one observed result. Returns true only when the value changed.
    ///
    /// `upgrade_only` lets an idle recovery probe restore health while leaving
    /// degradation ownership to an operation path.
    pub fn update(&self, next: bool, upgrade_only: bool) -> bool {
        let state = self.transition.lock();
        let previous = self.value.load(Ordering::Relaxed);
        if previous == next || (upgrade_only && !next) {
            return false;
        }
        self.value.store(next, Ordering::Relaxed);
        if state.published {
            if let Some(reporter) = &state.reporter {
                reporter(next);
            }
        }
        true
    }

    /// Disable reporting for direct retirement/shutdown paths.
    ///
    /// Persistent runners normally use [`Self::transition_publication`] so the
    /// health ownership handover and current-client swap share one transaction.
    pub fn deactivate(&self) {
        self.transition.lock().published = false;
    }

    /// Run the current-client mutation while transferring reporting ownership.
    ///
    /// The old and new transition locks stay held across `publish`, preventing a
    /// health update from landing between reporter deactivation, the ArcSwap
    /// mutation, and activation of the new generation. Activation establishes
    /// the new generation's current value as a baseline and emits no event.
    pub fn transition_publication<R>(previous: Option<&Self>, next: Option<&Self>, publish: impl FnOnce() -> R) -> R {
        match (previous, next) {
            (None, None) => publish(),
            (Some(previous), None) => {
                let mut previous_state = previous.transition.lock();
                previous_state.published = false;
                publish()
            }
            (None, Some(next)) => {
                let mut next_state = next.transition.lock();
                let result = publish();
                next_state.published = true;
                result
            }
            (Some(previous), Some(next)) if std::ptr::eq(previous, next) => {
                let mut state = previous.transition.lock();
                let result = publish();
                state.published = true;
                result
            }
            (Some(previous), Some(next)) => {
                let mut previous_state = previous.transition.lock();
                let mut next_state = next.transition.lock();
                previous_state.published = false;
                let result = publish();
                next_state.published = true;
                result
            }
        }
    }
}

/// Health status of a LinkSys client, exposed via admin API.
#[derive(Debug, Clone, Serialize)]
pub struct LinkSysHealth {
    pub name: String,
    pub system_type: String,
    pub connected: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
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
}
