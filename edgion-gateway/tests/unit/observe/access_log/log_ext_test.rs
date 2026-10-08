use super::*;
use edgion_resources::sensitive::SENSITIVE_MODE;
use std::sync::atomic::Ordering;

fn assert_sensitive_test_in_subprocess(test_name: &str, assertion: impl FnOnce()) {
    const CHILD_ENV: &str = "EDGION_LOG_EXT_SENSITIVE_CHILD";
    const TEST_MODULE: &str = "observe::access_log::log_ext::tests";
    crate::test_subprocess::assert_in_subprocess(TEST_MODULE, test_name, CHILD_ENV, test_name, assertion);
}

fn json_of(ext: &LogExt) -> String {
    serde_json::to_string(ext).unwrap()
}

#[test]
fn empty_ext_is_empty() {
    assert!(LogExt::default().is_empty());
}

#[test]
fn non_sensitive_value_is_clear_in_every_mode() {
    assert_sensitive_test_in_subprocess("non_sensitive_value_is_clear_in_every_mode", || {
        for mode in [0u8, 1, 2] {
            SENSITIVE_MODE.store(mode, Ordering::SeqCst);
            let mut ext = LogExt::default();
            ext.insert("tenant", "acme", false);
            assert_eq!(json_of(&ext), r#"{"tenant":"acme"}"#, "mode {mode}");
        }
    });
}

#[test]
fn sensitive_value_renders_per_mode() {
    // Mode 2 (wrap) is the only branch where `apply_sensitive` can panic
    // (`try_borrow_mut().expect(...)` in edgion-resources/src/sensitive.rs):
    // `LogExt::serialize` is the only caller that runs a two-step serde op
    // (`serialize_entry` = key + value) inside the sensitive-value closure,
    // exactly the reentrancy shape the doc comment on `Serialize for LogExt`
    // warns about. Exercise it here so that shape stays covered.
    assert_sensitive_test_in_subprocess("sensitive_value_renders_per_mode", || {
        let cases = [
            (0u8, r#"{"session":"abc"}"#),
            (1, r#"{"session":"***"}"#),
            (2, r#"{"session":"<E>abc</E>"}"#),
        ];
        for (mode, expected) in cases {
            SENSITIVE_MODE.store(mode, Ordering::SeqCst);
            let mut ext = LogExt::default();
            ext.insert("session", "abc", true);
            assert_eq!(json_of(&ext), expected, "mode {mode}");
        }
    });
}

#[test]
fn later_insert_overwrites_in_place_preserving_order() {
    let mut ext = LogExt::default();
    ext.insert("a", "1", false);
    ext.insert("b", "2", false);
    // Route tier overwrites the global tier's value for the same key.
    ext.insert("a", "9", false);
    assert_eq!(json_of(&ext), r#"{"a":"9","b":"2"}"#);
    assert_eq!(ext.len(), 2);
}

#[test]
fn overwrite_also_replaces_the_sensitive_flag() {
    assert_sensitive_test_in_subprocess("overwrite_also_replaces_the_sensitive_flag", || {
        SENSITIVE_MODE.store(1, Ordering::SeqCst);
        let mut ext = LogExt::default();
        ext.insert("k", "clear", false);
        ext.insert("k", "secret", true);
        assert_eq!(json_of(&ext), r#"{"k":"***"}"#);
    });
}

#[test]
fn value_is_truncated_to_the_cap() {
    let mut ext = LogExt::default();
    let long = "v".repeat(LOG_LABEL_MAX_VALUE_LEN + 50);
    ext.insert("k", &long, false);
    let json = json_of(&ext);
    // 2 quotes + the key + separators; assert on the value length itself.
    let value = json.trim_start_matches(r#"{"k":""#).trim_end_matches(r#""}"#);
    assert_eq!(value.len(), LOG_LABEL_MAX_VALUE_LEN);
}

#[test]
fn owned_insert_reuses_the_resolved_value_allocation() {
    let mut ext = LogExt::default();
    let value = String::from("resolved");
    let value_ptr = value.as_ptr();

    ext.insert("k", value, false);

    assert_eq!(ext.0[0].value.as_ptr(), value_ptr);
    assert_eq!(json_of(&ext), r#"{"k":"resolved"}"#);
}

#[test]
fn owned_insert_does_not_retain_oversized_capacity() {
    let mut ext = LogExt::default();
    let mut value = String::with_capacity(LOG_LABEL_MAX_VALUE_LEN * 16);
    value.push_str(&"v".repeat(LOG_LABEL_MAX_VALUE_LEN + 1));
    let original_capacity = value.capacity();

    ext.insert("k", value, false);

    assert_eq!(ext.0[0].value.len(), LOG_LABEL_MAX_VALUE_LEN);
    assert!(ext.0[0].value.capacity() < original_capacity);
}

#[test]
fn truncation_never_splits_a_utf8_char() {
    let mut ext = LogExt::default();
    // '€' is 3 bytes, so with an even cap (256) the boundary genuinely
    // straddles a char and the decrement branch actually runs. A 2-byte
    // char like 'é' would make `is_char_boundary(256)` trivially true and
    // never exercise the truncation logic this test is named for.
    let long = "€".repeat(LOG_LABEL_MAX_VALUE_LEN);
    ext.insert("k", &long, false);
    let json = json_of(&ext);
    assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
}

#[test]
fn over_cap_fields_are_dropped() {
    let mut ext = LogExt::default();
    for i in 0..(LOG_LABEL_MAX_COUNT + 5) {
        ext.insert(&format!("k{i}"), "v", false);
    }
    assert_eq!(ext.len(), LOG_LABEL_MAX_COUNT);
}

#[test]
fn over_cap_still_allows_overwriting_an_existing_key() {
    let mut ext = LogExt::default();
    for i in 0..LOG_LABEL_MAX_COUNT {
        ext.insert(&format!("k{i}"), "v", false);
    }
    // At the cap: a NEW key is dropped, but an existing key must still update,
    // otherwise the route tier could not override the global tier on a full map.
    ext.insert("new", "v", false);
    ext.insert("k0", "updated", false);
    assert_eq!(ext.len(), LOG_LABEL_MAX_COUNT);
    assert!(json_of(&ext).contains(r#""k0":"updated""#));
}

#[test]
fn attacker_controlled_value_cannot_break_out_of_the_json() {
    let mut ext = LogExt::default();
    // A header value the client fully controls. serde must escape it; a
    // hand-written `"k":"v"` byte buffer would let this forge entry fields.
    ext.insert("ua", r#"a","status":999,"x":"b"#, false);
    let json = json_of(&ext);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.as_object().unwrap().len(), 1, "forged a field: {json}");
    assert!(parsed.get("status").is_none(), "forged a field: {json}");
}

#[test]
fn backslash_and_control_chars_are_escaped() {
    let mut ext = LogExt::default();
    ext.insert("k", "a\\b\"c\nd", false);
    let json = json_of(&ext);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.get("k").unwrap().as_str().unwrap(), "a\\b\"c\nd");
}

#[test]
fn masking_decision_table() {
    use edgion_resources::resources::edgion_gateway_config::UnmaskedKeys;
    use edgion_resources::resources::edgion_plugins::LogExtSource;

    let w = UnmaskedKeys {
        header: vec!["user-agent".to_string()],
        ctx: vec!["tenant_id".to_string()],
        ..Default::default()
    };

    // Category 1 is never masked, whatever the whitelist says.
    assert!(!should_mask(&w, LogExtSource::RouteLabel, "app"));
    assert!(!should_mask(&w, LogExtSource::RouteAnnotation, "owner"));
    // Whitelisted category 2/3 is clear.
    assert!(!should_mask(&w, LogExtSource::Header, "user-agent"));
    assert!(!should_mask(&w, LogExtSource::Ctx, "tenant_id"));
    // Everything else is masked.
    assert!(should_mask(&w, LogExtSource::Header, "x-custom"));
    assert!(should_mask(&w, LogExtSource::Cookie, "session"));
    assert!(should_mask(&w, LogExtSource::Ctx, "other"));
}

#[tokio::test]
async fn invalid_route_reference_cannot_consume_same_name_log_declaration() {
    use crate::plugins::publication::{
        inputs::{GatewayInput, RouteInput, RouteKind},
        PluginPublisher,
    };
    use crate::routes::http::{conf_handler_impl::build_domain_route_rules_from_routes, routes_mgr::DomainRouteRules};
    use edgion_resources::{EdgionPlugins, Gateway, GatewayClass, HTTPRoute};
    use std::{collections::HashMap, sync::Arc};
    use tokio::io::AsyncWriteExt;
    let publisher = PluginPublisher::new();
    let class: GatewayClass = serde_json::from_value(
        serde_json::json!({"metadata":{"name":"class"},"spec":{"controllerName":"edgion.io/gateway-controller"}}),
    )
    .unwrap();
    publisher.replace_gateway_classes(HashMap::from([("class".into(), Arc::new(class))]));
    let gateway: Gateway = crate::test_current_status::from_value(serde_json::json!({"metadata":{"name":"gateway","namespace":"default"},"spec":{"gatewayClassName":"class","listeners":[{"name":"web","port":8080,"protocol":"HTTP"}]}})).unwrap();
    publisher.replace_gateways(HashMap::from([(
        "default/gateway".into(),
        GatewayInput::from_gateway(&gateway),
    )]));
    let plugin: EdgionPlugins = serde_json::from_value(serde_json::json!({"metadata":{"name":"decoy","namespace":"default"},"spec":{"accessLogExtern":[{"key":"tenant","from":"header","name":"x-tenant"}]}})).unwrap();
    publisher.replace_http_plugins(HashMap::from([("default/decoy".into(), Arc::new(plugin))]));
    let route: HTTPRoute = crate::test_current_status::from_value(serde_json::json!({
        "metadata":{"name":"route","namespace":"default"}, "spec":{"resolvedStatusController":"edgion.io/gateway-controller","parentRefs":[{"name":"gateway"}],"resolvedListeners":["0.0.0.0:8080/HTTP"],"rules":[{"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"decoy"}}]}]}
    })).unwrap();
    let route = Arc::new(route);
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("default/route".into(), RouteInput::from_http(route.clone()))]),
    );
    let owner = publisher.snapshot().gateway("default/gateway").unwrap();
    assert!(owner.scope.http("default/decoy").is_some());
    assert!(owner.scope.global_keys.is_empty() && owner.http_keys.is_empty());
    let (mut client, server) = tokio::io::duplex(4096);
    let mut session = Session::new_h1(Box::new(server));
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: test\r\nx-tenant: sentinel\r\n\r\n")
        .await
        .unwrap();
    assert!(session.read_request().await.unwrap());
    for invalid in [false, true] {
        let mut source = (*route).clone();
        if invalid {
            source.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0]
                .extension_ref
                .as_mut()
                .unwrap()
                .group = "".into();
        }
        let table = build_domain_route_rules_from_routes(
            &HashMap::from([("default/route".into(), Arc::new(source))]),
            &DomainRouteRules::new(),
        );
        let unit = table.catch_all_routes.as_ref().unwrap().route_rules_list[0].clone();
        let mut ctx = EdgionHttpContext::new();
        ctx.route_unit = Some(unit);
        assert!(ctx.capture_plugin_scope(owner.clone()));
        let ext = collect(&session, &ctx);
        if invalid {
            assert!(ext.is_empty());
        } else {
            assert!(json_of(&ext).contains("sentinel"));
        }
    }
}
