//! Background cleaner task for draining backends
//!
//! Periodically checks backends in draining state and removes them
//! when all connections have closed (count = 0).

use super::{backend_state, counter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{interval, Duration};

/// Cleaner task for draining backends
pub struct BackendCleaner {
    running: Arc<AtomicBool>,
}

impl BackendCleaner {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start the cleaner task
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

                // Check all draining backends
                let draining = backend_state::get_draining_backends();
                for addr in draining {
                    let count = counter::get_count(&addr);

                    if count == 0 {
                        // All connections closed, can remove
                        tracing::info!(
                            backend = %addr,
                            "Backend fully drained, removing state"
                        );
                        backend_state::remove(&addr);
                        counter::remove(&addr);
                    } else {
                        tracing::debug!(
                            backend = %addr,
                            count = count,
                            "Backend still draining"
                        );
                    }
                }
            }
        });

        tracing::info!("BackendCleaner started");
    }

    /// Stop the cleaner task
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
