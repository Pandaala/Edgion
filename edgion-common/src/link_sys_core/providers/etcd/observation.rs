//! Typed observations consumed only by the owning binary.
use std::{sync::Arc, time::Duration};

pub use etcd_client::observation::{Operation, Outcome};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceKind {
    Init,
    Probe,
    Discovery,
    Reconnect,
    Shutdown,
}
impl MaintenanceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Probe => "probe",
            Self::Discovery => "discovery",
            Self::Reconnect => "reconnect",
            Self::Shutdown => "shutdown",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceOutcome {
    Ok,
    Failed,
    Canceled,
}
impl MaintenanceOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct MaintenanceEvent {
    pub kind: MaintenanceKind,
    pub outcome: MaintenanceOutcome,
    pub elapsed: Duration,
    pub health_changed: Option<bool>,
}

/// Callbacks must return promptly, never panic, and never reenter the client.
/// `deactivate` fences all subsequent outputs for this generation.
pub trait EtcdObservation: Send + Sync {
    fn completed(&self, operation: Operation, outcome: Outcome);
    fn maintenance(&self, event: MaintenanceEvent);
    fn deactivate(&self);
}

pub(super) struct CompletionBridge(pub Arc<dyn EtcdObservation>);
impl etcd_client::observation::Observer for CompletionBridge {
    fn completed(&self, operation: Operation, outcome: Outcome) {
        self.0.completed(operation, outcome);
    }
}
