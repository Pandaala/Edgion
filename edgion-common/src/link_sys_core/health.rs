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
#[path = "../../tests/unit/link_sys_core/health_test.rs"]
mod tests;
