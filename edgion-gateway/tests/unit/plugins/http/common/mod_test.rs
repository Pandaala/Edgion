use super::*;

#[test]
fn format_duration_sub_second_uses_ms() {
    assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
    assert_eq!(format_duration(Duration::from_millis(50)), "50ms");
    assert_eq!(format_duration(Duration::from_millis(999)), "999ms");
}

#[test]
fn format_duration_one_second_and_above_uses_seconds() {
    assert_eq!(format_duration(Duration::from_millis(1000)), "1.0s");
    assert_eq!(format_duration(Duration::from_millis(1500)), "1.5s");
    assert_eq!(format_duration(Duration::from_secs(12)), "12.0s");
}

use crate::plugins::runtime::traits::session::MockPluginSession;

#[test]
fn resolve_rate_limit_key_returns_resolved_when_key_present() {
    let mut session = MockPluginSession::new();
    session.expect_key_get().return_const(Some("user-1".to_string()));
    let mut plugin_log = PluginLog::new("RateLimitLocal");
    let keys = vec![KeyGet::RemoteIp];

    let out = resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Deny, &mut plugin_log);

    assert!(matches!(out, RateLimitKey::Resolved(k) if k == "user-1"));
}

#[test]
fn resolve_rate_limit_key_falls_back_to_default_key() {
    let mut session = MockPluginSession::new();
    session.expect_key_get().return_const(None);
    let mut plugin_log = PluginLog::new("RateLimitLocal");
    let keys = vec![KeyGet::RemoteIp];

    let out = resolve_rate_limit_key(&mut session, &keys, Some("dflt"), &OnKeyMiss::Deny, &mut plugin_log);

    assert!(matches!(out, RateLimitKey::Resolved(k) if k == "dflt"));
    assert!(plugin_log.contains("key miss:default"));
}

#[test]
fn resolve_rate_limit_key_allows_on_missing_allow() {
    let mut session = MockPluginSession::new();
    session.expect_key_get().return_const(None);
    let mut plugin_log = PluginLog::new("RateLimitLocal");
    let keys = vec![KeyGet::RemoteIp];

    let out = resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Allow, &mut plugin_log);

    assert!(matches!(out, RateLimitKey::Allow));
    assert!(plugin_log.contains("key miss:allow"));
}

#[test]
fn resolve_rate_limit_key_denies_with_canonical_tag_on_missing_deny() {
    let mut session = MockPluginSession::new();
    session.expect_key_get().return_const(None);
    let mut plugin_log = PluginLog::new("RateLimitLocal");
    let keys = vec![KeyGet::RemoteIp];

    let out = resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Deny, &mut plugin_log);

    assert!(matches!(out, RateLimitKey::Deny));
    // Canonical key-miss-denial tag, shared by both rate-limit plugins.
    assert!(plugin_log.contains("Deny key miss"));
}

/// Resolve a two-header key configuration, returning the composed bucket key.
fn resolve_tenant_user_key(tenant: Option<&str>, user: Option<&str>) -> String {
    let mut session = MockPluginSession::new();
    set_key_headers(&mut session, &[("x-tenant", tenant), ("x-user", user)]);
    let mut plugin_log = PluginLog::new("RateLimitLocal");
    let keys = vec![
        KeyGet::Header {
            name: "X-Tenant".to_string(),
        },
        KeyGet::Header {
            name: "X-User".to_string(),
        },
    ];

    match resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Deny, &mut plugin_log) {
        RateLimitKey::Resolved(k) => k,
        _ => panic!("expected a resolved key"),
    }
}

#[test]
fn resolve_rate_limit_key_multi_source_separator_is_unambiguous() {
    // A plain "_" join would render both tuples as "t1_u_2", merging the two
    // clients into one bucket.
    let a = resolve_tenant_user_key(Some("t1"), Some("u_2"));
    let b = resolve_tenant_user_key(Some("t1_u"), Some("2"));
    assert_ne!(a, b);
}

#[test]
fn resolve_rate_limit_key_multi_source_unresolved_slot_keeps_position() {
    // Skipping an unresolved source would shift the remaining one, so both
    // tuples would render as "x".
    let a = resolve_tenant_user_key(None, Some("x"));
    let b = resolve_tenant_user_key(Some("x"), None);
    assert_ne!(a, b);
}

#[test]
fn compose_rate_limit_key_single_source_is_verbatim() {
    assert_eq!(
        compose_rate_limit_key(&[Some(Cow::Borrowed("192.168.1.1"))]),
        "192.168.1.1"
    );
}

#[test]
fn resolve_rate_limit_key_empty_key_list_still_rate_limits() {
    let mut session = MockPluginSession::new();
    session.expect_key_get().times(0);
    let mut plugin_log = PluginLog::new("RateLimitLocal");

    // No configured source is not a key miss: OnKeyMiss::Allow must not
    // turn an empty `key` list into a silent bypass of the limiter.
    let out = resolve_rate_limit_key(&mut session, &[], None, &OnKeyMiss::Allow, &mut plugin_log);

    assert!(matches!(out, RateLimitKey::Resolved(k) if k.is_empty()));
    let rendered = serde_json::to_value(&plugin_log).expect("plugin log must serialize");
    assert!(rendered.get("log").is_none());
}

#[test]
fn compose_rate_limit_key_multi_source_is_length_prefixed() {
    let key = compose_rate_limit_key(&[Some(Cow::Borrowed("192.168.1.1")), Some(Cow::Borrowed("apikey123"))]);
    assert_eq!(key, "11:192.168.1.1_9:apikey123");
}

#[test]
fn resolve_rate_limit_key_two_empty_values_only_allocate_output() {
    let mut session = MockPluginSession::new();
    set_key_headers(&mut session, &[("x-empty", Some(""))]);
    // Warm mock dispatch outside allocation measurement.
    let _ = session.request_header_map();
    let mut log = PluginLog::new("RateLimitLocal");
    let keys = [
        KeyGet::Header { name: "x-empty".into() },
        KeyGet::Header { name: "x-empty".into() },
    ];
    let (out, calls) = crate::test_allocation_counter::count_allocations(|| {
        resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Deny, &mut log)
    });
    assert!(matches!(out, RateLimitKey::Resolved(key) if key == "0:_0:"));
    assert_eq!(
        calls, 1,
        "two-source slots must not allocate separately from the output"
    );
}

#[test]
fn resolve_rate_limit_key_spilled_slots_preserve_order_and_empty_values() {
    let mut session = MockPluginSession::new();
    let keys = [
        KeyGet::Header { name: "x-empty".into() },
        KeyGet::Header {
            name: "x-missing".into(),
        },
        KeyGet::Ctx { name: "utf8".into() },
    ];
    set_key_headers(&mut session, &[("x-empty", Some(""))]);
    session
        .expect_get_ctx_var_ref()
        .withf(|name| name == "utf8")
        .times(1)
        .return_const(Some("é_:"));
    let mut log = PluginLog::new("RateLimitLocal");
    let out = resolve_rate_limit_key(&mut session, &keys, Some("fallback"), &OnKeyMiss::Deny, &mut log);
    assert!(matches!(out, RateLimitKey::Resolved(key) if key == "0:_-_4:é_:"));
    assert!(!log.contains("key miss:default"));
}

#[test]
fn resolve_rate_limit_key_all_missing_inline_and_spilled_slots_use_fallback() {
    for count in [2, 3] {
        let mut session = MockPluginSession::new();
        session.expect_key_get().times(0);
        session.expect_remote_ip().times(count).return_const(String::new());
        let keys = vec![KeyGet::RemoteIp; count];
        let mut log = PluginLog::new("RateLimitLocal");
        let out = resolve_rate_limit_key(&mut session, &keys, Some("fallback"), &OnKeyMiss::Deny, &mut log);
        assert!(matches!(out, RateLimitKey::Resolved(key) if key == "fallback"));
        assert!(log.contains("key miss:default"));
    }
}

fn set_key_headers(session: &mut MockPluginSession, values: &[(&str, Option<&str>)]) {
    let mut headers = http::HeaderMap::new();
    for (name, value) in values {
        if let Some(value) = value {
            headers.insert(
                http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                http::HeaderValue::from_str(value).unwrap(),
            );
        }
    }
    session.expect_request_header_map().return_const(headers);
    session.expect_key_get().times(0);
}

#[test]
fn borrowed_nonempty_key_parts_only_allocate_output() {
    let mut session = MockPluginSession::new();
    set_key_headers(&mut session, &[("x-tenant", Some("t1_2")), ("x-user", Some("user:3"))]);
    let _ = session.request_header_map();
    let keys = [
        KeyGet::Header {
            name: "x-tenant".into(),
        },
        KeyGet::Header { name: "x-user".into() },
    ];
    let mut log = PluginLog::new("RateLimitLocal");
    let (out, calls) = crate::test_allocation_counter::count_allocations(|| {
        resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Deny, &mut log)
    });
    assert!(matches!(out, RateLimitKey::Resolved(key) if key == "4:t1_2_6:user:3"));
    assert_eq!(calls, 1);
}

#[test]
fn composite_owned_source_and_borrowed_source_preserve_encoding() {
    let mut session = MockPluginSession::new();
    session
        .expect_key_get()
        .withf(|key| matches!(key, KeyGet::RemoteIpAndPath))
        .times(1)
        .return_once(|_| Some("192.0.2.1:/x".into()));
    session
        .expect_get_ctx_var_ref()
        .withf(|name| name == "user")
        .times(1)
        .return_const(Some("é_:"));
    let keys = [KeyGet::RemoteIpAndPath, KeyGet::Ctx { name: "user".into() }];
    let mut log = PluginLog::new("RateLimitLocal");
    let out = resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Deny, &mut log);
    assert!(matches!(out, RateLimitKey::Resolved(key) if key == "12:192.0.2.1:/x_4:é_:"));
}
