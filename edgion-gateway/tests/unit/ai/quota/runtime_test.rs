use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

const TEST_NOW_MS: u128 = 1_700_000_000_000;

fn quota_config(on_exceeded: serde_json::Value) -> AiTokenQuotaConfig {
    serde_json::from_value(json!({
        "redisRef": "tenant/redis",
        "key": [],
        "limits": [{
            "scope": "identityModel",
            "tokens": 100,
            "window": "1m"
        }],
        "onExceeded": on_exceeded
    }))
    .unwrap()
}

#[test]
fn redis_failure_policy_maps_explicit_allow_and_deny() {
    for (policy, expected_bypass) in [(OnRedisFailure::Allow, true), (OnRedisFailure::Deny, false)] {
        let mut config = quota_config(json!({"type": "Reject"}));
        config.on_redis_failure = policy;
        let plan = CompiledAiQuotaPlan::compile(&config, "scope").unwrap();
        assert_eq!(
            matches!(plan.redis_failure(), QuotaAdmission::DependencyBypass),
            expected_bypass
        );
        assert_eq!(
            matches!(plan.redis_failure(), QuotaAdmission::DependencyFailed),
            !expected_bypass
        );
    }
}

#[test]
fn quota_actions_are_frozen_during_compile() {
    let throttle =
        CompiledAiQuotaPlan::compile(&quota_config(json!({"type": "Throttle", "maxDelay": "250ms"})), "scope").unwrap();
    assert_eq!(
        throttle.on_exceeded(),
        &QuotaExceededAction::Throttle {
            max_delay: Duration::from_millis(250)
        }
    );

    let downgrade =
        CompiledAiQuotaPlan::compile(&quota_config(json!({"type": "Downgrade", "model": "economy"})), "scope").unwrap();
    assert_eq!(
        downgrade.on_exceeded(),
        &QuotaExceededAction::Downgrade {
            model: "economy".to_string()
        }
    );
}

struct ScriptedThrottleAdmission {
    calls: AtomicUsize,
    second_is_exhausted: bool,
}

impl QuotaAdmissionSource for ScriptedThrottleAdmission {
    async fn admit_once(
        &self,
        _session: &mut dyn PluginSession,
        _log: &mut PluginLog,
        _logical_model: &str,
        _maximum_request_duration: Duration,
    ) -> QuotaAdmission {
        match self.calls.fetch_add(1, Ordering::Relaxed) {
            0 => QuotaAdmission::Exhausted(Duration::from_millis(40)),
            1 if self.second_is_exhausted => QuotaAdmission::Exhausted(Duration::from_millis(25)),
            1 => QuotaAdmission::Bypass,
            call => panic!("one-shot throttle made unexpected admission call {call}"),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn throttle_waits_then_readmits_exactly_once() {
    let source = ScriptedThrottleAdmission {
        calls: AtomicUsize::new(0),
        second_is_exhausted: false,
    };
    let mut session = MockPluginSession::new();
    session
        .expect_ctx()
        .times(2)
        .return_const(crate::EdgionHttpContext::new());
    let mut log = PluginLog::new("AiProxy");
    let started = tokio::time::Instant::now();

    let admission = admit_with_one_shot_throttle(
        &source,
        &QuotaExceededAction::Throttle {
            max_delay: Duration::from_millis(100),
        },
        &mut session,
        &mut log,
        "model",
        Duration::from_secs(30),
    )
    .await;

    assert!(matches!(admission, QuotaAdmission::Bypass));
    assert_eq!(source.calls.load(Ordering::Relaxed), 2);
    assert_eq!(started.elapsed(), Duration::from_millis(40));
}

#[tokio::test(start_paused = true)]
async fn throttle_returns_second_exhaustion_without_looping() {
    let source = ScriptedThrottleAdmission {
        calls: AtomicUsize::new(0),
        second_is_exhausted: true,
    };
    let mut session = MockPluginSession::new();
    session
        .expect_ctx()
        .times(2)
        .return_const(crate::EdgionHttpContext::new());
    let mut log = PluginLog::new("AiProxy");
    let started = tokio::time::Instant::now();

    let admission = admit_with_one_shot_throttle(
        &source,
        &QuotaExceededAction::Throttle {
            max_delay: Duration::from_millis(100),
        },
        &mut session,
        &mut log,
        "model",
        Duration::from_secs(30),
    )
    .await;

    let QuotaAdmission::Exhausted(retry_after) = admission else {
        panic!("second exhausted admission must be returned");
    };
    assert_eq!(retry_after, Duration::from_millis(25));
    assert_eq!(source.calls.load(Ordering::Relaxed), 2);
    assert_eq!(started.elapsed(), Duration::from_millis(40));
}

#[test]
fn throttle_action_uses_one_absolute_policy_and_route_budget() {
    assert_eq!(
        throttle_action_budget(Duration::from_millis(250), Some(Duration::from_secs(1))),
        Duration::from_millis(250)
    );
    assert_eq!(
        throttle_action_budget(Duration::from_secs(3), Some(Duration::from_millis(40))),
        Duration::from_millis(40)
    );
    assert_eq!(
        remaining_throttle_budget(Duration::from_millis(250), Duration::from_millis(200)),
        Duration::from_millis(50)
    );
    assert_eq!(
        remaining_throttle_budget(Duration::from_millis(250), Duration::from_millis(300)),
        Duration::ZERO
    );
}

#[test]
fn quota_keys_hash_identity_and_share_one_cluster_slot() {
    let limits = vec![CompiledLimit {
        tokens: 100,
        window_ms: 60_000,
        scope: AiTokenQuotaScope::IdentityModel,
    }];
    let targets = build_targets(
        "tenant/plugins/#0",
        "sensitive-consumer",
        "model-a",
        "request1",
        &limits,
        Duration::from_secs(300),
        Duration::from_millis(500),
        TEST_NOW_MS,
    );
    assert_eq!(targets.len(), 1);
    assert!(!targets[0].bucket_key.contains("sensitive-consumer"));
    let bucket_slot = targets[0].bucket_key.split(['{', '}']).nth(1).unwrap();
    let request_slot = targets[0].request_key.split(['{', '}']).nth(1).unwrap();
    assert_eq!(bucket_slot, request_slot);
}

#[test]
fn identity_model_scope_separates_requested_names_but_identity_scope_does_not() {
    let identity_limits = vec![CompiledLimit {
        tokens: 100,
        window_ms: 60_000,
        scope: AiTokenQuotaScope::Identity,
    }];
    let model_limits = vec![CompiledLimit {
        scope: AiTokenQuotaScope::IdentityModel,
        ..identity_limits[0].clone()
    }];
    let identity_a = build_targets(
        "scope",
        "consumer",
        "model-a",
        "r1",
        &identity_limits,
        Duration::ZERO,
        Duration::from_secs(1),
        TEST_NOW_MS,
    );
    let identity_b = build_targets(
        "scope",
        "consumer",
        "model-b",
        "r1",
        &identity_limits,
        Duration::ZERO,
        Duration::from_secs(1),
        TEST_NOW_MS,
    );
    assert_eq!(identity_a[0].bucket_key, identity_b[0].bucket_key);
    let model_a = build_targets(
        "scope",
        "consumer",
        "model-a",
        "r1",
        &model_limits,
        Duration::ZERO,
        Duration::from_secs(1),
        TEST_NOW_MS,
    );
    let model_b = build_targets(
        "scope",
        "consumer",
        "model-b",
        "r1",
        &model_limits,
        Duration::ZERO,
        Duration::from_secs(1),
        TEST_NOW_MS,
    );
    assert_ne!(model_a[0].bucket_key, model_b[0].bucket_key);
}

#[test]
fn quota_ttl_counts_down_from_one_absolute_deadline() {
    let now = Instant::now();
    let deadline = now + Duration::from_secs(10);
    assert_eq!(remaining_ttl(deadline, now), Some(Duration::from_secs(10)));
    assert_eq!(
        remaining_ttl(deadline, now + Duration::from_secs(4)),
        Some(Duration::from_secs(6))
    );
    assert_eq!(remaining_ttl(deadline, now + Duration::from_secs(11)), None);
}

#[test]
fn quota_policy_identity_is_stable_across_reorder_and_limit_changes() {
    let first = CompiledLimit {
        tokens: 100,
        window_ms: 60_000,
        scope: AiTokenQuotaScope::Identity,
    };
    let second = CompiledLimit {
        tokens: 200,
        window_ms: 3_600_000,
        scope: AiTokenQuotaScope::IdentityModel,
    };
    let original = build_targets(
        "scope",
        "consumer",
        "model-a",
        "r1",
        &[first.clone(), second.clone()],
        Duration::from_secs(300),
        Duration::from_millis(500),
        TEST_NOW_MS,
    );
    for (limit, target) in [first.clone(), second.clone()].iter().zip(&original) {
        let single = build_targets(
            "scope",
            "consumer",
            "model-a",
            "r1",
            std::slice::from_ref(limit),
            Duration::from_secs(300),
            Duration::from_millis(500),
            TEST_NOW_MS,
        );
        assert_eq!(target.bucket_key, single[0].bucket_key);
        assert_eq!(target.request_key, single[0].request_key);
    }
    let mut changed_limit = first.clone();
    changed_limit.tokens = 999;
    let reordered = build_targets(
        "scope",
        "consumer",
        "model-a",
        "r1",
        &[second, changed_limit],
        Duration::from_secs(300),
        Duration::from_millis(500),
        TEST_NOW_MS,
    );

    assert_eq!(original[0].bucket_key, reordered[1].bucket_key);
    assert_eq!(original[0].request_key, reordered[1].request_key);
    assert_eq!(original[1].bucket_key, reordered[0].bucket_key);
    assert_eq!(original[1].request_key, reordered[0].request_key);
    assert_ne!(original[0].bucket_key, original[1].bucket_key);
}

#[test]
fn quota_window_boundary_rotates_bucket_and_retry_after() {
    let mut config = quota_config(json!({"type": "Reject"}));
    config.limits[0].scope = AiTokenQuotaScope::Identity;
    let plan = CompiledAiQuotaPlan::compile(&config, "scope").unwrap();
    let limits = &plan.limits;
    let build_at = |now_ms| {
        build_targets(
            "scope",
            "consumer",
            "model-a",
            "r1",
            limits,
            Duration::ZERO,
            Duration::from_secs(1),
            now_ms,
        )
    };

    let before = build_at(59_999);
    let boundary = build_at(60_000);
    let same_window = build_at(60_001);

    assert_ne!(before[0].bucket_key, boundary[0].bucket_key);
    assert_eq!(boundary[0].bucket_key, same_window[0].bucket_key);
    assert_eq!(before[0].retry_after, Duration::from_millis(1));
    assert_eq!(boundary[0].retry_after, Duration::from_secs(60));
}

#[test]
fn lease_refresh_is_sparse_and_stays_before_expiry() {
    assert_eq!(lease_refresh_interval(Duration::from_secs(10)), Duration::from_secs(5));
    assert_eq!(
        lease_refresh_interval(Duration::from_secs(1)),
        Duration::from_millis(999)
    );
    assert_eq!(
        lease_refresh_interval(Duration::from_millis(10)),
        Duration::from_millis(9)
    );
}
