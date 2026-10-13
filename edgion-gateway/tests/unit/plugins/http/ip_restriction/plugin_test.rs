use super::*;
use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::refs::ip_list_fields::PreparedIpListReferences;
use edgion_resources::EdgionPlugins;
use mockall::Sequence;
use serde_json::json;
use std::collections::HashMap;

fn owner(config: serde_json::Value) -> EdgionPlugins {
    serde_json::from_value(json!({"metadata":{"name":"actual","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"IpRestriction","config":config}]}}))
    .unwrap()
}
fn create(owner: &EdgionPlugins) -> Option<Box<dyn RequestFilter>> {
    let compiler = IpConditionCompiler::new(owner).ok()?;
    let prepared = PreparedIpListReferences::from_plugins(owner);
    let binding = prepared.bind_plugins(owner).ok()?;
    IpRestriction::create_bound(binding.http_entry(0)?, &compiler)
}
fn publish(store: &EdgionConfigDataStore, lists: &[(&str, &str, &[&str])]) {
    let data = lists
        .iter()
        .map(|(name, group, cidrs)| {
            let value = serde_json::from_value(json!({"metadata":{"name":name,"namespace":"default"},
            "spec":{"data":{"type":"IpList","config":{"items":[{"name":group,"cidrs":cidrs}]}}}}))
            .unwrap();
            (format!("default/{name}"), Arc::new(value))
        })
        .collect::<HashMap<_, _>>();
    store.full_set(&data);
}
fn session(store: &EdgionConfigDataStore, ip: &str, direct: bool) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    let snapshot = store.capture_snapshot();
    session.expect_ensure_config_data_view().times(1).returning(|| {});
    session
        .expect_ip_acl_decision()
        .times(1)
        .returning(move |acl, ip| acl.evaluate_in(Some(&snapshot), ip));
    if direct {
        session.expect_direct_peer_ip().times(1).return_const(ip.to_owned());
    } else {
        session.expect_remote_ip().times(1).return_const(ip.to_owned());
    }
    session
}
fn denied(session: &mut MockPluginSession) {
    session.expect_write_response_header().times(1).returning(|_, _| Ok(()));
    session.expect_write_response_body().times(1).returning(|_, _| Ok(()));
    session.expect_shutdown().times(1).returning(|| {});
}
async fn run(plugin: &dyn RequestFilter, session: &mut MockPluginSession) -> (PluginRunningResult, String) {
    let mut log = PluginLog::new(PLUGIN_NAME);
    let result = plugin.run_request(session, &mut log).await;
    (result, log.render_for_test())
}
#[test]
fn plugin_name_is_static_across_instances() {
    let a = owner(json!({"allowRefs":[{"name":"clients"}]}));
    let b = owner(json!({"denyRefs":[{"name":"blocked"}]}));
    let first = create(&a).unwrap();
    let second = create(&b).unwrap();
    assert_eq!(first.name(), PLUGIN_NAME);
    assert_eq!(second.name(), PLUGIN_NAME);
    assert!(std::ptr::eq(first.name().as_ptr(), second.name().as_ptr()));
}
#[tokio::test]
async fn init_failure_uses_fixed_fail_closed_response() {
    // Static construction failures are now sealed by the owning runtime; no detached
    // partly initialized plugin can write an operator-provided message.
    let invalid = owner(json!({"message":"do not expose this configuration detail"}));
    assert!(create(&invalid).is_none());
    for value in [json!(null), json!([]), json!([{ "cidrs":["192.0.2.0/24"] }])] {
        assert!(create(&owner(json!({"deny":value,"denyRefs":[{"name":"blocked"}]}))).is_none());
    }
}
#[tokio::test]
async fn partial_good_deny_uses_exact_prebuilt_custom_body() {
    let store = EdgionConfigDataStore::new();
    publish(&store, &[("blocked", "blocked", &["not-an-ip", "10.0.0.7/32"])]);
    let owner = owner(json!({"denyRefs":[{"name":"blocked"}],"message":"quoted \"slash\\line\n©","status":429}));
    let plugin = create(&owner).unwrap();
    let mut s = session(&store, "10.0.0.7", false);
    let mut sequence = Sequence::new();
    s.expect_write_response_header()
        .withf(|h, eos| {
            !eos && h.status.as_u16() == 429
                && h.headers.get("content-type").and_then(|v| v.to_str().ok()) == Some("application/json")
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    s.expect_write_response_body()
        .withf(|b, eos| *eos && b.as_deref() == Some(r#"{"message":"quoted \"slash\\line\n©"}"#.as_bytes()))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    s.expect_shutdown().times(1).in_sequence(&mut sequence).returning(|| {});
    assert_eq!(
        run(&*plugin, &mut s).await,
        (PluginRunningResult::ErrTerminateRequest, "deny:group=blocked".into())
    );
}
#[tokio::test]
async fn test_default_action_allow() {
    let store = EdgionConfigDataStore::new();
    publish(&store, &[("blocked", "default", &["10.0.0.100"])]);
    let owner = owner(json!({"denyRefs":[{"name":"blocked"}]}));
    let plugin = create(&owner).unwrap();
    assert_eq!(
        run(&*plugin, &mut session(&store, "172.16.0.1", false)).await,
        (PluginRunningResult::GoodNext, "allow:default".into())
    );
}
#[tokio::test]
async fn direct_peer_source_does_not_read_remote_ip() {
    let store = EdgionConfigDataStore::new();
    publish(&store, &[("blocked", "proxy", &["10.0.0.7/32"])]);
    let owner = owner(json!({"denyRefs":[{"name":"blocked"}],"ipSource":"DirectPeerIp"}));
    let plugin = create(&owner).unwrap();
    let mut s = session(&store, "10.0.0.7", true);
    denied(&mut s);
    assert_eq!(run(&*plugin, &mut s).await.0, PluginRunningResult::ErrTerminateRequest);
}
#[tokio::test]
async fn test_missing_ip_denied_in_whitelist_mode() {
    let store = EdgionConfigDataStore::new();
    let owner = owner(json!({"allowRefs":[{"name":"clients"}]}));
    let plugin = create(&owner).unwrap();
    let mut s = session(&store, "", false);
    denied(&mut s);
    assert_eq!(
        run(&*plugin, &mut s).await,
        (PluginRunningResult::ErrTerminateRequest, "deny:no-client-ip".into())
    );
}
#[tokio::test]
async fn test_create_compiles_matchers_whitelist_enforced() {
    // A received refs-only policy with default Allow still enforces authored
    // whitelist intent, even when no canonical list is currently available.
    let store = EdgionConfigDataStore::new();
    let owner = owner(json!({"allowRefs":[{"name":"clients"}]}));
    let plugin = create(&owner).unwrap();
    let mut s = session(&store, "10.0.0.1", false);
    denied(&mut s);
    assert_eq!(
        run(&*plugin, &mut s).await,
        (PluginRunningResult::ErrTerminateRequest, "deny:not-in-allow".into())
    );
}
#[tokio::test]
async fn test_whitelist_miss_emits_not_in_allow() {
    let store = EdgionConfigDataStore::new();
    publish(&store, &[("clients", "default", &["192.168.1.0/24"])]);
    let owner = owner(json!({"allowRefs":[{"name":"clients"}],"defaultAction":"deny"}));
    let plugin = create(&owner).unwrap();
    let mut s = session(&store, "10.0.0.1", false);
    let mut sequence = Sequence::new();
    s.expect_write_response_header()
        .withf(|h, eos| {
            !eos && h.status.as_u16() == 403
                && h.headers.get("content-type").and_then(|v| v.to_str().ok()) == Some("application/json")
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    s.expect_write_response_body()
        .withf(|b, eos| *eos && b.as_deref() == Some(DEFAULT_DENY_BODY))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    s.expect_shutdown().times(1).in_sequence(&mut sequence).returning(|| {});
    assert_eq!(
        run(&*plugin, &mut s).await,
        (PluginRunningResult::ErrTerminateRequest, "deny:not-in-allow".into())
    );
}
#[tokio::test]
async fn deny_refs_block_referenced_ip() {
    let store = EdgionConfigDataStore::new();
    publish(&store, &[("blocklist", "blocked", &["9.9.9.9/32"])]);
    let owner = owner(json!({"denyRefs":[{"name":"blocklist"}]}));
    let plugin = create(&owner).unwrap();
    let mut s = session(&store, "9.9.9.9", false);
    denied(&mut s);
    assert_eq!(
        run(&*plugin, &mut s).await,
        (PluginRunningResult::ErrTerminateRequest, "deny:group=blocked".into())
    );
    assert_eq!(
        run(&*plugin, &mut session(&store, "8.8.8.8", false)).await,
        (PluginRunningResult::GoodNext, "allow:default".into())
    );
}
#[tokio::test]
async fn referenced_deny_precedes_inline_allow_and_logs_group() {
    // The former inline allow source is now a separate canonical list, in its
    // original order. Deny priority and the mandatory ACL group log remain exact.
    let store = EdgionConfigDataStore::new();
    publish(
        &store,
        &[
            ("allow-all", "all", &["0.0.0.0/0"]),
            ("blocklist", "blocked", &["9.9.9.9/32"]),
        ],
    );
    let owner = owner(json!({"allowRefs":[{"name":"allow-all"}],"denyRefs":[{"name":"blocklist"}]}));
    let plugin = create(&owner).unwrap();
    let mut s = session(&store, "9.9.9.9", false);
    denied(&mut s);
    assert_eq!(
        run(&*plugin, &mut s).await,
        (PluginRunningResult::ErrTerminateRequest, "deny:group=blocked".into())
    );
}
#[tokio::test]
async fn allow_ref_hot_reload_reflects_iplist_content_change() {
    let store = EdgionConfigDataStore::new();
    publish(&store, &[("reload-list", "g", &["127.0.0.0/8"])]);
    let owner = owner(json!({"allowRefs":[{"name":"reload-list"}]}));
    let plugin = create(&owner).unwrap();
    let mut held = session(&store, "127.0.0.1", false);
    publish(&store, &[("reload-list", "g", &["10.0.0.0/8"])]);
    assert_eq!(run(&*plugin, &mut held).await.0, PluginRunningResult::GoodNext);
    let mut fresh = session(&store, "127.0.0.1", false);
    denied(&mut fresh);
    assert_eq!(
        run(&*plugin, &mut fresh).await.0,
        PluginRunningResult::ErrTerminateRequest
    );
    store.full_set(&HashMap::new());
    let mut missing = session(&store, "127.0.0.1", false);
    denied(&mut missing);
    assert_eq!(
        run(&*plugin, &mut missing).await.0,
        PluginRunningResult::ErrTerminateRequest
    );
    publish(&store, &[("reload-list", "g", &["127.0.0.0/8"])]);
    assert_eq!(
        run(&*plugin, &mut session(&store, "127.0.0.1", false)).await.0,
        PluginRunningResult::GoodNext
    );
}
#[tokio::test]
async fn denied_allow_ref_keeps_allow_mode_and_contributes_no_key() {
    let store = EdgionConfigDataStore::new();
    publish(&store, &[("corp", "corp", &["0.0.0.0/0"])]);
    let owner =
        owner(json!({"allowRefs":[{"name":"corp","refDenied":{"targetNamespace":"default","targetName":"corp"}}]}));
    let plugin = create(&owner).unwrap();
    let mut s = session(&store, "8.8.8.8", false);
    denied(&mut s);
    assert_eq!(
        run(&*plugin, &mut s).await,
        (PluginRunningResult::ErrTerminateRequest, "deny:not-in-allow".into())
    );
}
#[tokio::test]
async fn owning_http_invalid_slots_keep_allow_intent_and_deny_set_drop() {
    let store = EdgionConfigDataStore::new();
    publish(&store, &[("good", "set", &["198.51.100.8/32"])]);
    for bad in [json!({"name":"bad/name"}), json!({"name":"nets","namespace":""})] {
        for (side, sibling, allow) in [
            ("allowRefs", false, false),
            ("denyRefs", false, true),
            ("allowRefs", true, true),
            ("denyRefs", true, false),
        ] {
            let mut config = json!({});
            config[side] = json!([bad]);
            if sibling {
                config[side].as_array_mut().unwrap().push(json!({"name":"good"}));
            }
            let owner = owner(config);
            let plugin = create(&owner).unwrap();
            let mut s = session(&store, "198.51.100.8", false);
            if !allow {
                denied(&mut s);
            }
            assert_eq!(
                run(&*plugin, &mut s).await.0 == PluginRunningResult::GoodNext,
                allow,
                "{side}, sibling={sibling}, {bad}"
            );
        }
    }
}
#[test]
fn owning_http_invalid_owner_fails_separately_from_invalid_target() {
    let mut owner = owner(json!({"denyRefs":[{"name":"bad/name"}]}));
    owner.metadata.namespace = Some(String::new());
    assert!(create(&owner).is_none());
    owner.metadata.namespace = None;
    assert!(create(&owner).is_some());
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    owner.metadata.uid = Some("changed".into());
    assert!(prepared.bind_plugins(&owner).is_err());
}
#[tokio::test]
async fn blank_reference_name_preserves_structural_constructor_failure() {
    for side in ["allowRefs", "denyRefs"] {
        let mut config = json!({});
        config[side] = json!([{"name":""}]);
        assert!(create(&owner(config)).is_none());
    }
}

#[tokio::test]
async fn mapped_iplist_rules_enforce_deny_and_whitelist_for_both_query_forms() {
    let store = EdgionConfigDataStore::new();
    publish(&store, &[("mapped-list", "mapped", &["::ffff:192.0.2.7/120"])]);
    for side in ["denyRefs", "allowRefs"] {
        let mut config = json!({"defaultAction":"allow"});
        config[side] = json!([{"name":"mapped-list"}]);
        let owner = owner(config);
        let plugin = create(&owner).unwrap();
        for ip in ["192.0.2.7", "::ffff:192.0.2.7", "192.0.3.7", "::ffff:192.0.3.7"] {
            let in_range = ip.ends_with("192.0.2.7");
            let is_denied = if side == "denyRefs" { in_range } else { !in_range };
            let mut s = session(&store, ip, false);
            if is_denied {
                denied(&mut s);
            }
            let expected_log = match (side, in_range) {
                ("denyRefs", true) => "deny:group=mapped",
                ("denyRefs", false) => "allow:default",
                (_, true) => "allow:group=mapped",
                (_, false) => "deny:not-in-allow",
            };
            let expected_result = if is_denied {
                PluginRunningResult::ErrTerminateRequest
            } else {
                PluginRunningResult::GoodNext
            };
            assert_eq!(run(&*plugin, &mut s).await, (expected_result, expected_log.into()));
        }
    }
}
