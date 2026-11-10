use super::*;
use edgion_resources::resources::edgion_gateway_config::DnsResolverConfig;
use k8s_openapi::api::core::v1::{ServicePort, ServiceSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use parking_lot::Mutex as StdMutex;
use std::collections::BTreeMap;

#[derive(Default)]
struct Recorder {
    current: StdMutex<BTreeMap<String, Vec<EndpointSlice>>>,
    calls: StdMutex<usize>,
}

impl Recorder {
    fn calls(&self) -> usize {
        *self.calls.lock()
    }

    fn addresses(&self, service_key: &str) -> Vec<String> {
        self.current
            .lock()
            .get(service_key)
            .map(|slices| {
                slices
                    .iter()
                    .flat_map(|slice| slice.endpoints.iter())
                    .flat_map(|endpoint| endpoint.addresses.iter().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn ports(&self, service_key: &str) -> Vec<i32> {
        self.current.lock()[service_key][0]
            .ports
            .as_ref()
            .unwrap()
            .iter()
            .filter_map(|port| port.port)
            .collect()
    }
}

impl SlicePublisher for Recorder {
    fn publish(&self, service_key: &str, slices: Vec<EndpointSlice>) {
        *self.calls.lock() += 1;
        let mut current = self.current.lock();
        if slices.is_empty() {
            current.remove(service_key);
        } else {
            current.insert(service_key.to_string(), slices);
        }
    }

    fn published_service_keys(&self) -> BTreeSet<String> {
        self.current.lock().keys().cloned().collect()
    }
}

fn no_httpdns(_: &LinkSysKey) -> Option<Arc<PreparedHttpDns>> {
    None
}

fn lookup() -> &'static HttpDnsLookup {
    &no_httpdns
}

fn external_name(name: &str, hostname: &str, annotations: &[(&str, &str)], ports: &[i32]) -> Arc<Service> {
    Arc::new(Service {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some("ns".to_string()),
            annotations: (!annotations.is_empty()).then(|| {
                annotations
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect()
            }),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            type_: Some("ExternalName".to_string()),
            external_name: Some(hostname.to_string()),
            ports: Some(
                ports
                    .iter()
                    .map(|port| ServicePort {
                        port: *port,
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn key(name: &str) -> String {
    format!("ns/{name}")
}

fn answer(addrs: &[&str], ttl: Option<u64>) -> Result<DnsAnswer, DnsError> {
    Ok(DnsAnswer {
        addrs: addrs.iter().map(|addr| addr.parse().unwrap()).collect(),
        ttl: ttl.map(Duration::from_secs),
    })
}

fn inputs_with(config: DnsResolverConfig) -> TableInputs {
    TableInputs {
        policy: DnsRefreshPolicy::from_config(&config),
        ..Default::default()
    }
}

struct Harness {
    state: TableState,
    recorder: Recorder,
    now: Instant,
}

impl Harness {
    fn new() -> Self {
        Self::with_inputs(TableInputs::default())
    }

    fn with_inputs(inputs: TableInputs) -> Self {
        Self {
            state: TableState::new(inputs),
            recorder: Recorder::default(),
            now: Instant::now(),
        }
    }

    fn upsert(&mut self, name: &str, service: Arc<Service>) {
        self.state.update_services(
            vec![(key(name), service)],
            Vec::new(),
            lookup(),
            &self.recorder,
            self.now,
        );
    }

    fn remove(&mut self, name: &str) {
        self.state
            .update_services(Vec::new(), vec![key(name)], lookup(), &self.recorder, self.now);
    }

    fn advance(&mut self, secs: u64) {
        self.now += Duration::from_secs(secs);
    }

    /// Resolve every due target with the same outcome.
    fn resolve_due(&mut self, outcome: Result<DnsAnswer, DnsError>) -> usize {
        let jobs = self.state.take_due(self.now, usize::MAX);
        for job in &jobs {
            self.state
                .apply_result(job, outcome.clone(), self.now, 0.5, &self.recorder);
        }
        jobs.len()
    }
}

#[test]
fn gateway_mode_is_the_default_and_publishes_the_full_sorted_set() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "Api.Example.com.", &[], &[443]));
    assert_eq!(h.state.stats().entries, 1);
    assert_eq!(h.state.stats().pending_entries, 1);
    assert_eq!(h.recorder.calls(), 0, "nothing to publish before the first answer");
    assert!(h.state.pending_waiters().contains_key(&key("a")));

    assert_eq!(
        h.resolve_due(answer(&["10.0.0.3", "10.0.0.1", "10.0.0.2"], Some(60))),
        1
    );
    assert_eq!(
        h.recorder.addresses(&key("a")),
        vec!["10.0.0.1", "10.0.0.2", "10.0.0.3"]
    );
    assert_eq!(h.state.stats().pending_entries, 0);
    assert!(!h.state.pending_waiters().contains_key(&key("a")));
    let view = h.state.entries_view(h.now, 10);
    assert_eq!(view[0].hostname, "api.example.com", "hostname normalized");
    assert_eq!(view[0].status, EntryStatus::Ready);
    assert_eq!(view[0].next_refresh_in_secs, 30, "TTL 60 x 0.5");
}

#[test]
fn unchanged_answer_in_another_order_publishes_nothing() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "api.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1", "10.0.0.2"], Some(10)));
    let calls = h.recorder.calls();
    h.advance(10);
    assert_eq!(
        h.resolve_due(answer(&["10.0.0.2", "10.0.0.1", "10.0.0.2"], Some(10))),
        1
    );
    assert_eq!(
        h.recorder.calls(),
        calls,
        "unchanged set must not touch the coordinator"
    );
}

#[test]
fn changed_answer_republishes() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "api.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], Some(10)));
    h.advance(10);
    h.resolve_due(answer(&["10.0.0.9"], Some(10)));
    assert_eq!(h.recorder.addresses(&key("a")), vec!["10.0.0.9"]);
}

#[test]
fn deleting_the_service_withdraws_slices_and_deletes_the_entry() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "api.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    h.remove("a");
    assert!(h.recorder.addresses(&key("a")).is_empty());
    assert_eq!(h.state.stats(), DnsEndpointTableStats::default());
    assert!(h.state.next_due().is_none());
}

#[test]
fn services_sharing_a_target_share_one_entry() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "api.example.com", &[], &[80]));
    h.upsert("b", external_name("b", "api.example.com", &[], &[8080]));
    assert_eq!(h.state.stats().entries, 1);
    assert_eq!(h.resolve_due(answer(&["10.0.0.1"], None)), 1, "one query for both");
    assert_eq!(h.recorder.ports(&key("a")), vec![80]);
    assert_eq!(h.recorder.ports(&key("b")), vec![8080]);

    h.remove("a");
    assert_eq!(h.state.stats().entries, 1, "still referenced by b");
    assert_eq!(h.recorder.addresses(&key("b")), vec!["10.0.0.1"]);
    h.remove("b");
    assert_eq!(h.state.stats().entries, 0);
}

#[test]
fn a_later_service_on_a_ready_target_publishes_at_once() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "api.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    h.upsert("b", external_name("b", "api.example.com", &[], &[80]));
    assert_eq!(h.recorder.addresses(&key("b")), vec!["10.0.0.1"]);
    assert!(!h.state.pending_waiters().contains_key(&key("b")));
}

#[test]
fn relist_sweeps_services_missing_from_the_snapshot() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.upsert("b", external_name("b", "b.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    let snapshot = HashMap::from([(key("b"), external_name("b", "b.example.com", &[], &[80]))]);
    h.state.replace_services(&snapshot, lookup(), &h.recorder, h.now);
    assert!(h.recorder.addresses(&key("a")).is_empty());
    assert_eq!(h.recorder.addresses(&key("b")), vec!["10.0.0.1"]);
    assert_eq!(h.state.stats().entries, 1);

    h.state.replace_services(&HashMap::new(), lookup(), &h.recorder, h.now);
    assert_eq!(h.state.stats(), DnsEndpointTableStats::default());
    assert!(h.recorder.published_service_keys().is_empty());
}

#[test]
fn other_modes_and_invalid_annotations_are_not_owned() {
    let mut h = Harness::new();
    h.upsert(
        "ctl",
        external_name("ctl", "a.example.com", &[(DNS_RESOLUTION_MODE, "controller")], &[80]),
    );
    h.upsert(
        "sys",
        external_name("sys", "a.example.com", &[(DNS_RESOLUTION_MODE, "system")], &[80]),
    );
    h.upsert(
        "bad",
        external_name("bad", "a.example.com", &[(DNS_RESOLUTION_MODE, "per-request")], &[80]),
    );
    h.upsert(
        "family",
        external_name("family", "a.example.com", &[(DNS_LOOKUP_FAMILY, "v4")], &[80]),
    );
    let mut cluster_ip = (*external_name("ip", "a.example.com", &[], &[80])).clone();
    cluster_ip.spec.as_mut().unwrap().type_ = Some("ClusterIP".to_string());
    h.upsert("ip", Arc::new(cluster_ip));
    assert_eq!(h.state.stats().entries, 0);
    assert!(h.state.take_due(h.now, usize::MAX).is_empty());
}

#[test]
fn endpoint_table_switch_off_releases_and_annotation_still_opts_in() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.upsert(
        "b",
        external_name("b", "b.example.com", &[(DNS_RESOLUTION_MODE, "gateway")], &[80]),
    );
    h.resolve_due(answer(&["10.0.0.1"], None));
    assert_eq!(h.state.stats().entries, 2);

    let off = inputs_with(DnsResolverConfig {
        endpoint_table: false,
        ..Default::default()
    });
    h.state.set_inputs(off, lookup(), &h.recorder, h.now);
    assert!(
        h.recorder.addresses(&key("a")).is_empty(),
        "a falls back to system mode"
    );
    assert_eq!(
        h.recorder.addresses(&key("b")),
        vec!["10.0.0.1"],
        "explicit gateway mode stays"
    );
    assert_eq!(h.state.stats().entries, 1);

    h.state.set_inputs(TableInputs::default(), lookup(), &h.recorder, h.now);
    assert_eq!(h.state.stats().entries, 2, "switching back re-binds a");
}

#[test]
fn hostname_change_withdraws_and_rebinds() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "old.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    h.upsert("a", external_name("a", "new.example.com", &[], &[80]));
    assert!(
        h.recorder.addresses(&key("a")).is_empty(),
        "old addresses withdrawn at once"
    );
    let view = h.state.entries_view(h.now, 10);
    assert_eq!(view.len(), 1);
    assert_eq!(view[0].hostname, "new.example.com");
    assert_eq!(view[0].status, EntryStatus::Pending);
}

#[test]
fn port_change_republishes_without_requery() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    h.upsert("a", external_name("a", "a.example.com", &[], &[8080]));
    assert_eq!(h.recorder.ports(&key("a")), vec![8080]);
    assert!(h.state.take_due(h.now, usize::MAX).is_empty(), "no extra resolution");
}

#[test]
fn identical_service_update_publishes_nothing() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    let calls = h.recorder.calls();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    assert_eq!(h.recorder.calls(), calls);
}

#[test]
fn authoritative_empty_answer_withdraws_immediately() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    h.advance(5);
    h.resolve_due(Err(DnsError::NoAddresses));
    assert!(h.recorder.addresses(&key("a")).is_empty());
    let view = h.state.entries_view(h.now, 10);
    assert_eq!(view[0].status, EntryStatus::Gone);
    assert_eq!(h.state.stats().entries, 1, "entry lives while referenced");
}

#[test]
fn transient_failure_keeps_last_good_until_max_stale() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    h.advance(5);
    h.resolve_due(Err(DnsError::Io("timeout".to_string())));
    assert_eq!(h.recorder.addresses(&key("a")), vec!["10.0.0.1"], "serve stale");
    assert_eq!(h.state.entries_view(h.now, 10)[0].status, EntryStatus::Failing);
    assert_eq!(
        h.state.entries_view(h.now, 10)[0].next_refresh_in_secs,
        5,
        "first back-off is minRefresh"
    );
    h.advance(300);
    h.resolve_due(Err(DnsError::Io("timeout".to_string())));
    assert_eq!(
        h.recorder.addresses(&key("a")),
        vec!["10.0.0.1"],
        "maxStale counts from the first failure"
    );
    // The next retry (after the doubled back-off) lands past maxStale.
    h.advance(10);
    assert_eq!(h.resolve_due(Err(DnsError::Io("timeout".to_string()))), 1);
    assert!(
        h.recorder.addresses(&key("a")).is_empty(),
        "withdrawn once past maxStale"
    );
}

#[test]
fn a_long_ttl_refresh_still_gets_the_full_stale_tolerance() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    // TTL 3600 x 0.5 is clamped to maxRefresh (300 s) = maxStale.
    h.resolve_due(answer(&["10.0.0.1"], Some(3600)));
    h.advance(300);
    assert_eq!(h.resolve_due(Err(DnsError::Io("servfail".to_string()))), 1);
    assert_eq!(h.recorder.addresses(&key("a")), vec!["10.0.0.1"]);
}

fn with_resolver(name: &str, resolver: &str) -> Arc<Service> {
    external_name(
        name,
        "a.example.com",
        &[(edgion_resources::constants::annotations::edgion::DNS_RESOLVER, resolver)],
        &[80],
    )
}

#[test]
fn resolver_switch_keeps_the_previous_answer_until_the_new_target_answers() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    let calls = h.recorder.calls();
    h.upsert("a", with_resolver("a", "192.0.2.53"));
    assert_eq!(h.recorder.addresses(&key("a")), vec!["10.0.0.1"], "no withdrawal");
    assert_eq!(h.state.stats().entries, 2, "the previous entry stays referenced");
    assert!(
        !h.state.pending_waiters().contains_key(&key("a")),
        "a Service still serving its previous answer never waits"
    );
    assert_eq!(
        h.resolve_due(answer(&["10.0.0.2"], None)),
        1,
        "only the new target is due"
    );
    assert_eq!(h.recorder.addresses(&key("a")), vec!["10.0.0.2"]);
    assert_eq!(h.state.stats().entries, 1, "the previous entry is released");
    assert!(h.recorder.calls() > calls);
}

#[test]
fn a_failing_new_resolver_takes_effect_at_its_first_result() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    h.upsert("a", with_resolver("a", "192.0.2.53"));
    assert_eq!(h.recorder.addresses(&key("a")), vec!["10.0.0.1"]);
    h.resolve_due(Err(DnsError::Io("unreachable".to_string())));
    assert!(
        h.recorder.addresses(&key("a")).is_empty(),
        "a broken new resolver is never masked by the previous answer"
    );
    assert_eq!(h.state.stats().entries, 1, "the previous entry is released");
}

#[test]
fn the_bridged_entry_is_not_refreshed_through_the_replaced_resolver() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    h.upsert("a", with_resolver("a", "192.0.2.53"));
    h.advance(60);
    let jobs = h.state.take_due(h.now, usize::MAX);
    assert_eq!(jobs.len(), 1, "only the new target is queried");
    assert_ne!(jobs[0].target.resolver, "system");
}

#[test]
fn switching_back_during_a_switch_reuses_the_answered_entry() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    h.upsert("a", with_resolver("a", "192.0.2.53"));
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    assert_eq!(h.recorder.addresses(&key("a")), vec!["10.0.0.1"]);
    assert_eq!(h.state.stats().entries, 1);
    assert_eq!(h.state.stats().pending_entries, 0, "never re-resolved from scratch");
}

#[test]
fn a_family_switch_serves_only_the_new_family_meanwhile() {
    let mut h = Harness::new();
    h.upsert(
        "a",
        external_name("a", "a.example.com", &[(DNS_LOOKUP_FAMILY, "all")], &[80]),
    );
    h.resolve_due(answer(&["10.0.0.1", "2001:db8::1"], None));
    h.upsert(
        "a",
        external_name("a", "a.example.com", &[(DNS_LOOKUP_FAMILY, "ipv6Only")], &[80]),
    );
    assert_eq!(h.recorder.addresses(&key("a")), vec!["2001:db8::1"]);
}

#[test]
fn a_shorter_refresh_policy_pulls_far_refreshes_in() {
    let mut h = Harness::with_inputs(inputs_with(DnsResolverConfig {
        refresh_interval_seconds: Some(86_400),
        ..Default::default()
    }));
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    assert!(h.state.entries_view(h.now, 10)[0].next_refresh_in_secs > 80_000);
    h.state.set_inputs(
        inputs_with(DnsResolverConfig {
            refresh_interval_seconds: Some(30),
            ..Default::default()
        }),
        lookup(),
        &h.recorder,
        h.now,
    );
    assert!(h.state.entries_view(h.now, 10)[0].next_refresh_in_secs <= 33);
}

#[test]
fn failure_before_any_answer_releases_waiters_without_endpoints() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(Err(DnsError::Io("refused".to_string())));
    assert!(!h.state.pending_waiters().contains_key(&key("a")));
    assert_eq!(h.state.stats().pending_entries, 0);
    assert_eq!(h.recorder.calls(), 0);
}

#[test]
fn result_for_a_deleted_or_recreated_entry_is_discarded() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    let jobs = h.state.take_due(h.now, usize::MAX);
    h.remove("a");
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.state
        .apply_result(&jobs[0], answer(&["10.0.0.1"], None), h.now, 0.5, &h.recorder);
    assert!(h.recorder.addresses(&key("a")).is_empty(), "stale generation ignored");
    assert_eq!(h.state.stats().pending_entries, 1);
}

#[test]
fn in_flight_targets_are_not_taken_twice_and_cancel_returns_them() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    let jobs = h.state.take_due(h.now, usize::MAX);
    assert_eq!(jobs.len(), 1);
    assert!(h.state.take_due(h.now, usize::MAX).is_empty());
    assert!(h.state.next_due().is_none());
    h.state.cancel_job(&jobs[0]);
    assert_eq!(h.state.take_due(h.now, usize::MAX).len(), 1);
}

#[test]
fn take_due_honours_the_limit() {
    let mut h = Harness::new();
    for name in ["a", "b", "c"] {
        h.upsert(name, external_name(name, &format!("{name}.example.com"), &[], &[80]));
    }
    assert_eq!(h.state.take_due(h.now, 2).len(), 2);
    assert_eq!(h.state.take_due(h.now, 2).len(), 1);
}

#[test]
fn full_table_rejects_new_targets_and_retries_when_space_frees() {
    let mut h = Harness::with_inputs(inputs_with(DnsResolverConfig {
        max_entries: 1,
        ..Default::default()
    }));
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.upsert("b", external_name("b", "b.example.com", &[], &[80]));
    assert_eq!(h.state.stats().entries, 1);
    assert_eq!(h.state.stats().rejected_services, 1);
    h.upsert("c", external_name("c", "a.example.com", &[], &[80]));
    assert_eq!(
        h.state.stats().bound_services,
        2,
        "an existing target still accepts referrers"
    );

    h.remove("a");
    h.remove("c");
    assert_eq!(h.state.stats().rejected_services, 0);
    let view = h.state.entries_view(h.now, 10);
    assert_eq!(view[0].hostname, "b.example.com");
}

#[test]
fn reconcile_withdraws_slices_the_table_does_not_own() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    let copied = h.recorder.current.lock()[&key("a")].clone();
    h.recorder.publish(&key("orphan"), copied);
    assert_eq!(h.state.reconcile_published(&h.recorder), 1);
    assert_eq!(h.recorder.published_service_keys(), BTreeSet::from([key("a")]));
}

#[test]
fn service_resolver_annotation_changes_the_target_identity() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.upsert(
        "b",
        external_name(
            "b",
            "a.example.com",
            &[(
                edgion_resources::constants::annotations::edgion::DNS_RESOLVER,
                "192.0.2.53",
            )],
            &[80],
        ),
    );
    assert_eq!(h.state.stats().entries, 2, "different resolvers never share answers");
}

#[test]
fn rejected_service_binds_when_its_target_appears_at_the_cap() {
    let mut h = Harness::with_inputs(inputs_with(DnsResolverConfig {
        max_entries: 1,
        ..Default::default()
    }));
    h.upsert("a", external_name("a", "x.example.com", &[], &[80]));
    h.upsert("b", external_name("b", "y.example.com", &[], &[80]));
    assert_eq!(h.state.stats().rejected_services, 1);
    // A moves to b's hostname: x is released, y is created, the table is full
    // again, yet b must now share y.
    h.upsert("a", external_name("a", "y.example.com", &[], &[80]));
    assert_eq!(h.state.stats().rejected_services, 0);
    assert_eq!(h.state.stats().bound_services, 2);
    assert_eq!(h.state.stats().entries, 1);
}

#[test]
fn unchanged_inputs_do_not_recompute() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["10.0.0.1"], None));
    let calls = h.recorder.calls();
    h.state.set_inputs(TableInputs::default(), lookup(), &h.recorder, h.now);
    assert_eq!(h.recorder.calls(), calls);
}

#[test]
fn pending_map_and_released_signals_follow_the_first_answer() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    let map = h.state.pending_waiters_if_dirty().expect("a pending entry was created");
    assert!(map.contains_key(&key("a")));
    assert!(
        h.state.pending_waiters_if_dirty().is_none(),
        "unchanged set is not rebuilt"
    );
    h.resolve_due(answer(&["10.0.0.1"], None));
    assert!(h.state.pending_waiters_if_dirty().expect("left pending").is_empty());
    assert_eq!(h.state.take_released().len(), 1);
    assert!(h.state.take_released().is_empty());

    h.upsert("b", external_name("b", "b.example.com", &[], &[80]));
    h.remove("b");
    assert_eq!(h.state.take_released().len(), 1, "removal releases waiters");
    assert!(h.state.pending_waiters_if_dirty().expect("changed").is_empty());
}

#[test]
fn huge_record_ttl_schedules_without_panicking() {
    let mut h = Harness::with_inputs(inputs_with(DnsResolverConfig {
        ttl_factor: 1.0,
        max_refresh_seconds: 2_147_483_647,
        ..Default::default()
    }));
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(Ok(DnsAnswer {
        addrs: vec!["10.0.0.1".parse().unwrap()],
        ttl: Some(Duration::from_secs(u64::MAX)),
    }));
    assert_eq!(h.recorder.addresses(&key("a")), vec!["10.0.0.1"]);
    assert_eq!(h.state.stats().pending_entries, 0);
}

#[test]
fn link_local_only_answer_publishes_nothing() {
    let mut h = Harness::new();
    h.upsert("a", external_name("a", "a.example.com", &[], &[80]));
    h.resolve_due(answer(&["169.254.169.254"], None));
    assert!(h.recorder.addresses(&key("a")).is_empty());
    assert_eq!(h.state.stats().pending_entries, 0);
}
