// tls-06 last-known-good dynamic scenario.
//
// initial:  stash the valid fixture's expected serial, then wait until it is
//           served by the HTTPS listener and ResolvedRefs=True.
// apply:    updates_garbage/ replaces lkg-secret with a key/cert-mismatch payload.
// fallback: handshake STILL OK and the served serial equals the stashed serial
//           while the controller flips ResolvedRefs to False/InvalidCertificate.
// recovered: restore the valid Secret and verify the original serial.
// rotated:   apply a fresh valid certificate B and verify its new serial.
// fallback: reject another invalid update while continuing to serve B.
// cleanup: restore the valid Secret on both success and failure.
// The controller may reject the bad Secret before gateway publication; gateway
// store unit tests separately verify its last-known-good and rejection-log state.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestSuite};
use crate::suites;
use crate::tls_child::{self, OPERATION_TIMEOUT};
use anyhow::Context;
use base64::Engine;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::Instant;

fn initial() -> Box<dyn TestSuite> {
    // These assertions form a certificate-convergence barrier: an old served
    // certificate cannot pass merely because ResolvedRefs was already True.
    Box::new(ValidFixtureTestSuite {
        phase: "TLS Last-Known-Good - Initial Fixture Convergence",
    })
}
fn fallback() -> Box<dyn TestSuite> {
    Box::new(suites::LastKnownGoodFallbackTestSuite)
}

struct ValidFixtureTestSuite {
    phase: &'static str,
}

#[async_trait::async_trait]
impl TestSuite for ValidFixtureTestSuite {
    fn name(&self) -> &str {
        self.phase
    }

    fn test_cases(&self) -> Vec<TestCase> {
        suites::LastKnownGoodInitialTestSuite::recovery_test_cases()
    }
}

fn recovered() -> Box<dyn TestSuite> {
    Box::new(ValidFixtureTestSuite {
        phase: "TLS Last-Known-Good - Recovered Phase",
    })
}

fn rotated() -> Box<dyn TestSuite> {
    Box::new(ValidFixtureTestSuite {
        phase: "TLS Last-Known-Good - Valid Certificate Rotation",
    })
}

async fn generate_rotated_secret(ctx: &ScenarioCtx) -> anyhow::Result<PathBuf> {
    let output_dir = ctx
        .test_ctx
        .access_log_path
        .parent()
        .context("missing integration log directory")?
        .join("tls-rotation");
    std::fs::create_dir_all(&output_dir).context("failed to create TLS rotation directory")?;
    let cert_path = output_dir.join("rotated.crt");
    let key_path = output_dir.join("rotated.key");
    let mut command = Command::new("openssl");
    command
        .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes"])
        .arg("-keyout")
        .arg(&key_path)
        .arg("-out")
        .arg(&cert_path)
        .args([
            "-days",
            "365",
            "-subj",
            "/CN=lkg.example.com",
            "-addext",
            "subjectAltName=DNS:lkg.example.com",
        ]);
    let status = tls_child::output(command, None, Instant::now() + OPERATION_TIMEOUT)
        .await
        .context("failed to invoke openssl for TLS rotation")?;
    if !status.status.success() {
        anyhow::bail!("failed to generate valid TLS rotation certificate");
    }
    let cert = std::fs::read(&cert_path).context("failed to read rotated certificate")?;
    let key = std::fs::read(&key_path).context("failed to read rotated private key")?;
    let encoded_cert = base64::engine::general_purpose::STANDARD.encode(cert);
    let encoded_key = base64::engine::general_purpose::STANDARD.encode(key);
    let secret_path = output_dir.join("Secret_lkg-secret.yaml");
    std::fs::write(
        &secret_path,
        format!(
            "apiVersion: v1\nkind: Secret\nmetadata:\n  name: lkg-secret\n  namespace: edgion-test\ntype: kubernetes.io/tls\ndata:\n  tls.crt: {encoded_cert}\n  tls.key: {encoded_key}\n"
        ),
    )
    .context("failed to write rotated TLS Secret")?;
    Ok(secret_path)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "fallback",
        factory: fallback,
    },
    PhaseSuite {
        name: "recovered",
        factory: recovered,
    },
    PhaseSuite {
        name: "rotated",
        factory: rotated,
    },
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let scenario = SCENARIO;
        const VALID_SECRET: &str = "initial/Secret_lkg-secret.yaml";
        let initial_serial =
            suites::LastKnownGoodInitialTestSuite::stash_fixture_serial(&ctx.test_ctx, &ctx.resolve(VALID_SECRET))
                .await
                .map_err(anyhow::Error::msg)?;
        let update_path = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
            "DynamicTest/updates/garbage"
        } else {
            "updates_garbage"
        };
        let result = async {
            ctx.apply_dir(VALID_SECRET).await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;
            ctx.apply_dir(update_path).await?;
            ctx.wait_phase(scenario, "fallback", Duration::from_secs(15)).await?;
            ctx.apply_dir(VALID_SECRET).await?;
            ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await?;
            let rotated_secret = generate_rotated_secret(&ctx).await?;
            let rotated_serial =
                suites::LastKnownGoodInitialTestSuite::stash_fixture_serial(&ctx.test_ctx, &rotated_secret)
                    .await
                    .map_err(anyhow::Error::msg)?;
            if rotated_serial == initial_serial {
                anyhow::bail!("rotated certificate serial matches the original certificate");
            }
            let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
            crate::dynamic::apply::apply_path(&client, &rotated_secret).await?;
            ctx.wait_phase(scenario, "rotated", Duration::from_secs(15)).await?;
            ctx.apply_dir(update_path).await?;
            ctx.wait_phase(scenario, "fallback", Duration::from_secs(15)).await
        }
        .await;

        restore_after_result(result, async {
            suites::LastKnownGoodInitialTestSuite::stash_fixture_serial(&ctx.test_ctx, &ctx.resolve(VALID_SECRET))
                .await
                .map_err(anyhow::Error::msg)?;
            ctx.apply_dir(VALID_SECRET).await?;
            ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await
        })
        .await
    })
}

async fn restore_after_result(
    result: anyhow::Result<()>,
    restore: impl std::future::Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()> {
    match (result, restore.await) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.context("failed to restore the valid TLS Secret")),
        (Err(error), Err(restore_error)) => Err(error.context(format!(
            "also failed to restore the valid TLS Secret: {restore_error:#}"
        ))),
    }
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "TlsLastKnownGood",
    conf_path: "EdgionTls/LastKnownGood",
    run,
    phases: PHASES,
};

#[cfg(test)]
#[path = "../../../../tests/unit/client/dynamic/scenarios/edgion_tls_last_known_good_test.rs"]
mod tests;
