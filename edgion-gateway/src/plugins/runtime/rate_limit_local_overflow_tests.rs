//! Compiled local-admission/Redis-overflow tests using the real session adapter.

use std::sync::Arc;
use std::time::Duration;

use pingora_proxy::Session;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::ctx::EdgionHttpContext;
use crate::plugins::http::{compile_plugins, CompiledPlugins};
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

async fn request(compiled: &CompiledPlugins, grpc: bool) -> (EdgionHttpContext, String) {
    let (mut session, mut client) = session("").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = grpc;
    compiled
        .plugin_runtime
        .run_request_plugins(&mut session, &mut ctx)
        .await;
    let wire = if ctx.plugin_terminated() {
        let mut bytes = vec![0; 8192];
        let count = tokio::time::timeout(Duration::from_secs(1), client.read(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        String::from_utf8(bytes[..count].to_vec()).unwrap()
    } else {
        String::new()
    };
    (ctx, wire)
}

#[tokio::test]
async fn local_allowance_skips_unavailable_redis_then_overflow_denies() {
    let compiled = compile(overflow_resource("deny", "Deny"));
    let (first, _) = request(&compiled, false).await;
    assert!(
        !first.plugin_terminated(),
        "a local pass must not access unavailable Redis"
    );
    assert_eq!(first.final_downstream_status(), None);
    let (second, wire) = request(&compiled, false).await;
    assert!(second.plugin_terminated());
    assert_eq!(second.final_downstream_status(), Some(429));
    assert!(
        !wire.to_ascii_lowercase().contains("x-ratelimit-limit: 1"),
        "local overflow must not emit local quota headers"
    );
}

#[tokio::test]
async fn explicit_redis_fail_open_preserves_overflow_admission() {
    let compiled = compile(overflow_resource("allow", "Allow"));
    for _ in 0..3 {
        let (ctx, _) = request(&compiled, false).await;
        assert!(!ctx.plugin_terminated());
        assert_eq!(ctx.final_downstream_status(), None);
    }
}

#[tokio::test]
async fn default_local_reject_remains_terminal() {
    let mut value = overflow_resource("default-reject", "Allow");
    value["spec"]["requestPlugins"].as_array_mut().unwrap().truncate(1);
    value["spec"]["requestPlugins"][0]["config"]
        .as_object_mut()
        .unwrap()
        .remove("onLimitExceeded");
    let compiled = compile(value);
    assert!(!request(&compiled, false).await.0.plugin_terminated());
    assert_eq!(request(&compiled, false).await.0.final_downstream_status(), Some(429));
}

#[tokio::test]
async fn missing_producer_outcome_fails_closed_before_boolean_short_circuit() {
    for missing_key in [false, true] {
        for combinator in ["allOf", "anyOf"] {
            let mut value = overflow_resource("missing-outcome", "Allow");
            if missing_key {
                value["spec"]["requestPlugins"][0]["config"]["key"] = json!([{"type": "header", "name": "x-absent"}]);
                value["spec"]["requestPlugins"][0]["config"]["onKeyMiss"] = json!("Allow");
            } else {
                value["spec"]["requestPlugins"][0]["conditions"] =
                    json!({"run": {"allOf": [{"type": "keyExist", "key": {"type": "header", "name": "x-absent"}}]}});
            }
            let predicate = if combinator == "allOf" {
                json!({"type": "keyExist", "key": {"type": "header", "name": "x-absent"}})
            } else {
                json!({"type": "keyExist", "key": {"type": "header", "name": "host"}})
            };
            value["spec"]["requestPlugins"][1]["conditions"] =
                json!({"run": {combinator: [predicate, {"type": "rateLimitExceeded", "pluginAlias": "local-budget"}]}});
            let compiled = compile(value);
            let (ctx, wire) = request(&compiled, false).await;
            assert_eq!(
                ctx.final_downstream_status(),
                Some(500),
                "missing_key={missing_key}, combinator={combinator}"
            );
            assert!(!wire.contains("local-budget"));
            assert!(!wire.contains("intentionally-absent"));
        }
    }
}

#[tokio::test]
async fn malformed_dependency_seals_resource_even_when_consumer_would_skip() {
    let mut value = overflow_resource("invalid-reference", "Allow");
    value["spec"]["requestPlugins"][1]["conditions"] = json!({
        "skip": {"allOf": [{"type": "keyExist", "key": {"type": "header", "name": "host"}}]},
        "run": {"allOf": [{"type": "rateLimitExceeded", "pluginAlias": "missing"}]}
    });
    let resource: EdgionPlugins = serde_json::from_value(value).unwrap();
    let compiled = compile_plugins(&resource);
    assert!(!compiled.preparse_errors.is_empty());
    assert!(!compiled.is_activatable);
    assert_eq!(request(&compiled, false).await.0.final_downstream_status(), Some(500));
}

#[tokio::test]
async fn reused_context_does_not_reuse_another_resources_local_outcome() {
    let first = compile(overflow_resource("first-resource", "Allow"));
    let mut next_value = overflow_resource("second-resource", "Allow");
    next_value["spec"]["requestPlugins"][0]["conditions"] =
        json!({"run": {"allOf": [{"type": "keyExist", "key": {"type": "header", "name": "x-absent"}}]}});
    let second = compile(next_value);
    let (mut session, _client) = session("").await;
    let mut ctx = EdgionHttpContext::default();
    first.plugin_runtime.run_request_plugins(&mut session, &mut ctx).await;
    assert!(!ctx.plugin_terminated());
    second.plugin_runtime.run_request_plugins(&mut session, &mut ctx).await;
    assert_eq!(ctx.final_downstream_status(), Some(500));
}

#[tokio::test]
async fn repeated_invocation_clears_previous_outcome_when_producer_is_skipped() {
    let mut value = overflow_resource("repeat", "Allow");
    value["spec"]["requestPlugins"][0]["conditions"] =
        json!({"run": {"allOf": [{"type": "keyExist", "key": {"type": "header", "name": "x-run-local"}}]}});
    let compiled = compile(value);
    let (mut first_session, _first_client) = session("X-Run-Local: yes\r\n").await;
    let mut ctx = EdgionHttpContext::default();
    compiled
        .plugin_runtime
        .run_request_plugins(&mut first_session, &mut ctx)
        .await;
    assert!(!ctx.plugin_terminated());
    let (mut second_session, _second_client) = session("").await;
    compiled
        .plugin_runtime
        .run_request_plugins(&mut second_session, &mut ctx)
        .await;
    assert_eq!(ctx.final_downstream_status(), Some(500));
}

#[tokio::test]
async fn overflow_denial_uses_native_grpc_status() {
    let compiled = compile(overflow_resource("grpc", "Deny"));
    assert!(!request(&compiled, true).await.0.plugin_terminated());
    let (ctx, wire) = request(&compiled, true).await;
    assert!(ctx.plugin_terminated());
    assert_eq!(ctx.final_downstream_status(), Some(200));
    assert_eq!(ctx.request_info.grpc_status(), Some(8));
    assert!(wire.contains("grpc-status: 8"), "{wire}");
}
