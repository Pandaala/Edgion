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
                    self.log_type.record_drop();
                }
                return;
            }
        }
        self.log_type.record_drop();
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
                    self.log_type.record_drop();
                }
                return;
            }
        }
        self.log_type.record_drop();
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

    fn assert_counter(rendered: &str, name: &str, value: u64) {
        let expected = format!("{name} {value}");
        assert!(
            rendered.lines().any(|line| line == expected),
            "expected {expected:?} in metrics:\n{rendered}"
        );
    }

    fn assert_counter_absent(rendered: &str, name: &str) {
        assert!(!rendered.contains(name), "unexpected {name} in metrics:\n{rendered}");
    }

    #[test]
    fn drop_metrics_follow_logger_type_and_failure_path() {
        const CHILD_ENV: &str = "EDGION_TYPED_LOG_DROP_METRICS_TEST_CHILD";
        const TEST_MODULE: &str = "observe::access_log::logger::tests";
        const TEST_NAME: &str = "drop_metrics_follow_logger_type_and_failure_path";

        crate::test_subprocess::assert_in_subprocess(TEST_MODULE, TEST_NAME, CHILD_ENV, TEST_NAME, || {
            let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            let handle = recorder.handle();
            metrics::with_local_recorder(&recorder, || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("test runtime must build")
                    .block_on(async {
                        let (sender, _, _) = fake("tcp-unhealthy", false);
                        let mut tcp = AccessLogger::new_with_log_type(LogType::Tcp);
                        tcp.register(Box::new(sender));
                        tcp.send("tcp".into()).await;
                        let rendered = handle.render();
                        assert_counter(&rendered, "edgion_tcp_log_dropped_total", 1);
                        assert_counter_absent(&rendered, "edgion_access_log_dropped_total");
                        assert_counter_absent(&rendered, "edgion_tls_log_dropped_total");
                        assert_counter_absent(&rendered, "edgion_udp_log_dropped_total");

                        let (sender, _, _) = fake("tls-unhealthy", false);
                        let mut tls = AccessLogger::new_with_log_type(LogType::Tls);
                        tls.register(Box::new(sender));
                        tls.send_with_meta("tls".into(), &LogMeta::default()).await;
                        let rendered = handle.render();
                        assert_counter(&rendered, "edgion_tls_log_dropped_total", 1);
                        assert_counter(&rendered, "edgion_tcp_log_dropped_total", 1);
                        assert_counter_absent(&rendered, "edgion_access_log_dropped_total");
                        assert_counter_absent(&rendered, "edgion_udp_log_dropped_total");

                        let (sender, _, _) = fake("udp-unhealthy", false);
                        let mut udp = AccessLogger::new_with_log_type(LogType::Udp);
                        udp.register(Box::new(sender));
                        udp.send("udp".into()).await;
                        let rendered = handle.render();
                        assert_counter(&rendered, "edgion_udp_log_dropped_total", 1);
                        assert_counter(&rendered, "edgion_tcp_log_dropped_total", 1);
                        assert_counter(&rendered, "edgion_tls_log_dropped_total", 1);
                        assert_counter_absent(&rendered, "edgion_access_log_dropped_total");

                        let (remote, _, remote_fail) = fake("remote", true);
                        let (fallback, fallback_received, _) = fake("fallback", true);
                        remote_fail.store(true, Ordering::Relaxed);
                        let mut remote_logger = AccessLogger::new_with_log_type(LogType::Tcp);
                        remote_logger.register(Box::new(remote));
                        remote_logger.register(Box::new(fallback));
                        remote_logger.send("remote".into()).await;
                        assert_eq!(fallback_received.load(Ordering::Relaxed), 0);
                        assert_counter(&handle.render(), "edgion_tcp_log_dropped_total", 2);
                        assert_counter(&handle.render(), "edgion_tls_log_dropped_total", 1);
                        assert_counter(&handle.render(), "edgion_udp_log_dropped_total", 1);

                        let (remote, _, remote_fail) = fake("remote-meta", true);
                        remote_fail.store(true, Ordering::Relaxed);
                        let mut remote_meta = AccessLogger::new_with_log_type(LogType::Tls);
                        remote_meta.register(Box::new(remote));
                        remote_meta
                            .send_with_meta("remote-meta".into(), &LogMeta::default())
                            .await;
                        assert_counter(&handle.render(), "edgion_tls_log_dropped_total", 2);
                        assert_counter(&handle.render(), "edgion_tcp_log_dropped_total", 2);
                        assert_counter(&handle.render(), "edgion_udp_log_dropped_total", 1);

                        let (sender, _, _) = fake("access-unhealthy", false);
                        let mut access = AccessLogger::new();
                        access.register(Box::new(sender));
                        access.send("access".into()).await;
                        assert_counter(&handle.render(), "edgion_access_log_dropped_total", 1);
                        assert_counter(&handle.render(), "edgion_tcp_log_dropped_total", 2);
                        assert_counter(&handle.render(), "edgion_tls_log_dropped_total", 2);
                        assert_counter(&handle.render(), "edgion_udp_log_dropped_total", 1);

                        let (sender, _, _) = fake("default-access-unhealthy", false);
                        let mut default_access = AccessLogger::default();
                        default_access.register(Box::new(sender));
                        default_access.send("default-access".into()).await;
                        assert_counter(&handle.render(), "edgion_access_log_dropped_total", 2);

                        let zero_sender = AccessLogger::new_with_log_type(LogType::Udp);
                        zero_sender.send_with_meta("disabled".into(), &LogMeta::default()).await;
                        assert_counter(&handle.render(), "edgion_udp_log_dropped_total", 1);
                        assert_counter_absent(&handle.render(), "edgion_ssl_log_dropped_total");
                    });
            });
        });
    }

    #[tokio::test]
    async fn unresolved_otlp_reference_uses_first_healthy_fallback() {
        let otlp = crate::link_sys::providers::otlp::OtlpLogSender::new(edgion_resources::LinkSysRef {
            namespace: "fallback-test".into(),
            name: "missing-collector".into(),
        })
        .unwrap();
        let (fallback, received, _) = fake("fallback", true);
        let (third, third_received, _) = fake("third", true);
        let mut logger = AccessLogger::new();
        logger.register(Box::new(otlp));
        logger.register(Box::new(fallback));
        logger.register(Box::new(third));
        logger
            .send_with_meta("otlp-fallback-marker".into(), &LogMeta::default())
            .await;
        assert_eq!(received.load(Ordering::Relaxed), 1);
        assert_eq!(third_received.load(Ordering::Relaxed), 0);
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
