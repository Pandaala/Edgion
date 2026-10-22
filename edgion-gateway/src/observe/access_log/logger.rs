//! Access logger with pluggable data senders

use crate::link_sys::{DataSender, LogMeta, LogType};

/// Access logger that dispatches log entries to registered senders
pub struct AccessLogger {
    senders: Vec<Box<dyn DataSender<String>>>,
    log_type: LogType,
}

impl AccessLogger {
    pub fn new() -> Self {
        Self::new_with_log_type(LogType::Access)
    }

    pub(crate) fn new_with_log_type(log_type: LogType) -> Self {
        Self {
            senders: Vec::new(),
            log_type,
        }
    }

    /// Register a data sender
    pub fn register(&mut self, sender: Box<dyn DataSender<String>>) {
        self.senders.push(sender);
    }

    /// Number of registered senders (primary + optional fallback).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn sender_count(&self) -> usize {
        self.senders.len()
    }

    /// Initialize all senders
    pub async fn init(&mut self) -> anyhow::Result<()> {
        for sender in &mut self.senders {
            sender.init().await?;
        }
        Ok(())
    }

    /// Send log to first healthy sender.
    ///
    /// Routes to the first healthy sender only (Tier-1). If that sender's
    /// `send` fails (e.g. ES bulk channel momentarily full), the entry is
    /// dropped and the configured log type's dropped counter is incremented —
    /// the logger does NOT fall through to the next sender on a send error
    /// (that would be tier-2 send-fallback, intentionally out of scope).
    ///
    /// When all senders report unhealthy (e.g., writer task died, sender init
    /// failed), the entry is dropped and the configured log type's dropped
    /// counter is incremented so operators have a signal for sustained loss.
    ///
    /// A logger with zero senders is a disabled no-op and silently drops
    /// without touching the metric — disabled is an operator choice, not a
    /// fault.
    pub async fn send(&self, data: String) {
        if self.senders.is_empty() {
            return;
        }
        for sender in &self.senders {
            if sender.healthy() {
                if sender.send(data).await.is_err() {
                    self.log_type.record_drops(1);
                }
                return;
            }
        }
        self.log_type.record_drops(1);
    }

    /// Send log with pre-serialization metadata to the first healthy sender.
    ///
    /// Routes to the first healthy sender only (Tier-1). If that sender's
    /// `send_with_meta` fails, the entry is dropped and
    /// the configured log type's dropped counter is incremented — the logger
    /// does NOT fall through to the next sender on a send error.
    ///
    /// Falls back to `send` for senders that do not override `send_with_meta`
    /// (ES, Kafka, LocalFile). OTLP senders use `meta` to build typed LogRecords.
    pub async fn send_with_meta(&self, data: String, meta: &LogMeta<'_>) {
        if self.senders.is_empty() {
            return;
        }
        for sender in &self.senders {
            if sender.healthy() {
                if sender.send_with_meta(data, meta).await.is_err() {
                    self.log_type.record_drops(1);
                }
                return;
            }
        }
        self.log_type.record_drops(1);
    }
}

impl Default for AccessLogger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/observe/access_log/logger_test.rs"]
mod tests;
