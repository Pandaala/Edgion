use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::edgion_plugins::OnKeyMiss;
use parking_lot::Mutex;
use std::sync::Arc;

fn create_basic_config() -> RateLimitLocalConfig {
    let mut config = RateLimitLocalConfig {
        rate: 5,
        interval: "1s".to_string(),
        reject_message: Some("Too many requests".to_string()),
        ..Default::default()
    };
    let _ = config.validate();
    config
}

#[test]
fn plugin_name_is_shared_static_storage() {
    let config = create_basic_config();
    let plugin1 = RateLimitLocal::create(&config);
    let plugin2 = RateLimitLocal::create(&config);

    assert_eq!(plugin1.name(), "RateLimitLocal");
    assert!(std::ptr::eq(plugin1.name(), plugin2.name()));
}

#[tokio::test]
async fn reject_body_matches_previous_json_encoding_without_mutating_caller_config() {
    let message = "Too \"many\"\nrequests";
    let config = RateLimitLocalConfig {
        reject_message: Some(message.to_string()),
        on_key_miss: OnKeyMiss::Deny,
        ..Default::default()
    };
    let caller_snapshot = serde_json::to_value(&config).expect("config should serialize");
    let expected = Bytes::from(serde_json::json!({"message": message}).to_string());
    let plugin = RateLimitLocal::create(&config);

    assert_eq!(serde_json::to_value(&config).unwrap(), caller_snapshot);

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

    let mut plugin_log = PluginLog::new("RateLimitLocal");
    assert_eq!(
        plugin.run_request(&mut session, &mut plugin_log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_eq!(captured_body.lock().as_ref(), Some(&expected));
}

#[tokio::test]
async fn repeated_missing_key_denials_clone_the_precomputed_body() {
    let config = RateLimitLocalConfig {
        on_key_miss: OnKeyMiss::Deny,
        ..Default::default()
    };
    let plugin = RateLimitLocal::create(&config);
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
        let mut plugin_log = PluginLog::new("RateLimitLocal");
        assert_eq!(
            plugin.run_request(&mut session, &mut plugin_log).await,
            PluginRunningResult::ErrTerminateRequest
        );
    }

    let bodies = captured_bodies.lock();
    assert_eq!(bodies.len(), 2);
    assert!(!bodies[0].is_empty());
    assert_eq!(bodies[0], bodies[1]);
    assert!(std::ptr::eq(bodies[0].as_ptr(), bodies[1].as_ptr()));
}

#[tokio::test]
async fn test_separate_plugin_instances() {
    // Test that different plugin instances have separate rate limiting state
    let config = create_basic_config();
    let plugin1 = RateLimitLocal::create(&config);
    let plugin2 = RateLimitLocal::create(&config);

    let shared_ip = "shared_ip_test";

    // Exhaust plugin1's limit
    for _ in 0..5 {
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RateLimitLocal");
        mock_session.expect_key_get().return_const(Some(shared_ip.to_string()));
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));
        plugin1.run_request(&mut mock_session, &mut plugin_log).await;
    }

    // plugin2 should still allow (separate Rate instance)
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("RateLimitLocal");
    mock_session.expect_key_get().return_const(Some(shared_ip.to_string()));
    mock_session.expect_set_response_header().returning(|_, _| Ok(()));

    let result = plugin2.run_request(&mut mock_session, &mut plugin_log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(plugin_log.contains("Allow kl="));
}
