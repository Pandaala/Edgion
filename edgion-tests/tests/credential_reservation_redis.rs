//! Real-Redis coverage for the credential RPM/TPM reservation Lua ledger.
//!
//! The test is ignored during ordinary `cargo test` because it requires the
//! Redis fixture used by the integration runner. Run it explicitly with:
//!
//! ```text
//! docker compose -f edgion-tests/integration/conf/Services/redis/docker-compose.yaml up -d
//! cargo test -p edgion-tests --test credential_reservation_redis -- --ignored --nocapture
//! ```
//!
//! `EDGION_TEST_REDIS_ENDPOINT` and `EDGION_TEST_REDIS_PASSWORD` may override
//! the fixture defaults.

use std::time::Duration;

use anyhow::{Context, Result};
use edgion_gateway::link_sys::credential::CredentialState;
use edgion_gateway::link_sys::providers::redis::RedisLinkClient;
use edgion_gateway::plugins::common::redis_rate_limiter::{
    build_credential_reservation_keys, CredentialReservationKeys, CredentialReservationLimits,
    CredentialReservationOperation, CredentialReservationStatus, RedisRateLimiter,
};
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
use edgion_resources::resources::link_sys::redis::{RedisAuth, RedisClientConfig, RedisPool, RedisTimeout};

const KEY_PREFIX: &str = "edgion:test:aic:";

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

fn keys(scope: &str, request: &str) -> Result<CredentialReservationKeys> {
    build_credential_reservation_keys(
        KEY_PREFIX,
        b"integration",
        scope.as_bytes(),
        b"primary",
        1,
        request.as_bytes(),
    )
}

async fn apply(
    core: &RedisRateLimiter,
    client: &RedisLinkClient,
    keys: &CredentialReservationKeys,
    limits: CredentialReservationLimits,
    ttl: Duration,
    operation: CredentialReservationOperation,
) -> Result<edgion_gateway::plugins::common::redis_rate_limiter::CredentialReservationResult> {
    core.apply_credential_reservation(client, keys, limits, ttl, operation)
        .await
}

async fn remove(client: &RedisLinkClient, key_sets: &[&CredentialReservationKeys]) -> Result<()> {
    let keys = key_sets
        .iter()
        .flat_map(|keys| [keys.bucket.as_str(), keys.marker.as_str()])
        .collect::<Vec<_>>();
    client.del(&keys).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires the integration Redis fixture"]
async fn credential_reservation_lifecycle_uses_real_redis() -> Result<()> {
    let (config, auth) = redis_config();
    let client = RedisLinkClient::from_config("integration/credential-reservation", &config, &auth)?;
    client
        .init_for_test()
        .await
        .context("connect to the integration Redis fixture")?;
    let core = RedisRateLimiter::new();
    let run = uuid::Uuid::new_v4().simple().to_string();
    let ttl = Duration::from_secs(10);

    let admission_a = keys(&format!("{run}-admission"), "a")?;
    let admission_b = keys(&format!("{run}-admission"), "b")?;
    let admission_c = keys(&format!("{run}-admission"), "c")?;
    let admission_d = keys(&format!("{run}-admission"), "d")?;
    let admission_limits = CredentialReservationLimits {
        rpm: Some(2),
        tpm: Some(100),
    };
    let first = apply(
        &core,
        &client,
        &admission_a,
        admission_limits,
        ttl,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 20,
            max_output_tokens: 30,
        },
    )
    .await?;
    assert_eq!(first.status, CredentialReservationStatus::Reserved);
    assert_eq!((first.rpm_used, first.tpm_used), (1, 50));

    let duplicate = apply(
        &core,
        &client,
        &admission_a,
        admission_limits,
        ttl,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 20,
            max_output_tokens: 30,
        },
    )
    .await?;
    assert_eq!(duplicate.status, CredentialReservationStatus::Duplicate);
    assert_eq!((duplicate.rpm_used, duplicate.tpm_used), (1, 50));

    let denied_tpm = apply(
        &core,
        &client,
        &admission_b,
        admission_limits,
        ttl,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 1,
            max_output_tokens: 50,
        },
    )
    .await?;
    assert_eq!(denied_tpm.status, CredentialReservationStatus::DeniedTpm);
    assert_eq!((denied_tpm.rpm_used, denied_tpm.tpm_used), (1, 50));

    let second = apply(
        &core,
        &client,
        &admission_c,
        admission_limits,
        ttl,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 10,
            max_output_tokens: 10,
        },
    )
    .await?;
    assert_eq!(second.status, CredentialReservationStatus::Reserved);
    assert_eq!((second.rpm_used, second.tpm_used), (2, 70));

    let denied_rpm = apply(
        &core,
        &client,
        &admission_d,
        admission_limits,
        ttl,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 1,
            max_output_tokens: 1,
        },
    )
    .await?;
    assert_eq!(denied_rpm.status, CredentialReservationStatus::DeniedRpm);
    assert_eq!((denied_rpm.rpm_used, denied_rpm.tpm_used), (2, 70));

    let down = keys(&format!("{run}-down"), "request")?;
    let reconcile_limits = CredentialReservationLimits {
        rpm: Some(10),
        tpm: Some(100),
    };
    apply(
        &core,
        &client,
        &down,
        reconcile_limits,
        ttl,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 20,
            max_output_tokens: 30,
        },
    )
    .await?;
    let reconciled_down = apply(
        &core,
        &client,
        &down,
        reconcile_limits,
        ttl,
        CredentialReservationOperation::Reconcile { charged_tokens: 25 },
    )
    .await?;
    assert_eq!(reconciled_down.status, CredentialReservationStatus::Reconciled);
    assert_eq!(
        (
            reconciled_down.rpm_used,
            reconciled_down.tpm_used,
            reconciled_down.applied_tpm_delta,
        ),
        (1, 25, -25)
    );
    assert_eq!(
        apply(
            &core,
            &client,
            &down,
            reconcile_limits,
            ttl,
            CredentialReservationOperation::Reconcile { charged_tokens: 25 },
        )
        .await?
        .status,
        CredentialReservationStatus::Duplicate
    );
    assert_eq!(
        apply(
            &core,
            &client,
            &down,
            reconcile_limits,
            ttl,
            CredentialReservationOperation::Reconcile { charged_tokens: 26 },
        )
        .await?
        .status,
        CredentialReservationStatus::Conflict
    );

    let up = keys(&format!("{run}-up"), "request")?;
    apply(
        &core,
        &client,
        &up,
        reconcile_limits,
        ttl,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 10,
            max_output_tokens: 10,
        },
    )
    .await?;
    let reconciled_up = apply(
        &core,
        &client,
        &up,
        reconcile_limits,
        ttl,
        CredentialReservationOperation::Reconcile { charged_tokens: 150 },
    )
    .await?;
    assert_eq!(reconciled_up.status, CredentialReservationStatus::Reconciled);
    assert_eq!((reconciled_up.rpm_used, reconciled_up.tpm_used), (1, 150));
    assert_eq!(reconciled_up.applied_tpm_delta, 130);

    let release = keys(&format!("{run}-release"), "request")?;
    apply(
        &core,
        &client,
        &release,
        reconcile_limits,
        ttl,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 40,
            max_output_tokens: 60,
        },
    )
    .await?;
    let released = apply(
        &core,
        &client,
        &release,
        reconcile_limits,
        ttl,
        CredentialReservationOperation::ReleasePredispatch,
    )
    .await?;
    assert_eq!(released.status, CredentialReservationStatus::Released);
    assert_eq!(
        (released.rpm_used, released.tpm_used, released.applied_tpm_delta),
        (0, 0, -100)
    );
    assert_eq!(
        apply(
            &core,
            &client,
            &release,
            reconcile_limits,
            ttl,
            CredentialReservationOperation::ReleasePredispatch,
        )
        .await?
        .status,
        CredentialReservationStatus::Duplicate
    );

    let expiry = keys(&format!("{run}-expiry"), "request")?;
    let expiry_ttl = Duration::from_millis(100);
    apply(
        &core,
        &client,
        &expiry,
        reconcile_limits,
        expiry_ttl,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 10,
            max_output_tokens: 10,
        },
    )
    .await?;
    assert!(client.exists(&expiry.bucket).await?);
    assert!(client.exists(&expiry.marker).await?);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if !client.exists(&expiry.bucket).await? && !client.exists(&expiry.marker).await? {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("credential reservation keys did not expire")??;
    assert_eq!(
        apply(
            &core,
            &client,
            &expiry,
            reconcile_limits,
            ttl,
            CredentialReservationOperation::Reserve {
                estimated_input_tokens: 10,
                max_output_tokens: 10,
            },
        )
        .await?
        .status,
        CredentialReservationStatus::Reserved
    );

    remove(
        &client,
        &[
            &admission_a,
            &admission_b,
            &admission_c,
            &admission_d,
            &down,
            &up,
            &release,
            &expiry,
        ],
    )
    .await?;
    client.shutdown().await?;
    Ok(())
}
