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

    /// Send log to all healthy senders (zero-copy for single sender)
    pub async fn send(&self, data: String) {
        let healthy_senders: Vec<_> = self.senders.iter()
            .filter(|s| s.healthy())
            .collect();
        
        if healthy_senders.is_empty() {
            return;
        }
        
        // For all but last, clone and send
        for sender in &healthy_senders[..healthy_senders.len() - 1] {
            if let Err(e) = sender.send(data.clone()).await {
                tracing::warn!(sender = sender.name(), error = %e, "Failed to send access log");
            }
        }
        
        // Last sender gets ownership (zero-copy)
        if let Some(last) = healthy_senders.last() {
            if let Err(e) = last.send(data).await {
                tracing::warn!(sender = last.name(), error = %e, "Failed to send access log");
            }
        }
    }
}

impl Default for AccessLogger {
    fn default() -> Self {
        Self::new()
    }
}
