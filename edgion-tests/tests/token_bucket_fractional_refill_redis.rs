//! Real-Redis coverage for fractional TokenBucket refill state.
//!
//! The test is ignored during ordinary `cargo test` because it requires an
//! isolated Redis fixture. Run it explicitly with:
//!
//! ```text
//! docker compose -f edgion-tests/integration/conf/Services/redis/docker-compose.yaml up -d
//! cargo test -p edgion-tests --test token_bucket_fractional_refill_redis -- --ignored --nocapture
//! ```
//!
//! `EDGION_TEST_REDIS_ENDPOINT` and `EDGION_TEST_REDIS_PASSWORD` may override
//! the fixture defaults. Every scenario uses a unique hash-tagged key and
//! removes only that key during cleanup.

use std::{collections::HashMap, panic::AssertUnwindSafe};

use anyhow::{Context, Result};
use edgion_gateway::link_sys::credential::CredentialState;
use edgion_gateway::link_sys::providers::redis::RedisLinkClient;
use edgion_gateway::plugins::common::redis_rate_limiter::{build_redis_key, RateLimitResult, RedisRateLimiter};
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
use edgion_resources::resources::edgion_plugins::RateLimitAlgorithm;
use edgion_resources::resources::link_sys::redis::{RedisAuth, RedisClientConfig, RedisPool, RedisTimeout};
use futures::FutureExt;

const KEY_PREFIX: &str = "edgion:test:rl:fractional:";
const POLICY_IDX: usize = 0;
const RATE: u64 = 3;
const WINDOW_MS: i64 = 1_000;
const BASE_MS: i64 = 1_700_000_000_000;

fn redis_config() -> (RedisClientConfig, CredentialState<BasicAuthMaterial>) {
    let endpoint =
        std::env::var("EDGION_TEST_REDIS_ENDPOINT").unwrap_or_else(|_| "redis://127.0.0.1:16379".to_string());
    let password = std::env::var("EDGION_TEST_REDIS_PASSWORD").unwrap_or_else(|_| "edgion-test-pwd".to_string());
    let config = RedisClientConfig {
        endpoints: vec![endpoint],
        auth: Some(RedisAuth {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "integration-redis-credentials".to_string(),
                    namespace: None,
                },
                secret: None,
            },
        }),
        db: Some(0),
        timeout: Some(RedisTimeout {
            connect: Some("3s".to_string()),
            command: Some("2s".to_string()),
        }),
        pool: Some(RedisPool { size: Some(1) }),
        topology: None,
        tls: None,
    };
    let auth = CredentialState::Resolved(BasicAuthMaterial {
        username: None,
        password,
    });
    (config, auth)
}

fn redis_key(limit_key: &str) -> String {
    build_redis_key(KEY_PREFIX, "tb", POLICY_IDX, limit_key, "")
}

async fn attempt(
    core: &RedisRateLimiter,
    client: &RedisLinkClient,
    limit_key: &str,
    now_ms: i64,
    cost: u32,
) -> Result<RateLimitResult> {
    core.eval_policy(
        client,
        &RateLimitAlgorithm::TokenBucket,
        KEY_PREFIX,
        POLICY_IDX,
        limit_key,
        RATE,
        WINDOW_MS,
        now_ms,
        cost,
    )
    .await
}

async fn seed_state(
    client: &RedisLinkClient,
    key: &str,
    tokens: u64,
    timestamp: i64,
    remainder: Option<u64>,
) -> Result<()> {
    client.hset(key, "tokens", &tokens.to_string()).await?;
    client.hset(key, "ts", &timestamp.to_string()).await?;
    if let Some(remainder) = remainder {
        client.hset(key, "refill_remainder", &remainder.to_string()).await?;
    }
    Ok(())
}

async fn state(client: &RedisLinkClient, key: &str) -> Result<HashMap<String, String>> {
    client.hgetall(key).await.context("read persisted token-bucket state")
}

fn assert_state(state: &HashMap<String, String>, tokens: u64, timestamp: i64, remainder: u64) {
    assert_eq!(state.get("tokens"), Some(&tokens.to_string()));
    assert_eq!(state.get("ts"), Some(&timestamp.to_string()));
    assert_eq!(state.get("refill_remainder"), Some(&remainder.to_string()));
}

async fn fractional_cadence(core: &RedisRateLimiter, client: &RedisLinkClient, limit_key: &str) -> Result<()> {
    let drained = attempt(core, client, limit_key, BASE_MS, 3).await?;
    assert_eq!(
        (drained.allowed, drained.current, drained.remaining, drained.retry_ms),
        (true, 0, 0, 0)
    );

    let denied = attempt(core, client, limit_key, BASE_MS + 500, 2).await?;
    assert_eq!(
        (denied.allowed, denied.current, denied.remaining, denied.retry_ms),
        (false, 1, 0, 167)
    );
    assert_state(&state(client, &redis_key(limit_key)).await?, 1, BASE_MS + 500, 500);

    for (offset, expected_balance) in [(1_000, 1), (1_500, 0), (2_000, 0)] {
        let allowed = attempt(core, client, limit_key, BASE_MS + offset, 2).await?;
        assert_eq!(
            (allowed.allowed, allowed.current, allowed.remaining, allowed.retry_ms),
            (true, expected_balance, expected_balance, 0)
        );
    }

    assert_state(&state(client, &redis_key(limit_key)).await?, 0, BASE_MS + 2_000, 0);
    let ttl = client.ttl(&redis_key(limit_key)).await?;
    assert!(
        (1..=2).contains(&ttl),
        "the script must refresh its two-second expiry, got {ttl}"
    );
    Ok(())
}

async fn legacy_state_migration(core: &RedisRateLimiter, client: &RedisLinkClient, limit_key: &str) -> Result<()> {
    let key = redis_key(limit_key);
    seed_state(client, &key, 1, BASE_MS, None).await?;
    let allowed = attempt(core, client, limit_key, BASE_MS + 500, 2).await?;
    assert_eq!(
        (allowed.allowed, allowed.current, allowed.remaining, allowed.retry_ms),
        (true, 0, 0, 0)
    );
    assert_state(&state(client, &key).await?, 0, BASE_MS + 500, 500);
    Ok(())
}

async fn saturation_clears_credit(core: &RedisRateLimiter, client: &RedisLinkClient, limit_key: &str) -> Result<()> {
    let key = redis_key(limit_key);
    seed_state(client, &key, 2, BASE_MS, Some(500)).await?;
    let allowed = attempt(core, client, limit_key, BASE_MS + 500, 1).await?;
    assert_eq!(
        (allowed.allowed, allowed.current, allowed.remaining, allowed.retry_ms),
        (true, 2, 2, 0)
    );
    assert_state(&state(client, &key).await?, 2, BASE_MS + 500, 0);
    Ok(())
}

async fn clock_rollback_preserves_refill_state(
    core: &RedisRateLimiter,
    client: &RedisLinkClient,
    limit_key: &str,
) -> Result<()> {
    let key = redis_key(limit_key);
    seed_state(client, &key, 2, BASE_MS + 1_000, Some(500)).await?;
    let allowed = attempt(core, client, limit_key, BASE_MS + 500, 1).await?;
    assert_eq!(
        (allowed.allowed, allowed.current, allowed.remaining, allowed.retry_ms),
        (true, 1, 1, 0)
    );
    assert_state(&state(client, &key).await?, 1, BASE_MS + 1_000, 500);
    Ok(())
}

async fn retry_uses_fractional_credit(
    core: &RedisRateLimiter,
    client: &RedisLinkClient,
    limit_key: &str,
) -> Result<()> {
    let key = redis_key(limit_key);
    seed_state(client, &key, 0, BASE_MS, Some(500)).await?;
    let denied = attempt(core, client, limit_key, BASE_MS, 1).await?;
    assert_eq!(
        (denied.allowed, denied.current, denied.remaining, denied.retry_ms),
        (false, 0, 0, 167)
    );
    assert_state(&state(client, &key).await?, 0, BASE_MS, 500);
    Ok(())
}

async fn run_scenarios(core: &RedisRateLimiter, client: &RedisLinkClient, limit_keys: &[String]) -> Result<()> {
    fractional_cadence(core, client, &limit_keys[0]).await?;
    legacy_state_migration(core, client, &limit_keys[1]).await?;
    saturation_clears_credit(core, client, &limit_keys[2]).await?;
    clock_rollback_preserves_refill_state(core, client, &limit_keys[3]).await?;
    retry_uses_fractional_credit(core, client, &limit_keys[4]).await
}

#[tokio::test]
#[ignore = "requires the integration Redis fixture"]
async fn token_bucket_preserves_fractional_refill_state() -> Result<()> {
    let (config, auth) = redis_config();
    let client = RedisLinkClient::from_test_config("integration/token-bucket-fractional-refill", &config, &auth)?;
    client
        .init()
        .await
        .context("connect to the integration Redis fixture")?;
    let core = RedisRateLimiter::new();
    let limit_keys = (0..5)
        .map(|_| format!("{{{}}}", uuid::Uuid::new_v4().simple()))
        .collect::<Vec<_>>();

    let outcome = AssertUnwindSafe(run_scenarios(&core, &client, &limit_keys))
        .catch_unwind()
        .await;
    let redis_keys = limit_keys.iter().map(|key| redis_key(key)).collect::<Vec<_>>();
    let key_refs = redis_keys.iter().map(String::as_str).collect::<Vec<_>>();
    let cleanup = client.del(&key_refs).await;
    let shutdown = client.shutdown().await;
    match outcome {
        Ok(result) => result?,
        Err(panic) => std::panic::resume_unwind(panic),
    }
    cleanup?;
    shutdown?;
    Ok(())
}
