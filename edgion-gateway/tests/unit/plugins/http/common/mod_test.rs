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
    let (tenant, user) = (tenant.map(str::to_string), user.map(str::to_string));
    let mut session = MockPluginSession::new();
    session.expect_key_get().returning(move |k| match k {
        KeyGet::Header { name } if name == "X-Tenant" => tenant.clone(),
        KeyGet::Header { name } if name == "X-User" => user.clone(),
        _ => None,
    });
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
        compose_rate_limit_key(&[Some("192.168.1.1".to_string())]),
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
    let key = compose_rate_limit_key(&[Some("192.168.1.1".to_string()), Some("apikey123".to_string())]);
    assert_eq!(key, "11:192.168.1.1_9:apikey123");
}

#[test]
fn resolve_rate_limit_key_two_empty_values_only_allocate_output() {
    let mut session = MockPluginSession::new();
    session.expect_key_get().times(4).returning(|_| Some(String::new()));
    // Initialize mock dispatch bookkeeping outside the allocation measurement.
    assert_eq!(session.key_get(&KeyGet::RemoteIp), Some(String::new()));
    assert_eq!(session.key_get(&KeyGet::RemoteIp), Some(String::new()));
    let mut log = PluginLog::new("RateLimitLocal");
    let keys = [KeyGet::RemoteIp, KeyGet::RemoteIp];
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
    let keys: Vec<_> = (0..3)
        .map(|index| KeyGet::Header {
            name: format!("X-Key-{index}"),
        })
        .collect();
    let mut sequence = mockall::Sequence::new();
    for (key, value) in keys.iter().cloned().zip([Some(""), None, Some("é_:")]) {
        session
            .expect_key_get()
            .withf(move |actual| actual == &key)
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(move |_| value.map(str::to_owned));
    }
    let mut log = PluginLog::new("RateLimitLocal");
    let out = resolve_rate_limit_key(&mut session, &keys, Some("fallback"), &OnKeyMiss::Deny, &mut log);
    assert!(matches!(out, RateLimitKey::Resolved(key) if key == "0:_-_4:é_:"));
    assert!(!log.contains("key miss:default"));
}

#[test]
fn resolve_rate_limit_key_all_missing_inline_and_spilled_slots_use_fallback() {
    for count in [2, 3] {
        let mut session = MockPluginSession::new();
        session.expect_key_get().times(count).return_const(None);
        let keys = vec![KeyGet::RemoteIp; count];
        let mut log = PluginLog::new("RateLimitLocal");
        let out = resolve_rate_limit_key(&mut session, &keys, Some("fallback"), &OnKeyMiss::Deny, &mut log);
        assert!(matches!(out, RateLimitKey::Resolved(key) if key == "fallback"));
        assert!(log.contains("key miss:default"));
    }
}
