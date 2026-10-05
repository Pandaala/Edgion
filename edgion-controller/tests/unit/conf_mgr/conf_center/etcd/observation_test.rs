use super::*;
#[test]
fn retirement_is_idempotent_and_late_maintenance_is_ignored() {
    let observer = ControllerEtcdObserver::new();
    observer.deactivate();
    observer.deactivate();
    observer.maintenance(MaintenanceEvent {
        kind: MaintenanceKind::Probe,
        outcome: MaintenanceOutcome::Failed,
        elapsed: Duration::ZERO,
        health_changed: Some(false),
    });
    let state = observer.state.lock();
    assert!(!state.active);
    assert!(state.last.is_none());
}
