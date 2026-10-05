//! Toggle regex case matching on the shared mTLS listener and restore its fixture.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::port_config::PortKind;
use anyhow::{ensure, Context, Result};
use std::time::{Duration, Instant};

struct RegexSanSuite(bool);

impl TestSuite for RegexSanSuite {
    fn name(&self) -> &str {
        "AllowedSanRegexIgnoreCase"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionTls/mTLS")
    }

    fn port_kind(&self) -> PortKind {
        PortKind::Https
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let accepted = self.0;
        vec![TestCase::new(
            "regex_case_generation",
            "Fresh mTLS handshakes obey the published SAN case mode",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        // A control handshake rules out a listener, CA, or fixture outage.
                        probe(&ctx, "mtls.example.com", "valid-client", true).await?;
                        probe(&ctx, "mtls-san.example.com", "valid-client", accepted).await?;
                        probe(&ctx, "mtls-san.example.com", "nonmatching-client", false).await
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(15))]
    }
}

async fn probe(ctx: &TestContext, hostname: &str, certificate: &str, accepted: bool) -> Result<()> {
    let mut pem = std::fs::read(format!("edgion-tests/certs/mtls/{certificate}.crt"))
        .context("failed to read client certificate")?;
    pem.extend(
        std::fs::read(format!("edgion-tests/certs/mtls/{certificate}.key")).context("failed to read client key")?,
    );
    let target = tokio::net::lookup_host((ctx.target_host.as_str(), ctx.https_port))
        .await?
        .next()
        .context("mTLS target resolved to no addresses")?;
    // Each probe owns a fresh client so pooled connections and TLS resumption
    // cannot hide a policy update. Test fixtures use a self-signed server cert.
    let client = reqwest::Client::builder()
        .identity(reqwest::Identity::from_pem(&pem)?)
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .resolve(hostname, target)
        .timeout(Duration::from_secs(3))
        .build()?;
    let response = client
        .get(format!("https://{hostname}:{}/health", ctx.https_port))
        .send()
        .await;
    if accepted {
        let response = response.context("SAN policy unexpectedly rejected the client")?;
        ensure!(
            response.status().as_u16() == 200,
            "unexpected HTTP status {}",
            response.status()
        );
    } else {
        let error = response.err().context("SAN policy unexpectedly accepted the client")?;
        let detail = format!("{error:?}").to_ascii_lowercase();
        ensure!(
            !error.is_timeout()
                && (detail.contains("alert") || detail.contains("certificate") || detail.contains("handshake")),
            "expected TLS rejection, got {error:?}"
        );
    }
    Ok(())
}

fn accepted() -> Box<dyn TestSuite> {
    Box::new(RegexSanSuite(true))
}

fn rejected() -> Box<dyn TestSuite> {
    Box::new(RegexSanSuite(false))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "accepted",
        factory: accepted,
    },
    PhaseSuite {
        name: "rejected",
        factory: rejected,
    },
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        const ORIGINAL: &str = "DynamicTest/initial/";
        let result = async {
            ctx.apply_dir(ORIGINAL).await?;
            ctx.wait_phase(SCENARIO, "accepted", Duration::from_secs(20)).await?;
            for (path, phase) in [
                ("DynamicTest/updates/sensitive/", "rejected"),
                ("DynamicTest/updates/insensitive/", "accepted"),
                ("DynamicTest/updates/sensitive/", "rejected"),
                ("DynamicTest/updates/insensitive/", "accepted"),
            ] {
                ctx.apply_dir(path).await?;
                ctx.wait_phase(SCENARIO, phase, Duration::from_secs(20)).await?;
            }
            Ok(())
        }
        .await;
        // This scenario borrows the static mTLS SAN resource, so restore it on
        // both success and failure and wait for its original behavior to return.
        let restore = async {
            ctx.apply_dir(ORIGINAL).await?;
            ctx.wait_phase(SCENARIO, "accepted", Duration::from_secs(20)).await
        }
        .await;
        match (result, restore) {
            (Ok(()), restore) => restore,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(restore)) => Err(anyhow::anyhow!("{error:#}; restore also failed: {restore:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "AllowedSanRegexIgnoreCase",
    conf_path: "EdgionTls/mTLS",
    run,
    phases: PHASES,
};
