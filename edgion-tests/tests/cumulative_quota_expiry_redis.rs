//! Real-Redis regression coverage for shared cumulative quota retention.
//!
//! Uses only uniquely named keys and the production LinkSys/script executor.
//! No Gateway/Controller topology is started. Run explicitly against the Redis
//! integration fixture with:
//!
//! cargo test -p edgion-tests --test cumulative_quota_expiry_redis -- --ignored
//!
//! EDGION_TEST_REDIS_ENDPOINT and EDGION_TEST_REDIS_PASSWORD override the
//! integration fixture defaults. This standalone test does not prove Cluster,
//! failover, or full HTTP/SSE delivery behavior.

use std::{panic::AssertUnwindSafe, time::Duration};

use anyhow::{Context, Result};
use edgion_gateway::link_sys::{credential::CredentialState, providers::redis::RedisLinkClient};
use edgion_gateway::plugins::common::redis_rate_limiter::{
    build_cumulative_quota_keys, CumulativeQuotaKeys, CumulativeQuotaMode, CumulativeQuotaResult,
    CumulativeQuotaStatus, RedisRateLimiter,
};
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
use edgion_resources::resources::link_sys::redis::{RedisAuth, RedisClientConfig, RedisPool, RedisTimeout};
use futures::FutureExt;

const LIMIT: u64 = 1_000;
const ACTIVE_TTL: Duration = Duration::from_secs(30);
const FINAL_TTL: Duration = Duration::from_secs(1);

async fn connect() -> Result<RedisLinkClient> {
    let config = RedisClientConfig {
        endpoints: vec![
            std::env::var("EDGION_TEST_REDIS_ENDPOINT").unwrap_or_else(|_| "redis://127.0.0.1:16379".into())
        ],
        auth: Some(RedisAuth {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "integration-redis-credentials".into(),
                    namespace: None,
                },
                secret: None,
            },
        }),
        db: Some(0),
        timeout: Some(RedisTimeout {
            connect: Some("3s".into()),
            command: Some("2s".into()),
        }),
        pool: Some(RedisPool { size: Some(1) }),
        topology: None,
        tls: None,
    };
    let auth = CredentialState::Resolved(BasicAuthMaterial {
        username: None,
        password: std::env::var("EDGION_TEST_REDIS_PASSWORD").unwrap_or_else(|_| "edgion-test-pwd".into()),
    });
    let client = RedisLinkClient::from_test_config("integration/cumulative-quota-expiry", &config, &auth)?;
    client.init_for_test().await.context("connect to the Redis fixture")?;
    Ok(client)
}

async fn apply(
    core: &RedisRateLimiter,
    client: &RedisLinkClient,
    keys: &CumulativeQuotaKeys,
    revision: u64,
    target: u64,
    final_update: bool,
    ttl: Duration,
) -> Result<CumulativeQuotaResult> {
    core.apply_cumulative_quota(
        client,
        &keys.bucket,
        &keys.marker,
        revision,
        target,
        final_update,
        LIMIT,
        ttl,
        ttl,
        CumulativeQuotaMode::Force,
    )
    .await
}

async fn sequence(core: &RedisRateLimiter, client: &RedisLinkClient, keys: &[CumulativeQuotaKeys]) -> Result<()> {
    let [a, b, c, next_window, short_lived] = keys else {
        unreachable!("the fixture supplies five key pairs");
    };

    // A's acknowledged charge must survive B's shorter final retention.
    assert_eq!(apply(core, client, a, 1, 900, false, ACTIVE_TTL).await?.used, 900);
    assert_eq!(apply(core, client, b, 1, 10, true, FINAL_TTL).await?.used, 910);
    assert!(
        client.ttl(&a.bucket).await? >= 25,
        "B must not shorten A's shared bucket lease"
    );
    assert!((0..=1).contains(&client.ttl(&b.marker).await?));

    // All early-return refresh paths must preserve another request's lease.
    for (revision, target, final_update, expected) in [
        (1, 10, true, CumulativeQuotaStatus::Duplicate),
        (2, 11, true, CumulativeQuotaStatus::Conflict),
    ] {
        let result = apply(core, client, b, revision, target, final_update, FINAL_TTL).await?;
        assert_eq!(result.status, expected);
        assert_eq!(result.used, 910);
        assert!(client.ttl(&a.bucket).await? >= 25);
    }
    assert_eq!(apply(core, client, c, 2, 0, false, FINAL_TTL).await?.used, 910);
    for (revision, target, expected) in [
        (1, 0, CumulativeQuotaStatus::Stale),
        (2, 0, CumulativeQuotaStatus::Duplicate),
        (2, 1, CumulativeQuotaStatus::Conflict),
        (3, 0, CumulativeQuotaStatus::Applied),
    ] {
        let result = apply(core, client, c, revision, target, false, FINAL_TTL).await?;
        assert_eq!(result.status, expected);
        assert_eq!(result.used, 910);
        assert!(client.ttl(&a.bucket).await? >= 25);
    }

    // A zero-delta lease renewal must also extend a short bucket.
    assert_eq!(apply(core, client, short_lived, 1, 1, false, FINAL_TTL).await?.used, 1);
    assert_eq!(apply(core, client, short_lived, 2, 1, false, ACTIVE_TTL).await?.used, 1);
    assert!(client.ttl(&short_lived.bucket).await? >= 25);
    assert!(client.ttl(&short_lived.marker).await? >= 25);
    assert_eq!(
        apply(core, client, short_lived, 3, 0, false, FINAL_TTL).await?.status,
        CumulativeQuotaStatus::Conflict
    );
    assert!(client.ttl(&short_lived.bucket).await? >= 25);

    // A separate fresh final bucket must still receive a finite expiry.
    apply(core, client, next_window, 1, 0, true, FINAL_TTL).await?;
    assert!((0..=1).contains(&client.ttl(&next_window.bucket).await?));

    // No updates for A while only heartbeats would be flowing. Wait past the
    // short retention, but before the independent active lease renewal.
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert!(!client.exists(&b.marker).await?);
    assert!(!client.exists(&next_window.bucket).await?);
    assert!(!client.exists(&next_window.marker).await?);
    assert!(client.exists(&a.marker).await?);
    let resumed = apply(core, client, a, 2, 1_000, false, ACTIVE_TTL).await?;
    assert_eq!(resumed.status, CumulativeQuotaStatus::Applied);
    assert_eq!(
        resumed.used, 1_010,
        "previously acknowledged charges must not disappear"
    );
    assert!(
        resumed.used > LIMIT,
        "the resumed stream must see an over-limit aggregate"
    );
    assert_eq!(
        core.cumulative_quota_admission(client, &a.bucket, LIMIT).await?.status,
        CumulativeQuotaStatus::Denied
    );
    assert_eq!(
        core.cumulative_quota_admission(client, &next_window.bucket, LIMIT)
            .await?
            .status,
        CumulativeQuotaStatus::Admitted
    );

    // Conservative unknown settlement can retain its watermark, and known
    // final usage can still lower an estimate without shortening the bucket.
    assert_eq!(apply(core, client, c, 4, 50, false, ACTIVE_TTL).await?.used, 1_060);
    assert_eq!(apply(core, client, c, 5, 50, true, FINAL_TTL).await?.used, 1_060);
    assert_eq!(apply(core, client, a, 3, 950, true, FINAL_TTL).await?.used, 1_010);
    assert!((25..=30).contains(&client.ttl(&a.bucket).await?));
    assert!((0..=1).contains(&client.ttl(&a.marker).await?));
    Ok(())
}

#[tokio::test]
#[ignore = "requires the integration Redis fixture"]
async fn short_final_updates_preserve_other_active_cumulative_charges() -> Result<()> {
    let client = connect().await?;
    let core = RedisRateLimiter::new();
    let run = uuid::Uuid::new_v4().to_string();
    let mut keys = Vec::new();
    for (window, request) in [(1, "a"), (1, "b"), (1, "c"), (2, "next"), (3, "short")] {
        keys.push(build_cumulative_quota_keys(
            "edgion:test:expiry:",
            run.as_bytes(),
            window,
            request.as_bytes(),
        )?);
    }
    let outcome = AssertUnwindSafe(sequence(&core, &client, &keys)).catch_unwind().await;
    let cleanup = keys
        .iter()
        .flat_map(|keys| [keys.bucket.as_str(), keys.marker.as_str()])
        .collect::<Vec<_>>();
    let cleanup_result = client.del(&cleanup).await;
    client.shutdown().await?;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    cleanup_result?;
    outcome.unwrap()
}
