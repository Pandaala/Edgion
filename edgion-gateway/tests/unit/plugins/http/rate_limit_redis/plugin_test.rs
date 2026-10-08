use super::*;
use crate::plugins::common::redis_rate_limiter::{build_redis_key, truncate_limit_key, RateLimitResult};
use edgion_resources::common::KeyGet;
use edgion_resources::resources::edgion_plugins::OnKeyMiss;
use mockall::Sequence;
use parking_lot::Mutex;

#[test]
fn test_truncate_limit_key_utf8_boundary() {
    // "" = 12 bytes in UTF-8 (3 bytes per char)
    let key = "";
    // Truncating at 7 bytes should not split a 3-byte char
    let truncated = truncate_limit_key(key, 7);
    assert!(truncated.len() <= 7);
    assert_eq!(truncated, ""); // 6 bytes, safe boundary
}

#[test]
fn test_truncate_limit_key_over() {
    let key = "a".repeat(5000);
    assert_eq!(truncate_limit_key(&key, 4096).len(), 4096);
}

#[test]
fn test_build_redis_key_sliding_window() {
    let key = build_redis_key("edgion:rl:", "sw", 0, "192.168.1.1", "");
    assert_eq!(key, "edgion:rl:sw:0:192.168.1.1");
}

#[test]
fn test_rate_limit_result_from_lua_denied() {
    let result = RateLimitResult::from_lua_result(&[0, 100, 0, 500], 100).unwrap();
    assert!(!result.allowed);
    assert_eq!(result.current, 100);
    assert_eq!(result.remaining, 0);
    assert_eq!(result.retry_ms, 500);
}

#[test]
fn test_merge_result_one_denied() {
    let r1 = RateLimitResult {
        allowed: true,
        current: 5,
        remaining: 95,
        limit: 100,
        retry_ms: 0,
    };
    let r2 = RateLimitResult {
        allowed: false,
        current: 1001,
        remaining: 0,
        limit: 1000,
        retry_ms: 3000,
    };
    let merged = RateLimitResult::merge(Some(r1), r2);
    assert!(!merged.allowed);
    assert_eq!(merged.remaining, 0);
    assert_eq!(merged.retry_ms, 3000);
}

#[test]
fn scoped_key_prefix_namespaces_by_entry_identity() {
    let config = RateLimitRedisConfig::default();

    let a = RateLimitRedis::new(&config, "default/limiter-a/#0");
    let b = RateLimitRedis::new(&config, "default/limiter-b/#0");
    let a_second_entry = RateLimitRedis::new(&config, "default/limiter-a/#1");

    // The bucket namespace names the limiter, so an identical config in two
    // resources (or twice in one resource) never shares Redis keys.
    assert_eq!(a.scoped_key_prefix, "edgion:rl:default/limiter-a/#0:");
    assert_ne!(a.scoped_key_prefix, b.scoped_key_prefix);
    assert_ne!(a.scoped_key_prefix, a_second_entry.scoped_key_prefix);
}

#[tokio::test]
async fn graphql_body_override_preserves_canonical_retry_and_quota_headers() {
    use crate::plugins::http::common::graphql_request;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let config = unreachable_redis_config("default/graphql-reply-test", OnRedisFailure::Deny);
    let ordinary = RateLimitRedis::new(&config, "default/reply/#0");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&ordinary.reject_body).unwrap(),
        serde_json::json!({"message":"Rate limit exceeded"})
    );
    let body = graphql_request::error_body(429);
    let plugin = ordinary.with_json_rejection_body(body.clone());
    for retry in [Duration::ZERO, Duration::from_millis(500)] {
        let mut session = MockPluginSession::new();
        session.expect_set_response_header().times(1).returning(|name, value| {
            assert_eq!(name, "Content-Type");
            assert_eq!(value, "application/json");
            Ok(())
        });
        session
            .expect_write_response_header()
            .times(1)
            .returning(move |header, end| {
                assert!(!end);
                assert_eq!(header.status.as_u16(), 429);
                assert_eq!(header.headers["content-type"], "application/json");
                assert_eq!(header.headers["X-RateLimit-Limit"], "100");
                assert_eq!(header.headers["X-RateLimit-Remaining"], "0");
                if retry.is_zero() {
                    assert!(!header.headers.contains_key("Retry-After"));
                } else {
                    assert_eq!(header.headers["Retry-After"], "1");
                }
                Ok(())
            });
        let expected = body.clone();
        session
            .expect_write_response_body()
            .times(1)
            .returning(move |actual, end| {
                assert!(end);
                assert_eq!(actual.as_ref(), Some(&expected));
                Ok(())
            });
        let result = RateLimitResult {
            allowed: false,
            current: 100,
            remaining: 0,
            limit: 100,
            retry_ms: 500,
        };
        assert_eq!(
            plugin.reject_request(&mut session, retry, Some(&result)).await,
            PluginRunningResult::ErrTerminateRequest
        );
    }
}

#[test]
fn test_format_duration_milliseconds() {
    assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
    assert_eq!(format_duration(Duration::from_millis(50)), "50ms");
}

#[test]
fn absolute_reset_timestamp_ceils_sub_second_deadline() {
    assert_eq!(
        absolute_reset_timestamp_secs(UNIX_EPOCH, Duration::from_millis(500)),
        Some(1)
    );
}

#[test]
fn absolute_reset_timestamp_includes_fractional_current_second() {
    let now = UNIX_EPOCH + Duration::from_millis(1_750);
    assert_eq!(absolute_reset_timestamp_secs(now, Duration::from_millis(500)), Some(3));
}

#[test]
fn absolute_reset_timestamp_preserves_exact_second_deadline() {
    let now = UNIX_EPOCH + Duration::from_secs(10);
    assert_eq!(absolute_reset_timestamp_secs(now, Duration::from_secs(2)), Some(12));
}

#[test]
fn absolute_reset_timestamp_omits_pre_epoch_deadline() {
    let now = UNIX_EPOCH - Duration::from_nanos(1);
    assert_eq!(absolute_reset_timestamp_secs(now, Duration::ZERO), None);
}

#[test]
fn absolute_reset_timestamp_omits_unrepresentable_deadline() {
    assert_eq!(absolute_reset_timestamp_secs(SystemTime::now(), Duration::MAX), None);
}

#[tokio::test]
async fn construction_freezes_config_validity_and_invalid_runs_fail_before_dependencies() {
    use crate::plugins::runtime::traits::session::MockPluginSession;

    let valid_config = unreachable_redis_config("default/rlr-valid-config-test", OnRedisFailure::Allow);
    let valid_plugin = RateLimitRedis::new(&valid_config, "default/test-plugins/#0");
    assert!(valid_plugin.config_valid);

    // Invalid redis_ref (missing namespace slash) makes the config invalid.
    let config = RateLimitRedisConfig {
        redis_ref: "invalid".to_string(),
        ..Default::default()
    };
    let plugin = RateLimitRedis::new(&config, "default/test-plugins/#0");
    assert!(!plugin.config_valid);

    let mut mock_session = MockPluginSession::new();
    let mut sequence = Sequence::new();
    for _ in 0..2 {
        mock_session
            .expect_write_response_header()
            .withf(|response, end_of_stream| {
                !*end_of_stream
                    && response.status.as_u16() == 500
                    && response
                        .headers
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        == Some("application/json")
            })
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        mock_session
            .expect_write_response_body()
            .withf(|body, end_of_stream| {
                *end_of_stream && body.as_deref() == Some(br#"{"message":"Internal rate limiter error"}"#.as_slice())
            })
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
    }

    // No key/Redis/outcome expectations are installed: either dependency
    // path would panic the mock before the fixed defensive response.
    for _ in 0..2 {
        let mut plugin_log = PluginLog::new("RateLimitRedis");
        assert_eq!(
            plugin.run_request(&mut mock_session, &mut plugin_log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert_eq!(plugin_log.render_for_test(), "err: cfg load fail");
    }
}

// ── Task 9: dye-suite record_outcome wiring ─────────────────────────────

/// A validly-shaped config that is guaranteed to be `client.healthy() ==
/// false` / `get_redis_client == None` in the unit-test process (no
/// LinkSys runtime is ever registered under these names), so `run_request`
/// falls through to `apply_redis_failure_policy` deterministically.
fn unreachable_redis_config(redis_ref: &str, on_redis_failure: OnRedisFailure) -> RateLimitRedisConfig {
    RateLimitRedisConfig {
        redis_ref: redis_ref.to_string(),
        policies: vec![RateLimitPolicy {
            rate: 100,
            interval: "1s".to_string(),
            algorithm: RateLimitAlgorithm::default(),
            interval_duration: None,
        }],
        on_redis_failure,
        ..Default::default()
    }
}

fn missing_key_deny_config(redis_ref: &str, reject_message: Option<&str>) -> RateLimitRedisConfig {
    let mut config = unreachable_redis_config(redis_ref, OnRedisFailure::Allow);
    config.key = vec![KeyGet::Header {
        name: "x-missing".to_string(),
    }];
    config.on_key_miss = OnKeyMiss::Deny;
    config.reject_message = reject_message.map(str::to_string);
    config
}

#[test]
fn plugin_name_is_shared_static_storage() {
    let config = unreachable_redis_config("default/rlr-name-test", OnRedisFailure::Allow);
    let plugin1 = RateLimitRedis::create(&config, "default/test-plugins/#0");
    let plugin2 = RateLimitRedis::create(&config, "default/test-plugins/#1");

    assert_eq!(plugin1.name(), "RateLimitRedis");
    assert!(std::ptr::eq(plugin1.name(), plugin2.name()));
}

#[tokio::test]
async fn reject_body_matches_previous_json_encoding_without_mutating_caller_config() {
    use crate::plugins::runtime::traits::session::MockPluginSession;

    let message = "Too \"many\"\nrequests";
    let config = missing_key_deny_config("default/rlr-message-test", Some(message));
    let caller_snapshot = serde_json::to_value(&config).expect("config should serialize");
    let expected = Bytes::from(serde_json::json!({"message": message}).to_string());
    let plugin = RateLimitRedis::create(&config, "default/test-plugins/#0");

    assert_eq!(serde_json::to_value(&config).unwrap(), caller_snapshot);
    assert_eq!(config.reject_message.as_deref(), Some(message));
    assert!(config.policies[0].interval_duration.is_none());

    let captured_body = Arc::new(Mutex::new(None));
    let captured_body_for_mock = Arc::clone(&captured_body);
    let mut session = MockPluginSession::new();
    session.expect_key_get().return_const(None::<String>);
    session.expect_write_response_header().times(1).returning(|_, end| {
        assert!(!end);
        Ok(())
    });
    session
        .expect_write_response_body()
        .times(1)
        .returning(move |body, end| {
            assert!(end);
            *captured_body_for_mock.lock() = body;
            Ok(())
        });

    let mut plugin_log = PluginLog::new("RateLimitRedis");
    assert_eq!(
        plugin.run_request(&mut session, &mut plugin_log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_eq!(plugin_log.render_for_test(), "Deny key miss");
    assert_eq!(captured_body.lock().as_ref(), Some(&expected));
}

#[tokio::test]
async fn repeated_missing_key_denials_clone_the_precomputed_body() {
    use crate::plugins::runtime::traits::session::MockPluginSession;

    let config = missing_key_deny_config("default/rlr-body-clone-test", None);
    let plugin = RateLimitRedis::create(&config, "default/test-plugins/#0");
    let captured_bodies = Arc::new(Mutex::new(Vec::new()));
    let captured_bodies_for_mock = Arc::clone(&captured_bodies);
    let mut session = MockPluginSession::new();
    session.expect_key_get().times(2).return_const(None::<String>);
    session.expect_write_response_header().times(2).returning(|_, end| {
        assert!(!end);
        Ok(())
    });
    session
        .expect_write_response_body()
        .times(2)
        .returning(move |body, end| {
            assert!(end);
            captured_bodies_for_mock
                .lock()
                .push(body.expect("rejection must have a body"));
            Ok(())
        });

    for _ in 0..2 {
        let mut plugin_log = PluginLog::new("RateLimitRedis");
        assert_eq!(
            plugin.run_request(&mut session, &mut plugin_log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert_eq!(plugin_log.render_for_test(), "Deny key miss");
    }

    let bodies = captured_bodies.lock();
    assert_eq!(bodies.len(), 2);
    assert!(!bodies[0].is_empty());
    assert_eq!(bodies[0], bodies[1]);
    assert!(std::ptr::eq(bodies[0].as_ptr(), bodies[1].as_ptr()));
}

#[tokio::test]
async fn redis_unreachable_fail_open_records_outcome() {
    use crate::plugins::runtime::traits::session::MockPluginSession;

    let config = unreachable_redis_config("default/rlr-fail-open-test", OnRedisFailure::Allow);
    let plugin = RateLimitRedis::create(&config, "default/test-plugins/#0");

    let mut mock_session = MockPluginSession::new();
    mock_session.expect_key_get().return_const(Some("1.2.3.4".to_string()));
    mock_session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "redis-unreachable")
        .times(1)
        .return_const(());

    let mut plugin_log = PluginLog::new("RateLimitRedis");
    let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn redis_unreachable_fail_close_records_outcome_before_response_write() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use mockall::Sequence;

    let config = unreachable_redis_config("default/rlr-fail-close-test", OnRedisFailure::Deny);
    let plugin = RateLimitRedis::create(&config, "default/test-plugins/#0");

    let mut mock_session = MockPluginSession::new();
    mock_session.expect_key_get().return_const(Some("1.2.3.4".to_string()));

    // Ordering is the whole point of Task 9/10: the dye record must
    // precede the plugin's own `write_response_header` self-write, or
    // the queued header op never reaches the response.
    let mut seq = Sequence::new();
    mock_session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "redis-unreachable")
        .times(1)
        .in_sequence(&mut seq)
        .return_const(());
    mock_session
        .expect_write_response_header()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    mock_session.expect_write_response_body().returning(|_, _| Ok(()));

    let mut plugin_log = PluginLog::new("RateLimitRedis");
    let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

/// A normal rate-limit rejection (Redis reachable, over-limit) is a policy
/// decision, not a dependency failure — it must NOT call `record_outcome`.
/// Any call panics the mock (no `expect_record_outcome` installed).
#[tokio::test]
async fn normal_rate_limit_rejection_does_not_record_outcome() {
    use crate::plugins::runtime::traits::session::MockPluginSession;

    // redis_ref left invalid on purpose is not usable here (that's the
    // cfg-load-fail path, also un-recorded but for a different reason);
    // instead we exercise the `RateLimitKey::Deny` short-circuit, which
    // never reaches Redis and is unambiguously a policy decision.
    let config = RateLimitRedisConfig {
        redis_ref: "default/rlr-deny-key-test".to_string(),
        policies: vec![RateLimitPolicy {
            rate: 100,
            interval: "1s".to_string(),
            algorithm: RateLimitAlgorithm::default(),
            interval_duration: None,
        }],
        key: vec![edgion_resources::common::KeyGet::Header {
            name: "x-missing".to_string(),
        }],
        on_key_miss: edgion_resources::resources::edgion_plugins::OnKeyMiss::Deny,
        ..Default::default()
    };
    let plugin = RateLimitRedis::create(&config, "default/test-plugins/#0");

    let mut mock_session = MockPluginSession::new();
    mock_session.expect_key_get().return_const(None::<String>);
    mock_session.expect_write_response_header().returning(|_, _| Ok(()));
    mock_session.expect_write_response_body().returning(|_, _| Ok(()));
    // No `expect_record_outcome` — any call panics.

    let mut plugin_log = PluginLog::new("RateLimitRedis");
    let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}
