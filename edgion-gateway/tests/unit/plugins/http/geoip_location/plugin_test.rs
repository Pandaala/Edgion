use super::*;
use crate::geoip::{inject_lookup_fault, GeoIpProvider, GeoLookup, LookupFault};
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::EdgionHttpContext;
use edgion_resources::resources::edgion_plugins::plugin_configs::{DefaultAction, GeoRule, GeoRuleGroup, GeoRuleKey};

fn rule(source: GeoRuleSource, key: GeoRuleKey, values: &[&str]) -> GeoRule {
    GeoRule {
        source,
        key,
        values: values.iter().map(|s| s.to_string()).collect(),
        cidr_matcher: None,
    }
}

fn parse_only_config() -> GeoIpLocationConfig {
    GeoIpLocationConfig::default()
}

fn rule_mode_config(fail_open: bool) -> GeoIpLocationConfig {
    GeoIpLocationConfig {
        default_action: DefaultAction::Allow,
        fail_open: Some(fail_open),
        deny_rule_groups: vec![GeoRuleGroup {
            name: Some("block-ru".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
        }],
        allow_rule_groups: vec![],
    }
}

/// A session whose `ctx()` returns a fresh `EdgionHttpContext` and whose
/// `direct_peer_ip`/`remote_ip` are set, so `direct_peer_geo()`/`remote_geo()`
/// resolve live against `GeoIpProvider::global()` (cleared by the caller
/// under `crate::geoip::GEO_TEST_LOCK` for db-unavailable determinism).
fn session_with_ips(direct_peer_ip: &'static str, remote_ip: &'static str) -> MockPluginSession {
    let mut ctx = EdgionHttpContext::new();
    // `ctx.direct_peer_geo()`/`remote_geo()` resolve off `ctx.request_info`, a
    // separate field from the canonical PluginSession methods mocked
    // below — both must be set for
    // the two to agree, as the real session wrapper keeps them in sync.
    ctx.request_info.direct_peer_ip = direct_peer_ip.to_string();
    ctx.request_info.remote_ip = remote_ip.to_string();

    let mut s = MockPluginSession::new();
    s.expect_ctx().return_const(ctx);
    s.expect_direct_peer_ip().return_const(direct_peer_ip.to_string());
    s.expect_remote_ip().return_const(remote_ip.to_string());
    s
}

#[test]
fn name_is_static_across_instances() {
    let first = GeoIpLocation::create(&parse_only_config());
    let second = GeoIpLocation::create(&parse_only_config());

    assert_eq!(first.name(), PLUGIN_NAME);
    assert_eq!(second.name(), PLUGIN_NAME);
    assert!(std::ptr::eq(first.name(), second.name()));
}

#[tokio::test]
async fn parse_only_mode_never_denies_and_needs_no_fail_open() {
    let _lock = crate::geoip::GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();
    let plugin = GeoIpLocation::create(&parse_only_config());
    let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
    let mut log = PluginLog::new("GeoIpLocation");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "parse:ok");
}

#[tokio::test]
async fn db_unavailable_fail_closed_denies() {
    let _lock = crate::geoip::GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();
    let plugin = GeoIpLocation::create(&rule_mode_config(false));
    let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
    session.expect_set_response_header().returning(|name, value| {
        assert_eq!((name, value), ("content-type", "application/json"));
        Ok(())
    });
    let mut log = PluginLog::new("GeoIpLocation");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(br#"{"message":"Access denied"}"#)),
        }
    );
    assert_eq!(log.render_for_test(), "err: db:unavailable fail-closed");
}

#[tokio::test]
async fn db_unavailable_fail_open_passes() {
    let _lock = crate::geoip::GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();
    let plugin = GeoIpLocation::create(&rule_mode_config(true));
    let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
    let mut log = PluginLog::new("GeoIpLocation");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "err: db:unavailable fail-open");
}

#[tokio::test]
async fn cidr_only_config_is_unaffected_by_db_unavailability() {
    let _lock = crate::geoip::GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();
    let cfg = GeoIpLocationConfig {
        default_action: DefaultAction::Allow,
        fail_open: Some(false),
        allow_rule_groups: vec![GeoRuleGroup {
            name: Some("office".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
        }],
        ..Default::default()
    };
    let plugin = GeoIpLocation::create(&cfg);

    let mut allowed_session = session_with_ips("10.1.2.3", "10.1.2.3");
    let mut log = PluginLog::new("GeoIpLocation");
    let allowed = plugin.run_request(&mut allowed_session, &mut log).await;
    assert_eq!(allowed, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "allow");

    let mut denied_session = session_with_ips("8.8.8.8", "8.8.8.8");
    denied_session.expect_set_response_header().returning(|name, value| {
        assert_eq!((name, value), ("content-type", "application/json"));
        Ok(())
    });
    let mut log2 = PluginLog::new("GeoIpLocation");
    let denied = plugin.run_request(&mut denied_session, &mut log2).await;
    assert_eq!(
        denied,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(br#"{"message":"Access denied"}"#)),
        }
    );
    assert_eq!(log2.render_for_test(), "deny");
}

#[tokio::test]
async fn cidr_only_config_is_unaffected_by_lookup_failure() {
    let _lock = crate::geoip::GEO_TEST_LOCK.lock();
    load_fixture_db();
    let _fault = inject_lookup_fault(LookupFault::Decode);
    let cfg = GeoIpLocationConfig {
        default_action: DefaultAction::Allow,
        fail_open: Some(false),
        allow_rule_groups: vec![GeoRuleGroup {
            name: Some("office".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
        }],
        ..Default::default()
    };
    let plugin = GeoIpLocation::create(&cfg);

    let mut allowed_session = session_with_ips("10.1.2.3", "10.1.2.3");
    let mut log = PluginLog::new("GeoIpLocation");
    let allowed = plugin.run_request(&mut allowed_session, &mut log).await;
    assert_eq!(allowed, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "allow");

    let mut denied_session = session_with_ips("8.8.8.8", "8.8.8.8");
    denied_session.expect_set_response_header().returning(|name, value| {
        assert_eq!((name, value), ("content-type", "application/json"));
        Ok(())
    });
    let mut log2 = PluginLog::new("GeoIpLocation");
    let denied = plugin.run_request(&mut denied_session, &mut log2).await;
    assert_eq!(
        denied,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(br#"{"message":"Access denied"}"#)),
        }
    );
    assert_eq!(log2.render_for_test(), "deny");
}

#[tokio::test]
async fn init_failure_fails_closed() {
    let _lock = crate::geoip::GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();
    // Rule mode without failOpen fails validate_and_init at construction.
    let mut cfg = rule_mode_config(false);
    cfg.fail_open = None;
    let plugin = GeoIpLocation::create(&cfg);
    let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
    session.expect_set_response_header().returning(|name, value| {
        assert_eq!((name, value), ("content-type", "application/json"));
        Ok(())
    });
    let mut log = PluginLog::new("GeoIpLocation");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(br#"{"message":"Access denied"}"#)),
        }
    );
    assert_eq!(log.render_for_test(), "err: init fail: fail-closed");
}

fn load_fixture_db() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/geoip/edgion-test-city.mmdb");
    GeoIpProvider::global()
        .load_from(&path)
        .expect("city fixture must load");
}

#[tokio::test]
async fn lookup_failed_fail_closed_denies() {
    let _lock = crate::geoip::GEO_TEST_LOCK.lock();
    load_fixture_db();
    let _fault = inject_lookup_fault(LookupFault::Decode);
    let plugin = GeoIpLocation::create(&rule_mode_config(false));
    let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
    session.expect_set_response_header().returning(|name, value| {
        assert_eq!((name, value), ("content-type", "application/json"));
        Ok(())
    });
    let mut log = PluginLog::new("GeoIpLocation");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(br#"{"message":"Access denied"}"#)),
        }
    );
    assert_eq!(log.render_for_test(), "err: lookup:failed fail-closed");
}

#[tokio::test]
async fn lookup_failed_fail_open_passes() {
    let _lock = crate::geoip::GEO_TEST_LOCK.lock();
    load_fixture_db();
    let _fault = inject_lookup_fault(LookupFault::Decode);
    let plugin = GeoIpLocation::create(&rule_mode_config(true));
    let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
    let mut log = PluginLog::new("GeoIpLocation");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "err: lookup:failed fail-open");
}

#[tokio::test]
async fn parse_only_mode_passes_through_lookup_failure() {
    let _lock = crate::geoip::GEO_TEST_LOCK.lock();
    load_fixture_db();
    let _fault = inject_lookup_fault(LookupFault::Decode);
    let plugin = GeoIpLocation::create(&parse_only_config());
    let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
    let mut log = PluginLog::new("GeoIpLocation");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "parse:ok");
    let (remote, direct) = session.ctx().request_info.resolved_geo();
    assert_eq!(remote, Some(&GeoLookup::Failed));
    assert_eq!(direct, Some(&GeoLookup::Failed));
}
