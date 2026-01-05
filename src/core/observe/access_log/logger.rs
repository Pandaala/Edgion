//! Access logger with pluggable data senders

use crate::core::link_sys::DataSender;

/// Access logger that dispatches log entries to registered senders
pub struct AccessLogger {
    senders: Vec<Box<dyn DataSender<String>>>,
}

impl AccessLogger {
    pub fn new() -> Self {
        Self { senders: Vec::new() }
    }

    /// Register a data sender
    pub fn register(&mut self, sender: Box<dyn DataSender<String>>) {
        self.senders.push(sender);
    }

    /// Initialize all senders
    pub async fn init(&mut self) -> anyhow::Result<()> {
        for sender in &mut self.senders {
            sender.init().await?;
        }
        Ok(())
    }

    /// Send log to first healthy sender
    pub async fn send(&self, data: String) {
        for sender in &self.senders {
            if sender.healthy() {
                let _ = sender.send(data).await;
                return;
            }
        }
    }
}

impl Default for AccessLogger {
    fn default() -> Self {
        Self::new()
    }
}
