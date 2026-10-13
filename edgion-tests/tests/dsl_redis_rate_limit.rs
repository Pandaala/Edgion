//! End-to-end DSL builtin coverage on an exclusively owned standalone Redis.
//!
//! This ignored test uses SCRIPT FLUSH and CLIENT PAUSE. Run only with explicit
//! EDGION_TEST_REDIS_ISOLATED=1 and EDGION_TEST_REDIS_ENDPOINT. REDIS_CLI may
//! select a docker redis-cli wrapper. Data keys have bounded expiry.

use anyhow::{ensure, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use bytes::Bytes;
use edgion_gateway::link_sys::{get_global_link_sys_store, get_redis_client, providers::redis::RedisLinkClient};
use edgion_gateway::plugins::http::dsl::{
    lang::{
        bytecode::CompiledScript,
        validator::{compile_dsl_source, ValidationLimits},
        vm::{Vm, VmLimits},
    },
    runtime::{DslRateLimits, DslRuntime},
};
use edgion_gateway::plugins::runtime::log::EdgionPluginsLogToken;
use edgion_gateway::plugins::runtime::traits::session::RouteProto;
use edgion_gateway::plugins::{PluginLog, PluginSession, PluginSessionResult};
use edgion_gateway::{EdgionHttpContext, MirrorState, PendingHeaderOp, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::resources::{edgion_plugins::plugin_configs::dsl::DslConfig, link_sys::LinkSys};
use edgion_resources::{
    common::{KeyGet, KeySet},
    EdgionErrorCode,
};
use futures::{future::try_join_all, FutureExt};
use pingora_http::ResponseHeader;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    panic::AssertUnwindSafe,
    process::Command,
    sync::Arc,
    time::Duration,
};

// The scripts only write result headers. Any unexpected adapter interaction
// fails the test instead of silently supplying a permissive mock default.
#[derive(Default)]
struct ResultSession {
    headers: HashMap<String, String>,
}
#[async_trait]
impl PluginSession for ResultSession {
    fn ip_acl_decision(
        &self,
        _acl: &edgion_gateway::plugins::common::ip_acl::CompiledIpAcl,
        _ip: Option<std::net::IpAddr>,
    ) -> edgion_resources::resources::edgion_plugins::AccessDecision {
        panic!("unexpected session operation")
    }
    fn mark_ip_body_denied(&mut self) {
        panic!("unexpected session operation")
    }

    fn waf(&mut self) -> &mut dyn edgion_gateway::plugins::runtime::traits::WafSessionCapability {
        panic!("unexpected session operation")
    }

    fn ai(&mut self) -> &mut dyn edgion_gateway::plugins::runtime::traits::AiSessionCapability {
        panic!("unexpected AI access in Redis ResultSession")
    }

    fn request_uri(&self) -> &http::Uri {
        panic!("unexpected session operation")
    }
    fn request_header_map(&self) -> &http::HeaderMap {
        panic!("unexpected session operation")
    }
    fn header_value(&self, _name: &str) -> Option<String> {
        panic!("unexpected session operation")
    }
    fn request_headers(&self) -> Vec<(String, String)> {
        panic!("unexpected session operation")
    }
    fn get_query_param(&self, _name: &str) -> Option<String> {
        panic!("unexpected session operation")
    }
    fn get_cookie(&self, _name: &str) -> Option<String> {
        panic!("unexpected session operation")
    }
    fn get_path(&self) -> &str {
        panic!("unexpected session operation")
    }
    fn get_request_path(&self) -> &str {
        panic!("unexpected session operation")
    }
    fn get_uri(&self) -> String {
        panic!("unexpected session operation")
    }
    fn get_request_uri(&self) -> String {
        panic!("unexpected session operation")
    }
    fn get_query(&self) -> Option<String> {
        panic!("unexpected session operation")
    }
    fn request_query(&self) -> Option<&str> {
        panic!("unexpected session operation")
    }
    fn get_request_method(&self) -> Option<&str> {
        panic!("unexpected session operation")
    }
    fn get_method(&self) -> &str {
        panic!("unexpected session operation")
    }
    fn get_ctx_var(&self, _key: &str) -> Option<String> {
        panic!("unexpected session operation")
    }
    fn set_ctx_var(&mut self, _key: &str, _value: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn remove_ctx_var(&mut self, _key: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn get_path_param(&mut self, _name: &str) -> Option<String> {
        panic!("unexpected session operation")
    }
    fn matched_route_meta(&self, _proto: RouteProto, _selector: &str) -> Option<String> {
        panic!("unexpected session operation")
    }
    async fn write_response_header(
        &mut self,
        _resp: Box<ResponseHeader>,
        _end_of_stream: bool,
    ) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn set_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        self.headers.insert(name.into(), value.into());
        Ok(())
    }
    fn append_response_header(&mut self, _name: &str, _value: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn set_response_header_value(&mut self, _name: &str, _value: http::HeaderValue) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn append_response_header_value(&mut self, _name: &str, _value: http::HeaderValue) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn has_response_header(&self, _name: &str) -> bool {
        panic!("unexpected session operation")
    }
    fn remove_response_header(&mut self, _name: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn get_response_header(&self, _name: &str) -> Option<String> {
        panic!("unexpected session operation")
    }
    fn get_response_status(&self) -> Option<u16> {
        panic!("unexpected session operation")
    }
    fn set_response_status(&mut self, _status: u16) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn set_request_header(&mut self, _name: &str, _value: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn set_request_header_value(&mut self, _name: &str, _value: http::HeaderValue) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn append_request_header(&mut self, _name: &str, _value: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn append_request_header_value(&mut self, _name: &str, _value: http::HeaderValue) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn remove_request_header(&mut self, _name: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn set_request_query(&mut self, _query: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn set_upstream_uri(&mut self, _uri: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn set_upstream_host(&mut self, _host: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn set_upstream_method(&mut self, _method: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    async fn write_response_body(&mut self, _body: Option<Bytes>, _end_of_stream: bool) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    async fn shutdown(&mut self) {
        panic!("unexpected session operation")
    }
    fn direct_peer_ip(&self) -> &str {
        panic!("unexpected session operation")
    }
    fn downstream_peer_ip(&self) -> &str {
        panic!("unexpected session operation")
    }
    fn remote_ip(&self) -> &str {
        panic!("unexpected session operation")
    }
    fn set_remote_ip(&mut self, _addr: &str) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn apply_request_ids(
        &mut self,
        _default: Option<&edgion_resources::DefaultIdConfig>,
        _rules: &[edgion_gateway::request_trace_id::CompiledRequestIdRule],
    ) -> bool {
        panic!("unexpected session operation")
    }
    fn ctx(&self) -> &EdgionHttpContext {
        panic!("unexpected session operation")
    }
    fn record_error(&mut self, _status: EdgionErrorCode) {
        panic!("unexpected session operation")
    }
    fn start_edgion_plugins_log(&mut self, _name: String) -> EdgionPluginsLogToken {
        panic!("unexpected session operation")
    }
    fn push_to_edgion_plugins_log(&mut self, _token: &EdgionPluginsLogToken, _log: PluginLog) {
        panic!("unexpected session operation")
    }
    fn key_get(&self, _key: &KeyGet) -> Option<String> {
        panic!("unexpected session operation")
    }
    fn key_set(&mut self, _key: &KeySet, _value: Option<String>) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
    fn set_upstream_jump(&mut self, _target: UpstreamJumpTarget) -> SetJumpOutcome {
        panic!("unexpected session operation")
    }
    fn set_mirror_state(&mut self, _state: MirrorState) -> bool {
        panic!("unexpected session operation")
    }
    fn queue_local_reply_header_op(&mut self, _op: PendingHeaderOp) -> PluginSessionResult<()> {
        panic!("unexpected session operation")
    }
}

struct Fixture {
    endpoint: String,
    password: Option<String>,
    namespace: String,
}

impl Fixture {
    fn from_env() -> Result<Self> {
        ensure!(
            std::env::var("EDGION_TEST_REDIS_ISOLATED").as_deref() == Ok("1"),
            "SCRIPT FLUSH and CLIENT PAUSE require exclusive Redis ownership"
        );
        let endpoint =
            std::env::var("EDGION_TEST_REDIS_ENDPOINT").context("an explicit isolated Redis endpoint is required")?;
        ensure!(
            !endpoint.contains('@'),
            "use EDGION_TEST_REDIS_PASSWORD for authentication"
        );
        Ok(Self {
            endpoint,
            password: std::env::var("EDGION_TEST_REDIS_PASSWORD").ok(),
            namespace: format!("dsl-test-{}", uuid::Uuid::new_v4().simple()),
        })
    }

    fn admin(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new(std::env::var_os("REDIS_CLI").unwrap_or_else(|| "redis-cli".into()));
        cmd.args(["--raw", "-u", &self.endpoint])
            .args(args)
            .env_remove("REDISCLI_AUTH");
        if let Some(password) = &self.password {
            cmd.env("REDISCLI_AUTH", password);
        }
        let output = cmd.output().context("execute isolated Redis administration")?;
        ensure!(output.status.success(), "redis-cli failed");
        Ok(String::from_utf8(output.stdout)?.trim().into())
    }

    fn calls(&self, command: &str) -> Result<u64> {
        let prefix = format!("cmdstat_{command}:calls=");
        for line in self.admin(&["INFO", "commandstats"])?.lines() {
            if let Some(value) = line.strip_prefix(&prefix) {
                return Ok(value.split(',').next().context("command count missing")?.parse()?);
            }
        }
        Ok(0)
    }

    fn key(&self) -> String {
        format!("{}/cache", self.namespace)
    }

    async fn publish(&self, previous: Option<&Arc<RedisLinkClient>>, pool_size: usize) -> Result<Arc<RedisLinkClient>> {
        let mut config = json!({"endpoints": [self.endpoint], "db": 0,
            "pool": {"size": pool_size},
            "timeout": {"connect": 3000, "read": 2000, "write": 2000}});
        if let Some(password) = &self.password {
            config["auth"] = json!({"secretRef": {"name": "isolated-redis"},
                "secret": {"data": {"password": base64::engine::general_purpose::STANDARD.encode(password)}}});
        }
        let link: LinkSys = serde_json::from_value(json!({"apiVersion": "edgion.io/v1", "kind": "LinkSys",
            "metadata": {"namespace": self.namespace, "name": "cache"},
            "spec": {"type": "redis", "config": config}}))?;
        get_global_link_sys_store().update(HashMap::from([(self.key(), link)]), &HashSet::new());
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(client) = get_redis_client(&self.key()) {
                    if previous.is_none_or(|old| !Arc::ptr_eq(old, &client)) && client.ping().await.is_ok() {
                        return client;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("LinkSys did not publish a ready replacement client")
    }

    async fn cleanup(&self) -> Result<()> {
        get_global_link_sys_store().update(HashMap::new(), &HashSet::from([self.key()]));
        tokio::time::timeout(Duration::from_secs(5), async {
            while get_redis_client(&self.key()).is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("LinkSys removal was not observed")
    }

    fn runtime(&self, algorithm: &str, rate: u64, namespace: &str, timeout_ms: u64) -> Result<DslRuntime> {
        let config: DslConfig = serde_json::from_value(json!({"name": "redis-test",
            "linkSysRefs": [{"name": "cache", "namespace": self.namespace}],
            "rateLimitPolicies": [{"name": "requests", "link": "cache", "namespace": namespace,
                "algorithm": algorithm, "rate": rate, "interval": "10m", "timeoutMs": timeout_ms}]}))?;
        Ok(DslRuntime {
            redis_links: HashMap::from([("cache".into(), self.key())]),
            rate_limits: DslRateLimits::from_config(&config).map_err(anyhow::Error::msg)?,
            ..Default::default()
        })
    }
}

async fn invoke(runtime: &DslRuntime, arguments: &str) -> Result<HashMap<String, String>> {
    let source = format!(
        r#"
        let r = redis.rate_limit({arguments})
        resp.set_header("ok", to_str(r["ok"]))
        if r["ok"] {{
            let v = r["value"]
            resp.set_header("allowed", to_str(v["allowed"]))
            resp.set_header("current", to_str(v["current"]))
            resp.set_header("remaining", to_str(v["remaining"]))
            resp.set_header("limit", to_str(v["limit"]))
            resp.set_header("retry_ms", to_str(v["retry_ms"]))
        }} else {{
            if r["value"] != nil {{ return deny(500, "invalid error envelope") }}
            resp.set_header("error", r["error"])
        }}
    "#
    );
    let encoded = compile_dsl_source(&source, &ValidationLimits::default())
        .map_err(|errors| anyhow::anyhow!(errors.join("; ")))?;
    let script = CompiledScript::deserialize_base64(&encoded).map_err(anyhow::Error::msg)?;
    let vm = Vm::new(script, VmLimits::default())?;
    let mut session = ResultSession::default();
    let outcome = vm
        .execute(&mut session, &mut PluginLog::new("redis-test"), runtime)
        .await?;
    ensure!(
        matches!(outcome, edgion_resources::filters::PluginRunningResult::GoodNext),
        "script failed: {outcome:?}"
    );
    Ok(session.headers)
}

fn args(key: &str, cost: u64) -> String {
    format!(
        "\"cache\", \"requests\", {}, {cost}",
        serde_json::to_string(key).unwrap()
    )
}

fn decision(result: &HashMap<String, String>, allowed: bool, limit: u64) {
    assert_eq!(result["ok"], "true", "{result:?}");
    assert_eq!(result["allowed"], allowed.to_string());
    assert_eq!(result["limit"], limit.to_string());
    for field in ["current", "remaining", "retry_ms"] {
        assert!(result[field].parse::<i64>().unwrap() >= 0);
    }
}

async fn scenarios(fixture: &Fixture) -> Result<()> {
    let namespace = &fixture.namespace;
    let runtime = fixture.runtime("FixedWindow", 30, namespace, 2000)?;
    assert_eq!(
        invoke(&runtime, &args("not-ready", 1)).await?["error"],
        "provider_not_ready"
    );
    let mut client = fixture.publish(None, 4).await?;
    let loads = fixture.calls("script|load")?;

    for (generation, algorithm) in ["FixedWindow", "SlidingWindow", "TokenBucket"].into_iter().enumerate() {
        // Real wall time belongs to the builtin. Keep these balance assertions
        // away from a window rollover rather than assuming a frozen test clock.
        let remaining_ms = 600_000 - chrono::Utc::now().timestamp_millis().rem_euclid(600_000);
        if remaining_ms < 30_000 {
            tokio::time::sleep(Duration::from_millis(remaining_ms as u64 + 1)).await;
        }
        let runtime = fixture.runtime(algorithm, 30, namespace, 2000)?;
        assert_eq!(fixture.admin(&["SCRIPT", "FLUSH", "SYNC"])?, "OK");
        let eval = fixture.calls("eval")?;
        let sha = fixture.calls("evalsha")?;
        decision(&invoke(&runtime, &args("lifecycle", 3)).await?, true, 30);
        assert_eq!(fixture.calls("eval")?, eval + 1);
        assert_eq!(fixture.calls("evalsha")?, sha + 1);
        let reload = fixture.runtime(algorithm, 30, namespace, 2000)?;
        let warm = invoke(&reload, &args("lifecycle", 3)).await?;
        decision(&warm, true, 30);
        assert_eq!(warm["remaining"], "24");
        assert_eq!(fixture.calls("eval")?, eval + 1);
        assert_eq!(fixture.calls("evalsha")?, sha + 2);
        assert_eq!(fixture.admin(&["SCRIPT", "FLUSH", "SYNC"])?, "OK");
        decision(&invoke(&runtime, &args("lifecycle", 3)).await?, true, 30);
        assert_eq!(fixture.calls("eval")?, eval + 2);
        assert_eq!(fixture.calls("evalsha")?, sha + 3);
        client = fixture.publish(Some(&client), 5 + generation).await?;
        assert_eq!(fixture.admin(&["SCRIPT", "FLUSH", "SYNC"])?, "OK");
        let replaced = invoke(&runtime, &args("lifecycle", 3)).await?;
        decision(&replaced, true, 30);
        assert_eq!(replaced["remaining"], "18");
        assert_eq!(fixture.calls("eval")?, eval + 3);
        assert_eq!(fixture.calls("evalsha")?, sha + 4);

        let call = args("concurrent", 3);
        let outcomes = try_join_all((0..32).map(|_| invoke(&runtime, &call))).await?;
        for result in &outcomes {
            assert_eq!(result["ok"], "true", "{result:?}");
        }
        assert_eq!(outcomes.iter().filter(|r| r["allowed"] == "true").count(), 10);
        let denied = invoke(&runtime, &call).await?;
        decision(&denied, false, 30);
        assert_eq!(denied["remaining"], "0");
        assert!(denied["retry_ms"].parse::<i64>()? > 0);

        let isolated = fixture.runtime(algorithm, 30, &format!("{namespace}-separate"), 2000)?;
        decision(&invoke(&isolated, &call).await?, true, 30);
        let changed = fixture.runtime(algorithm, 33, namespace, 2000)?;
        let changed_result = invoke(&changed, &call).await?;
        decision(&changed_result, true, 33);
        assert_eq!(changed_result["remaining"], "30");
    }
    assert_eq!(fixture.calls("script|load")?, loads);

    let eval = fixture.calls("eval")?;
    let sha = fixture.calls("evalsha")?;
    for (arguments, error) in [
        (args("invalid", 0), "invalid_argument"),
        (args("invalid", 1_000_001), "invalid_argument"),
        (
            format!("\"cache\", \"requests\", \"{}\" + \"x\", 1", "x".repeat(4096)),
            "invalid_argument",
        ),
        ("\"cache\", \"requests\", \"invalid\", \"3\"".into(), "invalid_argument"),
        ("\"cache\", \"absent\", \"invalid\", 1".into(), "unknown_policy"),
    ] {
        assert_eq!(invoke(&runtime, &arguments).await?["error"], error);
    }
    assert_eq!(fixture.calls("eval")?, eval);
    assert_eq!(fixture.calls("evalsha")?, sha);

    // Discover the concrete key by its unique business key, then corrupt its
    // Redis type. Assertions drive the DSL adapter, not the shared limiter API.
    let runtime = fixture.runtime("TokenBucket", 30, namespace, 2000)?;
    let key = uuid::Uuid::new_v4().simple().to_string();
    decision(&invoke(&runtime, &args(&key, 3)).await?, true, 30);
    let keys = fixture.admin(&["KEYS", &format!("edgion:dsl:rl:*{key}*")])?;
    ensure!(keys.lines().count() == 1, "expected one unique token bucket key");
    assert_eq!(
        fixture.admin(&["SET", &keys, "NOSCRIPT is only data", "PX", "600000"])?,
        "OK"
    );
    let eval = fixture.calls("eval")?;
    let sha = fixture.calls("evalsha")?;
    assert_eq!(invoke(&runtime, &args(&key, 3)).await?["error"], "dependency_error");
    assert_eq!(fixture.calls("eval")?, eval);
    assert_eq!(fixture.calls("evalsha")?, sha + 1);
    assert_eq!(fixture.admin(&["SCRIPT", "FLUSH", "SYNC"])?, "OK");
    assert_eq!(invoke(&runtime, &args(&key, 3)).await?["error"], "dependency_error");
    assert_eq!(fixture.calls("eval")?, eval + 1);
    assert_eq!(fixture.calls("evalsha")?, sha + 2);
    assert_eq!(fixture.admin(&["GET", &keys])?, "NOSCRIPT is only data");

    let timeout_runtime = fixture.runtime("TokenBucket", 30, namespace, 20)?;
    let eval = fixture.calls("eval")?;
    let sha = fixture.calls("evalsha")?;
    assert_eq!(fixture.admin(&["CLIENT", "PAUSE", "200", "ALL"])?, "OK");
    let timed_out = invoke(&timeout_runtime, &args("timeout", 3)).await?;
    assert_eq!(timed_out["error"], "timeout");
    // Wait for an observable round trip to settle the bounded pause; timeout
    // does not prove that the debit never reached Redis.
    tokio::time::timeout(Duration::from_secs(3), client.ping()).await??;
    assert_eq!(fixture.calls("eval")?, eval);
    assert!(
        fixture.calls("evalsha")? <= sha + 1,
        "an uncertain timed-out debit must not be replayed"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusive Redis, redis-cli and EDGION_TEST_REDIS_ISOLATED=1"]
async fn dsl_rate_limit_preserves_atomicity_errors_and_reload_contracts() -> Result<()> {
    let fixture = Fixture::from_env()?;
    let outcome = AssertUnwindSafe(scenarios(&fixture)).catch_unwind().await;
    let cleanup = fixture.cleanup().await;
    match outcome {
        Ok(result) => result?,
        Err(panic) => std::panic::resume_unwind(panic),
    }
    cleanup
}
