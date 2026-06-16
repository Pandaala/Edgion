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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link_sys::DataSender;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    struct FakeSender {
        name: String,
        healthy: Arc<AtomicBool>,
        received: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl DataSender<String> for FakeSender {
        async fn init(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn healthy(&self) -> bool {
            self.healthy.load(Ordering::Relaxed)
        }
        async fn send(&self, _data: String) -> anyhow::Result<()> {
            self.received.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn name(&self) -> &str {
            &self.name
        }
    }

    fn fake(name: &str, healthy: bool) -> (FakeSender, Arc<AtomicUsize>) {
        let received = Arc::new(AtomicUsize::new(0));
        let sender = FakeSender {
            name: name.to_string(),
            healthy: Arc::new(AtomicBool::new(healthy)),
            received: Arc::clone(&received),
        };
        (sender, received)
    }

    #[tokio::test]
    async fn send_skips_unhealthy_and_falls_through_to_next() {
        let (first, first_rx) = fake("first", false);
        let (second, second_rx) = fake("second", true);
        let mut logger = AccessLogger::new();
        logger.register(Box::new(first));
        logger.register(Box::new(second));

        logger.send("hello".to_string()).await;

        assert_eq!(
            first_rx.load(Ordering::Relaxed),
            0,
            "unhealthy first sender must be skipped"
        );
        assert_eq!(
            second_rx.load(Ordering::Relaxed),
            1,
            "healthy second sender must receive"
        );
    }

    #[tokio::test]
    async fn send_picks_first_when_it_is_healthy() {
        let (first, first_rx) = fake("first", true);
        let (second, second_rx) = fake("second", true);
        let mut logger = AccessLogger::new();
        logger.register(Box::new(first));
        logger.register(Box::new(second));

        logger.send("hello".to_string()).await;

        assert_eq!(
            first_rx.load(Ordering::Relaxed),
            1,
            "healthy first sender wins (priority = order)"
        );
        assert_eq!(second_rx.load(Ordering::Relaxed), 0, "second sender must not be used");
    }

    #[tokio::test]
    async fn send_drops_when_all_unhealthy() {
        let (first, first_rx) = fake("first", false);
        let (second, second_rx) = fake("second", false);
        let mut logger = AccessLogger::new();
        logger.register(Box::new(first));
        logger.register(Box::new(second));

        // No panic, no delivery (the dropped metric is incremented; not asserted
        // here since there is no metrics recorder installed in this unit test).
        logger.send("hello".to_string()).await;

        assert_eq!(first_rx.load(Ordering::Relaxed), 0);
        assert_eq!(second_rx.load(Ordering::Relaxed), 0);
    }
}
