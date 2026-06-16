//! Kafka LinkSys runtime client.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use rskafka::client::partition::UnknownTopicHandling;
use rskafka::client::producer::aggregator::RecordAggregator;
use rskafka::client::producer::BatchProducerBuilder;
use rskafka::client::{ClientBuilder, SaslConfig};
use rskafka::record::Record;
use tokio::sync::mpsc;

use crate::link_sys::credential::{CredentialState, ResolvedCredential};
use crate::link_sys::lifecycle::LinkSysLifecycle;
use crate::link_sys::LinkSysHealth;
use edgion_resources::resources::link_sys::kafka::KafkaClientConfig;

const DEFAULT_CHANNEL_SIZE: usize = 4096;
const DEFAULT_LINGER_MS: u64 = 5;
const MAX_BATCH_SIZE: usize = 1_000_000;

/// Fold a single produce result into the shared `healthy` flag.
///
/// Returns `true` iff the flag actually flipped, so the caller emits metrics + a
/// (rate-limited) log only on a real transition. `compare_exchange` means that when
/// multiple per-topic tasks share one flag, only the task that wins a flip acts.
/// `Relaxed` is sufficient: this is a single independent advisory flag with no
/// happens-before relationship to `inner_client` (which carries its own sync via
/// `ArcSwapOption`). The flag is read only as a routing hint and does not protect
/// any other shared state. A stale failure CAS can spuriously flip an
/// already-recovered flag back to unhealthy; this is acceptable because the flag is
/// corrected on the next successful produce.
fn record_produce_result(healthy: &AtomicBool, ok: bool) -> bool {
    if ok {
        healthy
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    } else {
        healthy
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }
}

const FLIP_LOG_MIN_INTERVAL_MS: u64 = 5_000;

/// Process-start monotonic base for flip-log rate limiting.
static FLIP_CLOCK_BASE: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Pure rate-limit core (clock-injected, unit-testable without sleeping).
///
/// Returns `true` (and stamps `last`) iff this is the first flip (`last == 0` sentinel)
/// or at least `interval_ms` has passed since the last allowed flip. Shared across both
/// directions, so total flip-log rate is bounded to <=1 line per window per client even
/// under partial-outage ping-pong.
///
/// Best-effort throttle: the load/store pair is non-atomic, so concurrent first callers
/// may both return `true` (a one-off double-log). This is acceptable — the gate only
/// limits log volume, it is not a correctness barrier.
fn flip_log_allowed_at(last: &AtomicU64, now_ms: u64, interval_ms: u64) -> bool {
    let prev = last.load(Ordering::Relaxed);
    if prev != 0 && now_ms.saturating_sub(prev) < interval_ms {
        return false;
    }
    // Never store 0, so the "first flip" sentinel stays meaningful.
    last.store(now_ms.max(1), Ordering::Relaxed);
    true
}

/// Thin wrapper supplying the real monotonic clock and configured interval.
fn flip_log_allowed(last: &AtomicU64) -> bool {
    let now_ms = FLIP_CLOCK_BASE.elapsed().as_millis() as u64;
    flip_log_allowed_at(last, now_ms, FLIP_LOG_MIN_INTERVAL_MS)
}

pub struct KafkaLinkClient {
    name: String,
    config: KafkaClientConfig,
    auth: CredentialState<ResolvedCredential>,
    channel_size: usize,
    linger_ms: u64,
    inner_client: ArcSwapOption<rskafka::client::Client>,
    topics: DashMap<String, mpsc::Sender<String>>,
    healthy: Arc<AtomicBool>,
    /// Monotonic-ms timestamp of the last emitted flip log; gate for log rate limiting.
    last_flip_log_ms: Arc<AtomicU64>,
}

impl KafkaLinkClient {
    pub fn from_config(
        name: &str,
        config: &KafkaClientConfig,
        auth: &CredentialState<ResolvedCredential>,
    ) -> Result<Self> {
        if config.brokers.is_empty() {
            anyhow::bail!("Kafka brokers list is empty for '{}'", name);
        }
        Ok(Self {
            name: name.to_string(),
            config: config.clone(),
            auth: auth.clone(),
            channel_size: config.channel_size.unwrap_or(DEFAULT_CHANNEL_SIZE),
            linger_ms: config.linger_ms.unwrap_or(DEFAULT_LINGER_MS),
            inner_client: ArcSwapOption::empty(),
            topics: DashMap::new(),
            healthy: Arc::new(AtomicBool::new(false)),
            last_flip_log_ms: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    pub fn health_status(&self) -> LinkSysHealth {
        let connected = self.healthy();
        LinkSysHealth {
            name: self.name.clone(),
            system_type: "kafka".to_string(),
            connected,
            latency_ms: None,
            error: if connected {
                None
            } else {
                Some("not connected".to_string())
            },
        }
    }

    /// Non-blocking send to Kafka topic. Returns Err if not initialized or channel full.
    pub fn try_produce(&self, topic: &str, data: String) -> Result<()> {
        let client = self.inner_client.load_full();
        if client.is_none() {
            anyhow::bail!("Kafka [{}] not initialized", self.name);
        }

        let sender = self
            .topics
            .entry(topic.to_string())
            .or_insert_with(|| {
                let (tx, rx) = mpsc::channel(self.channel_size);
                let topic_owned = topic.to_string();
                let linger_ms = self.linger_ms;
                let name = self.name.clone();
                let healthy = Arc::clone(&self.healthy);
                let last_flip_log_ms = Arc::clone(&self.last_flip_log_ms);
                tokio::spawn(async move {
                    Self::run_producer_task(client, topic_owned, rx, linger_ms, name, healthy, last_flip_log_ms).await;
                });
                tx
            })
            .clone();

        sender
            .try_send(data)
            .map_err(|e| anyhow::anyhow!("Kafka [{}] topic {}: {}", self.name, topic, e))
    }

    async fn run_producer_task(
        client: Option<Arc<rskafka::client::Client>>,
        topic: String,
        mut rx: mpsc::Receiver<String>,
        linger_ms: u64,
        name: String,
        healthy: Arc<AtomicBool>,
        last_flip_log_ms: Arc<AtomicU64>,
    ) {
        let Some(client) = client else {
            tracing::error!(kafka = %name, %topic, "producer task started with no client");
            return;
        };
        let partition = match client
            .partition_client(topic.as_str(), 0_i32, UnknownTopicHandling::Retry)
            .await
        {
            Ok(p) => Arc::new(p),
            Err(e) => {
                tracing::error!(kafka = %name, %topic, "PartitionClient init failed: {}", e);
                healthy.store(false, Ordering::Relaxed);
                // Keep the gauge in sync with the flag: the task exits below and would
                // otherwise leave the gauge stuck at the init() baseline of 1.
                crate::observe::metrics::registry::record_link_sys_health(&name, false);
                // TODO(task-05): the producer task exits here and the topic entry is never
                // respawned, so this topic cannot self-recover. Task 05's background probe owns
                // rebuild/liveness — it must respawn this task, not merely flip the flag. Note
                // this path updates the gauge but not `record_link_sys_health_flip`; task 05
                // should also emit the flip counter once it gives this path real recovery.
                return;
            }
        };
        let producer = BatchProducerBuilder::new(partition)
            .with_linger(Duration::from_millis(linger_ms))
            .build(RecordAggregator::new(MAX_BATCH_SIZE));

        while let Some(data) = rx.recv().await {
            let record = Record {
                key: None,
                value: Some(data.into_bytes()),
                headers: BTreeMap::new(),
                timestamp: Utc::now(),
            };
            match producer.produce(record).await {
                Ok(_) => {
                    if record_produce_result(&healthy, true) {
                        crate::observe::metrics::registry::record_link_sys_health(&name, true);
                        crate::observe::metrics::registry::record_link_sys_health_flip(&name, true);
                        if flip_log_allowed(&last_flip_log_ms) {
                            tracing::info!(kafka = %name, %topic, "Kafka recovered: produce succeeded");
                        }
                    }
                }
                Err(e) => {
                    if record_produce_result(&healthy, false) {
                        crate::observe::metrics::registry::record_link_sys_health(&name, false);
                        crate::observe::metrics::registry::record_link_sys_health_flip(&name, false);
                        if flip_log_allowed(&last_flip_log_ms) {
                            tracing::warn!(kafka = %name, %topic, "Kafka degraded: produce failed: {}", e);
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl LinkSysLifecycle for KafkaLinkClient {
    async fn init(&self) -> Result<()> {
        let mut builder = ClientBuilder::new(self.config.brokers.clone());

        // TLS — TODO: rskafka 0.5 depends on rustls 0.21, while the rest of this
        // project uses rustls 0.23.  The two `ClientConfig` types are not compatible
        // across crate versions, so we cannot call
        // `crate::link_sys::tls::build_rustls_client_config` (which returns a 0.23
        // value) and pass it to `ClientBuilder::tls_config` (which expects 0.21).
        // Once rskafka ships a release that targets rustls 0.23 this section should be
        // replaced with:
        //   if let Some(client_config) =
        //       crate::link_sys::tls::build_rustls_client_config(tls_config)?
        //   {
        //       builder = builder.tls_config(Arc::new(client_config));
        //   }
        if let Some(tls_config) = &self.config.tls {
            let _ = tls_config; // suppress unused-variable warning
        }

        // SASL/PLAIN — the resolved credential carries the password from the K8s Secret;
        // the username lives directly in the CRD spec.
        if let Some(sasl) = &self.config.sasl {
            if let CredentialState::Resolved(ResolvedCredential::Basic(basic)) = &self.auth {
                builder = builder.sasl_config(SaslConfig::Plain {
                    username: sasl.username.clone(),
                    password: basic.password.clone(),
                });
            }
        }

        let client = builder.build().await?;
        self.inner_client.store(Some(Arc::new(client)));
        self.healthy.store(true, Ordering::Relaxed);
        crate::observe::metrics::registry::record_link_sys_health(&self.name, true);
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        self.topics.clear();
        self.inner_client.store(None);
        self.healthy.store(false, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::link_sys::kafka::KafkaClientConfig;

    #[test]
    fn test_kafka_link_client_from_config_empty_brokers_fails() {
        let config = KafkaClientConfig {
            brokers: vec![],
            sasl: None,
            tls: None,
            channel_size: None,
            linger_ms: None,
        };
        let auth = CredentialState::NotRequired;
        let result = KafkaLinkClient::from_config("test", &config, &auth);
        assert!(result.is_err());
        // Use `.err().unwrap()` instead of `.unwrap_err()` to avoid requiring
        // `KafkaLinkClient: Debug`.
        assert!(result.err().unwrap().to_string().contains("empty"));
    }

    #[test]
    fn test_kafka_link_client_from_config_ok() {
        let config = KafkaClientConfig {
            brokers: vec!["broker1:9092".to_string()],
            sasl: None,
            tls: None,
            channel_size: None,
            linger_ms: None,
        };
        let auth = CredentialState::NotRequired;
        let result = KafkaLinkClient::from_config("default/kafka-main", &config, &auth);
        assert!(result.is_ok());
        let client = result.unwrap();
        assert!(!client.healthy()); // not initialized yet
        assert_eq!(client.channel_size, 4096);
        assert_eq!(client.linger_ms, 5);
    }

    #[test]
    fn record_produce_result_flips_only_on_change() {
        let flag = AtomicBool::new(false);
        // false -> success -> flips to true
        assert!(record_produce_result(&flag, true));
        assert!(flag.load(Ordering::Relaxed));
        // already true + success -> no flip
        assert!(!record_produce_result(&flag, true));
        // true -> failure -> flips to false
        assert!(record_produce_result(&flag, false));
        assert!(!flag.load(Ordering::Relaxed));
        // already false + failure -> no flip
        assert!(!record_produce_result(&flag, false));
    }

    #[test]
    fn record_produce_result_single_winner_under_contention() {
        use std::sync::Arc;
        let flag = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let f = Arc::clone(&flag);
            handles.push(std::thread::spawn(move || record_produce_result(&f, true)));
        }
        let winners = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|&won| won)
            .count();
        assert_eq!(winners, 1, "exactly one task may win the false->true flip");
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn flip_log_allowed_at_rate_limits_within_window() {
        let last = AtomicU64::new(0);
        // first flip (sentinel 0) always logs
        assert!(flip_log_allowed_at(&last, 1_000, 5_000));
        // within the window -> suppressed
        assert!(!flip_log_allowed_at(&last, 2_000, 5_000));
        // window elapsed -> allowed again
        assert!(flip_log_allowed_at(&last, 6_000, 5_000));
        // never stores the 0 sentinel
        assert_ne!(last.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn flip_log_allowed_at_first_call_logs_even_at_time_zero() {
        let last = AtomicU64::new(0);
        // now_ms == 0 right after process start must still log (sentinel handling)
        assert!(flip_log_allowed_at(&last, 0, 5_000));
        // `now_ms.max(1)` must have stored 1, not the forbidden 0 sentinel
        assert_eq!(last.load(Ordering::Relaxed), 1);
    }
}
