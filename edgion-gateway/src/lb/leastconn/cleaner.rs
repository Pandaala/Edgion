//! Background cleaner task for draining backends.
//!
//! Periodically polls the draining registry and removes backends whose
//! in-flight connection count has reached zero.

use crate::lb::runtime_state;
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};
use tokio_util::sync::CancellationToken;

pub struct BackendCleaner {
    cancel: CancellationToken,
    handle: Option<JoinHandle<()>>,
}

impl BackendCleaner {
    pub fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
            handle: None,
        }
    }

    pub fn start(&mut self) {
        if self.handle.is_some() {
            tracing::warn!("BackendCleaner already running");
            return;
        }

        let cancel = self.cancel.clone();
        let handle = tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(10));

            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = cancel.cancelled() => break,
                }

                for (key, backend) in runtime_state::all_draining_backends() {
                    let count = backend.owned_conn();
                    let mut parts = key.split('\0');
                    let service_key = parts.next().unwrap_or(&key);
                    let addr_str = parts.next().unwrap_or("");
                    if count == 0 {
                        tracing::info!(
                            service_key = %service_key,
                            backend = %addr_str,
                            "Backend fully drained, removing state"
                        );
                        runtime_state::finish_draining(&key);
                    } else {
                        tracing::debug!(
                            service_key = %service_key,
                            backend = %addr_str,
                            count = count,
                            "Backend still draining"
                        );
                    }
                }
            }
        });

        self.handle = Some(handle);
        tracing::info!("BackendCleaner started");
    }

    pub async fn stop(&mut self) {
        self.cancel.cancel();
        if let Some(h) = self.handle.take() {
            let _ = h.await;
        }
        // Reset token so a subsequent start() gets a fresh, uncancelled token.
        self.cancel = CancellationToken::new();
        tracing::info!("BackendCleaner stopped");
    }
}

impl Default for BackendCleaner {
    fn default() -> Self {
        Self::new()
    }
}
