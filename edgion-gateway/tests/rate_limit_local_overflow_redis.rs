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
             "config": {"quotas":[{"name":"default", "key": [],
                 "policies": [{"rate": 1, "interval": "1h", "algorithm": "TokenBucket"}],}],"redisRef": "overflow-tests/intentionally-absent",
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

async fn request_details(
    compiled: &CompiledPlugins,
    grpc: bool,
) -> (Option<u16>, String, pingora_http::ResponseHeader) {
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
    let mut queued = pingora_http::ResponseHeader::build(200, None).unwrap();
    ctx.apply_pending_response_header_ops(&mut queued);
    (status, wire, queued)
}

async fn request(compiled: &CompiledPlugins, grpc: bool) -> (Option<u16>, String) {
    let (status, wire, _) = request_details(compiled, grpc).await;
    (status, wire)
}

struct RedisFixture {
    unique: String,
    link_key: String,
    client: Arc<edgion_gateway::link_sys::providers::redis::RedisLinkClient>,
}

impl Drop for RedisFixture {
    fn drop(&mut self) {
        edgion_gateway::link_sys::get_global_link_sys_store().update(
            std::collections::HashMap::new(),
            &std::collections::HashSet::from([self.link_key.clone()]),
        );
    }
}

async fn redis_fixture() -> RedisFixture {
    redis_fixture_at("redis://127.0.0.1:16379").await
}

async fn redis_fixture_at(endpoint: &str) -> RedisFixture {
    // This standalone integration-test process uses the same explicit loopback
    // fixture boundary as the canonical integration Gateway binary.
    edgion_common::test_mode::init_integration_testing_mode(true);
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
            "endpoints": [endpoint], "pool": {"size": 1},
            "auth": {"secretRef": {"name": "redis-test-auth"}},
            "timeout": {"connect": "3s", "command": "2s"}
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

    RedisFixture {
        unique,
        link_key,
        client,
    }
}

/// Uses the authenticated Redis fixture from LinkSys/Redis on port 16379.
/// Run explicitly with `cargo test -p edgion-gateway --test rate_limit_local_overflow_redis -- --ignored`.
#[tokio::test]
#[ignore = "requires the LinkSys/Redis integration fixture on 127.0.0.1:16379"]
async fn real_redis_shares_only_overflow_between_compiled_instances() {
    let fixture = redis_fixture().await;
    let RedisFixture {
        unique,
        link_key,
        client,
    } = &fixture;

    let mut value = overflow_resource(unique, "Deny");
    value["spec"]["requestPlugins"][1]["config"]["redisRef"] = json!(link_key);
    value["spec"]["requestPlugins"][1]["config"]["keyPrefix"] = json!("overflow-test:");
    let first_instance = compile(value.clone());
    let second_instance = compile(value);
    let bucket_key = format!("overflow-test:overflow-tests/{unique}/overflow-budget:q:7:default:tb:0:");
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

fn multi_resource(name: &str, link: &str, failure: &str, selected: Option<&str>) -> Value {
    json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"namespace": "overflow-tests", "name": name},
        "spec": {"requestPlugins": [{"type": "RateLimitRedis", "alias": "budgets", "config": {
            "redisRef": link, "keyPrefix": "multi-test:", "requestTimeout": "1s",
            "onRedisFailure": failure, "showLimitHeaders": true, "responseQuota": selected,
            "quotas": [
                {"name": "early", "key": [], "policies": [{"rate": 100, "interval": "1h"}]},
                {"name": "middle", "key": [], "policies": [{"rate": 100, "interval": "1h"}]},
                {"name": "later", "key": [], "policies": [{"rate": 100, "interval": "1h"}]}
            ]
        }}]}
    })
}

fn quota_key(name: &str, quota: &str) -> String {
    format!(
        "multi-test:overflow-tests/{name}/budgets:q:{}:{quota}:sw:0:",
        quota.len()
    )
}

fn assert_no_quota_headers(wire: &str, queued: &pingora_http::ResponseHeader) {
    assert!(!wire.to_ascii_lowercase().contains("x-ratelimit-"), "{wire}");
    for name in ["x-ratelimit-limit", "x-ratelimit-remaining", "x-ratelimit-reset"] {
        assert!(queued.headers.get(name).is_none(), "partial queued header {name}");
    }
}

/// A real later Lua error must retain the first debit, stop the final quota,
/// and render once according to the entry's failure policy without partial headers.
#[tokio::test]
#[ignore = "requires the LinkSys/Redis integration fixture on 127.0.0.1:16379"]
async fn real_redis_multi_quota_failure_and_skipped_response_selection() {
    let fixture = redis_fixture().await;
    let client = &fixture.client;
    for failure in ["Allow", "Deny"] {
        let name = format!("{}-{failure}", fixture.unique);
        let keys = [
            quota_key(&name, "early"),
            quota_key(&name, "middle"),
            quota_key(&name, "later"),
        ];
        // Corrupt only this test's second bucket: the sliding-window HMGET
        // yields WRONGTYPE, after the first Lua check has already committed.
        client
            .set(&keys[1], "wrong-type", Some(Duration::from_secs(60)))
            .await
            .unwrap();
        let compiled = compile(multi_resource(&name, &fixture.link_key, failure, Some("early")));
        let (status, wire, queued) = request_details(&compiled, false).await;
        assert_eq!(status, (failure == "Deny").then_some(429));
        assert_no_quota_headers(&wire, &queued);
        assert_eq!(wire.matches("HTTP/1.1 ").count(), usize::from(failure == "Deny"));
        assert_eq!(client.hget(&keys[0], "cur").await.unwrap().as_deref(), Some("1"));
        assert_eq!(client.get(&keys[1]).await.unwrap().as_deref(), Some("wrong-type"));
        assert!(!client.exists(&keys[2]).await.unwrap(), "later quota ran after failure");
        client
            .del(&keys.iter().map(String::as_str).collect::<Vec<_>>())
            .await
            .unwrap();
    }

    for selected in [Some("middle"), None] {
        let name = format!("{}-selection-{}", fixture.unique, selected.is_some());
        let mut value = multi_resource(&name, &fixture.link_key, "Deny", selected);
        value["spec"]["requestPlugins"][0]["config"]["quotas"][0]["policies"][0]["rate"] = json!(10);
        value["spec"]["requestPlugins"][0]["config"]["quotas"][1]["key"] =
            json!([{"type": "header", "name": "X-Absent-Quota-Key"}]);
        value["spec"]["requestPlugins"][0]["config"]["quotas"][1]["onKeyMiss"] = json!("Allow");
        let compiled = compile(value);
        let (status, wire, queued) = request_details(&compiled, false).await;
        assert!(status.is_none());
        if selected.is_some() {
            assert_no_quota_headers(&wire, &queued);
        } else {
            assert_eq!(queued.headers.get("x-ratelimit-limit").unwrap(), "10");
            assert_eq!(queued.headers.get("x-ratelimit-remaining").unwrap(), "9");
        }
        let keys = [
            quota_key(&name, "early"),
            quota_key(&name, "middle"),
            quota_key(&name, "later"),
        ];
        for index in [0, 2] {
            assert_eq!(client.hget(&keys[index], "cur").await.unwrap().as_deref(), Some("1"));
        }
        assert!(
            !client.exists(&keys[1]).await.unwrap(),
            "missing selected quota was debited"
        );
        client
            .del(&keys.iter().map(String::as_str).collect::<Vec<_>>())
            .await
            .unwrap();
    }
}

struct DelayedRedisProxy {
    endpoint: String,
    marker: Arc<parking_lot::Mutex<Option<Vec<u8>>>>,
    reached: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for DelayedRedisProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Delay only this connection's command containing the owned middle key.
/// Forward authentication and all earlier replies unchanged; never pause Redis.
async fn delayed_redis_proxy() -> DelayedRedisProxy {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("redis://{}", listener.local_addr().unwrap());
    let marker: Arc<parking_lot::Mutex<Option<Vec<u8>>>> = Arc::default();
    let reached = Arc::new(tokio::sync::Notify::new());
    let watched = marker.clone();
    let reached_copy = reached.clone();
    let task = tokio::spawn(async move {
        let (downstream, _) = listener.accept().await.unwrap();
        let upstream = tokio::net::TcpStream::connect("127.0.0.1:16379").await.unwrap();
        let (mut down_read, mut down_write) = downstream.into_split();
        let (mut up_read, mut up_write) = upstream.into_split();
        let forward = async {
            let mut chunk = [0u8; 8192];
            let mut tail = Vec::new();
            let mut delayed = false;
            loop {
                let count = down_read.read(&mut chunk).await.unwrap();
                if count == 0 {
                    break;
                }
                let needle = watched.lock().clone();
                if let Some(needle) = needle.filter(|_| !delayed) {
                    tail.extend_from_slice(&chunk[..count]);
                    if tail.windows(needle.len()).any(|window| window == needle) {
                        delayed = true;
                        reached_copy.notify_one();
                        tokio::time::sleep(Duration::from_millis(400)).await;
                    }
                    // Preserve a bounded suffix to detect split TCP segments.
                    let keep = needle.len().saturating_sub(1).min(tail.len());
                    tail.drain(..tail.len() - keep);
                }
                up_write.write_all(&chunk[..count]).await.unwrap();
            }
        };
        tokio::select! {
            _ = forward => {},
            _ = tokio::io::copy(&mut up_read, &mut down_write) => {},
        }
    });
    DelayedRedisProxy {
        endpoint,
        marker,
        reached,
        task,
    }
}

/// Exercise the aggregate deadline after one confirmed debit using a dedicated
/// transparent Redis connection; other suites' Redis connections are untouched.
#[tokio::test]
#[ignore = "requires the LinkSys/Redis integration fixture on 127.0.0.1:16379"]
async fn real_redis_multi_quota_aggregate_deadline_retains_early_debit() {
    let observer = redis_fixture().await;
    for failure in ["Allow", "Deny"] {
        let proxy = delayed_redis_proxy().await;
        let fixture = redis_fixture_at(&proxy.endpoint).await;
        let name = format!("{}-deadline-{failure}", fixture.unique);
        let keys = [
            quota_key(&name, "early"),
            quota_key(&name, "middle"),
            quota_key(&name, "later"),
        ];
        *proxy.marker.lock() = Some(keys[1].as_bytes().to_vec());
        let mut value = multi_resource(&name, &fixture.link_key, failure, Some("early"));
        value["spec"]["requestPlugins"][0]["config"]["requestTimeout"] = json!("100ms");
        let compiled = compile(value);
        let start = std::time::Instant::now();
        let (status, wire, queued) = request_details(&compiled, false).await;
        assert!(
            start.elapsed() < Duration::from_millis(350),
            "deadline did not cover the later check"
        );
        tokio::time::timeout(Duration::from_millis(50), proxy.reached.notified())
            .await
            .unwrap();
        assert_eq!(status, (failure == "Deny").then_some(429));
        assert_no_quota_headers(&wire, &queued);
        assert_eq!(wire.matches("HTTP/1.1 ").count(), usize::from(failure == "Deny"));
        assert_eq!(
            observer.client.hget(&keys[0], "cur").await.unwrap().as_deref(),
            Some("1")
        );
        assert!(
            !observer.client.exists(&keys[2]).await.unwrap(),
            "later quota ran after timeout"
        );
        // Let the held command finish to observe the documented uncertain debit.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if observer.client.exists(&keys[1]).await.unwrap() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            observer.client.hget(&keys[1], "cur").await.unwrap().as_deref(),
            Some("1")
        );
        assert!(!observer.client.exists(&keys[2]).await.unwrap());
        observer
            .client
            .del(&keys.iter().map(String::as_str).collect::<Vec<_>>())
            .await
            .unwrap();
    }
}
