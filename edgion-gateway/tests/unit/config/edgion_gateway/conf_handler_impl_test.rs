use super::*;
use crate::routes::http::proxy_http::preflight_handler::ResolvedPreflightMode;
use edgion_resources::resources::edgion_gateway_config::{
    DnsResolverConfig, EdgionGatewayConfigSpec, PreflightMode, PreflightPolicy, TcpTimeout, DEFAULT_TCP_CONNECT,
    DEFAULT_TCP_IDLE,
};
use std::collections::HashMap;

// Serialize against every test module that mutates gateway-wide stores.
use crate::backends::degrade_threshold::GLOBAL_STORE_TEST_LOCK as TEST_MUTEX;

fn make_config(name: &str) -> EdgionGatewayConfig {
    EdgionGatewayConfig::new(
        name,
        EdgionGatewayConfigSpec {
            current_status: None,
            server: None,
            http_timeout: None,
            retry: Default::default(),
            real_ip: None,
            forwarded_headers: Default::default(),
            security_protect: None,
            request_body: Default::default(),
            waf: Default::default(),
            global_plugins_ref: None,
            plugin_policy: None,
            access_log_extern: None,
            preflight_policy: None,
            link_sys: None,
            tcp_timeout: None,
            load_balancing: None,
            outbound_tls: None,
            dns_resolver: None,
        },
    )
}

fn make_config_with_l4_timeouts(name: &str, connect: &str, idle: &str) -> EdgionGatewayConfig {
    let mut config = make_config(name);
    config.spec.tcp_timeout = Some(TcpTimeout {
        connect_timeout: connect.to_string(),
        idle_timeout: idle.to_string(),
    });
    config
}

fn clear_store() {
    let mut bindings = EDGION_GATEWAY_CONFIG_WRITE_LOCK.lock();
    publish_snapshot(HashMap::new(), &mut bindings);
}

#[test]
fn binding_survives_missing_update_delete_recreate_and_prunes_expired_entries() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    assert_eq!(gateway_config_binding_stats().count, 0);
    let handler = EdgionGatewayConfigHandler::new();
    let unbound = bind_gateway_config(None);
    let bound = bind_gateway_config(Some("default"));
    assert!(!Arc::ptr_eq(&unbound, &bound));
    assert!(Arc::ptr_eq(&bound, &bind_gateway_config(Some("default"))));
    assert!(Arc::ptr_eq(&bound.load(), &default_gateway_config()));
    let config = |retries| {
        let mut config = make_config("default");
        config.spec.retry.attempts = retries;
        Arc::new(config)
    };
    handler.full_set(&HashMap::from([("default".into(), config(17))]));
    let pinned = bound.load();
    let old_runtime = Arc::downgrade(&pinned);
    assert_eq!(pinned.retry_attempts, 17);
    assert!(Arc::ptr_eq(&unbound.load(), &default_gateway_config()));
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("default".into(), config(23))]),
        HashSet::new(),
    );
    assert_eq!(bound.load().retry_attempts, 23);
    assert_eq!(pinned.retry_attempts, 17);
    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["default".into()]));
    assert!(Arc::ptr_eq(&bound.load(), &default_gateway_config()));
    assert!(Arc::ptr_eq(&bound, &bind_gateway_config(Some("default"))));
    handler.partial_update(
        HashMap::from([("default".into(), config(29))]),
        HashMap::new(),
        HashSet::new(),
    );
    assert_eq!(bound.load().retry_attempts, 29);
    handler.full_set(&HashMap::new());
    assert!(Arc::ptr_eq(&bound.load(), &default_gateway_config()));
    handler.full_set(&HashMap::from([("default".into(), config(31))]));
    assert_eq!(bound.load().retry_attempts, 31);
    drop(pinned);
    assert!(old_runtime.upgrade().is_none());

    let transient = bind_gateway_config(Some("transient"));
    drop(transient);
    assert_eq!(gateway_config_binding_stats().expired_count, 1);
    // Even rebinding an already live name must prune unrelated dead entries.
    assert!(Arc::ptr_eq(&bound, &bind_gateway_config(Some("default"))));
    assert_eq!(gateway_config_binding_stats().keys, vec!["default"]);
    drop(bound);
    assert_eq!(gateway_config_binding_stats().expired_count, 1);
    handler.full_set(&HashMap::new());
    let stats = gateway_config_binding_stats();
    assert_eq!(stats.count, 0);
    assert_eq!(stats.expired_count, 0);
    assert!(stats.keys.is_empty());
}

#[test]
fn concurrent_binding_and_publication_cannot_lose_the_new_generation() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    for retries in 10..30 {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let bind_barrier = Arc::clone(&barrier);
        let binder = std::thread::spawn(move || {
            bind_barrier.wait();
            bind_gateway_config(Some("concurrent-binding"))
        });
        let writer = std::thread::spawn(move || {
            let mut config = make_config("concurrent-binding");
            config.spec.retry.attempts = retries;
            barrier.wait();
            EdgionGatewayConfigHandler::new()
                .full_set(&HashMap::from([("concurrent-binding".into(), Arc::new(config))]));
        });
        let bound = binder.join().unwrap();
        writer.join().unwrap();
        assert_eq!(bound.load().retry_attempts, retries);
        assert!(Arc::ptr_eq(&bound, &bind_gateway_config(Some("concurrent-binding"))));
    }
    clear_store();
    assert_eq!(gateway_config_binding_stats().count, 0);
}

#[test]
fn test_full_set_replaces_existing() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut data = HashMap::new();
    data.insert("foo".to_string(), Arc::new(make_config("foo")));
    data.insert("bar".to_string(), Arc::new(make_config("bar")));
    handler.full_set(&data);
    assert_eq!(count_edgion_gateway_configs(), 2);

    // full_set again with only one item — old entries must be gone
    let mut data2 = HashMap::new();
    data2.insert("baz".to_string(), Arc::new(make_config("baz")));
    handler.full_set(&data2);
    assert_eq!(count_edgion_gateway_configs(), 1);
}

#[test]
fn test_add_deduplicates() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut add = HashMap::new();
    add.insert("foo".to_string(), Arc::new(make_config("foo")));
    handler.partial_update(add.clone(), HashMap::new(), std::collections::HashSet::new());
    // add same key again
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

    assert_eq!(
        count_edgion_gateway_configs(),
        1,
        "duplicate add must not create two entries"
    );
}

#[test]
fn test_update_overwrites_existing() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut add = HashMap::new();
    add.insert("foo".to_string(), Arc::new(make_config("foo")));
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

    let mut updated = make_config("foo");
    updated.spec.retry.attempts = 99;
    let mut upd = HashMap::new();
    upd.insert("foo".to_string(), Arc::new(updated));
    handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());

    let result = get_edgion_gateway_config_by_name("foo").expect("must exist after update");
    assert_eq!(result.spec.retry.attempts, 99);
    assert_eq!(count_edgion_gateway_configs(), 1);
}

#[test]
fn invalid_remote_ip_header_preserves_valid_body_policy() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut invalid = make_config("foo");
    invalid.spec.forwarded_headers.remote_ip_header = "Host".to_string();
    invalid.spec.request_body.enabled = false;
    invalid.spec.request_body.storage.memory_limit = "384KiB".to_string();
    invalid.spec.request_body.max_body_size = "3MiB".to_string();
    invalid.spec.request_body.storage.operation_timeout = "3s".to_string();
    handler.partial_update(
        HashMap::from([("foo".to_string(), Arc::new(invalid))]),
        HashMap::new(),
        HashSet::new(),
    );

    let resolved = get_resolved_gateway_config(Some("foo"));
    assert_eq!(resolved.remote_ip_header, "x-real-ip");
    assert_eq!(
        resolved.request_body,
        ResolvedRequestBodyConfig {
            enabled: false,
            memory_buffer_bytes: 384 * 1024,
            max_body_size_bytes: 3 * 1024 * 1024,
            max_buffered_size_bytes: 3 * 1024 * 1024,
            spill_to_disk: true,
            storage_operation_timeout: Duration::from_secs(3),
        }
    );
}

#[test]
fn invalid_retry_attempts_fall_back_before_request_snapshot() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut invalid = make_config("foo");
    invalid.spec.retry.attempts = edgion_resources::resources::edgion_gateway_config::MAX_RETRY_ATTEMPTS + 1;
    handler.partial_update(
        HashMap::from([("foo".to_string(), Arc::new(invalid))]),
        HashMap::new(),
        HashSet::new(),
    );

    assert_eq!(get_resolved_gateway_config(Some("foo")).retry_attempts, 2);
}

#[test]
fn test_update_miss_inserts() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    // update on empty store must insert (not silently discard)
    let mut upd = HashMap::new();
    upd.insert("ghost".to_string(), Arc::new(make_config("ghost")));
    handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());

    assert_eq!(count_edgion_gateway_configs(), 1, "update miss must insert");
}

#[test]
fn test_remove_works() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut add = HashMap::new();
    add.insert("foo".to_string(), Arc::new(make_config("foo")));
    add.insert("bar".to_string(), Arc::new(make_config("bar")));
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

    let mut remove = std::collections::HashSet::new();
    remove.insert("foo".to_string());
    handler.partial_update(HashMap::new(), HashMap::new(), remove);

    assert_eq!(count_edgion_gateway_configs(), 1);
    assert!(get_edgion_gateway_config_by_name("foo").is_none());
    assert!(get_edgion_gateway_config_by_name("bar").is_some());
}

#[test]
fn test_remove_miss_does_not_panic() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut remove = std::collections::HashSet::new();
    remove.insert("nonexistent".to_string());
    // must not panic
    handler.partial_update(HashMap::new(), HashMap::new(), remove);
    assert_eq!(count_edgion_gateway_configs(), 0);
}

#[test]
fn http_request_config_updates_joint_pair_and_pins_in_flight_value() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut initial = make_config("http-request-policy");
    initial.spec.forwarded_headers.remote_ip_header = "X-Initial-Remote-IP".to_string();
    initial.spec.request_body.enabled = false;
    initial.spec.request_body.storage.memory_limit = "256KiB".to_string();
    initial.spec.request_body.max_body_size = "1MiB".to_string();
    initial.spec.request_body.storage.spill_to_disk = false;
    initial.spec.request_body.storage.max_buffered_size = Some("512KiB".to_string());
    initial.spec.request_body.storage.operation_timeout = "750ms".to_string();
    initial.spec.preflight_policy = Some(PreflightPolicy {
        mode: PreflightMode::AllOptions,
        status_code: 299,
    });
    let mut add = HashMap::new();
    add.insert("http-request-policy".to_string(), Arc::new(initial));
    handler.partial_update(add, HashMap::new(), HashSet::new());

    let in_flight = get_resolved_gateway_config(Some("http-request-policy"));
    assert_eq!(in_flight.remote_ip_header, "x-initial-remote-ip");
    assert!(!in_flight.request_body.enabled);
    assert_eq!(in_flight.request_body.memory_buffer_bytes, 256 * 1024);
    assert_eq!(in_flight.request_body.max_body_size_bytes, 1024 * 1024);
    assert_eq!(
        in_flight.request_body.storage_operation_timeout,
        std::time::Duration::from_millis(750)
    );
    assert_eq!(
        in_flight.preflight_policy,
        ResolvedPreflightPolicy {
            mode: ResolvedPreflightMode::AllOptions,
            status_code: 299,
        }
    );

    let mut updated = make_config("http-request-policy");
    updated.spec.forwarded_headers.remote_ip_header = "X-Updated-Remote-IP".to_string();
    updated.spec.request_body.storage.memory_limit = "512KiB".to_string();
    updated.spec.request_body.max_body_size = "2MiB".to_string();
    updated.spec.request_body.storage.max_buffered_size = Some("1MiB".to_string());
    updated.spec.request_body.storage.operation_timeout = "2s".to_string();
    updated.spec.preflight_policy = Some(PreflightPolicy {
        mode: PreflightMode::CorsStandard,
        status_code: 205,
    });
    let mut update = HashMap::new();
    update.insert("http-request-policy".to_string(), Arc::new(updated));
    handler.partial_update(HashMap::new(), update, HashSet::new());

    let next = get_resolved_gateway_config(Some("http-request-policy"));
    assert_eq!(next.remote_ip_header, "x-updated-remote-ip");
    assert!(next.request_body.enabled);
    assert_eq!(next.request_body.memory_buffer_bytes, 512 * 1024);
    assert_eq!(next.request_body.max_body_size_bytes, 2 * 1024 * 1024);
    assert!(next.request_body.spill_to_disk);
    assert_eq!(next.request_body.capture_limit(), 1024 * 1024);
    assert!(!in_flight.request_body.spill_to_disk);
    assert_eq!(in_flight.request_body.max_buffered_size_bytes, 512 * 1024);
    assert_eq!(in_flight.request_body.capture_limit(), 256 * 1024);
    assert_eq!(next.request_body.storage_operation_timeout, Duration::from_secs(2));
    assert_eq!(
        next.preflight_policy,
        ResolvedPreflightPolicy {
            mode: ResolvedPreflightMode::CorsStandard,
            status_code: 205,
        }
    );
    assert_eq!(in_flight.remote_ip_header, "x-initial-remote-ip");
    assert!(!in_flight.request_body.enabled);
    assert_eq!(in_flight.request_body.memory_buffer_bytes, 256 * 1024);
    assert_eq!(in_flight.request_body.max_body_size_bytes, 1024 * 1024);
    assert_eq!(
        in_flight.request_body.storage_operation_timeout,
        Duration::from_millis(750)
    );
    assert_eq!(
        in_flight.preflight_policy,
        ResolvedPreflightPolicy {
            mode: ResolvedPreflightMode::AllOptions,
            status_code: 299,
        }
    );

    handler.partial_update(
        HashMap::new(),
        HashMap::new(),
        HashSet::from(["http-request-policy".to_string()]),
    );
    let default = get_resolved_gateway_config(None);
    assert!(Arc::ptr_eq(
        &get_resolved_gateway_config(Some("http-request-policy")),
        &default
    ));
    assert!(Arc::ptr_eq(&get_resolved_gateway_config(Some("missing")), &default));
    assert!(Arc::ptr_eq(&get_resolved_gateway_config(None), &default));
    assert_eq!(default.downstream_keepalive_request_limit, 10_000);
}

#[test]
fn runtime_egc_hot_reload_pins_security_and_compiled_http_policy() {
    let _guard = TEST_MUTEX.lock();
    let handler = EdgionGatewayConfigHandler;
    let mut initial = make_config("runtime-policy");
    initial.spec = serde_json::from_value(serde_json::json!({
        "retry": {"attempts": 7},
        "server": {"enableCompression": true, "downstreamKeepaliveRequestLimit": 19},
        "securityProtect": {
            "xForwardedForLimit": 31, "requireSniHostMatch": false,
            "allowLoopbackUpstream": true, "fallbackSni": "old.example.com",
            "tlsProxyLogRecord": false
        },
        "httpTimeout": {"client": {"readTimeout": "3s"}, "backend": {"defaultConnectTimeout": "4s"}},
        "tcpTimeout": {"connectTimeout": "5s", "idleTimeout": "6s"},
        "realIp": {"trustedIpRefs": [{"name": "local", "namespace": "default"}], "realIpHeader": "X-Forwarded-For"}
    }))
    .unwrap();
    handler.full_set(&HashMap::from([("runtime-policy".into(), Arc::new(initial))]));
    let old = get_resolved_gateway_config(Some("runtime-policy"));
    assert!(Arc::ptr_eq(&old, &get_resolved_gateway_config(Some("runtime-policy"))));
    assert_eq!(old.x_forwarded_for_limit, Some(31));
    assert!(!old.require_sni_host_match);
    assert!(old.allow_loopback_upstream);
    assert_eq!(old.fallback_sni.as_deref(), Some("old.example.com"));
    assert!(!old.tls_proxy_log_record);
    assert_eq!(old.retry_attempts, 7);
    assert!(old.enable_compression);
    assert_eq!(old.downstream_keepalive_request_limit, 19);
    assert_eq!(old.parsed_timeouts.client.read_timeout, Duration::from_secs(3));
    assert_eq!(old.parsed_timeouts.backend.connect_timeout, Duration::from_secs(4));
    assert_eq!(old.l4_timeouts.connect_timeout, Some(Duration::from_secs(5)));
    assert!(old.real_ip_policy.is_some());

    let mut updated = make_config("runtime-policy");
    updated.spec.security_protect = Some(
        serde_json::from_value(serde_json::json!({
            "xForwardedForLimit": 8, "fallbackSni": "new.example.com"
        }))
        .unwrap(),
    );
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("runtime-policy".into(), Arc::new(updated))]),
        HashSet::new(),
    );
    let next = get_resolved_gateway_config(Some("runtime-policy"));
    assert!(!Arc::ptr_eq(&old, &next));
    assert_eq!(next.x_forwarded_for_limit, Some(8));
    assert_eq!(next.fallback_sni.as_deref(), Some("new.example.com"));
    assert_eq!(next.downstream_keepalive_request_limit, 10_000);
    assert!(next.require_sni_host_match && next.tls_proxy_log_record);
    assert!(!next.allow_loopback_upstream && !next.enable_compression);
    assert!(next.real_ip_policy.is_none());
    assert_eq!(next.parsed_timeouts.client.read_timeout, Duration::from_secs(60));
    assert_eq!(old.fallback_sni.as_deref(), Some("old.example.com"));
    assert!(old.allow_loopback_upstream && old.enable_compression);
    assert_eq!(old.l4_timeouts.idle_timeout, Some(Duration::from_secs(6)));

    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["runtime-policy".into()]));
    let default = get_resolved_gateway_config(None);
    assert!(Arc::ptr_eq(
        &default,
        &get_resolved_gateway_config(Some("runtime-policy"))
    ));
    assert!(Arc::ptr_eq(&default, &get_resolved_gateway_config(Some("missing"))));
    assert!(default.fallback_sni.is_none());
    assert!(!default.allow_loopback_upstream);
    assert_eq!(next.fallback_sni.as_deref(), Some("new.example.com"));
    let old_weak = Arc::downgrade(&old);
    drop(old);
    assert!(
        old_weak.upgrade().is_none(),
        "published store must release superseded generations"
    );
    handler.full_set(&HashMap::new());
}

#[test]
fn waf_expensive_checks_follow_runtime_publication_and_request_pins() {
    let _guard = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler;
    let bound = bind_gateway_config(Some("waf-policy"));
    assert!(!bound.load().waf_expensive_checks);
    let mut enabled = make_config("waf-policy");
    enabled.spec.waf.expensive_checks = true;
    handler.full_set(&HashMap::from([("waf-policy".into(), Arc::new(enabled.clone()))]));
    let old_request = crate::EdgionHttpContext::with_gateway_config(bound.load());
    assert!(old_request.egc.waf_expensive_checks);
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("waf-policy".into(), Arc::new(make_config("waf-policy")))]),
        HashSet::new(),
    );
    let new_request = crate::EdgionHttpContext::with_gateway_config(bound.load());
    assert!(!new_request.egc.waf_expensive_checks);
    assert!(old_request.egc.waf_expensive_checks);
    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["waf-policy".into()]));
    assert!(!bound.load().waf_expensive_checks);
    handler.partial_update(
        HashMap::from([("waf-policy".into(), Arc::new(enabled))]),
        HashMap::new(),
        HashSet::new(),
    );
    assert!(bound.load().waf_expensive_checks);
    assert!(!new_request.egc.waf_expensive_checks);
    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["waf-policy".into()]));
    assert!(!bound.load().waf_expensive_checks);
    assert!(old_request.egc.waf_expensive_checks);
    handler.full_set(&HashMap::new());
}

#[test]
fn invalid_preflight_status_is_preserved_for_runtime_fail_close() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();
    let mut config = make_config("invalid-preflight-policy");
    config.spec.preflight_policy = Some(PreflightPolicy {
        mode: PreflightMode::AllOptions,
        status_code: 700,
    });

    handler.full_set(&HashMap::from([(
        "invalid-preflight-policy".to_string(),
        Arc::new(config),
    )]));

    assert_eq!(
        get_resolved_gateway_config(Some("invalid-preflight-policy")).preflight_policy,
        ResolvedPreflightPolicy {
            mode: ResolvedPreflightMode::AllOptions,
            status_code: 700,
        }
    );
}

#[test]
fn invalid_body_fields_preserve_valid_remote_ip_header() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut config = make_config("invalid-body-policy");
    config.spec.forwarded_headers.remote_ip_header = "X-Valid-Remote-IP".to_string();
    config.spec.request_body.enabled = false;
    config.spec.request_body.storage.memory_limit = "invalid".to_string();
    config.spec.request_body.max_body_size = "0".to_string();
    config.spec.request_body.storage.operation_timeout = "invalid".to_string();
    let mut data = HashMap::new();
    data.insert("invalid-body-policy".to_string(), Arc::new(config));
    handler.full_set(&data);

    let resolved = get_resolved_gateway_config(Some("invalid-body-policy"));
    assert_eq!(resolved.remote_ip_header, "x-valid-remote-ip");
    assert_eq!(
        resolved.request_body,
        ResolvedRequestBodyConfig {
            enabled: false,
            ..ResolvedRequestBodyConfig::default()
        }
    );
}

#[test]
fn l4_timeout_policy_updates_for_new_loads_and_pins_old_values() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    handler.partial_update(
        HashMap::from([(
            "l4-policy".to_string(),
            Arc::new(make_config_with_l4_timeouts("l4-policy", "3s", "5m")),
        )]),
        HashMap::new(),
        HashSet::new(),
    );
    let pinned = get_resolved_l4_timeouts(Some("l4-policy"));
    assert_eq!(pinned.connect_timeout, Some(Duration::from_secs(3)));
    assert_eq!(pinned.idle_timeout, Some(Duration::from_secs(300)));

    handler.partial_update(
        HashMap::new(),
        HashMap::from([(
            "l4-policy".to_string(),
            Arc::new(make_config_with_l4_timeouts("l4-policy", "0s", "0ms")),
        )]),
        HashSet::new(),
    );
    assert_eq!(
        pinned,
        ResolvedL4Timeouts {
            connect_timeout: Some(Duration::from_secs(3)),
            idle_timeout: Some(Duration::from_secs(300)),
        },
        "a connection-scoped copy must not change after publication"
    );
    assert_eq!(
        get_resolved_l4_timeouts(Some("l4-policy")),
        ResolvedL4Timeouts {
            connect_timeout: None,
            idle_timeout: None,
        },
        "a new connection load must observe the updated policy"
    );

    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["l4-policy".to_string()]));
    assert_eq!(
        get_resolved_l4_timeouts(Some("l4-policy")),
        ResolvedL4Timeouts::default()
    );
}

#[test]
fn l4_timeout_policy_preserves_defaults_isolation_and_invalid_fallback() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    handler.full_set(&HashMap::from([
        (
            "default".to_string(),
            Arc::new(make_config_with_l4_timeouts("default", "1s", "2s")),
        ),
        (
            "mixed".to_string(),
            Arc::new(make_config_with_l4_timeouts("mixed", "invalid", "0s")),
        ),
    ]));

    assert_eq!(
        get_resolved_l4_timeouts(None),
        ResolvedL4Timeouts {
            connect_timeout: Some(DEFAULT_TCP_CONNECT),
            idle_timeout: Some(DEFAULT_TCP_IDLE),
        },
        "no parametersRef must not select a real EGC named default"
    );
    assert_eq!(
        get_resolved_l4_timeouts(Some("default")),
        ResolvedL4Timeouts {
            connect_timeout: Some(Duration::from_secs(1)),
            idle_timeout: Some(Duration::from_secs(2)),
        }
    );
    assert_eq!(
        get_resolved_l4_timeouts(Some("mixed")),
        ResolvedL4Timeouts {
            connect_timeout: Some(DEFAULT_TCP_CONNECT),
            idle_timeout: None,
        },
        "each invalid field must fall back independently through its canonical resolver"
    );

    handler.full_set(&HashMap::new());
    assert_eq!(get_resolved_l4_timeouts(Some("default")), ResolvedL4Timeouts::default());
}

#[test]
fn l4_timeout_accessor_does_not_retain_an_obsolete_parent_snapshot() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();
    handler.full_set(&HashMap::from([(
        "l4-policy".to_string(),
        Arc::new(make_config_with_l4_timeouts("l4-policy", "4s", "6s")),
    )]));

    let old_snapshot = EDGION_GATEWAY_CONFIG_STORE.load_full();
    let weak = Arc::downgrade(&old_snapshot);
    assert_eq!(
        get_resolved_l4_timeouts(Some("l4-policy")).connect_timeout,
        Some(Duration::from_secs(4))
    );
    handler.full_set(&HashMap::new());
    drop(old_snapshot);

    assert!(
        weak.upgrade().is_none(),
        "the copied policy must not retain its parent snapshot"
    );
}

#[test]
fn dns_resolver_default_is_precomputed_with_stable_key_winner() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut key_b = make_config("metadata-first");
    key_b.spec.dns_resolver = Some(DnsResolverConfig {
        link_sys_ref: None,
        servers: Some(vec!["192.0.2.2:53".to_string()]),
        cache_ttl: "20s".to_string(),
    });
    let mut key_a = make_config("metadata-second");
    key_a.spec.dns_resolver = Some(DnsResolverConfig {
        link_sys_ref: None,
        servers: Some(vec!["192.0.2.1:53".to_string()]),
        cache_ttl: "10s".to_string(),
    });
    handler.full_set(&HashMap::from([
        ("b".to_string(), Arc::new(key_b)),
        ("a".to_string(), Arc::new(key_a)),
    ]));

    let in_flight = get_egc_dns_resolver_default().expect("lowest store key must win");
    assert_eq!(
        in_flight.candidate().cloned(),
        Some(ResolverCandidate::Servers(vec!["192.0.2.1:53".to_string()]))
    );
    assert_eq!(in_flight.cache_ttl(), Duration::from_secs(10));

    let without_dns = make_config("metadata-second");
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("a".to_string(), Arc::new(without_dns))]),
        HashSet::new(),
    );
    let next = get_egc_dns_resolver_default().expect("remaining resolver must become the winner");
    assert_eq!(
        next.candidate().cloned(),
        Some(ResolverCandidate::Servers(vec!["192.0.2.2:53".to_string()]))
    );
    assert_eq!(next.cache_ttl(), Duration::from_secs(20));
    assert!(!Arc::ptr_eq(&in_flight, &next));
    assert_eq!(
        in_flight.candidate().cloned(),
        Some(ResolverCandidate::Servers(vec!["192.0.2.1:53".to_string()])),
        "a loaded Arc must pin its candidate generation"
    );
    assert_eq!(
        in_flight.cache_ttl(),
        Duration::from_secs(10),
        "a loaded Arc must pin its TTL generation"
    );

    let mut invalid_system = make_config("metadata-first");
    invalid_system.spec.dns_resolver = Some(DnsResolverConfig {
        link_sys_ref: None,
        servers: None,
        cache_ttl: "invalid".to_string(),
    });
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("b".to_string(), Arc::new(invalid_system))]),
        HashSet::new(),
    );
    let invalid = get_egc_dns_resolver_default().expect("configured system resolver remains represented");
    assert_eq!(invalid.candidate(), None);
    assert_eq!(invalid.cache_ttl(), crate::backends::DEFAULT_DNS_CACHE_TTL);

    let mut zero_ttl = make_config("metadata-first");
    zero_ttl.spec.dns_resolver = Some(DnsResolverConfig {
        link_sys_ref: None,
        servers: Some(vec!["192.0.2.3:53".to_string()]),
        cache_ttl: "0s".to_string(),
    });
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("b".to_string(), Arc::new(zero_ttl))]),
        HashSet::new(),
    );
    assert_eq!(
        get_egc_dns_resolver_default()
            .expect("zero TTL is a valid configured generation")
            .cache_ttl(),
        Duration::ZERO
    );

    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["b".to_string()]));
    assert!(get_egc_dns_resolver_default().is_none());
}

#[test]
fn concurrent_partial_updates_preserve_every_key() {
    use std::sync::{Arc as StdArc, Barrier};

    let _g = TEST_MUTEX.lock();
    clear_store();

    const WRITERS: usize = 16;
    let barrier = StdArc::new(Barrier::new(WRITERS + 1));
    let mut handles = Vec::with_capacity(WRITERS);
    for index in 0..WRITERS {
        let barrier = StdArc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let name = format!("concurrent-{index}");
            let mut add = HashMap::new();
            add.insert(name.clone(), Arc::new(make_config(&name)));
            barrier.wait();
            EdgionGatewayConfigHandler::new().partial_update(add, HashMap::new(), HashSet::new());
        }));
    }

    barrier.wait();
    for handle in handles {
        handle.join().expect("concurrent policy writer must not panic");
    }

    assert_eq!(count_edgion_gateway_configs(), WRITERS);
    for index in 0..WRITERS {
        assert!(
            get_edgion_gateway_config_by_name(&format!("concurrent-{index}")).is_some(),
            "concurrent writer entry must survive"
        );
    }
}

#[test]
fn test_link_sys_config_webhook_max() {
    use edgion_resources::resources::edgion_gateway_config::{EdgionGatewayConfigSpec, LinkSysGlobalConfig};
    let spec = EdgionGatewayConfigSpec {
        current_status: None,
        link_sys: Some(LinkSysGlobalConfig {
            webhook_max_response_bytes: 65536,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(spec.link_sys.unwrap().webhook_max_response_bytes, 65536);
}

#[test]
fn test_list_and_count() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler::new();

    let mut add = HashMap::new();
    add.insert("a".to_string(), Arc::new(make_config("a")));
    add.insert("b".to_string(), Arc::new(make_config("b")));
    add.insert("c".to_string(), Arc::new(make_config("c")));
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

    assert_eq!(count_edgion_gateway_configs(), 3);
    assert_eq!(list_edgion_gateway_configs().len(), 3);
}

fn make_config_with_link_sys(name: &str, max_bytes: usize) -> EdgionGatewayConfig {
    use edgion_resources::resources::edgion_gateway_config::LinkSysGlobalConfig;
    let mut config = make_config(name);
    config.spec.link_sys = Some(LinkSysGlobalConfig {
        webhook_max_response_bytes: max_bytes,
        ..Default::default()
    });
    config
}

#[test]
fn full_set_propagates_link_sys_config() {
    use crate::link_sys::providers::webhook::manager::{configure_webhook_global_max, get_webhook_global_max};
    let _g = TEST_MUTEX.lock();
    clear_store();

    // Reset the global to a known sentinel before the test
    configure_webhook_global_max(16384);
    assert_eq!(get_webhook_global_max(), 16384);

    // Construct a config with a non-default linkSys.webhookMaxResponseBytes
    let config = make_config_with_link_sys("test-config", 123_456);

    let mut data = HashMap::new();
    data.insert("default/test-config".to_string(), Arc::new(config));

    let handler = EdgionGatewayConfigHandler;
    handler.full_set(&data);

    // Global should now reflect the input
    assert_eq!(get_webhook_global_max(), 123_456);

    // Reset to default for other tests
    configure_webhook_global_max(16384);
}

#[test]
fn test_load_balancing_degrade_threshold_applied() {
    use crate::backends::degrade_threshold::get_lb_degrade_default;
    use edgion_resources::resources::edgion_gateway_config::LoadBalancingConfig;

    let _g = TEST_MUTEX.lock();
    clear_store();

    let handler = EdgionGatewayConfigHandler::new();
    let mut cfg = make_config("gw-pt");
    cfg.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 50 });

    let mut data = HashMap::new();
    data.insert("gw-pt".to_string(), Arc::new(cfg));
    handler.full_set(&data);

    assert_eq!(get_lb_degrade_default(), 50);
    crate::backends::degrade_threshold::configure_lb_degrade_default(0);
}

#[test]
fn full_set_link_sys_aggregates_min_deterministically() {
    use crate::link_sys::providers::webhook::manager::{configure_webhook_global_max, get_webhook_global_max};
    let _g = TEST_MUTEX.lock();
    clear_store();
    configure_webhook_global_max(16384);

    // Two cluster-scoped configs with different caps; the strictest (min) wins
    // regardless of HashMap iteration order.
    let mut data = HashMap::new();
    data.insert("a".to_string(), Arc::new(make_config_with_link_sys("a", 65536)));
    data.insert("b".to_string(), Arc::new(make_config_with_link_sys("b", 16384)));

    let handler = EdgionGatewayConfigHandler;
    handler.full_set(&data);
    assert_eq!(get_webhook_global_max(), 16384, "min cap must win over hash order");

    configure_webhook_global_max(16384);
}

#[test]
fn remove_last_link_sys_config_resets_to_default() {
    use crate::link_sys::providers::webhook::manager::{configure_webhook_global_max, get_webhook_global_max};
    let _g = TEST_MUTEX.lock();
    clear_store();
    configure_webhook_global_max(16384);
    let handler = EdgionGatewayConfigHandler;

    // Add a single config lifting the cap to 64 KB.
    let mut add = HashMap::new();
    add.insert("cfg".to_string(), Arc::new(make_config_with_link_sys("cfg", 65536)));
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());
    assert_eq!(get_webhook_global_max(), 65536);

    // Removing the only config carrying link_sys must restore the default,
    // not leave the relaxed cap lingering.
    let mut remove = std::collections::HashSet::new();
    remove.insert("cfg".to_string());
    handler.partial_update(HashMap::new(), HashMap::new(), remove);
    assert_eq!(get_webhook_global_max(), 16384, "delete must reset cap to default");
}

fn make_config_with_link_sys_instances(name: &str, max_instances: usize) -> EdgionGatewayConfig {
    use edgion_resources::resources::edgion_gateway_config::LinkSysGlobalConfig;
    let mut config = make_config(name);
    config.spec.link_sys = Some(LinkSysGlobalConfig {
        max_instances_per_kind: max_instances,
        ..Default::default()
    });
    config
}

#[test]
fn full_set_link_sys_instance_cap_aggregates_min_deterministically() {
    use crate::link_sys::runtime::store::{
        configure_link_sys_max_instances_per_kind, get_link_sys_max_instances_per_kind, LINK_SYS_CAP_TEST_LOCK,
    };
    let _g = TEST_MUTEX.lock();
    let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
    clear_store();
    configure_link_sys_max_instances_per_kind(200);

    let mut data = HashMap::new();
    data.insert("a".to_string(), Arc::new(make_config_with_link_sys_instances("a", 500)));
    data.insert("b".to_string(), Arc::new(make_config_with_link_sys_instances("b", 50)));

    let handler = EdgionGatewayConfigHandler;
    handler.full_set(&data);
    assert_eq!(
        get_link_sys_max_instances_per_kind(),
        50,
        "min instance cap must win over hash order"
    );

    configure_link_sys_max_instances_per_kind(200);
}

#[test]
fn remove_last_link_sys_config_resets_instance_cap_to_default() {
    use crate::link_sys::runtime::store::{
        configure_link_sys_max_instances_per_kind, get_link_sys_max_instances_per_kind, LINK_SYS_CAP_TEST_LOCK,
    };
    let _g = TEST_MUTEX.lock();
    let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
    clear_store();
    configure_link_sys_max_instances_per_kind(200);
    let handler = EdgionGatewayConfigHandler;

    let mut add = HashMap::new();
    add.insert(
        "cfg".to_string(),
        Arc::new(make_config_with_link_sys_instances("cfg", 50)),
    );
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());
    assert_eq!(get_link_sys_max_instances_per_kind(), 50);

    let mut remove = std::collections::HashSet::new();
    remove.insert("cfg".to_string());
    handler.partial_update(HashMap::new(), HashMap::new(), remove);
    assert_eq!(
        get_link_sys_max_instances_per_kind(),
        200,
        "delete must reset instance cap to default"
    );
}

#[test]
fn load_balancing_degrade_threshold_aggregates_max_and_resets() {
    use crate::backends::degrade_threshold::{configure_lb_degrade_default, get_lb_degrade_default};
    use edgion_resources::resources::edgion_gateway_config::LoadBalancingConfig;
    let _g = TEST_MUTEX.lock();
    clear_store();
    configure_lb_degrade_default(0);
    let handler = EdgionGatewayConfigHandler;

    let mut lo = make_config("lo");
    lo.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 30 });
    let mut hi = make_config("hi");
    hi.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 70 });

    let mut data = HashMap::new();
    data.insert("lo".to_string(), Arc::new(lo));
    data.insert("hi".to_string(), Arc::new(hi));
    handler.full_set(&data);
    assert_eq!(get_lb_degrade_default(), 70, "max threshold must win over hash order");

    // Drop back to a single config; threshold follows the survivor.
    let mut only_lo = make_config("lo");
    only_lo.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 30 });
    let mut data2 = HashMap::new();
    data2.insert("lo".to_string(), Arc::new(only_lo));
    handler.full_set(&data2);
    assert_eq!(get_lb_degrade_default(), 30);

    // Empty store resets to the disabled default.
    handler.full_set(&HashMap::new());
    assert_eq!(
        get_lb_degrade_default(),
        0,
        "no load_balancing config must reset to disabled"
    );

    configure_lb_degrade_default(0);
}

#[test]
fn load_balancing_out_of_range_threshold_does_not_poison_max() {
    use crate::backends::degrade_threshold::{configure_lb_degrade_default, get_lb_degrade_default};
    use edgion_resources::resources::edgion_gateway_config::LoadBalancingConfig;
    let _g = TEST_MUTEX.lock();
    clear_store();
    configure_lb_degrade_default(0);
    let handler = EdgionGatewayConfigHandler;

    // One config is out of range (>100); it must be ignored rather than win the
    // max and clamp the whole gateway to 0 (disabled).
    let mut good = make_config("good");
    good.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 70 });
    let mut bad = make_config("bad");
    bad.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 150 });

    let mut data = HashMap::new();
    data.insert("good".to_string(), Arc::new(good));
    data.insert("bad".to_string(), Arc::new(bad));
    handler.full_set(&data);
    assert_eq!(
        get_lb_degrade_default(),
        70,
        "out-of-range threshold must not poison the max"
    );

    configure_lb_degrade_default(0);
}

#[test]
fn outbound_tls_safest_wins_and_resets() {
    use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
    use edgion_resources::resources::common::OutboundTlsConfig;
    let _g = TEST_MUTEX.lock();
    clear_store();
    // Reset global default to known state
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));

    let handler = EdgionGatewayConfigHandler;

    // One config with verify=false, one with no outbound_tls (verify defaults true)
    let mut cfg_a = make_config("a");
    cfg_a.spec.outbound_tls = Some(OutboundTlsConfig {
        verify: false,
        ..OutboundTlsConfig::default()
    });
    let cfg_b = make_config("b"); // outbound_tls: None -> verify=true by default

    let mut data = HashMap::new();
    data.insert("a".to_string(), Arc::new(cfg_a));
    data.insert("b".to_string(), Arc::new(cfg_b));
    handler.full_set(&data);
    // safest-wins: one block is absent (defaults true) → aggregate is true
    assert!(
        outbound_global_default().verify(),
        "one absent block must keep verify=true (safest-wins)"
    );

    // Both verify=false → aggregate is false
    let mut cfg_c = make_config("c");
    cfg_c.spec.outbound_tls = Some(OutboundTlsConfig {
        verify: false,
        ..OutboundTlsConfig::default()
    });
    let mut cfg_d = make_config("d");
    cfg_d.spec.outbound_tls = Some(OutboundTlsConfig {
        verify: false,
        ..OutboundTlsConfig::default()
    });
    let mut data2 = HashMap::new();
    data2.insert("c".to_string(), Arc::new(cfg_c));
    data2.insert("d".to_string(), Arc::new(cfg_d));
    handler.full_set(&data2);
    assert!(
        !outbound_global_default().verify(),
        "both verify=false must aggregate to false"
    );

    // Reset to default for subsequent tests
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    clear_store();
}

#[test]
fn publication_retains_original_egc_arcs_across_full_partial_and_delete() {
    use edgion_common::link_sys_core::outbound_tls::outbound_global_default;
    use edgion_resources::resources::common::OutboundTlsConfig;
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = EdgionGatewayConfigHandler;
    let mut first = make_config("actual");
    first.metadata.uid = Some("first-owner".into());
    first.spec.outbound_tls = Some(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    let first = Arc::new(first);
    handler.full_set(&HashMap::from([("actual".into(), Arc::clone(&first))]));
    assert!(Arc::ptr_eq(
        EDGION_GATEWAY_CONFIG_STORE.load().configs.get("actual").unwrap(),
        &first
    ));
    let captured = outbound_global_default();
    assert!(!captured.verify());
    let mut next = (*first).clone();
    next.metadata.uid = Some("next-owner".into());
    next.spec.outbound_tls.as_mut().unwrap().verify = true;
    let next = Arc::new(next);
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("actual".into(), Arc::clone(&next))]),
        HashSet::new(),
    );
    assert!(Arc::ptr_eq(
        EDGION_GATEWAY_CONFIG_STORE.load().configs.get("actual").unwrap(),
        &next
    ));
    assert!(outbound_global_default().verify());
    assert!(
        !captured.verify(),
        "captured generation stays immutable after publication"
    );
    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["actual".into()]));
    assert!(outbound_global_default().verify());
    assert!(EDGION_GATEWAY_CONFIG_STORE.load().configs.is_empty());
}

#[test]
fn outbound_tls_adopts_mtls_only_global_default() {
    use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
    use edgion_resources::resources::common::{OutboundTlsConfig, SecretObjectReference};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let _g = TEST_MUTEX.lock();
    clear_store();
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    let identity = rcgen::generate_simple_self_signed(vec!["client.example.com".to_string()])
        .expect("generate test client identity");
    let mut data_map = BTreeMap::new();
    data_map.insert("tls.crt".to_string(), ByteString(identity.cert.pem().into_bytes()));
    data_map.insert(
        "tls.key".to_string(),
        ByteString(identity.signing_key.serialize_pem().into_bytes()),
    );

    let mut config = make_config("mtls-only");
    config.spec.outbound_tls = Some(OutboundTlsConfig {
        client_certificate_ref: Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "global-client".to_string(),
            namespace: Some("default".into()),
        }),
        resolved_client_certificate: Some(Secret {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some("global-client".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            data: Some(data_map),
            ..Default::default()
        }),
        ..Default::default()
    });

    let handler = EdgionGatewayConfigHandler;
    handler.full_set(&HashMap::from([("mtls-only".to_string(), Arc::new(config))]));
    let global = outbound_global_default();
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    clear_store();
    assert!(global.client_reference().is_some());
    assert!(global.client_material().is_some());
}

#[test]
fn outbound_tls_conflicting_global_mtls_defaults_fail_closed() {
    use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
    use edgion_resources::resources::common::{OutboundTlsConfig, SecretObjectReference};

    let _g = TEST_MUTEX.lock();
    clear_store();
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));

    let mut first = make_config("first");
    first.spec.outbound_tls = Some(OutboundTlsConfig {
        client_certificate_ref: Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "client-a".to_string(),
            namespace: None,
        }),
        ..Default::default()
    });
    let mut second = make_config("second");
    second.spec.outbound_tls = Some(OutboundTlsConfig {
        client_certificate_ref: Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "client-b".to_string(),
            namespace: None,
        }),
        ..Default::default()
    });

    let handler = EdgionGatewayConfigHandler;
    handler.full_set(&HashMap::from([
        ("first".to_string(), Arc::new(first)),
        ("second".to_string(), Arc::new(second)),
    ]));
    let global = outbound_global_default();
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    clear_store();
    assert!(global.client_reference().is_some());
    assert!(global.client_material().is_none());
}

#[test]
fn partial_update_propagates_link_sys_config_on_add_and_update() {
    use crate::link_sys::providers::webhook::manager::{configure_webhook_global_max, get_webhook_global_max};
    let _g = TEST_MUTEX.lock();
    clear_store();

    configure_webhook_global_max(16384);
    let handler = EdgionGatewayConfigHandler;

    // ADD path
    let cfg_a = make_config_with_link_sys("cfg-a", 33_333);
    let mut add = HashMap::new();
    add.insert("default/cfg-a".to_string(), Arc::new(cfg_a));
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());
    assert_eq!(get_webhook_global_max(), 33_333);

    // UPDATE path
    let cfg_b = make_config_with_link_sys("cfg-a", 44_444);
    let mut upd = HashMap::new();
    upd.insert("default/cfg-a".to_string(), Arc::new(cfg_b));
    handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());
    assert_eq!(get_webhook_global_max(), 44_444);

    // Reset
    configure_webhook_global_max(16384);
}

// ------------------------------------------------------------------
// M2: a single config with validation.caCertificateRefs + resolved_ca_certificates
// is adopted as the cluster-wide default.
// ------------------------------------------------------------------

#[test]
fn aggregate_adopts_single_validation_block() {
    use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
    use edgion_resources::resources::common::CustomCACertificateRef;
    use edgion_resources::resources::common::{outbound_tls::OutboundTlsValidation, OutboundTlsConfig};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    let _g = TEST_MUTEX.lock();
    clear_store();
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));

    let handler = EdgionGatewayConfigHandler;

    let mut data_map = std::collections::BTreeMap::new();
    data_map.insert(
        "ca.crt".to_string(),
        ByteString(b"-----BEGIN CERTIFICATE-----\nMIIBglobal\n-----END CERTIFICATE-----\n".to_vec()),
    );
    let resolved = Secret {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some("my-ca-secret".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(data_map),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    };

    let mut cfg = make_config("egc-validation");
    cfg.spec.outbound_tls = Some(OutboundTlsConfig {
        verify: true,
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![CustomCACertificateRef {
                kind: "Secret".to_string(),
                name: "my-ca-secret".to_string(),
                group: Some(String::new()),
                namespace: Some("default".into()),
            }]),
            well_known_ca_certificates: None,
            hostname: None,
            subject_alt_names: None,
        }),
        resolved_ca_certificates: Some(vec![resolved]),
        client_certificate_ref: None,
        resolved_client_certificate: None,
    });

    let mut data = HashMap::new();
    data.insert("egc-validation".to_string(), Arc::new(cfg));
    handler.full_set(&data);

    // The single validation with caCertificateRefs is adopted as the cluster-wide
    // default, with the resolved CA certificates carried through.
    let global = outbound_global_default();
    assert!(global.validation().is_some(), "global validation must be adopted");
    assert!(
        global.validation().unwrap().ca_certificate_refs.is_some(),
        "ca_certificate_refs must be in the validation"
    );
    assert!(
        global.ca_material().is_some(),
        "resolved_ca_certificates must be carried into the global default"
    );

    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
}

// ------------------------------------------------------------------
// M-C3: outbound TLS aggregation — delete-path restore + CA adopt/conflict
// ------------------------------------------------------------------

#[test]
fn outbound_tls_delete_path_restores_verify_default() {
    use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
    use edgion_resources::resources::common::OutboundTlsConfig;
    let _g = TEST_MUTEX.lock();
    clear_store();
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));

    let handler = EdgionGatewayConfigHandler;

    // full_set: one EGC verify=false, one absent (defaults true) → safest-wins → true
    let mut cfg_a = make_config("a");
    cfg_a.spec.outbound_tls = Some(OutboundTlsConfig {
        verify: false,
        ..OutboundTlsConfig::default()
    });
    let cfg_b = make_config("b"); // outbound_tls: None → verify defaults true
    let mut data = HashMap::new();
    data.insert("a".to_string(), Arc::new(cfg_a));
    data.insert("b".to_string(), Arc::new(cfg_b));
    handler.full_set(&data);
    assert!(
        outbound_global_default().verify(),
        "safest-wins: one absent block must keep verify=true"
    );

    // partial_update removing all entries → default restored
    let mut remove = std::collections::HashSet::new();
    remove.insert("a".to_string());
    remove.insert("b".to_string());
    handler.partial_update(HashMap::new(), HashMap::new(), remove);
    assert!(
        outbound_global_default().verify(),
        "after removing all configs, verify must reset to default (true)"
    );

    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
}

#[test]
fn aggregate_conflicts_fallback_to_system() {
    use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
    use edgion_resources::resources::common::CustomCACertificateRef;
    use edgion_resources::resources::common::{outbound_tls::OutboundTlsValidation, OutboundTlsConfig};
    let _g = TEST_MUTEX.lock();
    clear_store();
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));

    let handler = EdgionGatewayConfigHandler;

    let mut cfg_a = make_config("ca-a");
    cfg_a.spec.outbound_tls = Some(OutboundTlsConfig {
        verify: true,
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![CustomCACertificateRef {
                kind: "Secret".to_string(),
                name: "ca-a-secret".to_string(),
                group: Some(String::new()),
                namespace: None,
            }]),
            well_known_ca_certificates: None,
            hostname: None,
            subject_alt_names: None,
        }),
        resolved_ca_certificates: None,
        client_certificate_ref: None,
        resolved_client_certificate: None,
    });
    let mut cfg_b = make_config("ca-b");
    cfg_b.spec.outbound_tls = Some(OutboundTlsConfig {
        verify: true,
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![CustomCACertificateRef {
                kind: "Secret".to_string(),
                name: "ca-b-secret".to_string(),
                group: Some(String::new()),
                namespace: None,
            }]),
            well_known_ca_certificates: None,
            hostname: None,
            subject_alt_names: None,
        }),
        resolved_ca_certificates: None,
        client_certificate_ref: None,
        resolved_client_certificate: None,
    });

    let mut data = HashMap::new();
    data.insert("ca-a".to_string(), Arc::new(cfg_a));
    data.insert("ca-b".to_string(), Arc::new(cfg_b));
    handler.full_set(&data);

    let global = outbound_global_default();
    assert!(
        global.validation().is_none(),
        "conflicting validations must fall back to None (System), got: {:?}",
        global.validation()
    );

    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
}

#[test]
fn dns_invalid_winner_does_not_select_inline_or_another_egc() {
    use edgion_common::dns::{resolver_select::resolve_effective_spec, ResolverSpec};
    for case in [
        "owner-namespace",
        "owner-empty-namespace",
        "owner-name",
        "target-namespace",
        "target-name",
    ] {
        let mut first = make_config("z-metadata");
        first.spec.dns_resolver=Some(serde_json::from_value(serde_json::json!({"linkSysRef":{"namespace":"dns","name":"resolver"},"servers":["192.0.2.1"],"cacheTtl":"17s"})).unwrap());
        match case {
            "owner-namespace" => first.metadata.namespace = Some("app".into()),
            "owner-empty-namespace" => first.metadata.namespace = Some("".into()),
            "owner-name" => first.metadata.name = None,
            "target-namespace" => {
                first
                    .spec
                    .dns_resolver
                    .as_mut()
                    .unwrap()
                    .link_sys_ref
                    .as_mut()
                    .unwrap()
                    .namespace = " dns ".into()
            }
            _ => {
                first
                    .spec
                    .dns_resolver
                    .as_mut()
                    .unwrap()
                    .link_sys_ref
                    .as_mut()
                    .unwrap()
                    .name = "".into()
            }
        }
        let mut other = make_config("a-metadata");
        other.spec.dns_resolver =
            Some(serde_json::from_value(serde_json::json!({"servers":["192.0.2.2"],"cacheTtl":"30s"})).unwrap());
        let snapshot = EdgionGatewayConfigSnapshot::new(HashMap::from([
            ("a".into(), Arc::new(first.clone())),
            ("b".into(), Arc::new(other)),
        ]));
        let selected = snapshot.dns_resolver_default.unwrap();
        assert!(selected.candidate().is_none(), "{case}");
        assert_eq!(selected.cache_ttl(), Duration::from_secs(17));
        assert!(matches!(
            resolve_effective_spec(&[selected.candidate()], |_| panic!("invalid reference lookup")),
            ResolverSpec::System
        ));
        first.spec.dns_resolver.as_mut().unwrap().link_sys_ref = None;
        let inline = ResolvedDnsResolverDefault::resolve(&first);
        assert_eq!(
            inline.candidate(),
            Some(&ResolverCandidate::Servers(vec!["192.0.2.1".into()]))
        );
        assert_eq!(inline.cache_ttl(), Duration::from_secs(17));
    }
}

#[test]
fn global_real_ip_valid_invalid_clean_publication_and_live_visibility_recovery() {
    use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
    let _guard = TEST_MUTEX.lock();
    let handler = EdgionGatewayConfigHandler;
    let make = |legacy: bool| {
        let mut config = serde_json::json!({"trustedIpRefs": [{"name": "proxies", "namespace": "ops"}]});
        if legacy {
            config["trustedIps"] = serde_json::Value::Null;
        }
        let owner: EdgionGatewayConfig = serde_json::from_value(
            serde_json::json!({"metadata": {"name": "trust-publication"}, "spec": {"realIp": config}}),
        )
        .unwrap();
        Arc::new(serde_json::from_value::<EdgionGatewayConfig>(serde_json::to_value(owner).unwrap()).unwrap())
    };
    handler.full_set(&HashMap::from([("trust-publication".into(), make(false))]));
    let good = get_resolved_gateway_config(Some("trust-publication"));
    let policy = good.real_ip_policy.as_ref().unwrap();
    let store = EdgionConfigDataStore::new();
    let publish = |visibility: &str| {
        let data = serde_json::from_value(serde_json::json!({"metadata": {"name": "proxies", "namespace": "ops"}, "spec": {"visibility": visibility, "data": {"type": "IpList", "config": {"items": [{"name": "local", "cidrs": ["127.0.0.0/8"]}]}}}})).unwrap();
        store.full_set(&HashMap::from([("ops/proxies".into(), Arc::new(data))]));
    };
    let resolve = || {
        policy
            .resolve_in(&store.capture_snapshot(), "127.0.0.1:123", Some("203.0.113.7"))
            .remote_ip
    };
    assert_eq!(resolve(), "127.0.0.1");
    publish("Cluster");
    let held = store.capture_snapshot();
    assert_eq!(resolve(), "203.0.113.7");
    publish("Namespace");
    assert_eq!(resolve(), "127.0.0.1");
    assert_eq!(
        policy.resolve_in(&held, "127.0.0.1", Some("203.0.113.7")).remote_ip,
        "203.0.113.7"
    );
    publish("Cluster");
    assert_eq!(resolve(), "203.0.113.7");
    assert!(Arc::ptr_eq(
        &good,
        &get_resolved_gateway_config(Some("trust-publication"))
    ));
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("trust-publication".into(), make(true))]),
        HashSet::new(),
    );
    assert!(get_resolved_gateway_config(Some("trust-publication"))
        .real_ip_policy
        .is_none());
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("trust-publication".into(), make(false))]),
        HashSet::new(),
    );
    assert!(get_resolved_gateway_config(Some("trust-publication"))
        .real_ip_policy
        .is_some());
    handler.partial_update(
        HashMap::new(),
        HashMap::new(),
        HashSet::from(["trust-publication".into()]),
    );
}
