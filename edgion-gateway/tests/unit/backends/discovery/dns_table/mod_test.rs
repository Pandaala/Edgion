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
    table.apply_result(
        &job,
        Ok(edgion_common::dns::DnsAnswer::without_ttl(vec!["10.0.0.1"
            .parse()
            .unwrap()])),
    );
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
