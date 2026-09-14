//! Real-Redis coverage for weighted TokenBucket partial-refill accumulation.
//!
//! The token-bucket Lua persists the recomputed balance on the deny branch as
//! well as the allow branch. Without that, a sub-cost refill is recomputed and
//! then discarded on every denied attempt, so a caller using a weighted cost
//! stays denied forever no matter how much refill time elapses. A source-level
//! contract test can only prove the persistence call is written; this test
//! proves the resulting balance actually accumulates in Redis.
//!
//! The test is ignored during ordinary `cargo test` because it requires the
//! Redis fixture used by the integration runner. Run it explicitly with:
//!
//! ```text
//! docker compose -f edgion-tests/integration/conf/Services/redis/docker-compose.yaml up -d
//! cargo test -p edgion-tests --test token_bucket_partial_refill_redis -- --ignored --nocapture
//! ```
//!
//! `EDGION_TEST_REDIS_ENDPOINT` and `EDGION_TEST_REDIS_PASSWORD` may override
//! the fixture defaults.

use std::panic::AssertUnwindSafe;

use anyhow::{Context, Result};
use edgion_gateway::link_sys::credential::CredentialState;
use edgion_gateway::link_sys::providers::redis::RedisLinkClient;
use edgion_gateway::plugins::common::redis_rate_limiter::{build_redis_key, RateLimitResult, RedisRateLimiter};
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
use edgion_resources::resources::edgion_plugins::RateLimitAlgorithm;
use edgion_resources::resources::link_sys::redis::{RedisAuth, RedisClientConfig, RedisPool, RedisTimeout};
use futures::FutureExt;

/// Key layout: `{KEY_PREFIX}tb:{POLICY_IDX}:{limit_key}`. The limit key carries
/// the cluster hash tag so every attempt of one run lands on the same slot.
const KEY_PREFIX: &str = "edgion:test:rl:";
const POLICY_IDX: usize = 0;

/// Bucket capacity and refill rate per `WINDOW_MS`, i.e. one token per second.
const RATE: u64 = 3;
const WINDOW_MS: i64 = 3_000;
/// Weighted cost: the whole point is that `COST > 1`, so a single refilled
/// token is not enough on its own and must survive the denied attempt.
const COST: u32 = 3;
/// Synthetic timeline base; the Lua only uses `now` for arithmetic against the
/// stored timestamp, so the test never depends on the wall clock.
const BASE_MS: i64 = 1_700_000_000_000;
/// Elapsed time that refills exactly one token.
const ONE_TOKEN_MS: i64 = WINDOW_MS / RATE as i64;

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

async fn attempt(
    core: &RedisRateLimiter,
    client: &RedisLinkClient,
    limit_key: &str,
    now_ms: i64,
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
        COST,
    )
    .await
}

async fn partial_refill_sequence(core: &RedisRateLimiter, client: &RedisLinkClient, limit_key: &str) -> Result<()> {
    // A fresh bucket starts full, so the first weighted attempt drains it.
    let drained = attempt(core, client, limit_key, BASE_MS).await?;
    assert!(drained.allowed, "a fresh bucket must admit the first weighted request");
    assert_eq!(drained.current, 0, "the weighted request must drain the bucket");
    assert_eq!(drained.remaining, 0);
    assert_eq!(drained.limit, RATE);
    assert_eq!(drained.retry_ms, 0);

    // One refilled token is below the cost: denied, but the token must persist.
    let one_token = attempt(core, client, limit_key, BASE_MS + ONE_TOKEN_MS).await?;
    assert!(!one_token.allowed, "one token must not satisfy a cost of {COST}");
    assert_eq!(one_token.current, 1, "the single refilled token must be persisted");
    assert_eq!(one_token.remaining, 0);
    assert_eq!(
        one_token.retry_ms, 2_000,
        "retry must cover the two tokens still missing"
    );

    // The second refilled token accumulates on top of the persisted one. This
    // is the assertion that fails when the deny branch drops the balance.
    let two_tokens = attempt(core, client, limit_key, BASE_MS + 2 * ONE_TOKEN_MS).await?;
    assert!(!two_tokens.allowed, "two tokens must not satisfy a cost of {COST}");
    assert_eq!(
        two_tokens.current, 2,
        "the second refill must accumulate on top of the denied attempt's balance"
    );
    assert_eq!(two_tokens.remaining, 0);
    assert_eq!(
        two_tokens.retry_ms, 1_000,
        "retry must cover the one token still missing"
    );

    // The third accumulated token finally reaches the weighted cost.
    let allowed = attempt(core, client, limit_key, BASE_MS + 3 * ONE_TOKEN_MS).await?;
    assert!(
        allowed.allowed,
        "three accumulated tokens must satisfy a cost of {COST} instead of denying forever"
    );
    assert_eq!(allowed.current, 0, "the admitted request must consume the accumulation");
    assert_eq!(allowed.remaining, 0);
    assert_eq!(allowed.retry_ms, 0);

    Ok(())
}

#[tokio::test]
#[ignore = "requires the integration Redis fixture"]
async fn token_bucket_accumulates_partial_refills_across_denials() -> Result<()> {
    let (config, auth) = redis_config();
    let client = RedisLinkClient::from_config("integration/token-bucket-partial-refill", &config, &auth)?;
    client
        .init_for_test()
        .await
        .context("connect to the integration Redis fixture")?;
    let core = RedisRateLimiter::new();

    let limit_key = format!("{{{}}}", uuid::Uuid::new_v4().simple());
    // Catch a failed assertion so the run's key is still dropped, then resume
    // the unwind with the original panic payload and message.
    let outcome = AssertUnwindSafe(partial_refill_sequence(&core, &client, &limit_key))
        .catch_unwind()
        .await;

    let key = build_redis_key(KEY_PREFIX, "tb", POLICY_IDX, &limit_key, "");
    let cleanup = client.del(&[key.as_str()]).await;
    let shutdown = client.shutdown().await;
    // Resume before propagating any cleanup error, so a failed assertion is
    // never replaced by a teardown error.
    match outcome {
        Ok(result) => result?,
        Err(panic) => std::panic::resume_unwind(panic),
    }
    cleanup?;
    shutdown?;
    Ok(())
}
