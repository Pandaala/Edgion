//! Ordered downstream group policy on one physical socket, independent of cert source.
use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::NoCertVerifier;
use anyhow::{ensure, Context, Result};
use rustls::pki_types::ServerName;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;
const PORT: u16 = 32006;
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const CONVERGENCE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
enum Generation {
    Initial,
    Updated,
    RejectedUpdate,
    Removed,
}

async fn handshake(ctx: &TestContext, host: &str, p384: bool, tls13: bool) -> Result<std::io::Result<TlsStream>> {
    let mut provider = rustls::crypto::ring::default_provider();
    provider.kx_groups = vec![if p384 {
        rustls::crypto::ring::kx_group::SECP384R1
    } else {
        rustls::crypto::ring::kx_group::SECP256R1
    }];
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[if tls13 {
            &rustls::version::TLS13
        } else {
            &rustls::version::TLS12
        }])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
        .with_no_client_auth();
    let tcp = timeout(IO_TIMEOUT, TcpStream::connect((ctx.target_host.as_str(), PORT)))
        .await
        .context("curve TCP timeout")??;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    timeout(
        IO_TIMEOUT,
        connector.connect(ServerName::try_from(host.to_owned())?, tcp),
    )
    .await
    .context("curve handshake timeout")
}

async fn assert_generation(ctx: &TestContext, generation: Generation) -> Result<()> {
    for tls13 in [false, true] {
        for host in ["ecdh-default.sandbox.example.com", "ecdh-edgion.sandbox.example.com"] {
            for p384 in [false, true] {
                let expected = match generation {
                    Generation::Initial => !p384,
                    Generation::Updated => p384,
                    Generation::RejectedUpdate => p384,
                    Generation::Removed => true,
                };
                let actual = handshake(ctx, host, p384, tls13).await?.is_ok();
                ensure!(
                    actual == expected,
                    "{host} TLS{} P-{}: expected {}, got {actual}",
                    if tls13 { "1.3" } else { "1.2" },
                    if p384 { 384 } else { 256 },
                    expected
                );
            }
        }
    }
    Ok(())
}

// Use one exact TLS 1.2 suite per probe, retaining TLS 1.3 suites for the
// minimum-version control. This verifies the negotiated policy, not just TCP.
async fn policy_handshake(ctx: &TestContext, host: &str, aes256: bool, tls13: bool) -> Result<bool> {
    let suite = if aes256 {
        rustls::CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384
    } else {
        rustls::CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256
    };
    let mut provider = rustls::crypto::ring::default_provider();
    provider
        .cipher_suites
        .retain(|candidate| candidate.suite() == suite || candidate.tls13().is_some());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[if tls13 {
            &rustls::version::TLS13
        } else {
            &rustls::version::TLS12
        }])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
        .with_no_client_auth();
    let tcp = timeout(IO_TIMEOUT, TcpStream::connect((ctx.target_host.as_str(), PORT))).await??;
    let result = timeout(
        IO_TIMEOUT,
        tokio_rustls::TlsConnector::from(Arc::new(config)).connect(ServerName::try_from(host.to_owned())?, tcp),
    )
    .await?;
    if let Ok(stream) = result {
        let connection = &stream.get_ref().1;
        ensure!(
            connection.protocol_version()
                == Some(if tls13 {
                    rustls::ProtocolVersion::TLSv1_3
                } else {
                    rustls::ProtocolVersion::TLSv1_2
                }),
            "unexpected protocol"
        );
        if !tls13 {
            ensure!(
                connection.negotiated_cipher_suite().map(|value| value.suite()) == Some(suite),
                "unexpected negotiated cipher"
            );
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

#[derive(Clone, Copy)]
enum PolicyGeneration {
    Minimum,
    Override,
    Removed,
}
struct PolicySuite(PolicyGeneration);
impl TestSuite for PolicySuite {
    fn name(&self) -> &str {
        "downstream_tls_defaults"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let generation = self.0;
        vec![TestCase::new(
            "negotiated_defaults",
            "Minimum and exact cipher defaults, per-identity replacement and removal",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result: Result<()> = async {
                        for (host, local) in [
                            ("ecdh-default.sandbox.example.com", false),
                            ("ecdh-edgion.sandbox.example.com", true),
                        ] {
                            ensure!(
                                policy_handshake(&ctx, host, false, true).await?,
                                "TLS1.3 positive control failed: {host}"
                            );
                            for aes256 in [false, true] {
                                let expected = match generation {
                                    PolicyGeneration::Minimum => local && !aes256,
                                    PolicyGeneration::Override => aes256 == local,
                                    PolicyGeneration::Removed => true,
                                };
                                ensure!(
                                    policy_handshake(&ctx, host, aes256, false).await? == expected,
                                    "{host} AES{} policy mismatch",
                                    if aes256 { 256 } else { 128 }
                                );
                            }
                        }
                        Ok(())
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(45))]
    }
}
fn policy() -> Box<dyn TestSuite> {
    Box::new(PolicySuite(PolicyGeneration::Minimum))
}
fn policy_override() -> Box<dyn TestSuite> {
    Box::new(PolicySuite(PolicyGeneration::Override))
}
fn policy_removed() -> Box<dyn TestSuite> {
    Box::new(PolicySuite(PolicyGeneration::Removed))
}

async fn retained_http(stream: &mut TlsStream, expected_status: u16) -> Result<()> {
    timeout(
        IO_TIMEOUT,
        stream.write_all(
            b"HEAD /health HTTP/1.1\r\nHost: ecdh-default.sandbox.example.com\r\nConnection: keep-alive\r\n\r\n",
        ),
    )
    .await??;
    let mut headers = Vec::new();
    timeout(IO_TIMEOUT, async {
        while !headers.ends_with(b"\r\n\r\n") {
            ensure!(headers.len() < 32 * 1024, "response headers exceed test bound");
            headers.push(stream.read_u8().await?);
        }
        anyhow::Ok(())
    })
    .await??;
    let line = std::str::from_utf8(&headers)?
        .lines()
        .next()
        .context("missing HTTP status")?;
    ensure!(
        line.split_whitespace().nth(1) == Some(expected_status.to_string().as_str()),
        "retained TLS response: {line}"
    );
    Ok(())
}

struct CurveSuite(Generation);
impl TestSuite for CurveSuite {
    fn name(&self) -> &str {
        "DownstreamEcdhCurves"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let generation = self.0;
        vec![TestCase::new(
            "curve_negotiation",
            "TLS1.2/1.3 curves obey the same EGC generation for both certificate sources",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match assert_generation(&ctx, generation).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(45))]
    }
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(CurveSuite(Generation::Initial))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(CurveSuite(Generation::Updated))
}
fn rejected_update() -> Box<dyn TestSuite> {
    Box::new(CurveSuite(Generation::RejectedUpdate))
}
fn removed() -> Box<dyn TestSuite> {
    Box::new(CurveSuite(Generation::Removed))
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "policy",
        factory: policy,
    },
    PhaseSuite {
        name: "override",
        factory: policy_override,
    },
    PhaseSuite {
        name: "policy_removed",
        factory: policy_removed,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
    PhaseSuite {
        name: "rejected_update",
        factory: rejected_update,
    },
    PhaseSuite {
        name: "removed",
        factory: removed,
    },
];
async fn restore(ctx: &ScenarioCtx) -> Result<()> {
    ctx.apply_dir("bootstrap").await?;
    ctx.apply_dir("initial").await?;
    ctx.wait_phase(SCENARIO, "initial", CONVERGENCE).await
}
fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            ctx.apply_dir("bootstrap").await?;
            restore(&ctx).await?;
            let mut retained = handshake(&ctx.test_ctx, "ecdh-default.sandbox.example.com", false, true).await??;
            retained_http(&mut retained, 200).await?;
            ctx.apply_dir("updates").await?;
            ctx.wait_phase(SCENARIO, "updated", CONVERGENCE).await?;
            retained_http(&mut retained, 200).await?;
            let rejected = ctx
                .apply_dir("updates_grouped/invalid")
                .await
                .err()
                .context("malformed EGC update must fail admission")?;
            ensure!(
                rejected.chain().any(|cause| cause
                    .to_string()
                    .starts_with("Request failed with status 400 Bad Request:")),
                "expected HTTP400 admission rejection, got {rejected:#}"
            );
            ctx.wait_phase(SCENARIO, "rejected_update", CONVERGENCE).await?;
            retained_http(&mut retained, 200).await?;
            ctx.apply_dir("updates_grouped/removed").await?;
            ctx.wait_phase(SCENARIO, "removed", CONVERGENCE).await?;
            retained_http(&mut retained, 200).await?;
            for phase in ["policy", "override", "policy_removed"] {
                ctx.apply_dir(&format!("updates_grouped/{phase}")).await?;
                ctx.wait_phase(SCENARIO, phase, CONVERGENCE).await?;
                retained_http(&mut retained, 200).await?;
            }
            Ok(())
        }
        .await;
        let restored = restore(&ctx).await;
        match (result, restored) {
            (Ok(()), restore) => restore,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(restore)) => Err(anyhow::anyhow!("{error:#}; restoration failed: {restore:#}")),
        }
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "DownstreamEcdhCurves",
    conf_path: "Gateway/DownstreamEcdhCurves/DynamicTest",
    run,
    phases: PHASES,
};
