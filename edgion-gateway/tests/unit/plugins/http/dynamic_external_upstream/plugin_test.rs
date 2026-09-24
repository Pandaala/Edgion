use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::EdgionHttpContext;
use edgion_resources::common::KeyGet;
use edgion_resources::resources::edgion_plugins::plugin_configs::{DomainTarget, ExtUpstreamExtract};
use mockall::Sequence;
use std::collections::HashMap;

fn base_config() -> DynamicExternalUpstreamConfig {
    DynamicExternalUpstreamConfig {
        from: KeyGet::Header {
            name: "X-Target-Region".to_string(),
        },
        extract: None,
        domain_map: HashMap::from([(
            "us".to_string(),
            DomainTarget {
                domain: "api-us.example.com".to_string(),
                port: Some(8443),
                tls: true,
                override_host: None,
                sni: None,
            },
        )]),
        on_missing: UpstreamFallback::Fallback,
        on_no_match: UpstreamFallback::Fallback,
        compiled_regex: None,
    }
}

#[test]
fn dynamic_external_upstream_name_is_static_across_instances() {
    let first = DynamicExternalUpstream::create(&base_config());
    let mut second_config = base_config();
    second_config.domain_map.insert(
        "eu".to_string(),
        DomainTarget {
            domain: "api-eu.example.com".to_string(),
            port: None,
            tls: false,
            override_host: None,
            sni: None,
        },
    );
    let second = DynamicExternalUpstream::create(&second_config);

    assert_eq!(first.name(), "DynamicExternalUpstream");
    assert_eq!(second.name(), "DynamicExternalUpstream");
}

#[tokio::test]
async fn regex_capture_selects_matching_domain_without_copying_the_key() {
    let config = DynamicExternalUpstreamConfig {
        extract: Some(ExtUpstreamExtract {
            regex: r"^region=(.+)$".to_string(),
            group: 1,
        }),
        ..base_config()
    };
    let plugin = DynamicExternalUpstream::create(&config);

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_key_get().return_const(Some("region=us".to_string()));
    session
        .expect_set_upstream_jump()
        .withf(|target| {
            matches!(
                target,
                UpstreamJumpTarget::External(jump)
                    if jump.domain == "api-us.example.com"
                        && jump.port == 8443
                        && jump.use_tls
                        && jump.sni == "api-us.example.com"
                        && !jump.preserve_raw_path
                        && jump.upstream_read_timeout_override.is_none()
            )
        })
        .times(1)
        .return_const(SetJumpOutcome::Set);
    session
        .expect_record_outcome()
        .withf(|outcome, value| *outcome == DyeOutcome::Success && value == "api-us.example.com:8443")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("DynamicExternalUpstream");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

/// A successful selection (`SetJumpOutcome::Set`) must record `Success`
/// with the resolved `"domain:port"` value — the value the deleted
/// `apply_dye_headers` call used to pass.
#[tokio::test]
async fn deu_records_success_outcome_with_domain_port() {
    let mut config = base_config();
    config.domain_map.get_mut("us").unwrap().override_host = Some("origin.example.com".to_string());
    let plugin = DynamicExternalUpstream::create(&config);

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_key_get().return_const(Some("us".to_string()));
    let mut sequence = Sequence::new();
    session
        .expect_set_upstream_jump()
        .withf(|target| {
            matches!(
                target,
                UpstreamJumpTarget::External(jump)
                    if jump.domain == "api-us.example.com"
                        && jump.port == 8443
                        && jump.use_tls
                        && jump.sni == "api-us.example.com"
                        && !jump.preserve_raw_path
                        && jump.upstream_read_timeout_override.is_none()
            )
        })
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(SetJumpOutcome::Set);
    session
        .expect_set_upstream_host()
        .withf(|host| host == "origin.example.com")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_| Ok(()));
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Success && v == "api-us.example.com:8443")
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(());

    let mut log = PluginLog::new("DynamicExternalUpstream");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(log.contains("upstream selected"));
}

#[tokio::test]
async fn rejected_jump_has_no_host_or_dye_side_effects() {
    let mut config = base_config();
    config.domain_map.get_mut("us").unwrap().override_host = Some("origin.example.com".to_string());
    let plugin = DynamicExternalUpstream::create(&config);

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_key_get().return_const(Some("us".to_string()));
    session
        .expect_set_upstream_jump()
        .withf(|target| matches!(target, UpstreamJumpTarget::External(jump) if jump.domain == "api-us.example.com"))
        .times(1)
        .return_const(SetJumpOutcome::Rejected);
    // No Host or Dye expectations: either side effect fails the mock.

    let mut log = PluginLog::new("DynamicExternalUpstream");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(
        serde_json::to_value(&log).unwrap()["log"],
        serde_json::json!(["jump locked, rejected"])
    );
}

/// Missing routing key with `on_missing: Reject` must record `Reject` with
/// cause `on-missing` and return the 400 local reply.
#[tokio::test]
async fn deu_records_reject_outcome_on_missing_key() {
    let config = DynamicExternalUpstreamConfig {
        on_missing: UpstreamFallback::Reject,
        ..base_config()
    };
    let plugin = DynamicExternalUpstream::create(&config);

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_key_get().return_const(None::<String>);
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Reject && v == "on-missing")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("DynamicExternalUpstream");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 400,
            body: Some(Bytes::from_static(b"Missing routing key for external upstream")),
        }
    );
}

/// `on_no_match: Fallback` (default) on a routing key with no domain entry must
/// record `Fallback` with cause `on-no-match` and pass the request through.
#[tokio::test]
async fn deu_records_fallback_outcome_on_no_match() {
    let plugin = DynamicExternalUpstream::create(&base_config());

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session
        .expect_key_get()
        .return_const(Some("unknown-region".to_string()));
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Fallback && v == "on-no-match")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("DynamicExternalUpstream");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}
