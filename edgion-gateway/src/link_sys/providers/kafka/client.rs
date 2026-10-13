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
use crate::link_sys::runtime::health_probe::{flip_log_allowed, new_published_health};
use edgion_common::link_sys_core::{outbound_tls::PreparedLinkSysTls, PublishedHealth};
use edgion_resources::resources::link_sys::kafka::{
    KafkaClientConfig, DEFAULT_KAFKA_CHANNEL_SIZE, DEFAULT_KAFKA_LINGER_MS, DEFAULT_KAFKA_MAX_PENDING_BYTES,
    DEFAULT_KAFKA_MAX_PENDING_RECORDS, DEFAULT_KAFKA_MAX_TOPICS, KAFKA_PRODUCE_TIMEOUT,
};
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

const MAX_BATCH_SIZE: usize = 1_000_000;
/// Consecutive probe failures before the probe rebuilds `inner_client` from the
/// configured bootstrap brokers (rskafka otherwise only reconnects to the
/// discovered topology, which never recovers a pod-IP/StatefulSet restart).
const KAFKA_REBUILD_THRESHOLD: u32 = 3;

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

/// Immutable connection input retained by initialization and probe recovery.
struct KafkaConnectionInput {
    owner: Arc<PreparedLinkSysTls>,
    auth: CredentialState<ResolvedCredential>,
    tls: Option<Arc<rustls::ClientConfig>>,
}

impl KafkaConnectionInput {
    fn config(&self) -> &KafkaClientConfig {
        let SystemConfig::Kafka(config) = &self.owner.owner().spec.config else {
            unreachable!("prepared Kafka provider is immutable")
        };
        config
    }

    fn client_builder(&self) -> ClientBuilder {
        let config = self.config();
        let mut builder = ClientBuilder::new(config.brokers.clone());
        if let Some(tls) = &self.tls {
            builder = builder.tls_config(Arc::clone(tls));
        }
        if let Some(sasl) = &config.sasl {
            if let CredentialState::Resolved(ResolvedCredential::Basic(basic)) = &self.auth {
                builder = builder.sasl_config(SaslConfig::Plain(Credentials::new(
                    sasl.username.clone(),
                    basic.password.clone(),
                )));
            }
        }
        builder
    }
}

pub struct KafkaLinkClient {
    name: String,
    connection: Arc<KafkaConnectionInput>,
    channel_size: usize,
    linger_ms: u64,
    inner_client: Arc<ArcSwapOption<rskafka::client::Client>>,
    topics: DashMap<String, TopicEntry>,
    /// Serializes new producer registration with retirement without locking existing-topic sends.
    producer_admission: parking_lot::Mutex<()>,
    topic_budget: Arc<TopicBudget>,
    queue_budget: Arc<QueueBudget>,
    healthy: Arc<PublishedHealth>,
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
    /// Build only from an actual immutable LinkSys owner.
    /// ```no_run
    /// use std::sync::Arc;
    /// use edgion_gateway::link_sys::providers::kafka::KafkaLinkClient;
    /// let owner = serde_json::from_value(serde_json::json!({"metadata":{"name":"kafka"},
    ///     "spec":{"type":"kafka","config":{"brokers":["kafka.example:9092"]}}})).unwrap();
    /// let client = KafkaLinkClient::from_link_sys(Arc::new(owner));
    /// ```
    /// ```compile_fail
    /// use edgion_gateway::link_sys::providers::kafka::KafkaLinkClient;
    /// use edgion_common::link_sys_core::credential::CredentialState;
    /// use edgion_resources::resources::link_sys::kafka::KafkaClientConfig;
    /// let config: KafkaClientConfig = serde_json::from_str(r#"{"brokers":["kafka.example:9092"]}"#).unwrap();
    /// let client = KafkaLinkClient::from_config("kafka", &config, &CredentialState::NotRequired);
    /// ```
    pub fn from_link_sys(owner: Arc<LinkSys>) -> Result<Self> {
        let candidate = super::apply::KafkaCandidate::new(owner)?;
        let actual = candidate.tls.owner();
        let name = format!(
            "{}/{}",
            actual.metadata.namespace.as_deref().unwrap_or("default"),
            actual.metadata.name.as_deref().unwrap_or_default()
        );
        Self::from_candidate(&name, &candidate)
    }

    pub(super) fn from_candidate(name: &str, candidate: &super::apply::KafkaCandidate) -> Result<Self> {
        let SystemConfig::Kafka(config) = &candidate.tls.owner().spec.config else {
            anyhow::bail!("Kafka construction requires an actual Kafka LinkSys");
        };
        if let Some(error) = config.validation_errors().into_iter().next() {
            anyhow::bail!("{} for '{}'", error, name);
        }
        let channel_size = config.channel_size.unwrap_or(DEFAULT_KAFKA_CHANNEL_SIZE);
        let max_topics = config.max_topics.unwrap_or(DEFAULT_KAFKA_MAX_TOPICS);
        let max_pending_records = config.max_pending_records.unwrap_or(DEFAULT_KAFKA_MAX_PENDING_RECORDS);
        let max_pending_bytes = config.max_pending_bytes.unwrap_or(DEFAULT_KAFKA_MAX_PENDING_BYTES);
        let linger_ms = config.linger_ms.unwrap_or(DEFAULT_KAFKA_LINGER_MS);
        let auth = crate::link_sys::credential::resolved_state(candidate.tls.owner());
        anyhow::ensure!(auth.is_buildable(), "Kafka authentication is {}", auth.kind());
        let tls = edgion_common::link_sys_core::tls::build_kafka_rustls_client_config(&candidate.tls)?.map(Arc::new);
        Ok(Self {
            name: name.to_string(),
            connection: Arc::new(KafkaConnectionInput {
                owner: Arc::clone(&candidate.tls),
                auth,
                tls,
            }),
            channel_size,
            linger_ms,
            inner_client: Arc::new(ArcSwapOption::empty()),
            topics: DashMap::new(),
            producer_admission: parking_lot::Mutex::new(()),
            topic_budget: TopicBudget::new(max_topics),
            queue_budget: QueueBudget::new(max_pending_records, max_pending_bytes),
            healthy: new_published_health(name, false),
            last_flip_log_ms: Arc::new(AtomicU64::new(0)),
            shutdown_tx: parking_lot::Mutex::new(None),
            probe_handle: parking_lot::Mutex::new(None),
            fingerprint: candidate.fingerprint().clone(),
            retired: Arc::new(AtomicBool::new(false)),
            shutdown_started: AtomicBool::new(false),
        })
    }

    pub(crate) fn runtime_fingerprint(&self) -> &RuntimeFingerprint {
        &self.fingerprint
    }

    pub(crate) fn connection_coverage(&self) -> ConnectionCoverage {
        let configured = self.connection.config().brokers.len().max(1);
        let live = if self.inner_client.load_full().is_some() && self.healthy() {
            self.connection.config().brokers.len()
        } else {
            0
        };
        ConnectionCoverage { live, configured }
    }

    pub(crate) fn mark_retired(&self) {
        let _admission = self.producer_admission.lock();
        self.healthy.deactivate();
        self.retired.store(true, Ordering::Relaxed);
        if let Some(handle) = self.probe_handle.lock().take() {
            handle.abort();
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn published_health(&self) -> &PublishedHealth {
        &self.healthy
    }

    /// SDK reconnects and probe recovery retain the constructed connector and
    /// actual owner; only SDK-required broker/credential strings are copied.
    async fn build_kafka_client(input: &KafkaConnectionInput) -> Result<rskafka::client::Client> {
        input
            .client_builder()
            .build()
            .await
            .map_err(|e| anyhow::anyhow!("Kafka client build failed: {}", e))
    }

    pub fn healthy(&self) -> bool {
        self.healthy.load()
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
                let producer_handle = tokio::spawn(async move {
                    Self::run_producer_task(client, topic_owned, rx, linger_ms, name, healthy, last_flip_log_ms).await;
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
                // KAFKA_PRODUCE_TIMEOUT wedge). Evict only after task completion so
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
        healthy: Arc<PublishedHealth>,
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
                healthy.update(false, false);
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
            let result = tokio::time::timeout(KAFKA_PRODUCE_TIMEOUT, producer.produce(record)).await;
            drop(reservation);
            match result {
                Ok(Ok(_)) => {
                    if healthy.update(true, false) && flip_log_allowed(&last_flip_log_ms) {
                        tracing::info!(kafka = %name, %topic, "Kafka recovered: produce succeeded");
                    }
                }
                Ok(Err(e)) => {
                    if healthy.update(false, false) && flip_log_allowed(&last_flip_log_ms) {
                        tracing::warn!(kafka = %name, %topic, "Kafka degraded: produce failed: {}", e);
                    }
                }
                Err(_elapsed) => {
                    // Produce wedged (stale/dead client). Exit so the channel closes
                    // and the next produce respawns this topic on the current
                    // inner_client (picks up a probe rebuild).
                    // The in-flight record and any messages still buffered in rx are lost
                    // when the task exits (degraded behavior, acceptable for a fire-and-forget sink).
                    healthy.update(false, false);
                    tracing::warn!(
                        kafka = %name, %topic,
                        "Kafka produce wedged > {}s; exiting producer task to force respawn",
                        KAFKA_PRODUCE_TIMEOUT.as_secs()
                    );
                    return;
                }
            }
        }
    }
}

impl KafkaLinkClient {
    pub async fn init(&self) -> Result<()> {
        let client = Self::build_kafka_client(&self.connection).await?;
        self.inner_client.store(Some(Arc::new(client)));
        self.healthy.update(true, false);

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
                let connection = Arc::clone(&self.connection);
                let name = self.name.clone();
                let fail_count = Arc::new(AtomicU32::new(0));
                let retired = Arc::clone(&self.retired);
                let healthy = Arc::clone(&self.healthy);
                move || {
                    let inner = Arc::clone(&inner);
                    let connection = Arc::clone(&connection);
                    let name = name.clone();
                    let fail_count = Arc::clone(&fail_count);
                    let retired = Arc::clone(&retired);
                    let healthy = Arc::clone(&healthy);
                    async move {
                        if retired.load(Ordering::Relaxed) {
                            // Echo the current flag so apply_health_result is a no-op
                            // and cannot emit a retired transition event.
                            return healthy.load();
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
                            match Self::build_kafka_client(&connection).await {
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
        self.healthy.update(false, false);
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/kafka/client_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/kafka/client_test/tls_config_tests.rs"]
mod tls_config_tests;
