//! Access logger with pluggable data senders

use crate::core::link_sys::DataSender;

/// Access logger that dispatches log entries to registered senders
pub struct AccessLogger {
    senders: Vec<Box<dyn DataSender>>,
}

impl AccessLogger {
    pub fn new() -> Self {
        Self { senders: Vec::new() }
    }

    /// Register a data sender
    pub fn register(&mut self, sender: Box<dyn DataSender>) {
        self.senders.push(sender);
    }

    /// Initialize all senders
    pub async fn init(&mut self) -> anyhow::Result<()> {
        for sender in &mut self.senders {
            sender.init().await?;
        }
        Ok(())
    }

    /// Send log to all healthy senders
    pub async fn send(&self, data: &str) {
        for sender in &self.senders {
            if sender.healthy() {
                if let Err(e) = sender.send(data).await {
                    tracing::warn!(sender = sender.name(), error = %e, "Failed to send access log");
                }
            }
        }
    }

    /// Sync send (non-blocking, spawns tasks)
    pub fn log(&self, data: String) {
        for sender in &self.senders {
            if sender.healthy() {
                let data = data.clone();
                // Fire and forget - each sender handles its own buffering
                let _ = sender.send(&data);
            }
        }
    }
}

impl Default for AccessLogger {
    fn default() -> Self {
        Self::new()
    }
}
