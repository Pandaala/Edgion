//! Gateway-owned Etcd maintenance logs and optional RPC counters.
use crate::{
    link_sys::runtime::health_probe::flip_log_allowed,
    observe::metrics::{registry::EtcdOperationMetrics, GatewayMetrics},
};
use edgion_common::link_sys_core::providers::etcd::observation::{
    EtcdObservation, MaintenanceEvent, MaintenanceKind, MaintenanceOutcome, Operation, Outcome,
};
use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;
use parking_lot::Mutex;
use std::{
    sync::{atomic::AtomicU64, Arc},
    time::Duration,
};

pub(super) struct GatewayEtcdObserver {
    link: String,
    active: Mutex<bool>,
    metrics: Option<Arc<EtcdOperationMetrics>>,
    logging_enabled: bool,
    level: tracing::Level,
    slow: Option<Duration>,
    last_optional: AtomicU64,
    last_health: AtomicU64,
}
impl GatewayEtcdObserver {
    pub(super) fn new(link: &str, config: &EtcdClientConfig) -> anyhow::Result<Arc<Self>> {
        let (namespace, name) = link
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("invalid LinkSys reference"))?;
        edgion_resources::output::LinkSysRef {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        }
        .validate()
        .map_err(anyhow::Error::msg)?;
        let observability = config.observability.as_ref();
        let metrics = observability
            .and_then(|v| v.metrics.as_ref())
            .is_some_and(|v| v.enabled);
        let logging = observability.and_then(|v| v.logging.as_ref());
        Ok(Arc::new(Self {
            link: link.to_owned(),
            active: Mutex::new(true),
            metrics: metrics.then(|| GatewayMetrics::etcd_operations(link)),
            logging_enabled: logging.is_some_and(|v| v.enabled),
            level: match logging.and_then(|v| v.level.as_deref()).unwrap_or("info") {
                "error" => tracing::Level::ERROR,
                "warn" => tracing::Level::WARN,
                "debug" => tracing::Level::DEBUG,
                "trace" => tracing::Level::TRACE,
                _ => tracing::Level::INFO,
            },
            slow: logging.and_then(|v| v.slow_log_threshold).map(Duration::from_millis),
            last_optional: AtomicU64::new(0),
            last_health: AtomicU64::new(0),
        }))
    }
    pub(super) fn observes_completions(&self) -> bool {
        self.metrics.is_some()
    }
    fn optional_event(&self, event: MaintenanceEvent) -> bool {
        self.logging_enabled
            && (event.outcome != MaintenanceOutcome::Ok
                || matches!(
                    event.kind,
                    MaintenanceKind::Init | MaintenanceKind::Reconnect | MaintenanceKind::Shutdown
                )
                || self.slow.is_some_and(|threshold| event.elapsed >= threshold))
    }
    fn log_maintenance(&self, event: MaintenanceEvent, retirement_requested: bool) {
        // tracing-allow: bounded background maintenance and generation retirement only; never RPC completions.
        macro_rules! emit { ($level:expr) => { tracing::event!($level, component = "link_sys_etcd", link = %self.link,
            operation = event.kind.as_str(), outcome = event.outcome.as_str(),
            elapsed_ms = event.elapsed.as_millis() as u64, retirement_requested,
            "Etcd maintenance") }; }
        match self.level {
            tracing::Level::ERROR => emit!(tracing::Level::ERROR),
            tracing::Level::WARN => emit!(tracing::Level::WARN),
            tracing::Level::DEBUG => emit!(tracing::Level::DEBUG),
            tracing::Level::TRACE => emit!(tracing::Level::TRACE),
            _ => emit!(tracing::Level::INFO),
        }
    }
}
impl EtcdObservation for GatewayEtcdObserver {
    fn completed(&self, operation: Operation, outcome: Outcome) {
        let active = self.active.lock();
        if *active {
            if let Some(metrics) = &self.metrics {
                metrics.record(operation, outcome);
            }
        }
    }
    fn maintenance(&self, event: MaintenanceEvent) {
        let active = self.active.lock();
        if !*active {
            return;
        }
        if let Some(healthy) = event.health_changed {
            if flip_log_allowed(&self.last_health) {
                // tracing-allow: bounded background probe health transitions, independent of optional logs.
                if healthy {
                    tracing::info!(link = %self.link, system = "etcd", "link-sys recovered");
                } else {
                    tracing::warn!(link = %self.link, system = "etcd", "link-sys degraded");
                }
            }
        }
        if self.optional_event(event) && flip_log_allowed(&self.last_optional) {
            self.log_maintenance(event, false);
        }
    }
    fn deactivate(&self) {
        let mut active = self.active.lock();
        if !*active {
            return;
        }
        if self.logging_enabled {
            self.log_maintenance(
                MaintenanceEvent {
                    kind: MaintenanceKind::Shutdown,
                    outcome: MaintenanceOutcome::Canceled,
                    elapsed: Duration::ZERO,
                    health_changed: None,
                },
                true,
            );
        }
        if let Some(metrics) = &self.metrics {
            metrics.deactivate();
        }
        *active = false;
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/etcd/observation_test.rs"]
mod tests;
