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
    /// dropped and `edgion_access_log_dropped_total` is incremented (I4) —
    /// the logger does NOT fall through to the next sender on a send error
    /// (that would be tier-2 send-fallback, intentionally out of scope).
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
                if sender.send(data).await.is_err() {
                    global_metrics().access_log_dropped();
                }
                return;
            }
        }
        global_metrics().access_log_dropped();
    }

    /// Send log with pre-serialization metadata to the first healthy sender.
    ///
    /// Routes to the first healthy sender only (Tier-1). If that sender's
    /// `send_with_meta` fails, the entry is dropped and
    /// `edgion_access_log_dropped_total` is incremented (I4) — the logger
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
                    global_metrics().access_log_dropped();
                }
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
        fail: Arc<AtomicBool>,
    }

    struct BorrowObservingSender {
        method_ptr: Arc<AtomicUsize>,
        path_ptr: Arc<AtomicUsize>,
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
            if self.fail.load(Ordering::Relaxed) {
                anyhow::bail!("simulated send failure");
            }
            self.received.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn name(&self) -> &str {
            &self.name
        }
    }

    #[async_trait]
    impl DataSender<String> for BorrowObservingSender {
        async fn init(&mut self) -> anyhow::Result<()> {
            Ok(())
        }

        fn healthy(&self) -> bool {
            true
        }

        async fn send(&self, _data: String) -> anyhow::Result<()> {
            anyhow::bail!("metadata-aware sender must receive send_with_meta")
        }

        async fn send_with_meta(&self, _data: String, meta: &LogMeta<'_>) -> anyhow::Result<()> {
            let method = meta.method.expect("test metadata includes method");
            let path = meta.path.expect("test metadata includes path");
            self.method_ptr.store(method.as_ptr() as usize, Ordering::Relaxed);
            self.path_ptr.store(path.as_ptr() as usize, Ordering::Relaxed);
            Ok(())
        }

        fn name(&self) -> &str {
            "borrow-observer"
        }
    }

    fn fake(name: &str, healthy: bool) -> (FakeSender, Arc<AtomicUsize>, Arc<AtomicBool>) {
        let received = Arc::new(AtomicUsize::new(0));
        let fail = Arc::new(AtomicBool::new(false));
        let sender = FakeSender {
            name: name.to_string(),
            healthy: Arc::new(AtomicBool::new(healthy)),
            received: Arc::clone(&received),
            fail: Arc::clone(&fail),
        };
        (sender, received, fail)
    }

    #[tokio::test]
    async fn send_skips_unhealthy_and_falls_through_to_next() {
        let (first, first_rx, _) = fake("first", false);
        let (second, second_rx, _) = fake("second", true);
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
        let (first, first_rx, _) = fake("first", true);
        let (second, second_rx, _) = fake("second", true);
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
        let (first, first_rx, _) = fake("first", false);
        let (second, second_rx, _) = fake("second", false);
        let mut logger = AccessLogger::new();
        logger.register(Box::new(first));
        logger.register(Box::new(second));

        // No panic, no delivery (the dropped metric is incremented; not asserted
        // here since there is no metrics recorder installed in this unit test).
        logger.send("hello".to_string()).await;

        assert_eq!(first_rx.load(Ordering::Relaxed), 0);
        assert_eq!(second_rx.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn send_does_not_fall_through_when_chosen_sender_errors() {
        let (first, first_rx, first_fail) = fake("first", true);
        let (second, second_rx, _) = fake("second", true);
        first_fail.store(true, Ordering::Relaxed);
        let mut logger = AccessLogger::new();
        logger.register(Box::new(first));
        logger.register(Box::new(second));

        logger.send("hello".to_string()).await;

        assert_eq!(first_rx.load(Ordering::Relaxed), 0, "first errored, nothing recorded");
        assert_eq!(
            second_rx.load(Ordering::Relaxed),
            0,
            "no fall-through to the second sender"
        );
    }

    // send_with_meta shares send's no-fall-through-on-error contract; FakeSender
    // does not override send_with_meta, so it defaults to send().
    #[tokio::test]
    async fn send_with_meta_does_not_fall_through_when_chosen_sender_errors() {
        let (first, first_rx, first_fail) = fake("first", true);
        let (second, second_rx, _) = fake("second", true);
        first_fail.store(true, Ordering::Relaxed);
        let mut logger = AccessLogger::new();
        logger.register(Box::new(first));
        logger.register(Box::new(second));

        logger.send_with_meta("hello".to_string(), &LogMeta::default()).await;

        assert_eq!(first_rx.load(Ordering::Relaxed), 0, "first errored, nothing recorded");
        assert_eq!(
            second_rx.load(Ordering::Relaxed),
            0,
            "no fall-through to the second sender"
        );
    }

    #[tokio::test]
    async fn send_with_meta_preserves_borrowed_method_and_path_through_trait_object() {
        let observed_method_ptr = Arc::new(AtomicUsize::new(0));
        let observed_path_ptr = Arc::new(AtomicUsize::new(0));
        let sender: Box<dyn DataSender<String>> = Box::new(BorrowObservingSender {
            method_ptr: Arc::clone(&observed_method_ptr),
            path_ptr: Arc::clone(&observed_path_ptr),
        });
        let mut logger = AccessLogger::new();
        logger.register(sender);

        let method = String::from("PATCH");
        let path = String::from("/v1/items/42");
        let meta = LogMeta {
            method: Some(method.as_str()),
            path: Some(path.as_str()),
            ..Default::default()
        };

        logger.send_with_meta("hello".to_string(), &meta).await;

        assert_eq!(observed_method_ptr.load(Ordering::Relaxed), method.as_ptr() as usize);
        assert_eq!(observed_path_ptr.load(Ordering::Relaxed), path.as_ptr() as usize);
    }
}
