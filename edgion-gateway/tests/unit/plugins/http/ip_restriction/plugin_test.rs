use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::edgion_config_data::IpGroup;
use edgion_resources::resources::edgion_plugins::DefaultAction;
use mockall::Sequence;

fn create_ip_config_allow_list() -> IpRestrictionConfig {
    let config = IpRestrictionConfig {
        allow: Some(vec![IpGroup {
            name: "default".into(),
            description: None,
            cidrs: vec!["192.168.1.0/24".into()],
        }]),
        deny: None,
        default_action: DefaultAction::Deny,
        allow_refs: None,
        deny_refs: None,
        ip_source: IpSource::RemoteIp,
        message: None,
        status: 403,
        allow_matcher: None,
        deny_matcher: None,
    };
    IpRestrictionConfig::new(config).unwrap()
}

fn create_ip_config_deny_list() -> IpRestrictionConfig {
    let config = IpRestrictionConfig {
        allow: None,
        deny: Some(vec![IpGroup {
            name: "default".into(),
            description: None,
            cidrs: vec!["10.0.0.100".into()],
        }]),
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: None,
        ip_source: IpSource::RemoteIp,
        message: None,
        status: 403,
        allow_matcher: None,
        deny_matcher: None,
    };
    IpRestrictionConfig::new(config).unwrap()
}

#[test]
fn plugin_name_is_static_across_instances() {
    let first = IpRestriction::create(&create_ip_config_allow_list(), "default");
    let second = IpRestriction::create(&create_ip_config_deny_list(), "default");

    assert_eq!(first.name(), PLUGIN_NAME);
    assert_eq!(second.name(), PLUGIN_NAME);
    assert!(std::ptr::eq(first.name().as_ptr(), second.name().as_ptr()));
}

#[tokio::test]
async fn init_failure_uses_fixed_fail_closed_response() {
    let config = IpRestrictionConfig {
        message: Some("do not expose this configuration detail".to_string()),
        ..Default::default()
    };
    let plugin = IpRestriction::create(&config, "default");
    let mut session = MockPluginSession::new();
    let mut sequence = Sequence::new();
    session
        .expect_write_response_header()
        .withf(|response, end_of_stream| {
            !end_of_stream
                && response.status.as_u16() == 403
                && response
                    .headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    == Some("application/json")
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_write_response_body()
        .withf(|body, end_of_stream| {
            *end_of_stream && body.as_deref() == Some(Bytes::from_static(INIT_FAILURE_BODY).as_ref())
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_shutdown()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|| {});
    let mut log = PluginLog::new(PLUGIN_NAME);

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_eq!(log.render_for_test(), "err: ip init fail");
}

#[tokio::test]
async fn partial_good_deny_uses_exact_prebuilt_custom_body() {
    let config = IpRestrictionConfig {
        allow: None,
        deny: Some(vec![IpGroup {
            name: "blocked".into(),
            description: None,
            cidrs: vec!["not-an-ip".into(), "10.0.0.7/32".into()],
        }]),
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: None,
        ip_source: IpSource::RemoteIp,
        message: Some("quoted \"slash\\line\n©".to_string()),
        status: 429,
        allow_matcher: None,
        deny_matcher: None,
    };
    let plugin = IpRestriction::create(&config, "default");
    let mut session = MockPluginSession::new();
    session.expect_remote_ip().return_const("10.0.0.7".to_string());
    let mut sequence = Sequence::new();
    session
        .expect_write_response_header()
        .withf(|response, end_of_stream| {
            !end_of_stream
                && response.status.as_u16() == 429
                && response
                    .headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    == Some("application/json")
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_write_response_body()
        .withf(|body, end_of_stream| {
            *end_of_stream && body.as_deref() == Some(r#"{"message":"quoted \"slash\\line\n©"}"#.as_bytes())
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_shutdown()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|| {});
    let mut log = PluginLog::new(PLUGIN_NAME);

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_eq!(log.render_for_test(), "deny:group=blocked");
}

#[tokio::test]
async fn test_default_action_allow() {
    let config = create_ip_config_deny_list();
    let plugin = IpRestriction::create(&config, "default");
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("IpRestriction");

    mock_session.expect_remote_ip().return_const("172.16.0.1".to_string());

    let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(plugin_log.contains("allow:default"));
}

#[tokio::test]
async fn direct_peer_source_does_not_read_remote_ip() {
    let config = IpRestrictionConfig {
        allow: None,
        deny: Some(vec![IpGroup {
            name: "proxy".into(),
            description: None,
            cidrs: vec!["10.0.0.7/32".into()],
        }]),
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: None,
        ip_source: IpSource::DirectPeerIp,
        message: None,
        status: 403,
        allow_matcher: None,
        deny_matcher: None,
    };
    let plugin = IpRestriction::create(&config, "default");
    let mut session = MockPluginSession::new();
    session.expect_direct_peer_ip().return_const("10.0.0.7".to_string());
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});
    let mut log = PluginLog::new("IpRestriction");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
}

#[tokio::test]
async fn test_missing_ip_denied_in_whitelist_mode() {
    // Allow list configured = whitelist mode: unknown IP must be denied
    let config = create_ip_config_allow_list();
    let plugin = IpRestriction::create(&config, "default");
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("IpRestriction");

    mock_session.expect_remote_ip().return_const("".to_string());
    mock_session.expect_write_response_header().returning(|_, _| Ok(()));
    mock_session.expect_write_response_body().returning(|_, _| Ok(()));
    mock_session.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(plugin_log.contains("deny:no-client-ip"));
}

/// Regression for fr-types-01: a config arriving via the gRPC/YAML path has
/// `allow_matcher`/`deny_matcher` == None (they are `#[serde(skip)]`). `create`
/// must compile them via `validate_and_init`; otherwise the evaluator falls
/// through to `Default(Allow)` and the whitelist is silently fail-open.
#[tokio::test]
async fn test_create_compiles_matchers_whitelist_enforced() {
    // Build the config exactly as deserialization yields it: matchers are None
    // and we do NOT route through `IpRestrictionConfig::new` (which would pre-build
    // them and mask the constructor bug). default_action stays Allow (the default).
    let config = IpRestrictionConfig {
        allow: Some(vec![IpGroup {
            name: "default".into(),
            description: None,
            cidrs: vec!["192.168.1.0/24".into()],
        }]),
        deny: None,
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: None,
        ip_source: IpSource::RemoteIp,
        message: None,
        status: 403,
        allow_matcher: None,
        deny_matcher: None,
    };

    let plugin = IpRestriction::create(&config, "default");
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("IpRestriction");

    // An IP outside the allow list must be denied.
    mock_session.expect_remote_ip().return_const("10.0.0.1".to_string());
    mock_session.expect_write_response_header().returning(|_, _| Ok(()));
    mock_session.expect_write_response_body().returning(|_, _| Ok(()));
    mock_session.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(plugin_log.contains("deny:not-in-allow"));
}

#[tokio::test]
async fn test_whitelist_miss_emits_not_in_allow() {
    let config = create_ip_config_allow_list();
    let plugin = IpRestriction::create(&config, "default");
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("IpRestriction");

    // An IP outside the configured allow range.
    mock_session.expect_remote_ip().return_const("10.0.0.1".to_string());
    let mut sequence = Sequence::new();
    mock_session
        .expect_write_response_header()
        .withf(|response, end_of_stream| {
            !end_of_stream
                && response.status.as_u16() == 403
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
            *end_of_stream && body.as_deref() == Some(Bytes::from_static(DEFAULT_DENY_BODY).as_ref())
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    mock_session
        .expect_shutdown()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|| {});

    let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(plugin_log.contains("deny:not-in-allow"));
}

#[tokio::test]
async fn deny_refs_block_referenced_ip() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigData, EdgionConfigDataRef, EdgionConfigDataSpec, IpGroup, IpListData,
    };
    use std::sync::Arc as StdArc;

    // Seed an IpList EdgionConfigData under a unique namespace.
    let ns = "http-ipr-denyref-unit";
    let spec = EdgionConfigDataSpec {
        current_status: None,
        enable: true,
        active: None,
        visibility: Default::default(),
        unknown_fields: Vec::new(),
        data: ConfigEntry::IpList(IpListData {
            items: vec![IpGroup {
                name: "blocked".into(),
                description: None,
                cidrs: vec!["9.9.9.9/32".into()],
            }],
        }),
    };
    let mut ed = EdgionConfigData::new("blocklist", spec);
    ed.metadata.namespace = Some(ns.to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(format!("{ns}/blocklist"), StdArc::new(ed));

    // Refs-only deny config (no inline allow/deny), default Allow.
    let config = IpRestrictionConfig {
        allow: None,
        deny: None,
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: Some(vec![EdgionConfigDataRef {
            name: "blocklist".into(),
            namespace: Some(ns.to_string()),
            ..Default::default()
        }]),
        ip_source: IpSource::RemoteIp,
        message: None,
        status: 403,
        allow_matcher: None,
        deny_matcher: None,
    };
    let plugin = IpRestriction::create(&config, ns);

    // 9.9.9.9 is in the referenced blocklist -> denied.
    let mut s = MockPluginSession::new();
    let mut log = PluginLog::new("IpRestriction");
    s.expect_remote_ip().return_const("9.9.9.9".to_string());
    s.expect_write_response_header().returning(|_, _| Ok(()));
    s.expect_write_response_body().returning(|_, _| Ok(()));
    s.expect_shutdown().returning(|| {});
    assert_eq!(
        plugin.run_request(&mut s, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_eq!(log.render_for_test(), "deny:group=blocked");

    // 8.8.8.8 not in blocklist -> allowed (default Allow).
    let mut s2 = MockPluginSession::new();
    let mut log2 = PluginLog::new("IpRestriction");
    s2.expect_remote_ip().return_const("8.8.8.8".to_string());
    assert_eq!(
        plugin.run_request(&mut s2, &mut log2).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(log2.render_for_test(), "allow:default");
}

#[tokio::test]
async fn referenced_deny_precedes_inline_allow_and_logs_group() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigData, EdgionConfigDataRef, EdgionConfigDataSpec, IpGroup, IpListData,
    };
    use std::sync::Arc as StdArc;

    let ns = "http-ipr-denyref-precedence-unit";
    let spec = EdgionConfigDataSpec {
        current_status: None,
        enable: true,
        active: None,
        visibility: Default::default(),
        unknown_fields: Vec::new(),
        data: ConfigEntry::IpList(IpListData {
            items: vec![IpGroup {
                name: "blocked".into(),
                description: None,
                cidrs: vec!["9.9.9.9/32".into()],
            }],
        }),
    };
    let mut ed = EdgionConfigData::new("blocklist", spec);
    ed.metadata.namespace = Some(ns.to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(format!("{ns}/blocklist"), StdArc::new(ed));

    let config = IpRestrictionConfig {
        allow: Some(vec![IpGroup {
            name: "all".into(),
            description: None,
            cidrs: vec!["0.0.0.0/0".into()],
        }]),
        deny: None,
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: Some(vec![EdgionConfigDataRef {
            name: "blocklist".into(),
            namespace: Some(ns.to_string()),
            ..Default::default()
        }]),
        ip_source: IpSource::RemoteIp,
        message: None,
        status: 403,
        allow_matcher: None,
        deny_matcher: None,
    };
    let plugin = IpRestriction::create(&config, ns);
    let mut session = MockPluginSession::new();
    session.expect_remote_ip().return_const("9.9.9.9".to_string());
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});
    let mut log = PluginLog::new(PLUGIN_NAME);

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_eq!(log.render_for_test(), "deny:group=blocked");
}

/// Approach-A hot reload: editing the referenced IpList's CONTENT (same key) must change
/// the data-plane decision WITHOUT rebuilding the plugin instance. The plugin stores only
/// the resolved key and re-resolves the matcher from the global store on every request.
#[tokio::test]
async fn allow_ref_hot_reload_reflects_iplist_content_change() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigData, EdgionConfigDataRef, EdgionConfigDataSpec, IpGroup, IpListData,
    };
    use std::sync::Arc as StdArc;

    let ns = "http-ipr-hotreload-unit";
    let key = format!("{ns}/reload-list");
    let seed = |cidr: &str| {
        let spec = EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: "g".into(),
                    description: None,
                    cidrs: vec![cidr.into()],
                }],
            }),
        };
        let mut ed = EdgionConfigData::new("reload-list", spec);
        ed.metadata.namespace = Some(ns.to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(&key, StdArc::new(ed));
    };

    // Initial: allowlist contains the loopback range.
    seed("127.0.0.0/8");
    let config = IpRestrictionConfig {
        allow: None,
        deny: None,
        default_action: DefaultAction::Allow,
        allow_refs: Some(vec![EdgionConfigDataRef {
            name: "reload-list".into(),
            namespace: Some(ns.to_string()),
            ..Default::default()
        }]),
        deny_refs: None,
        ip_source: IpSource::RemoteIp,
        message: None,
        status: 403,
        allow_matcher: None,
        deny_matcher: None,
    };
    let plugin = IpRestriction::create(&config, ns);

    // 127.0.0.1 is in the allowlist -> allowed.
    let mut s = MockPluginSession::new();
    let mut log = PluginLog::new("IpRestriction");
    s.expect_remote_ip().return_const("127.0.0.1".to_string());
    assert_eq!(
        plugin.run_request(&mut s, &mut log).await,
        PluginRunningResult::GoodNext,
        "127.0.0.1 should be allowed by the initial [127.0.0.0/8] list"
    );

    // Hot reload: replace the SAME IpList with a non-loopback range. Plugin instance unchanged.
    seed("10.0.0.0/8");

    // 127.0.0.1 is no longer in the allowlist -> whitelist miss -> denied.
    let mut s2 = MockPluginSession::new();
    let mut log2 = PluginLog::new("IpRestriction");
    s2.expect_remote_ip().return_const("127.0.0.1".to_string());
    s2.expect_write_response_header().returning(|_, _| Ok(()));
    s2.expect_write_response_body().returning(|_, _| Ok(()));
    s2.expect_shutdown().returning(|| {});
    assert_eq!(
        plugin.run_request(&mut s2, &mut log2).await,
        PluginRunningResult::ErrTerminateRequest,
        "after the IpList content changed to [10.0.0.0/8], 127.0.0.1 must be denied (hot reload)"
    );
}

/// A denied allow_ref must NOT flip allow_mode to false (which would allow all).
/// With allow_mode=true but zero allow matchers, a non-matching IP is denied.
#[tokio::test]
async fn denied_allow_ref_keeps_allow_mode_and_contributes_no_key() {
    use crate::plugins::runtime::edgion_config_data::resolve_ip_list_ref_keys;
    use edgion_resources::resources::edgion_config_data::EdgionConfigDataRef;
    let cfg = IpRestrictionConfig {
        allow_refs: Some(vec![EdgionConfigDataRef {
            name: "corp".into(),
            namespace: Some("team-b".into()),
            ref_denied: Some(edgion_resources::resources::common::RefDenied {
                target_namespace: "team-b".into(),
                target_name: "corp".into(),
                reason: None,
            }),
        }]),
        ..Default::default()
    };
    // allow_mode() must be true (the field is still present — whitelist intent preserved).
    assert!(cfg.allow_mode(), "whitelist intent must survive the denial marker");
    // The ref_denied entry contributes no key (fail-closed: deny-all with empty allow set).
    let keys = resolve_ip_list_ref_keys(cfg.allow_refs.as_ref(), "team-a");
    assert!(keys.is_empty(), "denied allow_ref must contribute no matcher key");
    // Evaluate path: whitelist mode + zero allow matchers → deny-all (fail-closed).
    // Empty ref slices mirror what on-request evaluation produces when allow_ref_keys is empty.
    use edgion_resources::resources::edgion_plugins::AccessDecision;
    let ip: std::net::IpAddr = "8.8.8.8".parse().unwrap();
    assert!(
        matches!(
            cfg.check_ip_access_with_refs(&ip, &[], &[]),
            AccessDecision::DenyByWhitelistMiss
        ),
        "whitelist miss with no allow matchers must be DENIED (deny-all fail-closed)",
    );

    let plugin = IpRestriction::create(&cfg, "team-a");
    let mut session = MockPluginSession::new();
    session.expect_remote_ip().return_const("8.8.8.8".to_string());
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});
    let mut log = PluginLog::new(PLUGIN_NAME);
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_eq!(log.render_for_test(), "deny:not-in-allow");
}
