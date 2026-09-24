use super::*;
use crate::plugins::stream::stream_plugin_trait::StreamLayer;
use edgion_resources::resources::edgion_stream_plugins::{
    ConnectionRateLimitConfig, PerListenerLimit, PerSourceIpLimit,
};
use std::net::IpAddr;

fn cfg_local_only(rate: u64) -> ConnectionRateLimitConfig {
    ConnectionRateLimitConfig {
        redis_ref: None,
        algorithm: RateLimitAlgorithm::SlidingWindow,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: Some(PerListenerLimit {
            rate,
            interval: "1s".to_string(),
            redis_distributed: false,
            on_redis_failure: OnRedisFailure::Deny,
            interval_duration: None,
        }),
        per_source_ip: None,
        redis_timeout: "100ms".to_string(),
    }
}

#[tokio::test]
async fn reload_keeps_local_budget_independent_of_redis_prefix_and_old_parameters() {
    let mut cfg = cfg_local_only(1);
    cfg.per_listener.as_mut().unwrap().interval = "1h".to_string();
    let scope = "default/local-reload-prefix/#0";
    let ctx = StreamContext::new(IpAddr::from([10, 0, 0, 1]), 64120, StreamLayer::Stage1);
    let old = ConnectionRateLimitPlugin::new(&cfg, scope);
    assert!(matches!(old.on_connection(&ctx).await, StreamPluginResult::Allow));

    cfg.key_prefix = "different:".to_string();
    let prefix_only = ConnectionRateLimitPlugin::new(&cfg, scope);
    assert_ne!(prefix_only.scoped_key_prefix, old.scoped_key_prefix);
    assert!(matches!(
        prefix_only.on_connection(&ctx).await,
        StreamPluginResult::Deny(_)
    ));

    cfg.per_listener.as_mut().unwrap().rate = 3;
    let raised = ConnectionRateLimitPlugin::new(&cfg, scope);
    assert_eq!(old.pl_rate, Some(1));
    assert_eq!(raised.pl_rate, Some(3));
    assert!(matches!(raised.on_connection(&ctx).await, StreamPluginResult::Allow));
    assert!(matches!(old.on_connection(&ctx).await, StreamPluginResult::Allow));
    for plugin in [&prefix_only, &raised, &old, &raised] {
        assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
    }
}

#[tokio::test]
async fn redis_denial_does_not_refund_local_token_and_next_attempt_stops_locally() {
    let mut cfg = cfg_local_only(1);
    let listener = cfg.per_listener.as_mut().unwrap();
    listener.interval = "1h".to_string();
    listener.redis_distributed = true;
    cfg.redis_ref = Some("default/local-gate-test-unavailable".to_string());
    let plugin = ConnectionRateLimitPlugin::new(&cfg, "default/local-before-redis/#0");
    let ctx = StreamContext::new(IpAddr::from([10, 0, 0, 1]), 64121, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await,
            StreamPluginResult::Deny(reason) if reason == "connrate:deny:listener:redis"));
    assert!(matches!(plugin.on_connection(&ctx).await,
            StreamPluginResult::Deny(reason) if reason == "connrate:deny:listener:local"));
}

#[tokio::test]
async fn redis_source_failure_default_allow_preserves_local_enforcement() {
    let mut cfg = cfg_local_only(1);
    cfg.per_listener.as_mut().unwrap().interval = "1h".to_string();
    cfg.redis_ref = Some("default/local-source-test-unavailable".to_string());
    cfg.per_source_ip = Some(PerSourceIpLimit {
        rate: 1,
        interval: "1s".to_string(),
        on_redis_failure: OnRedisFailure::Allow,
        interval_duration: None,
    });
    let plugin = ConnectionRateLimitPlugin::new(&cfg, "default/local-source-failure/#0");
    let ctx = StreamContext::new(IpAddr::from([10, 0, 0, 1]), 64122, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow));
    assert!(matches!(plugin.on_connection(&ctx).await,
            StreamPluginResult::Deny(reason) if reason == "connrate:deny:listener:local"));
}

#[tokio::test]
async fn exhausted_generation_denies_new_runtime_without_disabling_live_one() {
    let mut cfg = cfg_local_only(2);
    cfg.per_listener.as_mut().unwrap().interval = "1h".to_string();
    let scope = "default/local-generation-exhaustion/#0";
    let old = ConnectionRateLimitPlugin::new(&cfg, scope);
    old.local_gate.as_ref().unwrap().exhaust_generations_for_test();
    let exhausted = ConnectionRateLimitPlugin::new(&cfg, scope);
    assert!(exhausted.init_failed);
    assert!(exhausted.local_gate.is_none());
    let ctx = StreamContext::new(IpAddr::from([10, 0, 0, 1]), 64123, StreamLayer::Stage1);
    assert!(matches!(exhausted.on_connection(&ctx).await,
            StreamPluginResult::Deny(reason) if reason == "connrate:deny:invalid-config"));
    assert!(matches!(old.on_connection(&ctx).await, StreamPluginResult::Allow));
    assert!(matches!(old.on_connection(&ctx).await, StreamPluginResult::Allow));
    assert!(matches!(old.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
}

#[test]
fn redis_timeout_is_parsed_from_config() {
    let mut cfg = cfg_local_only(10);
    cfg.redis_timeout = "750ms".to_string();
    let plugin = ConnectionRateLimitPlugin::new(&cfg, "default/test-esp/#0");
    assert_eq!(plugin.redis_timeout, std::time::Duration::from_millis(750));
}

#[test]
fn redis_timeout_is_clamped_to_cap() {
    let mut cfg = cfg_local_only(10);
    cfg.redis_timeout = "30s".to_string();
    let plugin = ConnectionRateLimitPlugin::new(&cfg, "default/test-esp/#0");
    assert_eq!(plugin.redis_timeout, std::time::Duration::from_millis(1000));
}

#[tokio::test]
async fn local_gate_denies_over_rate() {
    // Use a unique port so the global registry slot is isolated from other tests.
    let port = 64111u16;
    let plugin = ConnectionRateLimitPlugin::new(&cfg_local_only(2), "default/test-esp/#0");
    let ctx = StreamContext::new(IpAddr::from([10, 0, 0, 1]), port, StreamLayer::Stage1);
    let mut admitted = 0;
    for _ in 0..5 {
        if matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow) {
            admitted += 1;
        }
    }
    // capacity 2 in a 1s window, all within the same test instant
    assert!(admitted <= 2, "expected <=2 admits, got {admitted}");
}

#[tokio::test]
async fn invalid_config_without_dimension_denies_before_runtime_work() {
    let mut cfg = cfg_local_only(1);
    cfg.per_listener = None;
    let plugin = ConnectionRateLimitPlugin::new(&cfg, "default/invalid-empty/#0");
    let ctx = StreamContext::new(IpAddr::from([10, 0, 0, 1]), 64112, StreamLayer::Stage1);

    assert!(matches!(
        plugin.on_connection(&ctx).await,
        StreamPluginResult::Deny(reason) if reason == "connrate:deny:invalid-config"
    ));
}

#[tokio::test]
async fn missing_redis_ref_denies_even_when_failure_policy_defaults_allow() {
    let mut cfg = cfg_local_only(1);
    cfg.per_listener = None;
    cfg.per_source_ip = Some(PerSourceIpLimit {
        rate: 1,
        interval: "1s".to_string(),
        on_redis_failure: OnRedisFailure::Allow,
        interval_duration: None,
    });
    let plugin = ConnectionRateLimitPlugin::new(&cfg, "default/invalid-redis/#0");
    let ctx = StreamContext::new(IpAddr::from([10, 0, 0, 1]), 64113, StreamLayer::Stage1);

    assert!(matches!(
        plugin.on_connection(&ctx).await,
        StreamPluginResult::Deny(reason) if reason == "connrate:deny:invalid-config"
    ));
}
