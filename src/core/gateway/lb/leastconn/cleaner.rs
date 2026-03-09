//! Background cleaner task for draining backends
//!
//! Periodically iterates all service-scoped runtime state and removes
//! backends whose connection count has reached zero while in draining state.

use crate::core::gateway::lb::runtime_state;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{interval, Duration};

pub struct BackendCleaner {
    running: Arc<AtomicBool>,
}

impl BackendCleaner {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self) {
        if self.running.swap(true, Ordering::SeqCst) {
            tracing::warn!("BackendCleaner already running");
            return;
        }

        let running = self.running.clone();

        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(10));

            while running.load(Ordering::SeqCst) {
                tick.tick().await;

                for service_key in runtime_state::all_service_keys() {
                    let draining = runtime_state::get_draining_backends(&service_key);
                    for addr in draining {
                        let count = runtime_state::get_count(&service_key, &addr);

                        if count == 0 {
                            tracing::info!(
                                service_key = %service_key,
                                backend = %addr,
                                "Backend fully drained, removing state"
                            );
                            runtime_state::remove_backend(&service_key, &addr);
                        } else {
                            tracing::debug!(
                                service_key = %service_key,
                                backend = %addr,
                                count = count,
                                "Backend still draining"
                            );
                        }
                    }
                }
            }
        });

        tracing::info!("BackendCleaner started");
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!("BackendCleaner stopped");
    }
}

impl Default for BackendCleaner {
    fn default() -> Self {
        Self::new()
    }
}
