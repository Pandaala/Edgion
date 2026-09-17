//! Kafka LinkSys runtime client.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use arc_swap::ArcSwapOption;
use chrono::Utc;
use dashmap::DashMap;
use rskafka::client::partition::UnknownTopicHandling;
use rskafka::client::producer::aggregator::RecordAggregator;
use rskafka::client::producer::BatchProducerBuilder;
use rskafka::client::{ClientBuilder, Credentials, SaslConfig};
use rskafka::record::Record;
use tokio::sync::mpsc;

use crate::link_sys::credential::{CredentialState, ResolvedCredential};
use crate::link_sys::runtime::apply::{ConnectionCoverage, RuntimeFingerprint};
use crate::link_sys::runtime::health_probe::flip_log_allowed;
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::kafka::{
    KafkaClientConfig, DEFAULT_KAFKA_CHANNEL_SIZE, DEFAULT_KAFKA_LINGER_MS, DEFAULT_KAFKA_MAX_PENDING_BYTES,
    DEFAULT_KAFKA_MAX_PENDING_RECORDS, DEFAULT_KAFKA_MAX_TOPICS,
};
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

const MAX_BATCH_SIZE: usize = 1_000_000;
/// Consecutive probe failures before the probe rebuilds `inner_client` from the
/// configured bootstrap brokers (rskafka otherwise only reconnects to the
/// discovered topology, which never recovers a pod-IP/StatefulSet restart).
const KAFKA_REBUILD_THRESHOLD: u32 = 3;

/// Per-produce wedge detector. rskafka's default backoff has no deadline, so a
/// dead/old-IP broker makes `produce().await` hang forever; this timeout forces
/// the task to exit (closing its channel) so the self-heal + rebuild can respawn
/// it on the current client. Generous relative to `linger_ms`.
/// `from_config` rejects linger durations at or above this ceiling so every
/// accepted configuration leaves time for the produce operation itself.
const PRODUCE_TIMEOUT: Duration = Duration::from_secs(30);

struct QueueBudget {
    max_records: usize,
    max_bytes: usize,
    pending_records: AtomicUsize,
    pending_bytes: AtomicUsize,
}

impl QueueBudget {
    fn new(max_records: usize, max_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            max_records,
            max_bytes,
            pending_records: AtomicUsize::new(0),
            pending_bytes: AtomicUsize::new(0),
        })
    }

    fn try_reserve(self: &Arc<Self>, bytes: usize) -> Result<QueueReservation, QueueBudgetError> {
        if !try_reserve_counter(&self.pending_bytes, bytes, self.max_bytes) {
            return Err(QueueBudgetError::Bytes);
        }
        if !try_reserve_counter(&self.pending_records, 1, self.max_records) {
            self.pending_bytes.fetch_sub(bytes, Ordering::AcqRel);
            return Err(QueueBudgetError::Records);
        }
        Ok(QueueReservation {
            budget: Arc::clone(self),
            bytes,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueBudgetError {
    Records,
    Bytes,
}

struct QueueReservation {
    budget: Arc<QueueBudget>,
    bytes: usize,
}

impl Drop for QueueReservation {
    fn drop(&mut self) {
        self.budget.pending_records.fetch_sub(1, Ordering::AcqRel);
        self.budget.pending_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct TopicBudget {
    max_topics: usize,
    topics: AtomicUsize,
}

impl TopicBudget {
    fn new(max_topics: usize) -> Arc<Self> {
        Arc::new(Self {
            max_topics,
            topics: AtomicUsize::new(0),
        })
    }

    fn try_reserve(self: &Arc<Self>) -> Option<TopicReservation> {
        try_reserve_counter(&self.topics, 1, self.max_topics).then(|| TopicReservation {
            budget: Arc::clone(self),
        })
    }
}

struct TopicReservation {
    budget: Arc<TopicBudget>,
}

impl Drop for TopicReservation {
    fn drop(&mut self) {
        self.budget.topics.fetch_sub(1, Ordering::AcqRel);
    }
}

fn try_reserve_counter(counter: &AtomicUsize, amount: usize, maximum: usize) -> bool {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current.checked_add(amount).filter(|next| *next <= maximum)
        })
        .is_ok()
}

struct QueuedRecord {
    data: String,
    reservation: QueueReservation,
}

struct TopicEntry {
    sender: mpsc::Sender<QueuedRecord>,
    producer_handle: tokio::task::JoinHandle<()>,
    _reservation: TopicReservation,
}

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
fn emit_kafka_health(retired: &AtomicBool, name: &str, ok: bool) {
    if retired.load(Ordering::Relaxed) {
        return;
    }
    crate::observe::metrics::registry::record_link_sys_health(name, ok);
    crate::observe::metrics::registry::record_link_sys_health_flip(name, ok);
}

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

/// Build the rustls `ClientConfig` for the rskafka TLS connector.
///
/// Delegates custom-CA / client-mTLS / verify-gate resolution to the shared
/// [`crate::link_sys::tls::build_rustls_client_config`]. That helper returns
/// `None` when the operator supplied no custom CA and no client identity; unlike
/// reqwest / fred, rskafka has **no implicit default TLS connector** — omitting
/// `tls_config` yields a plaintext connection — so in that case we still build an
/// explicit system-roots (built-in webpki) verifying config to honor
/// `tls.enabled: true`.
fn build_kafka_tls_config(tls: &TlsConfig) -> Result<rustls::ClientConfig> {
    if let Some(cfg) = crate::link_sys::tls::build_rustls_client_config(tls)? {
        return Ok(cfg);
    }
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

pub struct KafkaLinkClient {
    name: String,
    config: KafkaClientConfig,
    auth: CredentialState<ResolvedCredential>,
    channel_size: usize,
    linger_ms: u64,
    inner_client: Arc<ArcSwapOption<rskafka::client::Client>>,
    topics: DashMap<String, TopicEntry>,
    /// Serializes new producer registration with retirement without locking existing-topic sends.
    producer_admission: parking_lot::Mutex<()>,
    topic_budget: Arc<TopicBudget>,
    queue_budget: Arc<QueueBudget>,
    healthy: Arc<AtomicBool>,
    /// Monotonic-ms timestamp of the last emitted flip log; gate for log rate limiting.
    last_flip_log_ms: Arc<AtomicU64>,
    /// Probe shutdown signal (populated in init, fired in shutdown).
    shutdown_tx: parking_lot::Mutex<Option<tokio::sync::watch::Sender<bool>>>,
    /// Background health probe handle; aborted in shutdown so a probe flip can't race the healthy=false write.
    probe_handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    fingerprint: RuntimeFingerprint,
    retired: Arc<AtomicBool>,
    shutdown_started: AtomicBool,
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
        let channel_size = config.channel_size.unwrap_or(DEFAULT_KAFKA_CHANNEL_SIZE);
        let max_topics = config.max_topics.unwrap_or(DEFAULT_KAFKA_MAX_TOPICS);
        let max_pending_records = config.max_pending_records.unwrap_or(DEFAULT_KAFKA_MAX_PENDING_RECORDS);
        let max_pending_bytes = config.max_pending_bytes.unwrap_or(DEFAULT_KAFKA_MAX_PENDING_BYTES);
        let linger_ms = config.linger_ms.unwrap_or(DEFAULT_KAFKA_LINGER_MS);
        if channel_size == 0 {
            anyhow::bail!("Kafka channel size must be greater than zero for '{}'", name);
        }
        if max_topics == 0 {
            anyhow::bail!("Kafka max topics must be greater than zero for '{}'", name);
        }
        if max_pending_records == 0 {
            anyhow::bail!("Kafka max pending records must be greater than zero for '{}'", name);
        }
        if max_pending_bytes == 0 {
            anyhow::bail!("Kafka max pending bytes must be greater than zero for '{}'", name);
        }
        if Duration::from_millis(linger_ms) >= PRODUCE_TIMEOUT {
            anyhow::bail!(
                "Kafka linger duration must be shorter than the {} second produce timeout for '{}'",
                PRODUCE_TIMEOUT.as_secs(),
                name
            );
        }
        Ok(Self {
            name: name.to_string(),
            config: config.clone(),
            auth: auth.clone(),
            channel_size,
            linger_ms,
            inner_client: Arc::new(ArcSwapOption::empty()),
            topics: DashMap::new(),
            producer_admission: parking_lot::Mutex::new(()),
            topic_budget: TopicBudget::new(max_topics),
            queue_budget: QueueBudget::new(max_pending_records, max_pending_bytes),
            healthy: Arc::new(AtomicBool::new(false)),
            last_flip_log_ms: Arc::new(AtomicU64::new(0)),
            shutdown_tx: parking_lot::Mutex::new(None),
            probe_handle: parking_lot::Mutex::new(None),
            fingerprint: RuntimeFingerprint::unspecified(),
            retired: Arc::new(AtomicBool::new(false)),
            shutdown_started: AtomicBool::new(false),
        })
    }

    /// Build from a resolved Kafka LinkSys, including the apply fingerprint.
    pub(crate) fn from_linksys(name: &str, ls: &LinkSys) -> Result<Arc<Self>> {
        let SystemConfig::Kafka(config) = &ls.spec.config else {
            anyhow::bail!("Kafka from_linksys called with a non-Kafka LinkSys");
        };
        let auth = crate::link_sys::credential::resolved_state(ls);
        let fingerprint = super::apply::fingerprint_kafka(ls)?;
        let mut client = Self::from_config(name, config, &auth)?;
        client.fingerprint = fingerprint;
        Ok(Arc::new(client))
    }

    pub(crate) fn runtime_fingerprint(&self) -> &RuntimeFingerprint {
        &self.fingerprint
    }

    pub(crate) fn connection_coverage(&self) -> ConnectionCoverage {
        let configured = self.config.brokers.len().max(1);
        let live = if self.inner_client.load_full().is_some() && self.healthy() {
            self.config.brokers.len()
        } else {
            0
        };
        ConnectionCoverage { live, configured }
    }

    pub(crate) fn mark_retired(&self) {
        let _admission = self.producer_admission.lock();
        self.retired.store(true, Ordering::Relaxed);
        if let Some(handle) = self.probe_handle.lock().take() {
            handle.abort();
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Build a connected rskafka client from brokers + resolved auth. Shared by
    /// `init()` and the background-probe rebuild so the two cannot diverge.
    async fn build_kafka_client(
        brokers: &[String],
        auth: &CredentialState<ResolvedCredential>,
        config: &KafkaClientConfig,
    ) -> Result<rskafka::client::Client> {
        let mut builder = ClientBuilder::new(brokers.to_vec());

        // TLS — rskafka 0.6 targets rustls 0.23 (matching the workspace), so the
        // shared outbound resolver / builder can drive the connector directly:
        // custom CA (REPLACE), client mTLS identity, and the verify gate all flow
        // through `build_rustls_client_config` (provider_can_disable=false — the
        // rustls connector always verifies, so verify=false is rejected there).
        if let Some(tls_config) = &config.tls {
            if tls_config.enabled {
                builder = builder.tls_config(Arc::new(build_kafka_tls_config(tls_config)?));
            }
        }

        // SASL/PLAIN — the resolved credential carries the password from the K8s Secret;
        // the username lives directly in the CRD spec.
        if let Some(sasl) = &config.sasl {
            if let CredentialState::Resolved(ResolvedCredential::Basic(basic)) = auth {
                builder = builder.sasl_config(SaslConfig::Plain(Credentials::new(
                    sasl.username.clone(),
                    basic.password.clone(),
                )));
            }
        }

        builder
            .build()
            .await
            .map_err(|e| anyhow::anyhow!("Kafka client build failed: {}", e))
    }

    pub fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// Non-blocking send to Kafka topic. Returns Err if not initialized, retired, or channel full.
    pub fn try_produce(&self, topic: &str, data: String) -> Result<()> {
        if self.retired.load(Ordering::Relaxed) {
            anyhow::bail!("Kafka [{}] retired", self.name);
        }
        let client = self.inner_client.load_full();
        if client.is_none() {
            anyhow::bail!("Kafka [{}] not initialized", self.name);
        }

        let reservation = self.queue_budget.try_reserve(data.len()).map_err(|limit| match limit {
            QueueBudgetError::Records => anyhow::anyhow!(
                "Kafka [{}] aggregate pending record limit ({}) reached",
                self.name,
                self.queue_budget.max_records
            ),
            QueueBudgetError::Bytes => anyhow::anyhow!(
                "Kafka [{}] aggregate pending byte limit ({}) reached",
                self.name,
                self.queue_budget.max_bytes
            ),
        })?;

        let sender = match self.topics.entry(topic.to_string()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => entry.get().sender.clone(),
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let _admission = self.producer_admission.lock();
                if self.retired.load(Ordering::Relaxed) {
                    anyhow::bail!("Kafka [{}] retired", self.name);
                }
                let topic_reservation = self.topic_budget.try_reserve().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Kafka [{}] topic limit ({}) reached",
                        self.name,
                        self.topic_budget.max_topics
                    )
                })?;
                let (tx, rx) = mpsc::channel(self.channel_size);
                let topic_owned = topic.to_string();
                let linger_ms = self.linger_ms;
                let name = self.name.clone();
                let healthy = Arc::clone(&self.healthy);
                let last_flip_log_ms = Arc::clone(&self.last_flip_log_ms);
                let retired = Arc::clone(&self.retired);
                let producer_handle = tokio::spawn(async move {
                    Self::run_producer_task(
                        client,
                        topic_owned,
                        rx,
                        linger_ms,
                        name,
                        healthy,
                        last_flip_log_ms,
                        retired,
                    )
                    .await;
                });
                entry.insert(TopicEntry {
                    sender: tx.clone(),
                    producer_handle,
                    _reservation: topic_reservation,
                });
                tx
            }
        };

        let record = QueuedRecord { data, reservation };

        match sender.try_send(record) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_record)) => {
                // The producer receiver closed (PartitionClient init failure, or a
                // PRODUCE_TIMEOUT wedge). Evict only after task completion so
                // shutdown retains a handle for any task still finishing.
                // The next produce can respawn on the current inner_client.
                self.topics.remove_if(topic, |_, entry| {
                    entry.sender.is_closed() && entry.producer_handle.is_finished()
                });
                anyhow::bail!("Kafka [{}] topic {}: producer task closed", self.name, topic)
            }
            Err(mpsc::error::TrySendError::Full(_record)) => {
                anyhow::bail!("Kafka [{}] topic {}: per-topic channel is full", self.name, topic)
            }
        }
    }

    async fn run_producer_task(
        client: Option<Arc<rskafka::client::Client>>,
        topic: String,
        mut rx: mpsc::Receiver<QueuedRecord>,
        linger_ms: u64,
        name: String,
        healthy: Arc<AtomicBool>,
        last_flip_log_ms: Arc<AtomicU64>,
        retired: Arc<AtomicBool>,
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
                if !retired.load(Ordering::Relaxed) {
                    crate::observe::metrics::registry::record_link_sys_health(&name, false);
                }
                // The task exits here, closing its channel; `try_produce`'s
                // self-heal evicts the dead entry so the next produce respawns.
                // Broker-level recovery is owned by the background probe.
                return;
            }
        };
        let producer = BatchProducerBuilder::new(partition)
            .with_linger(Duration::from_millis(linger_ms))
            .build(RecordAggregator::new(MAX_BATCH_SIZE));

        while let Some(queued) = rx.recv().await {
            let QueuedRecord { data, reservation } = queued;
            let record = Record {
                key: None,
                value: Some(data.into_bytes()),
                headers: BTreeMap::new(),
                timestamp: Utc::now(),
            };
            let result = tokio::time::timeout(PRODUCE_TIMEOUT, producer.produce(record)).await;
            drop(reservation);
            match result {
                Ok(Ok(_)) => {
                    if record_produce_result(&healthy, true) {
                        emit_kafka_health(&retired, &name, true);
                        if flip_log_allowed(&last_flip_log_ms) {
                            tracing::info!(kafka = %name, %topic, "Kafka recovered: produce succeeded");
                        }
                    }
                }
                Ok(Err(e)) => {
                    if record_produce_result(&healthy, false) {
                        emit_kafka_health(&retired, &name, false);
                        if flip_log_allowed(&last_flip_log_ms) {
                            tracing::warn!(kafka = %name, %topic, "Kafka degraded: produce failed: {}", e);
                        }
                    }
                }
                Err(_elapsed) => {
                    // Produce wedged (stale/dead client). Exit so the channel closes
                    // and the next produce respawns this topic on the current
                    // inner_client (picks up a probe rebuild).
                    // The in-flight record and any messages still buffered in rx are lost
                    // when the task exits (degraded behavior, acceptable for a fire-and-forget sink).
                    if record_produce_result(&healthy, false) {
                        emit_kafka_health(&retired, &name, false);
                    }
                    tracing::warn!(
                        kafka = %name, %topic,
                        "Kafka produce wedged > {}s; exiting producer task to force respawn",
                        PRODUCE_TIMEOUT.as_secs()
                    );
                    return;
                }
            }
        }
    }
}

impl KafkaLinkClient {
    pub async fn init(&self) -> Result<()> {
        let client = Self::build_kafka_client(&self.config.brokers, &self.auth, &self.config).await?;
        self.inner_client.store(Some(Arc::new(client)));
        self.healthy.store(true, Ordering::Relaxed);
        crate::observe::metrics::registry::record_link_sys_health(&self.name, true);

        // Background health probe: broker liveness via list_topics; after N consecutive
        // failures, rebuild inner_client from config so the per-produce timeout in
        // run_producer_task picks up a fresh client.
        let (shutdown_sender, shutdown_rx) = tokio::sync::watch::channel(false);
        let probe_handle = crate::link_sys::runtime::health_probe::spawn_health_probe(
            crate::link_sys::runtime::health_probe::ProbeConfig {
                name: self.name.clone(),
                system_type: "kafka",
                base_interval: Duration::from_secs(10),
                max_backoff: Duration::from_secs(30),
                probe_timeout: Duration::from_secs(5),
                upgrade_only: false,
            },
            Arc::clone(&self.healthy),
            shutdown_rx,
            {
                let inner = Arc::clone(&self.inner_client);
                let brokers = self.config.brokers.clone();
                let auth = self.auth.clone();
                let config = self.config.clone();
                let name = self.name.clone();
                let fail_count = Arc::new(AtomicU32::new(0));
                let retired = Arc::clone(&self.retired);
                let healthy = Arc::clone(&self.healthy);
                move || {
                    let inner = Arc::clone(&inner);
                    let brokers = brokers.clone();
                    let auth = auth.clone();
                    let config = config.clone();
                    let name = name.clone();
                    let fail_count = Arc::clone(&fail_count);
                    let retired = Arc::clone(&retired);
                    let healthy = Arc::clone(&healthy);
                    async move {
                        if retired.load(Ordering::Relaxed) {
                            // Echo the current flag so apply_health_result is a no-op
                            // and cannot overwrite a successor generation's gauge.
                            return healthy.load(Ordering::Relaxed);
                        }
                        // Pre-increment BEFORE any await so a driver-timeout cancellation at the
                        // list_topics().await below still counts as a failure: rskafka's metadata
                        // call can hang indefinitely on dead brokers, and the driver cancels the
                        // future at the await, skipping any post-await bookkeeping.
                        let n = fail_count.fetch_add(1, Ordering::Relaxed) + 1;

                        // Once over the threshold, REBUILD FIRST — do not re-probe the (possibly
                        // wedged) current client, whose list_topics() would hang and be cancelled
                        // before we ever reach the rebuild. build_kafka_client reconnects from the
                        // configured bootstrap brokers (fresh DNS), recovering pod-IP churn.
                        if n >= KAFKA_REBUILD_THRESHOLD {
                            match Self::build_kafka_client(&brokers, &auth, &config).await {
                                Ok(c) => {
                                    let c = Arc::new(c);
                                    inner.store(Some(Arc::clone(&c)));
                                    fail_count.store(0, Ordering::Relaxed);
                                    return c.list_topics().await.is_ok();
                                }
                                Err(e) => {
                                    tracing::warn!(kafka = %name, "probe rebuild failed: {}", e);
                                    // Leave fail_count over threshold so the next cycle retries the rebuild.
                                    return false;
                                }
                            }
                        }

                        // Below threshold: normal liveness probe on the current client.
                        if let Some(client) = inner.load_full() {
                            if client.list_topics().await.is_ok() {
                                fail_count.store(0, Ordering::Relaxed);
                                return true;
                            }
                        }
                        false
                    }
                }
            },
        );
        *self.shutdown_tx.lock() = Some(shutdown_sender);
        *self.probe_handle.lock() = Some(probe_handle);

        Ok(())
    }

    pub async fn shutdown(&self) -> Result<()> {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.mark_retired();
        if let Some(tx) = self.shutdown_tx.lock().take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.probe_handle.lock().take() {
            handle.abort();
        }
        let topic_keys: Vec<_> = self.topics.iter().map(|entry| entry.key().clone()).collect();
        let mut producer_handles = Vec::with_capacity(topic_keys.len());
        for topic in topic_keys {
            if let Some((_, entry)) = self.topics.remove(&topic) {
                entry.producer_handle.abort();
                producer_handles.push(entry.producer_handle);
            }
        }
        for handle in producer_handles {
            let _ = handle.await;
        }
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
            max_topics: None,
            max_pending_records: None,
            max_pending_bytes: None,
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
            max_topics: None,
            max_pending_records: None,
            max_pending_bytes: None,
            linger_ms: None,
        };
        let auth = CredentialState::NotRequired;
        let result = KafkaLinkClient::from_config("default/kafka-main", &config, &auth);
        assert!(result.is_ok());
        let client = result.unwrap();
        assert!(!client.healthy()); // not initialized yet
        assert_eq!(client.channel_size, DEFAULT_KAFKA_CHANNEL_SIZE);
        assert_eq!(client.topic_budget.max_topics, DEFAULT_KAFKA_MAX_TOPICS);
        assert_eq!(client.queue_budget.max_records, DEFAULT_KAFKA_MAX_PENDING_RECORDS);
        assert_eq!(client.queue_budget.max_bytes, DEFAULT_KAFKA_MAX_PENDING_BYTES);
        assert_eq!(client.linger_ms, DEFAULT_KAFKA_LINGER_MS);
    }

    #[test]
    fn try_produce_rejects_retired_generation() {
        let config = KafkaClientConfig {
            brokers: vec!["broker1:9092".to_string()],
            sasl: None,
            tls: None,
            channel_size: None,
            max_topics: None,
            max_pending_records: None,
            max_pending_bytes: None,
            linger_ms: None,
        };
        let client = KafkaLinkClient::from_config("default/kafka-main", &config, &CredentialState::NotRequired)
            .expect("valid kafka config");
        client.mark_retired();
        let error = client
            .try_produce("logs", "payload".to_string())
            .err()
            .expect("retired generation must not accept produce");
        assert!(error.to_string().contains("retired"));
    }

    #[tokio::test]
    async fn shutdown_waits_for_pending_producer_tasks_to_terminate() {
        struct NotifyOnDrop(Option<tokio::sync::oneshot::Sender<()>>);

        impl Drop for NotifyOnDrop {
            fn drop(&mut self) {
                if let Some(tx) = self.0.take() {
                    let _ = tx.send(());
                }
            }
        }

        let config = KafkaClientConfig {
            brokers: vec!["broker1:9092".to_string()],
            sasl: None,
            tls: None,
            channel_size: None,
            max_topics: None,
            max_pending_records: None,
            max_pending_bytes: None,
            linger_ms: None,
        };
        let client = KafkaLinkClient::from_config("default/kafka-main", &config, &CredentialState::NotRequired)
            .expect("valid kafka config");
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
        let producer_handle = tokio::spawn(async move {
            let _on_drop = NotifyOnDrop(Some(stopped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        started_rx.await.expect("producer started");
        let (sender, _receiver) = mpsc::channel(1);
        let reservation = client.topic_budget.try_reserve().expect("topic slot");
        client.topics.insert(
            "logs".to_string(),
            TopicEntry {
                sender,
                producer_handle,
                _reservation: reservation,
            },
        );

        tokio::time::timeout(Duration::from_secs(1), client.shutdown())
            .await
            .expect("shutdown must terminate the pending producer")
            .expect("shutdown succeeds");
        stopped_rx.await.expect("producer was dropped before shutdown returned");
        assert!(client.topics.is_empty());
        assert_eq!(client.topic_budget.topics.load(Ordering::Acquire), 0);
    }

    #[test]
    fn test_kafka_link_client_from_config_rejects_zero_channel_size() {
        let config = KafkaClientConfig {
            brokers: vec!["broker1:9092".to_string()],
            sasl: None,
            tls: None,
            channel_size: Some(0),
            max_topics: None,
            max_pending_records: None,
            max_pending_bytes: None,
            linger_ms: None,
        };
        let result = KafkaLinkClient::from_config("default/kafka-main", &config, &CredentialState::NotRequired);

        let error = result.err().expect("zero channel size must be rejected");
        assert!(error.to_string().contains("channel size"));
    }

    #[test]
    fn test_kafka_link_client_from_config_rejects_linger_at_produce_timeout() {
        let config = KafkaClientConfig {
            brokers: vec!["broker1:9092".to_string()],
            sasl: None,
            tls: None,
            channel_size: None,
            max_topics: None,
            max_pending_records: None,
            max_pending_bytes: None,
            linger_ms: Some(PRODUCE_TIMEOUT.as_millis() as u64),
        };
        let result = KafkaLinkClient::from_config("default/kafka-main", &config, &CredentialState::NotRequired);

        let error = result.err().expect("linger at the produce timeout must be rejected");
        assert!(error.to_string().contains("produce timeout"));
    }

    #[test]
    fn queue_budget_enforces_record_limit_and_rolls_back_byte_reservation() {
        let budget = QueueBudget::new(1, 100);
        let first = budget.try_reserve(7).expect("first record must fit");

        assert!(matches!(budget.try_reserve(11), Err(QueueBudgetError::Records)));
        assert_eq!(budget.pending_records.load(Ordering::Acquire), 1);
        assert_eq!(budget.pending_bytes.load(Ordering::Acquire), 7);

        drop(first);
        assert_eq!(budget.pending_records.load(Ordering::Acquire), 0);
        assert_eq!(budget.pending_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn queue_budget_enforces_bytes_and_releases_on_drop() {
        let budget = QueueBudget::new(4, 5);
        let first = budget.try_reserve(3).expect("first payload must fit");
        let second = budget.try_reserve(2).expect("exact byte limit must fit");

        assert!(matches!(budget.try_reserve(1), Err(QueueBudgetError::Bytes)));
        drop(first);
        assert_eq!(budget.pending_records.load(Ordering::Acquire), 1);
        assert_eq!(budget.pending_bytes.load(Ordering::Acquire), 2);
        drop(second);
        assert_eq!(budget.pending_records.load(Ordering::Acquire), 0);
        assert_eq!(budget.pending_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn topic_budget_reuses_released_slots() {
        let budget = TopicBudget::new(1);
        let first = budget.try_reserve().expect("first topic must fit");

        assert!(budget.try_reserve().is_none());
        drop(first);
        assert!(budget.try_reserve().is_some());
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
}

#[cfg(test)]
mod tls_config_tests {
    use super::build_kafka_tls_config;
    use edgion_resources::constants::secret_keys::tls as tls_keys;
    use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn install_provider() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    fn gen_ca_leaf() -> (String, String, String) {
        let ca = rcgen::generate_simple_self_signed(vec!["kafka-ca".into()]).unwrap();
        let leaf = rcgen::generate_simple_self_signed(vec!["kafka.example.com".into()]).unwrap();
        (ca.cert.pem(), leaf.cert.pem(), leaf.signing_key.serialize_pem())
    }

    fn secret(data: &[(&str, &[u8])]) -> Secret {
        let m = data
            .iter()
            .map(|(k, v)| (k.to_string(), ByteString(v.to_vec())))
            .collect::<BTreeMap<_, _>>();
        Secret {
            data: Some(m),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        }
    }

    fn ca_ref(name: &str) -> OutboundTlsLocal {
        OutboundTlsLocal {
            // Pin verify explicitly so the test does not depend on the shared
            // process-global outbound default that other tests may mutate.
            verify: Some(true),
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                    group: String::new(),
                    kind: "Secret".into(),
                    name: name.into(),
                    namespace: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn tls_with(outbound: OutboundTlsLocal, ca: Option<Vec<Secret>>, client: Option<Secret>) -> TlsConfig {
        TlsConfig {
            enabled: true,
            outbound,
            resolved_ca_certificates: ca,
            resolved_client_certificate: client,
        }
    }

    /// enabled TLS with no custom CA and no client identity must still yield a
    /// verifying system-roots config (rskafka has no implicit default TLS).
    #[test]
    fn system_roots_fallback_when_no_material() {
        install_provider();
        let tls = tls_with(
            OutboundTlsLocal {
                verify: Some(true),
                ..Default::default()
            },
            None,
            None,
        );
        assert!(
            build_kafka_tls_config(&tls).is_ok(),
            "enabled TLS with no custom material must build a system-roots config"
        );
    }

    #[test]
    fn custom_ca_builds() {
        install_provider();
        let (ca, _c, _k) = gen_ca_leaf();
        let tls = tls_with(
            ca_ref("my-ca"),
            Some(vec![secret(&[(tls_keys::CA_CERT, ca.as_bytes())])]),
            None,
        );
        assert!(build_kafka_tls_config(&tls).is_ok());
    }

    #[test]
    fn ca_and_client_mtls_builds() {
        install_provider();
        let (ca, cert, key) = gen_ca_leaf();
        let tls = tls_with(
            ca_ref("my-ca"),
            Some(vec![secret(&[(tls_keys::CA_CERT, ca.as_bytes())])]),
            Some(secret(&[("tls.crt", cert.as_bytes()), ("tls.key", key.as_bytes())])),
        );
        let cfg = build_kafka_tls_config(&tls).expect("CA + mTLS identity must build");
        assert!(
            cfg.client_auth_cert_resolver.has_certs(),
            "client cert must be installed"
        );
    }

    /// rskafka's rustls connector always verifies (provider_can_disable=false),
    /// so verify=false must be rejected up front, never silently honored.
    #[test]
    fn verify_false_rejected() {
        install_provider();
        let tls = tls_with(
            OutboundTlsLocal {
                verify: Some(false),
                ..Default::default()
            },
            None,
            None,
        );
        assert!(build_kafka_tls_config(&tls).is_err());
    }
}
