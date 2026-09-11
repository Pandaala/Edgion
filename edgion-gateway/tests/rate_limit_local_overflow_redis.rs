//! Real Redis overflow admission through independently compiled Gateway runtimes.

use std::sync::Arc;
use std::time::Duration;

use pingora_proxy::Session;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use edgion_gateway::ctx::EdgionHttpContext;
use edgion_gateway::plugins::http::{compile_plugins, CompiledPlugins};
use edgion_resources::resources::EdgionPlugins;

fn overflow_resource(name: &str, failure: &str) -> Value {
    json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"namespace": "overflow-tests", "name": name},
        "spec": {"requestPlugins": [
            {"type": "RateLimitLocal", "alias": "local-budget", "config": {
                "rate": 1, "interval": "1h", "key": [],
                "onLimitExceeded": "Continue", "showLimitHeaders": false
            }},
            {"type": "RateLimitRedis", "alias": "overflow-budget",
             "conditions": {"run": {"allOf": [
                 {"type": "rateLimitExceeded", "pluginAlias": "local-budget"}
             ]}},
             "config": {"redisRef": "overflow-tests/intentionally-absent", "key": [],
                 "policies": [{"rate": 1, "interval": "1h", "algorithm": "TokenBucket"}],
                 "onRedisFailure": failure, "showLimitHeaders": true
             }}
        ]}
    })
}

fn compile(value: Value) -> Arc<CompiledPlugins> {
    let resource: EdgionPlugins = serde_json::from_value(value).unwrap();
    let compiled = compile_plugins(&resource);
    assert!(compiled.preparse_errors.is_empty(), "{:?}", compiled.preparse_errors);
    compiled
}

async fn session(extra_headers: &str) -> (Session, tokio::io::DuplexStream) {
    let (mut client, server) = tokio::io::duplex(8192);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(format!("GET /overflow HTTP/1.1\r\nHost: test\r\n{extra_headers}\r\n").as_bytes())
        .await
        .unwrap();
    assert!(session.read_request().await.unwrap());
    (session, client)
}

async fn request(compiled: &CompiledPlugins, grpc: bool) -> (Option<u16>, String) {
    let (mut session, mut client) = session("").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = grpc;
    compiled
        .plugin_runtime
        .run_request_plugins(&mut session, &mut ctx)
        .await;
    let status = session.response_written().map(|header| header.status.as_u16());
    let wire = if status.is_some() {
        let mut bytes = vec![0; 8192];
        let count = tokio::time::timeout(Duration::from_secs(1), client.read(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        String::from_utf8(bytes[..count].to_vec()).unwrap()
    } else {
        String::new()
    };
    (status, wire)
}

/// Uses the authenticated Redis fixture from LinkSys/Redis on port 16379.
/// Run explicitly with `cargo test -p edgion-gateway --test rate_limit_local_overflow_redis -- --ignored`.
#[tokio::test]
#[ignore = "requires the LinkSys/Redis integration fixture on 127.0.0.1:16379"]
async fn real_redis_shares_only_overflow_between_compiled_instances() {
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::time::{SystemTime, UNIX_EPOCH};

    use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;

    use edgion_gateway::link_sys::{get_global_link_sys_store, get_redis_client};

    let unique = format!(
        "real-{}",
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    );
    let link_key = format!("overflow-tests/{unique}");
    let mut link: LinkSys = serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1", "kind": "LinkSys",
        "metadata": {"namespace": "overflow-tests", "name": unique},
        "spec": {"type": "redis", "config": {
            "endpoints": ["redis://127.0.0.1:16379"], "pool": {"size": 1},
            "auth": {"secretRef": {"name": "redis-test-auth"}},
            "timeout": {"connect": 3000, "read": 2000, "write": 2000}
        }}
    }))
    .unwrap();
    let SystemConfig::Redis(config) = &mut link.spec.config else {
        panic!("Redis fixture")
    };
    config.auth.as_mut().unwrap().slot.secret = Some(Secret {
        data: Some(BTreeMap::from([(
            "password".to_string(),
            ByteString(b"edgion-test-pwd".to_vec()),
        )])),
        ..Default::default()
    });
    let store = get_global_link_sys_store();
    store.update(HashMap::from([(link_key.clone(), link)]), &HashSet::new());
    struct RemoveLink(String);
    impl Drop for RemoveLink {
        fn drop(&mut self) {
            get_global_link_sys_store().update(HashMap::new(), &HashSet::from([self.0.clone()]));
        }
    }
    let _cleanup = RemoveLink(link_key.clone());
    let client = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(client) = get_redis_client(&link_key).filter(|client| client.healthy()) {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("start the authenticated LinkSys/Redis fixture before this test");

    let mut value = overflow_resource(&unique, "Deny");
    value["spec"]["requestPlugins"][1]["config"]["redisRef"] = json!(link_key);
    value["spec"]["requestPlugins"][1]["config"]["keyPrefix"] = json!("overflow-test:");
    let first_instance = compile(value.clone());
    let second_instance = compile(value);
    let bucket_key = format!("overflow-test:overflow-tests/{unique}/overflow-budget:tb:0:");
    assert!(client.hgetall(&bucket_key).await.unwrap().is_empty());

    // Each compiled instance has independent CMS state. Neither local pass
    // creates Redis state; the two overflows then consume one shared token.
    assert!(request(&first_instance, false).await.0.is_none());
    assert!(request(&second_instance, false).await.0.is_none());
    assert!(client.hgetall(&bucket_key).await.unwrap().is_empty());
    assert!(request(&first_instance, false).await.0.is_none());
    let state = client.hgetall(&bucket_key).await.unwrap();
    assert_eq!(state.get("tokens").map(String::as_str), Some("0"));
    let (denied, wire) = request(&second_instance, false).await;
    assert_eq!(denied, Some(429));
    assert!(wire.to_ascii_lowercase().contains("x-ratelimit-remaining: 0"));
    client.del(&[&bucket_key]).await.unwrap();
}
