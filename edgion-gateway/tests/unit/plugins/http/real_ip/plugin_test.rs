use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use mockall::predicate::eq;
use serde_json::json;

fn create_test_config() -> RealIpConfig {
    use edgion_resources::resources::edgion_config_data::IpGroup;
    RealIpConfig {
        trusted_ips: vec![IpGroup {
            name: "private".to_string(),
            description: None,
            cidrs: vec!["192.168.0.0/16".to_string(), "198.51.100.0/24".to_string()],
        }],
        real_ip_header: "X-Forwarded-For".to_string(),
        recursive: true,
        max_trusted_hops: None,
    }
}

#[test]
fn test_extract_recursive() {
    let config = create_test_config(); // recursive: true
    let extractor = RealIpExtractor::new(
        &config.trusted_ips,
        config.real_ip_header.clone(),
        config.recursive,
        config.max_trusted_hops,
    )
    .unwrap();

    // Case 1: Right-to-left, find first non-trusted
    let result = extractor.extract_from_header_value("192.168.1.254", "203.0.113.1, 198.51.100.2, 192.168.1.1");
    assert_eq!(result, "203.0.113.1");

    // Case 2: All trusted, use leftmost (Nginx behavior)
    let result = extractor.extract_from_header_value("192.168.1.254", "192.168.1.1, 198.51.100.2");
    assert_eq!(result, "192.168.1.1");

    // Case 3: Single untrusted IP
    let result = extractor.extract_from_header_value("192.168.1.254", "8.8.8.8");
    assert_eq!(result, "8.8.8.8");
}

#[test]
fn test_client_not_trusted() {
    let config = create_test_config();
    let extractor = RealIpExtractor::new(
        &config.trusted_ips,
        config.real_ip_header.clone(),
        config.recursive,
        config.max_trusted_hops,
    )
    .unwrap();

    // An untrusted downstream peer remains the RemoteIp.
    let result = extractor.extract_from_header_value("8.8.8.8", "203.0.113.1");
    assert_eq!(result, "8.8.8.8");
}

#[test]
fn name_and_header_log_are_prepared_once() {
    let first = RealIp::create(&create_test_config());
    let second = RealIp::create(&create_test_config());
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

        let plugin = RealIp::create(&create_test_config());
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("x-forwarded-for", "203.0.113.1, 192.168.1.1")]);
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
        assert_eq!(entries[0], json!("header xff"));
        assert_eq!(entries[1], json!("trusted proxy:match"));
        assert!(entries[2].as_str().unwrap().starts_with("ip updated: "));
        assert!(!serde_json::to_string(&log).unwrap().contains("203.0.113.1"));
    })
    .await;
}

#[tokio::test]
async fn missing_header_keeps_remote_ip_without_calling_setter() {
    let plugin = RealIp::create(&create_test_config());
    let mut session = MockPluginSession::new();
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
    let plugin = RealIp::create(&create_test_config());
    let mut session = MockPluginSession::new();
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
        let plugin = RealIp::create(&create_test_config());
        let mut session = MockPluginSession::new();
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
    let plugin = RealIp::create(&create_test_config());
    let mut session = MockPluginSession::new();
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

#[tokio::test]
async fn missing_extractor_does_not_read_session() {
    let plugin = RealIp::create(&RealIpConfig::default());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new(plugin.name());

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(serde_json::to_value(&log).unwrap()["log"], json!(["err: no extractor"]));
}
