//! Kafka LinkSys runtime client.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

pub struct KafkaLinkClient {
    name: String,
    config: KafkaClientConfig,
    auth: CredentialState<ResolvedCredential>,
    channel_size: usize,
    linger_ms: u64,
    inner_client: ArcSwapOption<rskafka::client::Client>,
    topics: DashMap<String, mpsc::Sender<String>>,
    healthy: Arc<AtomicBool>,
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
                tokio::spawn(async move {
                    Self::run_producer_task(client, topic_owned, rx, linger_ms, name, healthy).await;
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
            if let Err(e) = producer.produce(record).await {
                tracing::warn!(kafka = %name, %topic, "produce failed: {}", e);
                healthy.store(false, Ordering::Relaxed);
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
}
