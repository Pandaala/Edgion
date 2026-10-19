use super::*;
use edgion_resources::resources::link_sys::kafka::KafkaClientConfig;

impl KafkaLinkClient {
    pub(in crate::link_sys::providers::kafka) fn assert_retained_connection_input(
        &self,
        prepared: &Arc<PreparedLinkSysTls>,
        actual: &LinkSys,
    ) {
        let input = Arc::clone(&self.connection);
        assert!(Arc::ptr_eq(&input, &self.connection));
        assert!(Arc::ptr_eq(&input.owner, prepared));
        assert!(std::ptr::eq(input.owner.owner(), actual));
        let tls = Arc::clone(input.tls.as_ref().expect("enabled TLS has a connector"));
        for _ in 0..2 {
            let next = Arc::clone(&input);
            let _ = next.client_builder();
            assert!(Arc::ptr_eq(&tls, next.tls.as_ref().unwrap()));
            assert!(Arc::ptr_eq(&next.owner, prepared));
        }
    }
}

fn from_test_config(
    name: &str,
    config: &KafkaClientConfig,
    auth: &CredentialState<ResolvedCredential>,
) -> Result<KafkaLinkClient> {
    assert!(matches!(auth, CredentialState::NotRequired));
    let (namespace, name) = name.split_once('/').unwrap_or(("default", name));
    let owner = serde_json::from_value(serde_json::json!({
        "metadata":{"name":name,"namespace":namespace},
        "spec":{"type":"kafka","config":config}
    }))
    .unwrap();
    KafkaLinkClient::from_link_sys(Arc::new(owner))
}

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
    let result = from_test_config("test", &config, &auth);
    assert!(result.is_err());
    // Use `.err().unwrap()` instead of `.unwrap_err()` to avoid requiring
    // `KafkaLinkClient: Debug`.
    assert_eq!(
        result.err().unwrap().to_string(),
        "Kafka configuration has no brokers for 'default/test'"
    );
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
    let result = from_test_config("default/kafka-main", &config, &auth);
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
    let client =
        from_test_config("default/kafka-main", &config, &CredentialState::NotRequired).expect("valid kafka config");
    client.mark_retired();
    let error = client
        .try_produce("logs", "payload".to_string())
        .expect_err("retired generation must not accept produce");
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
    let client =
        from_test_config("default/kafka-main", &config, &CredentialState::NotRequired).expect("valid kafka config");
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
    let result = from_test_config("default/kafka-main", &config, &CredentialState::NotRequired);

    let error = result.err().expect("zero channel size must be rejected");
    assert_eq!(
        error.to_string(),
        "Kafka channelSize must be greater than zero for 'default/kafka-main'"
    );
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
        linger_ms: Some(KAFKA_PRODUCE_TIMEOUT.as_millis().try_into().unwrap()),
    };
    let result = from_test_config("default/kafka-main", &config, &CredentialState::NotRequired);

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
fn producer_health_flips_only_on_change() {
    let flag = PublishedHealth::new(false);
    // false -> success -> flips to true
    assert!(flag.update(true, false));
    assert!(flag.load());
    // already true + success -> no flip
    assert!(!flag.update(true, false));
    // true -> failure -> flips to false
    assert!(flag.update(false, false));
    assert!(!flag.load());
    // already false + failure -> no flip
    assert!(!flag.update(false, false));
}

#[test]
fn producer_health_single_winner_under_contention() {
    use std::sync::Arc;
    let flag = Arc::new(PublishedHealth::new(false));
    let mut handles = Vec::new();
    for _ in 0..16 {
        let f = Arc::clone(&flag);
        handles.push(std::thread::spawn(move || f.update(true, false)));
    }
    let winners = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|&won| won)
        .count();
    assert_eq!(winners, 1, "exactly one task may win the false->true flip");
    assert!(flag.load());
}

#[test]
fn owning_constructor_rejects_pending_and_invalid_sasl_instead_of_anonymous_connection() {
    for secret in [
        serde_json::Value::Null,
        serde_json::json!({"metadata":{"name":"password","namespace":"default"},"data":{}}),
    ] {
        let owner = serde_json::from_value(serde_json::json!({
            "metadata":{"name":"kafka"},"spec":{"type":"kafka","config":{
                "brokers":["kafka.example:9092"],"sasl":{"username":"alice",
                    "password":{"secretRef":{"name":"password"},"secret":secret}}
            }}
        }))
        .unwrap();
        let error = KafkaLinkClient::from_link_sys(Arc::new(owner))
            .err()
            .expect("unusable authentication must fail");
        assert!(error.to_string().contains("authentication"));
    }
}

#[test]
fn constructor_matches_shared_kafka_linger_admission() {
    for (linger, valid) in [
        (None, true),
        (Some(0), true),
        (Some(29999), true),
        (Some(30000), false),
        (Some(u64::MAX), false),
    ] {
        let owner: LinkSys = serde_json::from_value(serde_json::json!({
            "metadata": {"namespace": "default", "name": "kafka"},
            "spec": {"type": "kafka", "config": {"brokers": ["broker.example:9092"], "lingerMs": linger}}
        }))
        .unwrap();
        let errors = owner.validate_config();
        assert_eq!(errors.is_empty(), valid, "linger {linger:?}");
        let result = KafkaLinkClient::from_link_sys(Arc::new(owner));
        if valid {
            assert_eq!(result.unwrap().linger_ms, linger.unwrap_or(DEFAULT_KAFKA_LINGER_MS));
        } else {
            assert_eq!(
                result.err().unwrap().to_string(),
                format!("{} for 'default/kafka'", errors[0])
            );
        }
    }
}

#[test]
fn constructor_preserves_first_error_order_for_invalid_scalar_fields() {
    let mut owner: LinkSys = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "default", "name": "kafka"},
        "spec": {"type": "kafka", "config": {"brokers": [], "channelSize": 0, "maxTopics": 0,
            "maxPendingRecords": 0, "maxPendingBytes": 0, "lingerMs": 30000}}
    }))
    .unwrap();
    let expected = [
        "Kafka configuration has no brokers",
        "Kafka channelSize must be greater than zero",
        "Kafka maxTopics must be greater than zero",
        "Kafka maxPendingRecords must be greater than zero",
        "Kafka maxPendingBytes must be greater than zero",
        "Kafka lingerMs must be shorter than the 30 second produce timeout",
    ];
    for (index, message) in expected.iter().enumerate() {
        assert_eq!(owner.validate_config(), expected[index..]);
        let error = KafkaLinkClient::from_link_sys(Arc::new(owner.clone())).err().unwrap();
        assert_eq!(error.to_string(), format!("{message} for 'default/kafka'"));
        let SystemConfig::Kafka(config) = &mut owner.spec.config else {
            unreachable!()
        };
        match index {
            0 => config.brokers.push("broker.example:9092".into()),
            1 => config.channel_size = None,
            2 => config.max_topics = None,
            3 => config.max_pending_records = None,
            4 => config.max_pending_bytes = None,
            5 => config.linger_ms = None,
            _ => unreachable!(),
        }
    }
    assert!(KafkaLinkClient::from_link_sys(Arc::new(owner)).is_ok());
}

#[tokio::test]
async fn existing_topic_sender_borrows_key_and_preserves_channel() {
    let config: KafkaClientConfig = serde_json::from_value(serde_json::json!({
        "brokers": ["broker.example:9092"]
    }))
    .unwrap();
    let client = from_test_config("default/kafka-hit", &config, &CredentialState::NotRequired).unwrap();
    let (sender, receiver) = mpsc::channel(1);
    let producer_handle = tokio::spawn(std::future::pending::<()>());
    let reservation = client.topic_budget.try_reserve().unwrap();
    client.topics.insert(
        "existing-log-topic".into(),
        TopicEntry {
            sender: sender.clone(),
            producer_handle,
            _reservation: reservation,
        },
    );
    let mut snapshot = None;
    let (result, open_allocations) =
        crate::test_allocation_counter::count_allocations(|| client.topic_sender("existing-log-topic", &mut snapshot));
    let open = result.unwrap();
    assert!(open.same_channel(&sender));
    assert!(!open.is_closed());
    drop(receiver);
    let (result, closed_allocations) =
        crate::test_allocation_counter::count_allocations(|| client.topic_sender("existing-log-topic", &mut snapshot));
    let closed = result.unwrap();
    assert!(closed.same_channel(&sender));
    assert!(closed.is_closed());
    assert_eq!(client.topics.len(), 1);
    assert_eq!(client.topic_budget.topics.load(Ordering::Acquire), 1);
    client.shutdown().await.unwrap();
    assert!(client.topics.is_empty());
    assert_eq!(client.topic_budget.topics.load(Ordering::Acquire), 0);
    assert_eq!(open_allocations, 0);
    assert_eq!(closed_allocations, 0);
}
