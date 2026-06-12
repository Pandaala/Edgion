//! Graceful shutdown support for sync runtime
//!
//! Provides signal handling (SIGTERM/SIGINT) and cancellation token propagation
//! to enable graceful shutdown of all resource controllers.

use std::sync::Arc;
use tokio::signal;
use tokio::sync::watch;

/// Shutdown signal receiver
///
/// Clone this and pass to each ResourceController to receive shutdown notifications.
#[derive(Clone)]
pub struct ShutdownSignal {
    receiver: watch::Receiver<bool>,
}

impl ShutdownSignal {
    /// Check if shutdown has been requested
    pub fn is_shutdown(&self) -> bool {
        *self.receiver.borrow()
    }

    /// Wait until shutdown is requested
    pub async fn wait(&mut self) {
        // If already shutdown, return immediately
        if self.is_shutdown() {
            return;
        }
        // Wait for change
        let _ = self.receiver.changed().await;
    }
}

/// Shutdown controller that manages the shutdown signal
pub struct ShutdownController {
    sender: watch::Sender<bool>,
}

impl ShutdownController {
    /// Create a new shutdown controller and signal pair
    pub fn new() -> (Self, ShutdownSignal) {
        let (sender, receiver) = watch::channel(false);
        (Self { sender }, ShutdownSignal { receiver })
    }

    /// Trigger shutdown
    pub fn shutdown(&self) {
        let _ = self.sender.send(true);
    }

    /// Check if shutdown has been triggered
    pub fn is_shutdown(&self) -> bool {
        *self.sender.borrow()
    }
}

/// Shutdown handle that can be shared across threads
#[derive(Clone)]
pub struct ShutdownHandle {
    inner: Arc<ShutdownController>,
    signal: ShutdownSignal,
}

impl ShutdownHandle {
    /// Create a new shutdown handle
    pub fn new() -> Self {
        let (controller, signal) = ShutdownController::new();
        Self {
            inner: Arc::new(controller),
            signal,
        }
    }

    /// Get a signal receiver for this handle
    pub fn signal(&self) -> ShutdownSignal {
        self.signal.clone()
    }

    /// Trigger shutdown
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    /// Check if shutdown has been triggered
    pub fn is_shutdown(&self) -> bool {
        self.inner.is_shutdown()
    }

    /// Wait for OS signals (SIGTERM/SIGINT) and trigger shutdown
    ///
    /// This should be spawned as a background task.
    pub async fn wait_for_signals(self) {
        let ctrl_c = async {
            match signal::ctrl_c().await {
                Ok(()) => Some("SIGINT"),
                Err(e) => {
                    tracing::warn!(
                        component = "sync_runtime",
                        error = %e,
                        "Failed to install CTRL+C handler, signal handling may not work"
                    );
                    None
                }
            }
        };

        #[cfg(unix)]
        let terminate = async {
            match signal::unix::signal(signal::unix::SignalKind::terminate()) {
                Ok(mut sig) => {
                    sig.recv().await;
                    Some("SIGTERM")
                }
                Err(e) => {
                    tracing::warn!(
                        component = "sync_runtime",
                        error = %e,
                        "Failed to install SIGTERM handler, signal handling may not work"
                    );
                    // Wait forever since we can't handle SIGTERM
                    std::future::pending::<Option<&str>>().await
                }
            }
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<Option<&str>>();

        let signal_name = tokio::select! {
            result = ctrl_c => result,
            result = terminate => result,
        };

        if let Some(sig) = signal_name {
            tracing::info!(
                component = "sync_runtime",
                signal = sig,
                "Received {}, initiating graceful shutdown",
                sig
            );
        }

        self.shutdown();
    }
}

impl Default for ShutdownHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shutdown_signal() {
        let handle = ShutdownHandle::new();
        let mut signal = handle.signal();

        assert!(!signal.is_shutdown());
        assert!(!handle.is_shutdown());

        handle.shutdown();

        assert!(signal.is_shutdown());
        assert!(handle.is_shutdown());

        // wait should return immediately
        signal.wait().await;
    }
}
