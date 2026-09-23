//! Listener-scoped TLSRoute hostnames on the shared Controller/Gateway topology.
//! Bootstrap listeners and independent controls persist; only the target Route
//! and one Gateway hostname change online. No process restart is required.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::NoCertVerifier;
use anyhow::{ensure, Context, Result};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName};
use serde_json::Value;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const NS: &str = "edgion-test";
const TARGET: &str = "tls-attachments-target";
const PORT_A: u16 = 31975;
const PORT_B: u16 = 31976;
const PHASE_TIMEOUT: Duration = Duration::from_secs(30);
const IO_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
enum State {
    Absent,
    Initial,
    ParentRemoved,
    HostnameRemoved,
    ListenerChanged,
    ListenerRenamed,
    BroadParent,
}

fn connector() -> Result<tokio_rustls::TlsConnector> {
    // Both HTTPS backends belong to test_server; 30052 requires this existing
    // client identity, while 30051 simply does not request it.
    let cert_dir = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
        Path::new("edgion-tests/certs/mtls").to_path_buf()
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("certs/mtls")
    };
    let certs = CertificateDer::pem_file_iter(cert_dir.join("valid-client.crt"))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_file(cert_dir.join("valid-client.key"))?;
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
        .with_client_auth_cert(certs, key)?;
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

/// A denial must be an explicit peer closure during ClientHello processing.
/// Connect errors, timeouts, TLS alerts, and malformed HTTP always fail the test.
async fn backend(
    ctx: &TestContext,
    connector: &tokio_rustls::TlsConnector,
    port: u16,
    hostname: &'static str,
) -> Result<Option<u16>> {
    let tcp = tokio::time::timeout(IO_TIMEOUT, TcpStream::connect((ctx.target_host.as_str(), port)))
        .await
        .context("TCP connect timed out")??;
    let sni = ServerName::try_from(hostname)?;
    let mut tls = match tokio::time::timeout(IO_TIMEOUT, connector.connect(sni, tcp)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error))
            if matches!(
                error.kind(),
                ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe
            ) =>
        {
            return Ok(None);
        }
        Ok(Err(error)) => return Err(error).context("TLS handshake failed without a route closure"),
        Err(_) => anyhow::bail!("TLS handshake timed out"),
    };
    let request = format!("GET /echo HTTP/1.1\r\nHost: {hostname}\r\nConnection: close\r\n\r\n");
    tokio::time::timeout(IO_TIMEOUT, tls.write_all(request.as_bytes()))
        .await
        .context("TLS write timed out")??;
    tokio::time::timeout(IO_TIMEOUT, async {
        let mut response = Vec::new();
        let mut chunk = [0; 1024];
        loop {
            let count = tls.read(&mut chunk).await?;
            ensure!(count != 0, "backend closed before its identity was received");
            ensure!(response.len() + count <= 8192, "backend response exceeded probe cap");
            response.extend_from_slice(&chunk[..count]);
            let response = std::str::from_utf8(&response)?;
            if let Some((headers, body)) = response.split_once("\r\n\r\n") {
                ensure!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "unexpected HTTP status");
                if let Some((identity, _)) = body.split_once('\n') {
                    let port = identity
                        .strip_prefix("Server: 0.0.0.0:")
                        .context("missing exact backend identity")?
                        .parse()?;
                    return Ok(Some(port));
                }
            }
        }
    })
    .await
    .context("backend identity read timed out")?
}

async fn expect_backend(
    ctx: &TestContext,
    connector: &tokio_rustls::TlsConnector,
    port: u16,
    hostname: &'static str,
    expected: Option<u16>,
) -> Result<()> {
    let actual = backend(ctx, connector, port, hostname)
        .await
        .with_context(|| format!("probe {hostname}:{port}"))?;
    ensure!(
        actual == expected,
        "{hostname}:{port}: expected {expected:?}, got {actual:?}"
    );
    Ok(())
}

async fn target_projection(ctx: &TestContext, listener_name: Option<&str>, broad_parent: bool) -> Result<()> {
    let gateway_url = std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host));
    for (base, prefix, token) in [
        (ctx.admin_api_url(), "configserver", ctx.admin_token.clone()),
        (
            gateway_url,
            "configclient",
            std::env::var("EDGION_GATEWAY_ADMIN_TOKEN")
                .ok()
                .or_else(|| ctx.admin_token.clone()),
        ),
    ] {
        let mut request = ctx
            .http_client
            .get(format!("{}/{prefix}/TLSRoute/list", base.trim_end_matches('/')))
            .timeout(IO_TIMEOUT);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let body: Value = request.send().await?.error_for_status()?.json().await?;
        ensure!(body["success"] == true, "resource listing failed");
        let items = body["data"].as_array().context("resource list has no data array")?;
        let target = items
            .iter()
            .find(|item| item["metadata"]["namespace"] == NS && item["metadata"]["name"] == TARGET);
        if broad_parent {
            let parents = target.context("target is not published")?["spec"]["parentRefs"]
                .as_array()
                .context("target has no parents")?;
            ensure!(
                parents
                    .iter()
                    .any(|p| p["name"] == "tls-attachments-a" && p["sectionName"].is_null()),
                "broad parent has not reached {prefix}"
            );
        }
        if let Some(listener_name) = listener_name {
            let attachments = target.context("target is not published")?["spec"]["resolvedListeners"]
                .as_array()
                .context("target has no attachments")?;
            ensure!(
                attachments
                    .iter()
                    .any(|a| a["gatewayName"] == "tls-attachments-a" && a["listenerName"] == listener_name),
                "listener rename has not reached {prefix}"
            );
        } else {
            ensure!(target.is_none(), "target remains in {prefix}");
        }
    }
    Ok(())
}

async fn check(ctx: &TestContext, state: State) -> Result<()> {
    let connector = connector()?;
    // A failed listener/backend cannot masquerade as successful route removal.
    for (port, host, expected) in [
        (PORT_A, "control-a.attachments.example.com", 30051),
        (PORT_B, "control-b.attachments.example.com", 30052),
        (PORT_A, "b.attachments.example.com", 30052),
    ] {
        expect_backend(ctx, &connector, port, host, Some(expected)).await?;
    }
    let a_present = matches!(
        state,
        State::Initial | State::HostnameRemoved | State::ListenerRenamed | State::BroadParent
    );
    let b_present = matches!(
        state,
        State::Initial | State::ParentRemoved | State::ListenerChanged | State::ListenerRenamed | State::BroadParent
    );
    expect_backend(
        ctx,
        &connector,
        PORT_A,
        "a.attachments.example.com",
        a_present.then_some(30051),
    )
    .await?;
    expect_backend(
        ctx,
        &connector,
        PORT_B,
        "b.attachments.example.com",
        b_present.then_some(30051),
    )
    .await?;
    // The wildcard is narrowed by A's exact listener, but remains a wildcard
    // on B's unrestricted listener. The wildcard root never matches.
    for (port, host, expected) in [
        (PORT_A, "one.wild.attachments.example.com", Some(30051)),
        (PORT_A, "two.wild.attachments.example.com", None),
        (PORT_B, "two.wild.attachments.example.com", Some(30051)),
        (PORT_B, "deep.two.wild.attachments.example.com", Some(30051)),
        (PORT_B, "wild.attachments.example.com", None),
        (PORT_B, "x.open.attachments.example.com", Some(30052)),
        (PORT_B, "y.open.attachments.example.com", Some(30052)),
        (PORT_B, "z.open.attachments.example.com", None),
    ] {
        expect_backend(ctx, &connector, port, host, expected).await?;
    }
    if matches!(state, State::Absent) {
        target_projection(ctx, None, false).await?;
    } else if matches!(state, State::ListenerRenamed) {
        target_projection(ctx, Some("renamed"), true).await?;
    } else if matches!(state, State::BroadParent) {
        target_projection(ctx, Some("main"), true).await?;
    } else if matches!(state, State::Initial) {
        target_projection(ctx, Some("main"), false).await?;
    }
    Ok(())
}

struct AttachmentSuite(State);

impl TestSuite for AttachmentSuite {
    fn name(&self) -> &str {
        "TLSRoute attachment hostnames"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let state = self.0;
        vec![TestCase::new(
            "attachment_domains_and_backend_identity",
            "Validate scoped candidates and online removal with independent healthy controls",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check(&ctx, state).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(PHASE_TIMEOUT)]
    }
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(AttachmentSuite(State::Absent))
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(AttachmentSuite(State::Initial))
}
fn parent_removed() -> Box<dyn TestSuite> {
    Box::new(AttachmentSuite(State::ParentRemoved))
}
fn hostname_removed() -> Box<dyn TestSuite> {
    Box::new(AttachmentSuite(State::HostnameRemoved))
}
fn listener_changed() -> Box<dyn TestSuite> {
    Box::new(AttachmentSuite(State::ListenerChanged))
}

fn listener_renamed() -> Box<dyn TestSuite> {
    Box::new(AttachmentSuite(State::ListenerRenamed))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "broad_parent",
        factory: || Box::new(AttachmentSuite(State::BroadParent)),
    },
    PhaseSuite {
        name: "listener_renamed",
        factory: listener_renamed,
    },
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "parent_removed",
        factory: parent_removed,
    },
    PhaseSuite {
        name: "hostname_removed",
        factory: hostname_removed,
    },
    PhaseSuite {
        name: "listener_changed",
        factory: listener_changed,
    },
];

async fn delete_target(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let response = client.delete("TLSRoute", Some(NS), TARGET).await?;
    ensure!(
        response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND,
        "target cleanup failed: {}",
        response.status()
    );
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            // Restore bootstrap state in case a broad loader preloaded updates.
            ctx.apply_dir("../bootstrap").await?;
            delete_target(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", PHASE_TIMEOUT).await?;
            ctx.apply_dir("initial").await?;
            ctx.wait_phase(SCENARIO, "initial", PHASE_TIMEOUT).await?;
            // Keep the broad parent in place while only Gateway listener identity changes.
            ctx.apply_dir("updates/broad-parent").await?;
            ctx.wait_phase(SCENARIO, "broad_parent", PHASE_TIMEOUT).await?;
            ctx.apply_dir("updates/rename").await?;
            ctx.wait_phase(SCENARIO, "listener_renamed", PHASE_TIMEOUT).await?;
            ctx.apply_dir("../bootstrap/Gateway_a.yaml").await?;
            ctx.apply_dir("initial").await?;
            ctx.wait_phase(SCENARIO, "initial", PHASE_TIMEOUT).await?;
            for (update, phase) in [
                ("updates/parent", "parent_removed"),
                ("updates/hostname", "hostname_removed"),
                ("updates/listener", "listener_changed"),
            ] {
                ctx.apply_dir(update).await?;
                ctx.wait_phase(SCENARIO, phase, PHASE_TIMEOUT).await?;
                ctx.apply_dir("../bootstrap/Gateway_a.yaml").await?;
                ctx.apply_dir("initial").await?;
                ctx.wait_phase(SCENARIO, "initial", PHASE_TIMEOUT).await?;
            }
            delete_target(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", PHASE_TIMEOUT).await?;
            ctx.apply_dir("initial").await?;
            ctx.wait_phase(SCENARIO, "initial", PHASE_TIMEOUT).await
        }
        .await;

        let cleanup = async {
            ctx.apply_dir("../bootstrap/Gateway_a.yaml").await?;
            delete_target(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", PHASE_TIMEOUT).await
        }
        .await;
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.context("attachment scenario cleanup failed")),
            (Err(error), Err(cleanup)) => Err(error.context(format!("cleanup also failed: {cleanup:#}"))),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "TlsRouteAttachments",
    conf_path: "TLSRoute/Attachments/DynamicTest",
    run,
    phases: PHASES,
};
