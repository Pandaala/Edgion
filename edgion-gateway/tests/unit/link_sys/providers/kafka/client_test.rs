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
