use super::*;
use crate::services::dns_resolver::targets::DnsResolverTargets;
use edgion_resources::constants::annotations::edgion::DNS_RESOLUTION_MODE;
use k8s_openapi::api::discovery::v1::EndpointSlice;

fn httpdns_owner(config: edgion_resources::resources::link_sys::HttpDnsResolverConfig) -> Arc<LinkSys> {
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    let mut owner = LinkSys::new(
        "dns",
        LinkSysSpec {
            config: SystemConfig::HttpDns(config),
            current_status: None,
            unknown_fields: vec![],
        },
    );
    owner.metadata.namespace = Some("test".into());
    owner.metadata.resource_version = Some("same-rv".into());
    Arc::new(owner)
}

#[tokio::test(flavor = "current_thread")]
async fn httpdns_preparation_reuses_failures_and_recovers() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = serde_json::from_value(serde_json::json!({"preset":"aliyun","fallback":{"type":"none"}})).unwrap();
    let original = httpdns_owner(config);
    let key = LinkSysKey {
        namespace: "test".into(),
        name: "dns".into(),
    };
    let mut state = TargetState::default();
    let ready = state.prepare(&key, Arc::clone(&original)).unwrap();
    assert!(ready.is_ready());
    let mut malformed = (*original).clone();
    malformed.metadata.name = None;
    let malformed = Arc::new(malformed);
    let failed = state.prepare(&key, Arc::clone(&malformed)).unwrap();
    assert!(!failed.is_ready() && !Arc::ptr_eq(&ready, &failed));
    assert!(resolve(
        &ResolverSpec::HttpDns(Arc::clone(&failed)),
        "localhost",
        DnsQueryPolicy::Native
    )
    .await
    .is_err());
    assert!(Arc::ptr_eq(&failed, &state.prepare(&key, malformed).unwrap()));
    let repaired = state.prepare(&key, original).unwrap();
    assert!(repaired.is_ready() && !Arc::ptr_eq(&failed, &repaired));
}

fn service(name: &str, annotations: &[(&str, &str)], ports: &[i32]) -> Service {
    let mut all: serde_json::Map<String, serde_json::Value> = annotations
        .iter()
        .map(|(key, value)| (key.to_string(), serde_json::Value::from(*value)))
        .collect();
    all.entry(DNS_RESOLUTION_MODE.to_string())
        .or_insert_with(|| "controller".into());
    let ports: Vec<serde_json::Value> = ports
        .iter()
        .map(|port| serde_json::json!({"name": format!("p{port}"), "port": port, "protocol": "TCP"}))
        .collect();
    serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "app", "name": name, "uid": "uid", "annotations": all},
        "spec": {"type": "ExternalName", "externalName": "backend.example.com", "ports": ports}
    }))
    .unwrap()
}

struct Harness {
    runtime: Arc<ControllerRuntime>,
    targets: DnsResolverTargets,
    slices: DnsSyntheticSlices,
    state: ResolverState,
    now: Instant,
}

impl Harness {
    fn new() -> Self {
        Self {
            runtime: Arc::new(ControllerRuntime::default()),
            targets: DnsResolverTargets::new(),
            slices: DnsSyntheticSlices::new(),
            state: ResolverState::new(&DnsResolverConfig::default(), 0),
            now: Instant::now(),
        }
    }

    fn plan(&mut self) -> (Vec<ResolveJob>, Vec<String>) {
        self.state
            .plan(self.targets.snapshot(), &self.runtime, &self.slices, self.now)
    }

    fn apply_all(&mut self, result: Result<Vec<&str>, DnsError>) -> Vec<String> {
        let (jobs, mut changed) = self.plan();
        for job in &jobs {
            changed.extend(self.apply(job, result.clone()));
        }
        changed
    }

    fn apply(&mut self, job: &ResolveJob, result: Result<Vec<&str>, DnsError>) -> Vec<String> {
        let addrs = result.map(|addrs| addrs.iter().map(|addr| addr.parse().unwrap()).collect());
        self.state.apply(job, addrs, &self.slices, self.now)
    }

    fn addresses(&self, slice: &str) -> Vec<String> {
        self.slices
            .get(&format!("app/{slice}"))
            .map(|slice: EndpointSlice| {
                slice
                    .endpoints
                    .iter()
                    .flat_map(|endpoint| endpoint.addresses.iter().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[test]
fn resolves_publishes_and_suppresses_unchanged_answers() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    let changed = h.apply_all(Ok(vec!["10.0.0.2", "10.0.0.1"]));
    assert_eq!(changed, vec!["app/svc-edgion-ctl-dns".to_string()]);
    assert_eq!(h.addresses("svc-edgion-ctl-dns"), vec!["10.0.0.1", "10.0.0.2"]);

    let (jobs, _) = h.plan();
    assert!(jobs.is_empty(), "not due before the refresh interval");

    h.now += Duration::from_secs(20);
    let changed = h.apply_all(Ok(vec!["10.0.0.1", "10.0.0.2", "10.0.0.1"]));
    assert!(changed.is_empty(), "an unchanged answer enqueues nothing");
}

#[test]
fn authoritative_empty_withdraws_and_transient_failure_keeps_last_good() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    h.apply_all(Ok(vec!["10.0.0.1"]));
    h.now += Duration::from_secs(20);
    assert!(h.apply_all(Err(DnsError::Io("timeout".into()))).is_empty());
    assert_eq!(h.addresses("svc-edgion-ctl-dns"), vec!["10.0.0.1"]);
    h.now += Duration::from_secs(20);
    assert_eq!(h.apply_all(Err(DnsError::NoAddresses)).len(), 1);
    assert!(h.addresses("svc-edgion-ctl-dns").is_empty());
    assert!(h.slices.service_keys().is_empty());
}

#[test]
fn removed_target_and_mode_switch_withdraw_slices() {
    let mut h = Harness::new();
    let svc = service("svc", &[], &[80]);
    h.targets.sync_published(&svc);
    h.apply_all(Ok(vec!["10.0.0.1"]));
    h.targets
        .sync_published(&service("svc", &[(DNS_RESOLUTION_MODE, "gateway")], &[80]));
    let (jobs, changed) = h.plan();
    assert!(jobs.is_empty());
    assert_eq!(changed, vec!["app/svc-edgion-ctl-dns".to_string()]);
    assert!(h.slices.service_keys().is_empty());

    h.targets.sync_published(&svc);
    h.apply_all(Ok(vec!["10.0.0.1"]));
    h.targets.remove_published(&svc);
    let (_, changed) = h.plan();
    assert_eq!(changed.len(), 1);
    assert!(h.slices.service_keys().is_empty());
}

#[test]
fn orphaned_slices_without_state_are_withdrawn() {
    let mut h = Harness::new();
    let svc = service("svc", &[], &[80]);
    h.slices.replace_service(
        0,
        "app/svc",
        edgion_common::dns::endpoint_slices::build_dns_endpoint_slices(
            "app",
            "svc",
            svc.spec.as_ref().unwrap().ports.as_deref().unwrap(),
            &["10.0.0.1".parse().unwrap()],
            DnsSliceOrigin::Controller,
        ),
    );
    let (_, changed) = h.plan();
    assert_eq!(changed.len(), 1);
    assert!(h.slices.service_keys().is_empty());
}

#[test]
fn edits_republish_last_good_at_once_and_requery() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    h.apply_all(Ok(vec!["10.0.0.1", "2001:db8::1"]));
    assert!(h.slices.get("app/svc-edgion-ctl-dns").is_some());
    assert!(
        h.slices.get("app/svc-edgion-ctl-dns-ipv6").is_none(),
        "default family is ipv4Only"
    );

    h.targets
        .sync_published(&service("svc", &[(DNS_LOOKUP_FAMILY, "all")], &[80]));
    let (jobs, changed) = h.plan();
    assert_eq!(jobs.len(), 1, "a revised target is queried at once");
    assert_eq!(changed, vec!["app/svc-edgion-ctl-dns-ipv6".to_string()]);
    assert_eq!(h.addresses("svc-edgion-ctl-dns-ipv6"), vec!["2001:db8::1"]);

    h.targets
        .sync_published(&service("svc", &[(DNS_LOOKUP_FAMILY, "all")], &[8080]));
    let (_, changed) = h.plan();
    assert_eq!(changed.len(), 2, "port edit republishes both families");
    let slice = h.slices.get("app/svc-edgion-ctl-dns").unwrap();
    assert_eq!(slice.ports.unwrap()[0].port, Some(8080));
}

#[test]
fn invalid_family_withdraws_without_querying() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    h.apply_all(Ok(vec!["10.0.0.1"]));
    h.targets
        .sync_published(&service("svc", &[(DNS_LOOKUP_FAMILY, "v4")], &[80]));
    let (jobs, changed) = h.plan();
    assert!(jobs.is_empty());
    assert_eq!(changed.len(), 1);
    assert!(h.slices.service_keys().is_empty());
}

#[test]
fn next_due_follows_the_fixed_interval() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    h.apply_all(Ok(vec!["10.0.0.1"]));
    assert_eq!(h.state.next_due(h.now), h.now + DEFAULT_REFRESH);
    assert_eq!(
        ResolverState::new(&DnsResolverConfig::default(), 0).next_due(h.now),
        h.now + DEFAULT_REFRESH
    );
}

#[test]
fn default_resolver_falls_back_to_system_without_inline_servers() {
    let runtime = ControllerRuntime::default();
    assert!(matches!(controller_default_resolver(&runtime), ResolverSpec::System));
}

#[test]
fn parse_refresh_interval_units_defaults_and_clamps() {
    assert_eq!(parse_refresh_interval("15s"), Duration::from_secs(15));
    assert_eq!(parse_refresh_interval("2m"), Duration::from_secs(120));
    assert_eq!(parse_refresh_interval("1h30m"), Duration::from_secs(5400));
    assert_eq!(parse_refresh_interval("bogus"), DEFAULT_REFRESH);
    assert_eq!(parse_refresh_interval("0s"), MIN_REFRESH);
    assert_eq!(parse_refresh_interval("500ms"), MIN_REFRESH);
}

#[test]
fn controller_default_candidate_prefers_ref_then_servers() {
    let mut config = DnsResolverConfig {
        link_sys_ref: Some("dns/resolver".into()),
        servers: vec!["192.0.2.53".into()],
        ..Default::default()
    };
    assert!(matches!(
        controller_default_candidate(&config),
        Some(ResolverCandidate::HttpDnsRef(key)) if key.namespace == "dns" && key.name == "resolver"
    ));
    config.link_sys_ref = None;
    assert!(matches!(
        controller_default_candidate(&config),
        Some(ResolverCandidate::Servers(servers)) if servers.servers() == ["192.0.2.53"]
    ));
    config.servers.clear();
    assert!(controller_default_candidate(&config).is_none());
}

#[test]
fn active_httpdns_pair_failure_with_same_rv_cannot_reuse_ready_or_hide_missing_get_material() {
    use edgion_resources::resources::link_sys::SystemConfig;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = serde_json::from_value(
        serde_json::json!({"preset":"aliyun","fallback":{"type":"none"},"connection":{"tls":{
            "enabled":true,"verify":false,"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"test"}}]
        }}}),
    )
    .unwrap();
    let original = httpdns_owner(config);
    let key = LinkSysKey {
        namespace: "test".into(),
        name: "dns".into(),
    };
    let mut state = TargetState::default();
    let ready = state.prepare(&key, Arc::clone(&original)).unwrap();
    assert!(ready.is_ready());
    for case in ["group", "material-namespace", "missing-get-material"] {
        let mut malformed = (*original).clone();
        let SystemConfig::HttpDns(config) = &mut malformed.spec.config else {
            unreachable!()
        };
        let tls = config.connection.as_mut().unwrap().tls.as_mut().unwrap();
        match case {
            "group" => {
                tls.outbound
                    .validation
                    .as_mut()
                    .unwrap()
                    .ca_certificate_refs
                    .as_mut()
                    .unwrap()[0]
                    .group = Some("other.example".into())
            }
            "material-namespace" => {
                tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.namespace = Some("other".into())
            }
            "missing-get-material" => tls.resolved_ca_certificates = None,
            _ => unreachable!(),
        }
        let malformed = Arc::new(malformed);
        let failed = state.prepare(&key, Arc::clone(&malformed)).unwrap();
        assert!(!failed.is_ready() && !Arc::ptr_eq(&ready, &failed), "{case}");
        assert!(Arc::ptr_eq(&failed, &state.prepare(&key, malformed).unwrap()), "{case}");
        assert!(state.prepare(&key, Arc::clone(&original)).unwrap().is_ready(), "{case}");
    }
}

struct LinkSysPassthrough;

#[async_trait::async_trait]
impl crate::conf_mgr::sync_runtime::resource_processor::ProcessorHandler<LinkSys> for LinkSysPassthrough {}

/// A runtime whose processed LinkSys cache holds `owners`.
fn runtime_with_link_sys(owners: impl IntoIterator<Item = LinkSys>) -> Arc<ControllerRuntime> {
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor::<
        LinkSys,
    >::new("LinkSys", 16, Arc::new(LinkSysPassthrough), runtime.clone()));
    for owner in owners {
        processor.save(owner, None);
    }
    runtime.processor_registry.register(processor);
    runtime
}

fn resolver_link_sys() -> LinkSys {
    serde_json::from_value(serde_json::json!({"apiVersion":"edgion.io/v1","kind":"LinkSys","metadata":{"name":"resolver","namespace":"dns"},"spec":{"type":"httpdns","config":{"preset":"aliyun","connection":{"timeoutMs":0},"fallback":{"type":"none"}}}})).unwrap()
}

fn dns_service(reference: &str) -> Service {
    serde_json::from_value(serde_json::json!({"metadata":{"namespace":"app","name":"source","uid":"current-uid","annotations":{"edgion.io/dns-resolver-ref":reference,"edgion.io/dns-resolution-mode":"controller"}},"spec":{"type":"ExternalName","externalName":"192.0.2.1","ports":[{"port":80}]}})).unwrap()
}

fn dns_grant(runtime: &ControllerRuntime, kind: &str) {
    let group = if kind == "Service" {
        ""
    } else {
        "gateway.networking.k8s.io"
    };
    runtime.ref_grant_store.upsert("dns/allow".into(),Arc::new(serde_json::from_value(serde_json::json!({"metadata":{"name":"allow","namespace":"dns"},"spec":{"from":[{"group":group,"kind":kind,"namespace":"app"}],"to":[{"group":"edgion.io","kind":"LinkSys","name":"resolver"}]}})).unwrap()));
}

#[tokio::test]
async fn dns_producer_gates_tenant_and_preserves_default_authority() {
    let runtime = runtime_with_link_sys([resolver_link_sys()]);
    let mut source = dns_service("dns/resolver");
    let mut state = TargetState::default();
    for kind in [None, Some("Gateway"), Some("Service"), None, Some("Service")] {
        runtime.ref_grant_store.remove("dns/allow");
        if let Some(kind) = kind {
            dns_grant(&runtime, kind);
        }
        let spec = effective_spec(
            &runtime,
            service_resolver_candidate(&source, &runtime),
            &None,
            &mut state,
        );
        assert_eq!(state.prepared.is_some(), kind == Some("Service"), "{kind:?}");
        assert_eq!(matches!(spec, ResolverSpec::HttpDns(_)), kind == Some("Service"));
    }
    runtime.ref_grant_store.remove("dns/allow");
    let default_candidate = controller_default_candidate(&DnsResolverConfig {
        link_sys_ref: Some("dns/resolver".into()),
        ..Default::default()
    });
    effective_spec(
        &runtime,
        service_resolver_candidate(&source, &runtime),
        &default_candidate,
        &mut state,
    );
    assert!(
        state.prepared.is_some(),
        "administrative default independently admits the same key"
    );
    for reference in ["bare", "/resolver", "dns/", "dns/a/b"] {
        source
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert("edgion.io/dns-resolver-ref".into(), reference.into());
        effective_spec(
            &runtime,
            service_resolver_candidate(&source, &runtime),
            &None,
            &mut state,
        );
        assert!(state.prepared.is_none(), "{reference}");
    }
}

#[tokio::test]
async fn producer_validation_off_still_rejects_invalid_identity_before_lookup() {
    const CHILD: &str = "EDGION_R6A_PRODUCER_VALIDATION_OFF_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "services::dns_resolver::service::tests::producer_validation_off_still_rejects_invalid_identity_before_lookup",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
        return;
    }
    let mut config = crate::config::EdgionControllerConfig::default();
    config.validation.enable_reference_grant_validation = false;
    crate::config::init_controller_config(config);
    let runtime = runtime_with_link_sys([resolver_link_sys()]);
    let mut source = dns_service("dns/resolver");
    let mut state = TargetState::default();
    assert!(
        matches!(
            effective_spec(
                &runtime,
                service_resolver_candidate(&source, &runtime),
                &None,
                &mut state
            ),
            ResolverSpec::HttpDns(_)
        ),
        "valid cross-namespace target reaches lookup without a Grant"
    );
    for case in ["owner", "target"] {
        if case == "owner" {
            source.metadata.namespace = Some("".into());
        } else {
            source.metadata.namespace = Some("app".into());
            source
                .metadata
                .annotations
                .as_mut()
                .unwrap()
                .insert("edgion.io/dns-resolver-ref".into(), "dns/a/b".into());
        }
        assert!(
            matches!(
                effective_spec(
                    &runtime,
                    service_resolver_candidate(&source, &runtime),
                    &None,
                    &mut state
                ),
                ResolverSpec::System
            ),
            "{case}"
        );
    }
}

#[test]
fn controller_family_override_is_strict_and_independent_of_resolver_selection() {
    let mut service = dns_service("");
    assert_eq!(
        service_lookup_family(&service, DnsLookupFamily::Ipv6Only),
        Ok(DnsLookupFamily::Ipv6Only)
    );
    service
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(DNS_LOOKUP_FAMILY.into(), "all".into());
    assert_eq!(
        service_lookup_family(&service, DnsLookupFamily::Ipv4Only),
        Ok(DnsLookupFamily::All)
    );
    for invalid in ["", "ipv4", " all", "ALL"] {
        service
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(DNS_LOOKUP_FAMILY.into(), invalid.into());
        assert!(service_lookup_family(&service, DnsLookupFamily::Ipv4Only).is_err());
    }
}

#[test]
fn hostname_change_never_inherits_the_old_answer() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    h.apply_all(Ok(vec!["10.0.0.1"]));
    let mut moved = service("svc", &[], &[80]);
    moved.spec.as_mut().unwrap().external_name = Some("other.example.com".into());
    h.targets.sync_published(&moved);
    let (jobs, changed) = h.plan();
    assert_eq!(changed, vec!["app/svc-edgion-ctl-dns".to_string()], "withdrawn at once");
    assert!(h.addresses("svc-edgion-ctl-dns").is_empty());
    assert!(h.apply(&jobs[0], Err(DnsError::Io("timeout".into()))).is_empty());
    assert!(
        h.addresses("svc-edgion-ctl-dns").is_empty(),
        "a failing new hostname serves nothing, not the old IPs"
    );
}

#[test]
fn blank_external_name_withdraws() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    h.apply_all(Ok(vec!["10.0.0.1"]));
    let mut blank = service("svc", &[], &[80]);
    blank.spec.as_mut().unwrap().external_name = Some("  ".into());
    h.targets.sync_published(&blank);
    let (jobs, changed) = h.plan();
    assert!(jobs.is_empty());
    assert_eq!(changed.len(), 1);
    assert!(h.slices.service_keys().is_empty());
}

#[test]
fn transient_failures_keep_last_good_only_up_to_max_stale() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    h.apply_all(Ok(vec!["10.0.0.1"]));
    h.now += Duration::from_secs(200);
    h.apply_all(Err(DnsError::Io("nxdomain via system".into())));
    assert_eq!(h.addresses("svc-edgion-ctl-dns"), vec!["10.0.0.1"]);
    h.now += Duration::from_secs(200);
    h.apply_all(Err(DnsError::Io("nxdomain via system".into())));
    assert!(h.addresses("svc-edgion-ctl-dns").is_empty(), "past MAX_STALE");
}

#[test]
fn results_for_an_older_revision_are_discarded() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    let (old_jobs, _) = h.plan();
    h.targets.sync_published(&service("svc", &[], &[8080]));
    let (new_jobs, _) = h.plan();
    assert_eq!(new_jobs.len(), 1, "a revised target is planned even while in flight");
    assert!(h.apply(&old_jobs[0], Ok(vec!["10.0.0.9"])).is_empty());
    assert!(h.slices.service_keys().is_empty());
    h.apply(&new_jobs[0], Ok(vec!["10.0.0.1"]));
    assert_eq!(h.addresses("svc-edgion-ctl-dns"), vec!["10.0.0.1"]);
}

#[test]
fn in_flight_targets_are_not_planned_twice() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    let (jobs, _) = h.plan();
    assert_eq!(jobs.len(), 1);
    h.now += Duration::from_secs(60);
    assert!(h.plan().0.is_empty(), "due again but still in flight");
    h.apply(&jobs[0], Ok(vec!["10.0.0.1"]));
    assert!(h.plan().0.is_empty(), "the next refresh counts from completion");
    h.now += DEFAULT_REFRESH;
    assert_eq!(h.plan().0.len(), 1);
}

#[test]
fn a_restarted_task_keeps_published_slices_until_its_first_answer() {
    let mut h = Harness::new();
    let svc = service("svc", &[], &[80]);
    h.targets.sync_published(&svc);
    h.apply_all(Ok(vec!["10.0.0.1"]));
    // A respawned task starts from an empty state on the same store epoch.
    h.state = ResolverState::new(&DnsResolverConfig::default(), h.slices.epoch());
    let (jobs, changed) = h.plan();
    assert!(changed.is_empty(), "no withdrawal before the first answer");
    assert_eq!(h.addresses("svc-edgion-ctl-dns"), vec!["10.0.0.1"]);
    assert!(h.apply(&jobs[0], Err(DnsError::Io("down".into()))).is_empty());
    assert_eq!(h.addresses("svc-edgion-ctl-dns"), vec!["10.0.0.1"]);
}

#[test]
fn a_restarted_task_never_adopts_slices_of_another_source() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    h.apply_all(Ok(vec!["10.0.0.1"]));
    // The hostname changes while no task runs (for example during a panic restart).
    let mut moved = service("svc", &[], &[80]);
    moved.spec.as_mut().unwrap().external_name = Some("other.example.com".into());
    h.targets.sync_published(&moved);
    h.state = ResolverState::new(&DnsResolverConfig::default(), h.slices.epoch());
    let (jobs, changed) = h.plan();
    assert_eq!(changed.len(), 1, "the old hostname's answer is withdrawn at once");
    assert!(h.slices.service_keys().is_empty());
    assert_eq!(jobs.len(), 1);
}

#[test]
fn a_restart_does_not_extend_max_stale() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    h.apply_all(Ok(vec!["10.0.0.1"]));
    h.now += Duration::from_secs(200);
    h.state = ResolverState::new(&DnsResolverConfig::default(), h.slices.epoch());
    let (jobs, _) = h.plan();
    assert!(h.apply(&jobs[0], Err(DnsError::Io("down".into()))).is_empty());
    assert_eq!(h.addresses("svc-edgion-ctl-dns"), vec!["10.0.0.1"]);
    // 400 s after the original answer: past MAX_STALE although the task is new.
    h.now += Duration::from_secs(200);
    h.apply_all(Err(DnsError::Io("down".into())));
    assert!(h.addresses("svc-edgion-ctl-dns").is_empty());
}

#[test]
fn in_flight_targets_do_not_pull_the_wake_time_into_the_past() {
    let mut h = Harness::new();
    h.targets.sync_published(&service("svc", &[], &[80]));
    let (jobs, _) = h.plan();
    // The query outlives the refresh period.
    h.now += Duration::from_secs(30);
    assert!(h.state.next_due(h.now) > h.now, "an in-flight target is not due");
    h.apply(&jobs[0], Ok(vec!["10.0.0.1"]));
    assert_eq!(
        h.state.next_due(h.now),
        h.now + h.state.period,
        "the next refresh counts from completion"
    );
}

#[tokio::test]
async fn a_restart_with_no_targets_still_withdraws_orphaned_slices() {
    let runtime = Arc::new(ControllerRuntime::default());
    let svc = service("svc", &[], &[80]);
    runtime.dns_synthetic_slices.replace_service(
        runtime.dns_synthetic_slices.epoch(),
        "app/svc",
        edgion_common::dns::endpoint_slices::build_dns_endpoint_slices(
            "app",
            "svc",
            svc.spec.as_ref().unwrap().ports.as_deref().unwrap(),
            &["10.0.0.1".parse().unwrap()],
            DnsSliceOrigin::Controller,
        ),
    );
    assert!(runtime.dns_resolver_targets.is_empty());
    ensure_dns_resolver_running(&runtime);
    assert!(
        runtime.dns_resolver_task.is_running(),
        "published slices without targets still need a task to withdraw them"
    );
    runtime.reset();
    assert!(!runtime.dns_resolver_task.is_running());
}
