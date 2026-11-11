use super::*;
use k8s_openapi::api::core::v1::{ServicePort, ServiceSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

#[derive(Default)]
struct NullSink;

impl SliceSink for NullSink {
    fn publish_batch(&self, _: Vec<(String, Vec<EndpointSlice>)>) {}

    fn published_service_keys(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
}

fn table() -> &'static DnsEndpointTable {
    Box::leak(Box::new(DnsEndpointTable::new(Box::new(NullSink), Box::new(|_| None))))
}

fn services() -> HashMap<String, Arc<Service>> {
    HashMap::from([(
        "ns/a".to_string(),
        Arc::new(Service {
            metadata: ObjectMeta {
                name: Some("a".to_string()),
                namespace: Some("ns".to_string()),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                type_: Some("ExternalName".to_string()),
                external_name: Some("a.example.com".to_string()),
                ports: Some(vec![ServicePort {
                    port: 80,
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }),
    )])
}

#[tokio::test]
async fn wait_returns_immediately_without_pending_targets() {
    let table = table();
    let started = std::time::Instant::now();
    table.wait_first_answer("ns/a").await;
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[tokio::test]
async fn wait_returns_for_services_the_table_does_not_own() {
    let table = table();
    table.on_services_full(&services());
    assert_eq!(table.stats().pending_entries, 1);
    let started = std::time::Instant::now();
    table.wait_first_answer("ns/other").await;
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[tokio::test(start_paused = true)]
async fn wait_wakes_on_the_first_answer() {
    let table = table();
    table.on_services_full(&services());
    let job = table.state.lock().take_due(Instant::now(), 1).remove(0);
    let waiter = tokio::spawn(table.wait_first_answer("ns/a"));
    tokio::task::yield_now().await;
    table.apply_results(vec![(
        job,
        Ok(edgion_common::dns::DnsAnswer::without_ttl(vec!["10.0.0.1"
            .parse()
            .unwrap()])),
    )]);
    let started = Instant::now();
    waiter.await.unwrap();
    assert!(
        started.elapsed() < FIRST_ANSWER_WAIT,
        "woken by the answer, not the timeout"
    );
    assert_eq!(table.stats().pending_entries, 0);
}

#[tokio::test(start_paused = true)]
async fn wait_is_bounded() {
    let table = table();
    table.on_services_full(&services());
    let started = Instant::now();
    table.wait_first_answer("ns/a").await;
    assert_eq!(started.elapsed(), FIRST_ANSWER_WAIT);
}

#[tokio::test]
async fn removing_the_service_releases_waiters() {
    let table = table();
    table.on_services_full(&services());
    let waiter = tokio::spawn(table.wait_first_answer("ns/a"));
    tokio::task::yield_now().await;
    table.on_services_full(&HashMap::new());
    tokio::time::timeout(Duration::from_millis(500), waiter)
        .await
        .expect("released before the wait bound")
        .unwrap();
    assert_eq!(table.stats(), DnsEndpointTableStats::default());
}

#[tokio::test(start_paused = true)]
async fn another_service_leaving_a_shared_entry_does_not_end_the_wait() {
    let table = table();
    let mut both = services();
    let mut b = (*both["ns/a"]).clone();
    b.metadata.name = Some("b".to_string());
    both.insert("ns/b".to_string(), Arc::new(b));
    table.on_services_full(&both);
    assert_eq!(table.stats().entries, 1, "a and b share one pending target");
    let started = Instant::now();
    let waiter = tokio::spawn(table.wait_first_answer("ns/a"));
    tokio::task::yield_now().await;
    table.on_services_full(&services());
    waiter.await.unwrap();
    assert_eq!(
        started.elapsed(),
        FIRST_ANSWER_WAIT,
        "a still waits for its own (still pending) target"
    );
}

#[test]
fn batched_publications_merge_per_service() {
    let sink = NullSink;
    let publisher = BatchingPublisher {
        sink: &sink,
        batch: Mutex::new(PublishBatch::default()),
    };
    let slice = EndpointSlice::default();
    publisher.publish("ns/a", Vec::new());
    publisher.publish("ns/b", vec![slice.clone()]);
    publisher.publish("ns/a", vec![slice.clone()]);
    let batch = publisher.batch.into_inner().entries;
    assert_eq!(batch.len(), 2, "one entry per Service");
    assert_eq!(batch[0].0, "ns/a");
    assert_eq!(batch[0].1.len(), 1, "the last publication wins");
}

fn external_service(name: &str, hostname: &str, annotations: &[(&str, &str)]) -> (String, Arc<Service>) {
    let annotations = annotations
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    (
        format!("ns/{name}"),
        Arc::new(Service {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some("ns".to_string()),
                annotations: Some(annotations),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                type_: Some("ExternalName".to_string()),
                external_name: Some(hostname.to_string()),
                ports: Some(vec![ServicePort {
                    port: 80,
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }),
    )
}

#[test]
fn a_switch_finishing_in_a_result_batch_rebinds_refused_services() {
    use edgion_resources::constants::annotations::edgion::DNS_RESOLVER;
    use edgion_resources::resources::edgion_gateway_config::DnsResolverConfig;

    let table = table();
    let inputs = TableInputs {
        policy: DnsRefreshPolicy::from_config(&DnsResolverConfig {
            max_entries: 2,
            ..Default::default()
        }),
        ..Default::default()
    };
    table.mutate(|state, lookup, publisher, now| state.set_inputs(inputs, lookup, publisher, now));
    let answered = || Ok(DnsAnswer::without_ttl(vec!["10.0.0.1".parse().unwrap()]));
    let upsert = |service: (String, Arc<Service>)| {
        table.on_services_partial(&HashMap::from([service]), &HashMap::new(), &HashSet::new());
    };

    upsert(external_service("a", "a.example.com", &[]));
    let job = table.state.lock().take_due(Instant::now(), 1).remove(0);
    table.apply_results(vec![(job, answered())]);
    // A resolver switch bridges on the old entry, filling the table.
    upsert(external_service("a", "a.example.com", &[(DNS_RESOLVER, "192.0.2.53")]));
    upsert(external_service("c", "c.example.com", &[]));
    assert_eq!(table.stats().rejected_services, 1);
    let job = table.state.lock().take_due(Instant::now(), 1).remove(0);
    table.apply_results(vec![(job, answered())]);
    assert_eq!(
        table.stats().rejected_services,
        0,
        "the slot the bridge freed is offered"
    );
    assert_eq!(table.stats().entries, 2);
}
