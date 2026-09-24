use crate::conf_sync::{ConfigSyncServer, ServerCache};
use edgion_common::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use edgion_resources::redact::AdminRedactGuard;
use edgion_resources::resources::link_sys::webhook::ResolvedSecrets;
use edgion_resources::resources::link_sys::{LinkSys, LinkSysSpec, SystemConfig, WebhookServiceConfig};
use kube::api::ObjectMeta;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Build a `ServerCache<LinkSys>` containing one LinkSys whose webhook config
/// carries a known plaintext value in `resolved_secrets`.
fn make_link_sys_cache() -> ServerCache<LinkSys> {
    let cache = ServerCache::<LinkSys>::new(10);

    let mut secrets = BTreeMap::new();
    secrets.insert("ns/n:k".to_string(), "supersecret".to_string());
    let webhook_config = WebhookServiceConfig {
        resolved_secrets: Some(ResolvedSecrets(secrets)),
        ..Default::default()
    };

    let link_sys = LinkSys {
        metadata: ObjectMeta {
            name: Some("test-webhook".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Webhook(webhook_config),
        },
        status: None,
    };

    cache.apply_change(ResourceChange::InitAdd, link_sys, Some(1));
    cache
}

/// `ListDataSimple.data` is `Bytes` now; tests assert on its UTF-8 view.
fn data_str(d: &bytes::Bytes) -> &str {
    std::str::from_utf8(d).expect("list payload is serde_json output, always valid UTF-8")
}

/// Prove that holding `AdminRedactGuard` across `ConfigSyncServer::list()` causes
/// `resolved_secrets` to appear as `"[redacted]"` instead of the real value.
///
/// This is the exact code path exercised by `AdminState::cache_list_resources` after
/// the Task-9 guard placement: `cache_list_resources` calls `server.list(kind_str)`
/// inside a block holding `_redact`.
///
/// Building a fully initialised `AdminState` (with a ready `ConfigSyncServer`) in a
/// unit test requires running the full `ConfCenter::init_resources` lifecycle, which
/// is impractical here.  The guard placement in `cache_list_resources` is therefore
/// validated at the `ConfigSyncServer` level — the layer the guard directly covers.
#[test]
fn configserver_redacts_resolved_secrets() {
    let cache = make_link_sys_cache();
    let server = ConfigSyncServer::new();
    server.register_watch_obj("LinkSys", Arc::new(cache));

    // Without guard: real value must be present (confirms the adapter IS the gate).
    let no_guard = server.list("LinkSys").expect("list without guard");
    assert!(
        data_str(&no_guard.data).contains("supersecret"),
        "without guard: real value must appear — got: {}",
        data_str(&no_guard.data)
    );
    assert!(
        !data_str(&no_guard.data).contains("[redacted]"),
        "without guard: must not redact — got: {}",
        data_str(&no_guard.data)
    );

    // With guard: secret must be redacted and real value must not appear.
    let with_guard = {
        let _redact = AdminRedactGuard::new();
        server.list("LinkSys").expect("list with guard")
    };
    assert!(
        data_str(&with_guard.data).contains("[redacted]"),
        "with guard: must redact — got: {}",
        data_str(&with_guard.data)
    );
    assert!(
        !data_str(&with_guard.data).contains("supersecret"),
        "with guard: real value must not leak — got: {}",
        data_str(&with_guard.data)
    );
}

/// The cached transport path ships real secrets; the admin path stays
/// redacted; neither poisons the other, in either call order.
#[tokio::test]
async fn sync_cached_path_ships_real_secrets_and_stays_unpoisoned() {
    // AdminRedactGuard is a thread-local. This test relies on the
    // current-thread runtime (#[tokio::test] default): every poll of
    // list_sync_cached runs on the guard's thread, so redact_active() is
    // observable across the .await. On a multi-thread runtime this test
    // would silently assert nothing.
    let cache = make_link_sys_cache();
    let server = ConfigSyncServer::new();
    server.register_watch_obj("LinkSys", Arc::new(cache));

    // conf_sync transport (cached): real value.
    let synced = server.list_sync_cached("LinkSys").await.expect("cached list");
    assert!(data_str(&synced.data).contains("supersecret"));

    {
        let _redact = AdminRedactGuard::new();
        // Admin path: redacted.
        let admin = server.list("LinkSys").expect("admin list");
        assert!(!data_str(&admin.data).contains("supersecret"));
        assert!(data_str(&admin.data).contains("[redacted]"));
        // Even through the cached method, a redacting caller never sees the
        // warm real-bytes entry.
        let guarded = server.list_sync_cached("LinkSys").await.expect("guarded list");
        assert!(!data_str(&guarded.data).contains("supersecret"));
        assert!(data_str(&guarded.data).contains("[redacted]"));
    }

    // The cache still serves real bytes afterwards (not poisoned).
    let again = server.list_sync_cached("LinkSys").await.expect("cached list again");
    assert!(data_str(&again.data).contains("supersecret"));
}
