// TLSRoute policies use tlsRoutePlugins in both termination modes.
// Transport failures and timeouts are failures, never successful policy denials.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::make_tls_connector;
use anyhow::{Context, Result};
use rustls::pki_types::ServerName;
use std::io::ErrorKind;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TlsStreamPluginsTestSuite;

fn is_policy_close(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe
    )
}

/// Returns true only for the exact echo/HTTP status, false for a peer closure.
/// A healthy control route must be probed separately when expecting denial.
pub(crate) async fn probe_tls_policy(
    ctx: &TestContext,
    port: u16,
    hostname: &'static str,
    passthrough: bool,
) -> Result<bool> {
    let tcp = tokio::time::timeout(
        Duration::from_secs(3),
        TcpStream::connect((ctx.target_host.as_str(), port)),
    )
    .await
    .context("TCP connect timed out")?
    .context("TCP connect failed")?;
    let connector = make_tls_connector();
    let sni = ServerName::try_from(hostname)?;
    let mut tls = match tokio::time::timeout(Duration::from_secs(3), connector.connect(sni, tcp)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) if is_policy_close(&error) => return Ok(false),
        Ok(Err(error)) => return Err(error).context("TLS handshake failed without a policy closure"),
        Err(_) => anyhow::bail!("TLS handshake timed out"),
    };
    let payload = if passthrough {
        b"GET /health HTTP/1.1\r\nHost: tls-policy-probe\r\nConnection: close\r\n\r\n".as_slice()
    } else {
        b"tls-policy-echo".as_slice()
    };
    match tokio::time::timeout(Duration::from_secs(3), tls.write_all(payload)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) if is_policy_close(&error) => return Ok(false),
        Ok(Err(error)) => return Err(error).context("TLS write failed"),
        Err(_) => anyhow::bail!("TLS write timed out"),
    }
    let expected = if passthrough {
        b"HTTP/1.1 200".as_slice()
    } else {
        payload
    };
    let mut received = vec![0; expected.len()];
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut offset = 0;
        while offset < received.len() {
            match tls.read(&mut received[offset..]).await {
                Ok(0) if offset == 0 => return Ok(false),
                Ok(0) => anyhow::bail!("TLS response closed after a partial payload"),
                Ok(count) => offset += count,
                Err(error) if offset == 0 && is_policy_close(&error) => return Ok(false),
                Err(error) => return Err(error).context("TLS read failed"),
            }
        }
        anyhow::ensure!(received == expected, "unexpected TLS response: {received:?}");
        Ok(true)
    })
    .await
    .context("TLS read timed out")?
}

impl TlsStreamPluginsTestSuite {
    fn case(name: &'static str, port: u16, hostname: &'static str, allowed: bool) -> TestCase {
        TestCase::new(
            name,
            "TLSRoute Stage-2 admission with a healthy TLS echo control",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        anyhow::ensure!(
                            probe_tls_policy(&ctx, 31282, "test.sp-allow.example.com", false).await?,
                            "control TLS echo was denied"
                        );
                        anyhow::ensure!(
                            probe_tls_policy(&ctx, port, hostname, false).await? == allowed,
                            "TLSRoute policy did not produce the expected admission result"
                        );
                        Ok::<_, anyhow::Error>(())
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
    }
}

impl TestSuite for TlsStreamPluginsTestSuite {
    fn name(&self) -> &str {
        "TLSRoute StreamPlugins"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::case(
                "tls_stream_plugin_allow_localhost",
                31282,
                "test.sp-allow.example.com",
                true,
            ),
            Self::case("tls_stream_plugin_deny_all", 31283, "test.sp-deny.example.com", false),
            Self::case("tls_route_plugins_deny", 31287, "test.sp-route-deny.example.com", false),
            Self::case("tls_stream_plugin_short_name", 31283, "test.sp-deny.example.com", false),
        ]
    }
}
