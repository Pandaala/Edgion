//! Script-cache lifecycle coverage against an exclusively owned standalone Redis.
//!
//! This test flushes the server-wide script cache. Never point it at a shared
//! Redis, even with a separate database. Requires redis-cli and explicit opt-in:
//!
//! ```text
//! EDGION_TEST_REDIS_ISOLATED=1 EDGION_TEST_REDIS_ENDPOINT=redis://127.0.0.1:17379 \
//! cargo test -p edgion-tests --test redis_script_lifecycle -- --ignored
//! ```
//!
//! Set EDGION_TEST_REDIS_PASSWORD when authentication is required; REDIS_CLI may
//! select a redis-cli wrapper or alternate executable. No Cluster or
//! failover guarantees are inferred from this standalone fixture.

use std::{panic::AssertUnwindSafe, process::Command, time::Duration};

use anyhow::{ensure, Context, Result};
use edgion_gateway::link_sys::{credential::CredentialState, providers::redis::RedisLinkClient};
use edgion_gateway::plugins::common::redis_rate_limiter::{
    build_credential_reservation_keys, build_cumulative_quota_keys, build_redis_key, scripts::*,
    CredentialReservationLimits, CredentialReservationOperation, CredentialReservationStatus, CumulativeQuotaMode,
    CumulativeQuotaStatus, RedisRateLimiter,
};
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
use edgion_resources::resources::edgion_plugins::RateLimitAlgorithm;
use edgion_resources::resources::link_sys::redis::{RedisAuth, RedisClientConfig, RedisPool, RedisTimeout};
use futures::{future::try_join_all, FutureExt};
use sha1::{Digest, Sha1};

const PREFIX: &str = "edgion:test:script-lifecycle:";
const WINDOW_MS: i64 = 600_000;
const NOW_MS: i64 = 1_800_000_000_000;
const SOURCES: [&str; 5] = [
    SLIDING_WINDOW_SCRIPT,
    FIXED_WINDOW_SCRIPT,
    TOKEN_BUCKET_SCRIPT,
    CUMULATIVE_QUOTA_SCRIPT,
    CREDENTIAL_RESERVATION_SCRIPT,
];

struct Fixture {
    endpoint: String,
    password: Option<String>,
}

impl Fixture {
    fn from_env() -> Result<Self> {
        ensure!(
            std::env::var("EDGION_TEST_REDIS_ISOLATED").as_deref() == Ok("1"),
            "SCRIPT FLUSH requires EDGION_TEST_REDIS_ISOLATED=1 and exclusive server ownership"
        );
        let endpoint = std::env::var("EDGION_TEST_REDIS_ENDPOINT")
            .context("an explicit isolated EDGION_TEST_REDIS_ENDPOINT is required")?;
        ensure!(
            !endpoint.contains('@'),
            "pass authentication through EDGION_TEST_REDIS_PASSWORD"
        );
        Ok(Self {
            endpoint,
            password: std::env::var("EDGION_TEST_REDIS_PASSWORD").ok(),
        })
    }

    // CLI access is restricted to test fixture administration and inspection;
    // every rate-limit operation uses the production LinkSys execution path.
    fn admin(&self, args: &[&str]) -> Result<String> {
        let mut command = Command::new(std::env::var_os("REDIS_CLI").unwrap_or_else(|| "redis-cli".into()));
        command.args(["--raw", "-u", &self.endpoint]).args(args);
        command.env_remove("REDISCLI_AUTH");
        if let Some(password) = &self.password {
            command.env("REDISCLI_AUTH", password);
        }
        let output = command.output().context("run redis-cli for the isolated fixture")?;
        ensure!(output.status.success(), "redis-cli failed");
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn flush_scripts(&self) -> Result<()> {
        assert_eq!(self.admin(&["SCRIPT", "FLUSH", "SYNC"])?, "OK");
        Ok(())
    }

    fn assert_cached(&self, selected: Option<usize>) -> Result<()> {
        for (index, source) in SOURCES.iter().enumerate() {
            let hash = format!("{:x}", Sha1::digest(source.as_bytes()));
            assert_eq!(
                self.admin(&["SCRIPT", "EXISTS", &hash])?,
                if selected == Some(index) { "1" } else { "0" },
                "only the selected script may be loaded (index {index})"
            );
        }
        Ok(())
    }

    fn calls(&self, command: &str) -> Result<u64> {
        let info = self.admin(&["INFO", "commandstats"])?;
        let prefix = format!("cmdstat_{command}:calls=");
        for line in info.lines() {
            if let Some(value) = line.strip_prefix(&prefix) {
                return Ok(value.split(',').next().context("missing command count")?.parse()?);
            }
        }
        Ok(0)
    }

    async fn connect(&self, name: &str) -> Result<RedisLinkClient> {
        let mut config = RedisClientConfig {
            endpoints: vec![self.endpoint.clone()],
            db: Some(0),
            timeout: Some(RedisTimeout {
                connect: Some("3s".to_string()),
                command: Some("2s".to_string()),
            }),
            pool: Some(RedisPool { size: Some(4) }),
            auth: None,
            topology: None,
            tls: None,
        };
        let auth = if let Some(password) = &self.password {
            config.auth = Some(RedisAuth {
                slot: SecretSlot {
                    secret_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "isolated-redis".into(),
                        namespace: None,
                    },
                    secret: None,
                },
            });
            CredentialState::Resolved(BasicAuthMaterial {
                username: None,
                password: password.clone(),
            })
        } else {
            CredentialState::NotRequired
        };
        let client = RedisLinkClient::from_config(name, &config, &auth)?;
        client.init_for_test().await?;
        Ok(client)
    }
}

async fn invoke(core: &RedisRateLimiter, client: &RedisLinkClient, script: usize, run: &str, step: u64) -> Result<()> {
    let key = format!("{{{run}-{script}}}");
    if script < 3 {
        let algorithms = [
            RateLimitAlgorithm::SlidingWindow,
            RateLimitAlgorithm::FixedWindow,
            RateLimitAlgorithm::TokenBucket,
        ];
        let result = core
            .eval_policy(client, &algorithms[script], PREFIX, 0, &key, 30, WINDOW_MS, NOW_MS, 3)
            .await?;
        assert!(result.allowed);
        assert_eq!(result.remaining, i64::try_from(30 - step * 3)?);
        let suffix = if script == 1 {
            (NOW_MS / WINDOW_MS).to_string()
        } else {
            String::new()
        };
        let redis_key = build_redis_key(PREFIX, ["sw", "fw", "tb"][script], 0, &key, &suffix);
        let stored = match script {
            0 => client.hget(&redis_key, "cur").await?,
            1 => client.get(&redis_key).await?,
            _ => client.hget(&redis_key, "tokens").await?,
        }
        .context("policy must persist its balance")?
        .parse::<u64>()?;
        assert_eq!(stored, if script == 2 { 30 - step * 3 } else { step * 3 });
    } else if script == 3 {
        let keys = build_cumulative_quota_keys(PREFIX, run.as_bytes(), 1, b"request")?;
        let result = core
            .apply_cumulative_quota(
                client,
                &keys.bucket,
                &keys.marker,
                step,
                step * 3,
                false,
                30,
                Duration::from_secs(600),
                Duration::from_secs(600),
                CumulativeQuotaMode::Conditional,
            )
            .await?;
        assert_eq!(result.status, CumulativeQuotaStatus::Applied);
        assert_eq!(
            (result.used, result.applied_delta, result.remaining),
            (step * 3, 3, 30 - step * 3)
        );
        assert_eq!(client.hget(&keys.bucket, "used").await?, Some((step * 3).to_string()));
    } else {
        let keys = build_credential_reservation_keys(
            PREFIX,
            b"test",
            run.as_bytes(),
            b"primary",
            1,
            step.to_string().as_bytes(),
        )?;
        let result = core
            .apply_credential_reservation(
                client,
                &keys,
                CredentialReservationLimits {
                    rpm: Some(10),
                    tpm: Some(30),
                },
                Duration::from_secs(600),
                CredentialReservationOperation::Reserve {
                    estimated_input_tokens: 1,
                    max_output_tokens: 2,
                },
            )
            .await?;
        assert_eq!(result.status, CredentialReservationStatus::Reserved);
        assert_eq!((result.rpm_used, result.tpm_used), (step, step * 3));
        assert_eq!(client.hget(&keys.bucket, "tpm").await?, Some((step * 3).to_string()));
    }
    Ok(())
}

async fn lifecycle(fixture: &Fixture, client: &RedisLinkClient) -> Result<()> {
    let run = uuid::Uuid::new_v4().simple().to_string();
    let initial_loads = fixture.calls("script|load")?;
    for script in 0..5 {
        fixture.flush_scripts()?;
        let core = RedisRateLimiter::new();
        fixture.assert_cached(None)?;
        let before_eval = fixture.calls("eval")?;
        let before_sha = fixture.calls("evalsha")?;
        invoke(&core, client, script, &run, 1).await?;
        fixture.assert_cached(Some(script))?;
        assert_eq!(fixture.calls("eval")?, before_eval + 1);
        assert_eq!(fixture.calls("evalsha")?, before_sha + 1);
        // A new plugin instance must reuse the warm remote script without loads.
        invoke(&RedisRateLimiter::new(), client, script, &run, 2).await?;
        assert_eq!(fixture.calls("eval")?, before_eval + 1);
        assert_eq!(fixture.calls("evalsha")?, before_sha + 2);
        fixture.flush_scripts()?;
        invoke(&core, client, script, &run, 3).await?;
        fixture.assert_cached(Some(script))?;
        assert_eq!(fixture.calls("eval")?, before_eval + 2);
        assert_eq!(fixture.calls("evalsha")?, before_sha + 3);

        // The same core must work with a replacement LinkSys pool and cold cache.
        let replacement = fixture.connect("integration/script-replacement").await?;
        fixture.flush_scripts()?;
        let outcome = invoke(&core, &replacement, script, &run, 4).await;
        replacement.shutdown().await?;
        outcome?;
        fixture.assert_cached(Some(script))?;
        assert_eq!(fixture.calls("eval")?, before_eval + 3);
        assert_eq!(fixture.calls("evalsha")?, before_sha + 4);
    }
    assert_eq!(
        fixture.calls("script|load")?,
        initial_loads,
        "the executor must never preload scripts"
    );

    for algorithm in [
        RateLimitAlgorithm::SlidingWindow,
        RateLimitAlgorithm::FixedWindow,
        RateLimitAlgorithm::TokenBucket,
    ] {
        fixture.flush_scripts()?;
        let core = RedisRateLimiter::new();
        let key = format!("{{{}}}", uuid::Uuid::new_v4().simple());
        let attempts = (0..32).map(|_| core.eval_policy(client, &algorithm, PREFIX, 0, &key, 30, WINDOW_MS, NOW_MS, 3));
        let results = try_join_all(attempts).await?;
        assert_eq!(results.iter().filter(|result| result.allowed).count(), 10);
        let denied = core
            .eval_policy(client, &algorithm, PREFIX, 0, &key, 30, WINDOW_MS, NOW_MS, 3)
            .await?;
        assert!(!denied.allowed);
        assert_eq!(denied.remaining, 0);
    }

    // A warm script that encounters a Redis execution error must not be replayed.
    let core = RedisRateLimiter::new();
    let key = format!("{{{}}}", uuid::Uuid::new_v4().simple());
    let redis_key = build_redis_key(PREFIX, "tb", 0, &key, "");
    client
        .set(&redis_key, "NOSCRIPT is only data", Some(Duration::from_secs(600)))
        .await?;
    let before_eval = fixture.calls("eval")?;
    let before_sha = fixture.calls("evalsha")?;
    let error = core
        .eval_policy(
            client,
            &RateLimitAlgorithm::TokenBucket,
            PREFIX,
            0,
            &key,
            30,
            WINDOW_MS,
            NOW_MS,
            3,
        )
        .await
        .expect_err("a string cannot be read as a token-bucket hash");
    assert!(error.to_string().contains("WRONGTYPE"));
    assert_eq!(fixture.calls("eval")?, before_eval);
    assert_eq!(fixture.calls("evalsha")?, before_sha + 1);
    assert_eq!(client.get(&redis_key).await?.as_deref(), Some("NOSCRIPT is only data"));

    // After cache loss, the one permitted fallback may also fail. Propagate
    // that failure without retrying either command or changing stored data.
    fixture.flush_scripts()?;
    let before_eval = fixture.calls("eval")?;
    let before_sha = fixture.calls("evalsha")?;
    let error = core
        .eval_policy(
            client,
            &RateLimitAlgorithm::TokenBucket,
            PREFIX,
            0,
            &key,
            30,
            WINDOW_MS,
            NOW_MS,
            3,
        )
        .await
        .expect_err("the fallback must propagate the token-bucket type error");
    assert!(error.to_string().contains("WRONGTYPE"));
    assert_eq!(fixture.calls("eval")?, before_eval + 1);
    assert_eq!(fixture.calls("evalsha")?, before_sha + 1);
    assert_eq!(client.get(&redis_key).await?.as_deref(), Some("NOSCRIPT is only data"));
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusive standalone Redis, redis-cli and EDGION_TEST_REDIS_ISOLATED=1"]
async fn redis_scripts_recover_without_preloading_or_losing_weighted_debits() -> Result<()> {
    let fixture = Fixture::from_env()?;
    let client = fixture.connect("integration/script-lifecycle").await?;
    let outcome = AssertUnwindSafe(lifecycle(&fixture, &client)).catch_unwind().await;
    // Unique data keys expire after at most two windows; no data-wide flush is used.
    let shutdown = client.shutdown().await;
    match outcome {
        Ok(result) => result?,
        Err(panic) => std::panic::resume_unwind(panic),
    }
    shutdown?;
    Ok(())
}
