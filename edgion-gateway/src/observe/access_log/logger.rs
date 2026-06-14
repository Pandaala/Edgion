//! Access logger with pluggable data senders

use crate::link_sys::{DataSender, LogMeta};
use crate::observe::metrics::global_metrics;

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
    ///
    /// When all senders report unhealthy (e.g., writer task died, sender init
    /// failed), the entry is dropped and `edgion_access_log_dropped_total` is
    /// incremented so operators have a signal for sustained log loss.
    ///
    /// A logger with zero senders is the "access logging disabled" no-op
    /// installed by `init_access_logger` and silently drops without touching
    /// the dropped metric — disabled is an operator choice, not a fault.
    pub async fn send(&self, data: String) {
        if self.senders.is_empty() {
            return;
        }
        for sender in &self.senders {
            if sender.healthy() {
                let _ = sender.send(data).await;
                return;
            }
        }
        global_metrics().access_log_dropped();
    }

    /// Send log with pre-serialization metadata to the first healthy sender.
    ///
    /// Falls back to `send` for senders that do not override `send_with_meta`
    /// (ES, Kafka, LocalFile). OTLP senders use `meta` to build typed LogRecords.
    pub async fn send_with_meta(&self, data: String, meta: LogMeta) {
        if self.senders.is_empty() {
            return;
        }
        for sender in &self.senders {
            if sender.healthy() {
                let _ = sender.send_with_meta(data, &meta).await;
                return;
            }
        }
        global_metrics().access_log_dropped();
    }
}

impl Default for AccessLogger {
    fn default() -> Self {
        Self::new()
    }
}
