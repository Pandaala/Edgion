use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use mockall::predicate::eq;
use serde_json::json;

fn create_test_config() -> RealIpConfig {
    serde_json::from_value(json!({"trustedIpRefs": [{"name": "proxies"}]})).unwrap()
}

fn create_plugin(config: &RealIpConfig) -> Box<dyn RequestFilter> {
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(json!({
        "metadata": {"name": "real-ip", "namespace": "default"},
        "spec": {"requestPlugins": [{"type": "RealIp", "config": config}]}
    }))
    .unwrap();
    let compiler = IpConditionCompiler::new(&owner).unwrap();
    let edgion_resources::resources::edgion_plugins::EdgionPlugin::RealIp(actual) =
        &owner.spec.request_plugins.as_ref().unwrap()[0].plugin
    else {
        unreachable!()
    };
    RealIp::create(actual, &compiler).unwrap()
}

fn trust_mock(session: &mut MockPluginSession) {
    use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
    use edgion_common::conf_sync::traits::ConfHandler;
    let store = EdgionConfigDataStore::new();
    let data = serde_json::from_value(json!({"metadata": {"name": "proxies", "namespace": "default"}, "spec": {"data": {"type": "IpList", "config": {"items": [{"name": "private", "cidrs": ["192.168.0.0/16", "198.51.100.0/24"]}]}}}})).unwrap();
    store.full_set(&std::collections::HashMap::from([(
        "default/proxies".into(),
        Arc::new(data),
    )]));
    let view = Arc::new(store.capture_snapshot());
    let available = view.clone();
    session.expect_ensure_config_data_view().times(1).return_const(());
    session
        .expect_ip_list_availability()
        .returning(move |key| available.ip_list_availability(key));
    session
        .expect_ip_list_contains()
        .returning(move |key, ip| view.ip_list_contains(key, ip));
}

#[test]
fn name_and_header_log_are_prepared_once() {
    let first = create_plugin(&create_test_config());
    let second = create_plugin(&create_test_config());
    assert_eq!(first.name(), "RealIp");
    assert!(std::ptr::eq(first.name().as_ptr(), second.name().as_ptr()));

    assert_eq!(RealIp::header_log("X-FORWARDED-FOR"), "header xff");
    assert_eq!(RealIp::header_log("x-real-IP"), "header real-ip");
    assert_eq!(RealIp::header_log("FORWARDED"), "header forwarded");
    assert_eq!(RealIp::header_log(" X-Forwarded-For"), "header custom");
    assert_eq!(RealIp::header_log("X-Client-IP"), "header custom");
}

#[tokio::test]
async fn trusted_peer_updates_remote_ip_with_one_read_per_input() {
    const CHILD_ENV: &str = "EDGION_REAL_IP_MASKED_LOG_CHILD";
    const TEST_NAME: &str = "trusted_peer_updates_remote_ip_with_one_read_per_input";
    const TEST_MODULE: &str = "plugins::http::real_ip::plugin::tests";

    crate::test_subprocess::assert_in_subprocess_async(TEST_MODULE, TEST_NAME, CHILD_ENV, TEST_NAME, || async {
        edgion_resources::sensitive::SENSITIVE_MODE.store(1, std::sync::atomic::Ordering::SeqCst);

        for (configured_name, request_name, expected_header_log) in [
            ("X-FoRwArDeD-FoR", "x-forwarded-for", "header xff"),
            ("X-CuStOm-Ip", "x-custom-ip", "header custom"),
        ] {
            let mut config = create_test_config();
            config.real_ip_header = configured_name.into();
            let plugin = create_plugin(&config);
            let mut session = MockPluginSession::new();
            trust_mock(&mut session);
            setup_header_mocks(&mut session, &[(request_name, "203.0.113.1, 192.168.1.1")]);
            session
                .expect_downstream_peer_ip()
                .times(1)
                .return_const("192.168.1.254".to_string());
            session
                .expect_remote_ip()
                .times(1)
                .return_const("192.168.1.254".to_string());
            session
                .expect_set_remote_ip()
                .with(eq("203.0.113.1"))
                .times(1)
                .returning(|_| Ok(()));
            let mut log = PluginLog::new(plugin.name());

            assert_eq!(
                plugin.run_request(&mut session, &mut log).await,
                PluginRunningResult::GoodNext
            );
            let entries = serde_json::to_value(&log).unwrap()["log"].as_array().unwrap().clone();
            assert_eq!(entries[0], json!(expected_header_log));
            assert_eq!(entries[1], json!("trusted proxy:match"));
            assert!(entries[2].as_str().unwrap().starts_with("ip updated: "));
            assert!(!serde_json::to_string(&log).unwrap().contains("203.0.113.1"));
        }
    })
    .await;
}

#[tokio::test]
async fn missing_header_keeps_remote_ip_without_calling_setter() {
    let plugin = create_plugin(&create_test_config());
    let mut session = MockPluginSession::new();
    trust_mock(&mut session);
    setup_header_mocks(&mut session, &[]);
    session
        .expect_downstream_peer_ip()
        .times(1)
        .return_const("192.168.1.254".to_string());
    session
        .expect_remote_ip()
        .times(1)
        .return_const("192.168.1.254".to_string());
    session.expect_set_remote_ip().times(0);
    let mut log = PluginLog::new(plugin.name());

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(
        serde_json::to_value(&log).unwrap()["log"],
        json!(["header xff", "trusted proxy:match", "ip unchanged"])
    );
}

#[tokio::test]
async fn untrusted_peer_ignores_forwarded_header() {
    let plugin = create_plugin(&create_test_config());
    let mut session = MockPluginSession::new();
    trust_mock(&mut session);
    setup_header_mocks(&mut session, &[("x-forwarded-for", "203.0.113.1")]);
    session
        .expect_downstream_peer_ip()
        .times(1)
        .return_const("8.8.8.8".to_string());
    session.expect_remote_ip().times(1).return_const("8.8.8.8".to_string());
    session.expect_set_remote_ip().times(0);
    let mut log = PluginLog::new(plugin.name());

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(
        serde_json::to_value(&log).unwrap()["log"],
        json!(["header xff", "trusted proxy:miss", "ip unchanged"])
    );
}

#[tokio::test]
async fn invalid_or_empty_peer_resolves_to_unchanged_unspecified_ip() {
    for peer in ["", "not-an-address"] {
        let plugin = create_plugin(&create_test_config());
        let mut session = MockPluginSession::new();
        trust_mock(&mut session);
        setup_header_mocks(&mut session, &[]);
        session
            .expect_downstream_peer_ip()
            .times(1)
            .return_const(peer.to_string());
        session.expect_remote_ip().times(1).return_const("0.0.0.0".to_string());
        session.expect_set_remote_ip().times(0);
        let mut log = PluginLog::new(plugin.name());

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(
            serde_json::to_value(&log).unwrap()["log"],
            json!(["header xff", "trusted proxy:miss", "ip unchanged"]),
            "peer={peer:?}"
        );
    }
}

#[tokio::test]
async fn setter_failure_is_non_terminal_and_statically_logged() {
    let plugin = create_plugin(&create_test_config());
    let mut session = MockPluginSession::new();
    trust_mock(&mut session);
    setup_header_mocks(&mut session, &[("x-forwarded-for", "203.0.113.1")]);
    session
        .expect_downstream_peer_ip()
        .times(1)
        .return_const("192.168.1.254".to_string());
    session
        .expect_remote_ip()
        .times(1)
        .return_const("192.168.1.254".to_string());
    session
        .expect_set_remote_ip()
        .with(eq("203.0.113.1"))
        .times(1)
        .returning(|_| Err(Box::new(std::io::Error::other("test setter failure"))));
    let mut log = PluginLog::new(plugin.name());

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(
        serde_json::to_value(&log).unwrap()["log"],
        json!(["header xff", "trusted proxy:match", "err: set remote IP failed"])
    );
}

#[test]
fn invalid_policy_cannot_construct_or_retain_a_filter() {
    let mut owner: edgion_resources::EdgionPlugins = serde_json::from_value(json!({
        "metadata": {"name": "real-ip", "namespace": "default"},
        "spec": {"requestPlugins": [{"type": "RealIp", "config": {"trustedIpRefs": [{"name": "proxies"}]}}]}
    }))
    .unwrap();
    if let edgion_resources::resources::edgion_plugins::EdgionPlugin::RealIp(config) =
        &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin
    {
        config.trusted.invalid_input = true;
    }
    assert!(IpConditionCompiler::new(&owner).is_err());
}

fn policy_owner(global: bool) -> serde_json::Value {
    let config = json!({"trustedIpRefs": [{"name": "primary", "namespace": "default"}, {"name": "secondary", "namespace": "default"}]});
    if global {
        json!({"metadata": {"name": "real-ip"}, "spec": {"realIp": config}})
    } else {
        json!({"metadata": {"name": "real-ip", "namespace": "default"}, "spec": {"requestPlugins": [{"type": "RealIp", "config": config}]}})
    }
}

fn bound_policy(global: bool) -> RealIpPolicy {
    if global {
        let owner: edgion_resources::EdgionGatewayConfig = serde_json::from_value(policy_owner(true)).unwrap();
        let compiler = IpConditionCompiler::from_gateway_config(&owner).unwrap();
        RealIpPolicy::bind(owner.spec.real_ip.as_ref().unwrap(), &compiler).unwrap()
    } else {
        let owner: edgion_resources::EdgionPlugins = serde_json::from_value(policy_owner(false)).unwrap();
        let compiler = IpConditionCompiler::new(&owner).unwrap();
        let edgion_resources::resources::edgion_plugins::EdgionPlugin::RealIp(config) =
            &owner.spec.request_plugins.as_ref().unwrap()[0].plugin
        else {
            unreachable!()
        };
        RealIpPolicy::bind(config, &compiler).unwrap()
    }
}

fn trust_store(
    primary: Option<(&[&str], &str)>,
    secondary: Option<(&[&str], &str)>,
) -> crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore {
    use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
    use edgion_common::conf_sync::traits::ConfHandler;
    let store = EdgionConfigDataStore::new();
    let mut entries = std::collections::HashMap::new();
    for (name, source) in [("primary", primary), ("secondary", secondary)] {
        if let Some((cidrs, visibility)) = source {
            let data = serde_json::from_value(json!({"metadata": {"name": name, "namespace": "default"}, "spec": {"visibility": visibility, "data": {"type": "IpList", "config": {"items": [{"name": "main", "cidrs": cidrs}]}}}})).unwrap();
            entries.insert(format!("default/{name}"), Arc::new(data));
        }
    }
    store.full_set(&entries);
    store
}

#[test]
fn actual_owner_policies_require_every_list_and_recover_without_rebuilding() {
    use edgion_common::conf_sync::traits::ConfHandler;
    for global in [false, true] {
        let policy = bound_policy(global);
        let store = trust_store(
            Some((&["192.168.0.0/16", "invalid-cidr"], "Cluster")),
            Some((&["198.51.100.0/24"], "Cluster")),
        );
        let held = store.capture_snapshot();
        let resolve = |view: &EdgionConfigDataSnapshot| {
            policy.resolve_in(
                view,
                "192.168.1.254:45678",
                Some("203.0.113.7, 198.51.100.2, 192.168.1.1"),
            )
        };
        assert_eq!(
            resolve(&held).remote_ip.to_string(),
            "203.0.113.7",
            "partial-good IpList entries remain usable"
        );
        store.full_set(&std::collections::HashMap::new());
        assert_eq!(
            resolve(&store.capture_snapshot()).remote_ip.to_string(),
            "192.168.1.254"
        );
        assert_eq!(
            resolve(&held).remote_ip.to_string(),
            "203.0.113.7",
            "held view covers peer and all hops"
        );
        let missing_secondary = trust_store(Some((&["192.168.0.0/16"], "Cluster")), None);
        assert_eq!(
            resolve(&missing_secondary.capture_snapshot()).remote_ip.to_string(),
            "192.168.1.254",
            "a matching first list cannot hide a missing later list"
        );
        let restored = trust_store(
            Some((&["192.168.0.0/16"], "Cluster")),
            Some((&["198.51.100.0/24"], "Cluster")),
        );
        assert_eq!(
            resolve(&restored.capture_snapshot()).remote_ip.to_string(),
            "203.0.113.7"
        );
        let empty = trust_store(Some((&["invalid-cidr"], "Cluster")), Some((&[], "Cluster")));
        assert_eq!(
            resolve(&empty.capture_snapshot()).remote_ip.to_string(),
            "192.168.1.254"
        );
        let private = trust_store(
            Some((&["192.168.0.0/16"], "Namespace")),
            Some((&["198.51.100.0/24"], "Namespace")),
        );
        assert_eq!(
            resolve(&private.capture_snapshot()).remote_ip.to_string(),
            if global { "192.168.1.254" } else { "203.0.113.7" }
        );
    }
}

fn assert_canonical_trust(cidrs: &[&str], matching: &[&str], nonmatching: &[&str]) {
    let policy = bound_policy(false);
    let store = trust_store(Some((cidrs, "Namespace")), Some((&[], "Namespace")));
    let view = store.capture_snapshot();
    for peer in matching {
        assert!(
            policy.resolve_in(&view, peer, None).downstream_trusted,
            "expected trust: {peer}"
        );
    }
    for peer in nonmatching {
        assert!(
            !policy.resolve_in(&view, peer, None).downstream_trusted,
            "unexpected trust: {peer}"
        );
    }
}

#[test]
fn test_valid_ipv4_single() {
    assert_canonical_trust(&["192.168.1.1"], &["192.168.1.1"], &["192.168.1.2"]);
}
#[test]
fn test_valid_ipv4_cidr() {
    assert_canonical_trust(
        &["10.0.0.0/8", "192.168.1.0/24"],
        &["10.0.0.1", "192.168.1.50"],
        &["172.16.0.1"],
    );
}
#[test]
fn test_valid_ipv6() {
    assert_canonical_trust(&["::1", "2001:db8::/32"], &["::1", "2001:db8::1"], &["2001:db9::1"]);
}
#[test]
fn test_mixed_valid_invalid() {
    assert_canonical_trust(&["10.0.0.0/8", "172.16.0.0/12"], &["10.0.0.1", "172.16.0.1"], &[]);
}
#[test]
fn test_invalid_cidrs_skipped() {
    let policy = bound_policy(false);
    let store = trust_store(
        Some((
            &[
                "10.0.0.0/8",
                "invalid-cidr",
                "192.168.0.0/16",
                "300.0.0.0/8",
                "172.16.0.0/12",
            ],
            "Namespace",
        )),
        Some((&[], "Namespace")),
    );
    assert_eq!(
        policy
            .resolve_in(
                &store.capture_snapshot(),
                "192.168.1.254:45678",
                Some("203.0.113.1, 10.0.0.1")
            )
            .remote_ip
            .to_string(),
        "203.0.113.1"
    );
}
#[test]
fn test_all_invalid_cidrs() {
    let policy = bound_policy(false);
    let store = trust_store(
        Some((&["invalid-cidr", "not-an-ip/24"], "Namespace")),
        Some((&[], "Namespace")),
    );
    assert_eq!(
        policy
            .resolve_in(&store.capture_snapshot(), "192.168.1.1:45678", Some("203.0.113.1"))
            .remote_ip
            .to_string(),
        "192.168.1.1"
    );
}

#[tokio::test]
async fn unavailable_plugin_trust_replaces_an_earlier_global_remote_ip_with_effective_peer() {
    use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
    let plugin = create_plugin(&create_test_config());
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("x-forwarded-for", "203.0.113.1")]);
    let view = Arc::new(EdgionConfigDataStore::new().capture_snapshot());
    session.expect_ensure_config_data_view().times(1).return_const(());
    session
        .expect_ip_list_availability()
        .returning(move |key| view.ip_list_availability(key));
    session.expect_ip_list_contains().times(0);
    session
        .expect_downstream_peer_ip()
        .times(1)
        .return_const("192.168.1.254".to_string());
    session
        .expect_remote_ip()
        .times(1)
        .return_const("203.0.113.9".to_string());
    session
        .expect_set_remote_ip()
        .with(eq("192.168.1.254"))
        .times(1)
        .returning(|_| Ok(()));
    let mut log = PluginLog::new(plugin.name());
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn mapped_ipv6_keeps_text_comparison_and_setter_boundary() {
    const MAPPED: &str = "::ffff:203.0.113.1";
    for (current, changed) in [("203.0.113.1", true), (MAPPED, false)] {
        let plugin = create_plugin(&create_test_config());
        let mut session = MockPluginSession::new();
        trust_mock(&mut session);
        setup_header_mocks(&mut session, &[("x-forwarded-for", MAPPED)]);
        session
            .expect_downstream_peer_ip()
            .times(1)
            .return_const("192.168.1.254".to_string());
        session.expect_remote_ip().times(1).return_const(current.to_string());
        session
            .expect_set_remote_ip()
            .with(eq(MAPPED))
            .times(usize::from(changed))
            .returning(|_| Ok(()));
        let mut log = PluginLog::new(plugin.name());
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        let serialized = serde_json::to_value(&log).unwrap();
        let entries = serialized["log"].as_array().unwrap();
        assert_eq!(entries[0], "header xff");
        assert_eq!(entries[1], "trusted proxy:match");
        if changed {
            assert!(entries[2].as_str().unwrap().starts_with("ip updated: "));
        } else {
            assert_eq!(entries[2], "ip unchanged");
        }
    }
}
