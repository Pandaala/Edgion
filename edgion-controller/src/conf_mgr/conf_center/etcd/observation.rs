//! Controller-owned, bounded Etcd maintenance diagnostics.
use edgion_common::link_sys_core::providers::etcd::observation::{
    EtcdObservation, MaintenanceEvent, MaintenanceKind, MaintenanceOutcome, Operation, Outcome,
};
use parking_lot::Mutex;
use std::time::{Duration, Instant};

pub(super) struct ControllerEtcdObserver {
    state: Mutex<State>,
}
struct State {
    active: bool,
    last: Option<Instant>,
}
impl ControllerEtcdObserver {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                active: true,
                last: None,
            }),
        }
    }
}
impl EtcdObservation for ControllerEtcdObserver {
    fn completed(&self, _: Operation, _: Outcome) {}
    fn maintenance(&self, event: MaintenanceEvent) {
        let mut state = self.state.lock();
        if !state.active {
            return;
        }
        let report = event.outcome != MaintenanceOutcome::Ok
            || event.health_changed.is_some()
            || matches!(
                event.kind,
                MaintenanceKind::Init | MaintenanceKind::Reconnect | MaintenanceKind::Shutdown
            );
        if !report || state.last.is_some_and(|last| last.elapsed() < Duration::from_secs(5)) {
            return;
        }
        state.last = Some(Instant::now());
        if event.outcome == MaintenanceOutcome::Failed || event.health_changed == Some(false) {
            tracing::warn!(
                component = "etcd_center",
                operation = event.kind.as_str(),
                outcome = event.outcome.as_str(),
                elapsed_ms = event.elapsed.as_millis() as u64,
                "Etcd maintenance failed"
            );
        } else {
            tracing::info!(
                component = "etcd_center",
                operation = event.kind.as_str(),
                outcome = event.outcome.as_str(),
                elapsed_ms = event.elapsed.as_millis() as u64,
                "Etcd maintenance"
            );
        }
    }
    fn deactivate(&self) {
        let mut state = self.state.lock();
        if state.active {
            tracing::info!(component = "etcd_center", "Etcd retirement requested");
            state.active = false;
        }
    }
}
#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/etcd/observation_test.rs"]
mod tests;
