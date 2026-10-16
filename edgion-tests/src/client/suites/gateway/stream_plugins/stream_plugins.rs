// Gateway StreamPlugins (ConnectionFilter) Test Suite
//
// Tests the ConnectionFilter feature that blocks/allows TCP connections
// before TLS handshake or HTTP processing, using EdgionStreamPlugins resources.
//
// Required config files (in edgion-tests/integration/conf/Gateway/StreamPlugins/):
// - 01_EdgionStreamPlugins.yaml           # deny-all-ips resource
// - 02_EdgionStreamPlugins_allow.yaml     # allow-all-ips resource
// - 03_Gateway_allowed.yaml               # no-filter control Gateway
// - 04_Gateway_denied.yaml                # bare-name deny Gateway
// - 05_Gateway_qualified_name_rejected.yaml # qualified reference must fail closed
// - HTTPRoute.yaml                        # Route for no-filter gateway
// - TCPRoute_denied.yaml                  # Route for denied gateway
// - TCPRoute_qualified_name_rejected.yaml # Route for qualified-name Gateway
// - 06_Gateway_bare_name.yaml             # bare-name allow gateway (port 31269)
// - 07_TCPRoute_bare_name.yaml            # Route for bare-name gateway
//
// Port allocation (from ports.json "Gateway/StreamPlugins"):
// - 31270: HTTP (no ConnectionFilter, control group)
// - 31271: TCP (deny-all ConnectionFilter)
// - 31272: TCP (qualified Gateway reference rejected, fail-close)
// - 31269: TCP (allow-all ConnectionFilter, bare name -> namespace normalization)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};
use std::{future::Future, io};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct StreamPluginsTestSuite;

/// Observe accepted connections closing promptly without application data.
/// Closure is consistent with rejection, but does not identify the closing owner.
async fn observe_rejections<F, C, S>(
    mut connect: F,
    attempts: usize,
    connect_timeout: Duration,
    close_timeout: Duration,
) -> Result<(), String>
where
    F: FnMut() -> C,
    C: Future<Output = io::Result<S>>,
    S: AsyncRead + Unpin,
{
    let mut closed_count = 0;
    let mut first_failure = None;
    for attempt in 1..=attempts {
        let observation = match tokio::time::timeout(connect_timeout, connect()).await {
            Ok(Ok(mut stream)) => {
                // Stage1 closes before application input. Read directly so a
                // write failure cannot hide application bytes already queued.
                let mut buf = [0u8; 16];
                match tokio::time::timeout(close_timeout, stream.read(&mut buf)).await {
                    Ok(Ok(0)) => Ok(()),
                    Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionReset => Ok(()),
                    Ok(Ok(_)) => Err("received application data".to_string()),
                    Ok(Err(error)) => Err(format!("close observation failed ({:?})", error.kind())),
                    Err(_) => Err(format!("established connection did not close within {close_timeout:?}")),
                }
            }
            Ok(Err(error)) => Err(format!("TCP establishment failed ({:?})", error.kind())),
            Err(_) => Err(format!("TCP establishment timed out after {connect_timeout:?}")),
        };
        match observation {
            Ok(()) => closed_count += 1,
            Err(failure) if first_failure.is_none() => {
                first_failure = Some(format!("attempt {attempt}: {failure}"));
            }
            Err(_) => {}
        }
    }
    match first_failure {
        Some(failure) => Err(format!(
            "Only {closed_count}/{attempts} established connections closed without application data; {failure}"
        )),
        None => Ok(()),
    }
}

impl StreamPluginsTestSuite {
    /// Test: HTTP listener without ConnectionFilter should work normally
    fn test_http_no_filter() -> TestCase {
        TestCase::new(
            "http_no_connection_filter",
            "HTTP listener without ConnectionFilter should accept requests normally",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder()
                        .no_proxy()
                        .timeout(Duration::from_secs(5))
                        .build()
                        .unwrap();
                    let url = format!("http://{}:{}/health", ctx.target_host, ctx.http_port);

                    let mut request = client.get(&url);
                    if let Some(host) = &ctx.http_host {
                        request = request.header("host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            if response.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "HTTP request accepted (no ConnectionFilter)".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 2xx, got {}", response.status()))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: TCP connection to deny-all gateway should be rejected at TCP level
    fn test_tcp_denied() -> TestCase {
        TestCase::new(
            "tcp_connection_denied_by_filter",
            "TCP connection to deny-all gateway should be rejected immediately",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, ctx.tcp_port);

                    match observe_rejections(
                        || TcpStream::connect(&addr),
                        1,
                        Duration::from_secs(3),
                        Duration::from_secs(2),
                    )
                    .await
                    {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Deny-all target: established connection closed without application data".to_string(),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), format!("Deny-all target: {error}")),
                    }
                })
            },
        )
    }

    /// Test: a qualified Gateway-level StreamPlugins reference must fail closed.
    fn test_tcp_qualified_name_rejected() -> TestCase {
        TestCase::new(
            "tcp_connection_rejected_for_qualified_gateway_reference",
            "Qualified Gateway stream-plugins reference should fail closed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, ctx.tcp_filtered_port);
                    match observe_rejections(
                        || TcpStream::connect(&addr),
                        1,
                        Duration::from_secs(3),
                        Duration::from_secs(2),
                    )
                    .await
                    {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Qualified Gateway reference: established connection closed without application data"
                                .to_string(),
                        ),
                        Err(error) => {
                            TestResult::failed(start.elapsed(), format!("Qualified Gateway reference: {error}"))
                        }
                    }
                })
            },
        )
    }

    /// Test: a Gateway whose stream-plugins annotation uses the BARE name
    /// (no namespace) must normalize it with the Gateway's own namespace,
    /// resolve the allow-all resource, and pass connections through.
    /// If the bare-name normalization branch regresses, the listener
    /// fail-closes and this allow-through assertion fails.
    fn test_tcp_bare_name_allowed() -> TestCase {
        TestCase::new(
            "tcp_connection_allowed_by_bare_name_filter",
            "Bare-name stream-plugins annotation should normalize and pass connections through",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // This listener is specific to this suite, so its port lives in
                    // ports.json rather than on the shared TestContext.
                    let bare_port = match crate::port_config::PortConfig::load() {
                        Ok(cfg) => cfg
                            .get_ports("Gateway/StreamPlugins")
                            .ok()
                            .and_then(|ports| ports.tcp_bare),
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to load ports.json: {}", e));
                        }
                    };
                    let Some(bare_port) = bare_port else {
                        return TestResult::failed(
                            start.elapsed(),
                            "ports.json Gateway/StreamPlugins.tcp_bare is not set".to_string(),
                        );
                    };
                    let addr = format!("{}:{}", ctx.target_host, bare_port);
                    let test_data = b"Hello BareName StreamPlugins";

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }
                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await {
                                Ok(Ok(n)) if n > 0 => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Bare-name annotation resolved; connection passed through ({} bytes echoed)",
                                        n
                                    ),
                                ),
                                Ok(Ok(_)) => TestResult::failed(
                                    start.elapsed(),
                                    "Connection closed immediately (0 bytes) — bare name may have fail-closed"
                                        .to_string(),
                                ),
                                Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("Read failed: {}", e)),
                                Err(_) => TestResult::failed(start.elapsed(), "Read timed out".to_string()),
                            }
                        }
                        Ok(Err(e)) => TestResult::failed(
                            start.elapsed(),
                            format!("Connection refused (bare name likely fail-closed): {}", e),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "Connection timed out (bare name likely fail-closed)".to_string(),
                        ),
                    }
                })
            },
        )
    }

    /// Test: Multiple rapid connections to denied gateway should all be rejected
    fn test_tcp_denied_multiple() -> TestCase {
        TestCase::new(
            "tcp_multiple_denied_connections",
            "Multiple rapid TCP connections to deny-all gateway should all be rejected",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, ctx.tcp_port);
                    match observe_rejections(
                        || TcpStream::connect(&addr),
                        5,
                        Duration::from_secs(2),
                        Duration::from_secs(1),
                    )
                    .await
                    {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Deny-all target: all 5/5 established connections closed without application data"
                                .to_string(),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), format!("Deny-all target: {error}")),
                    }
                })
            },
        )
    }
}

impl TestSuite for StreamPluginsTestSuite {
    fn name(&self) -> &str {
        "Gateway StreamPlugins (ConnectionFilter)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_http_no_filter(),
            Self::test_tcp_denied(),
            Self::test_tcp_qualified_name_rejected(),
            Self::test_tcp_bare_name_allowed(),
            Self::test_tcp_denied_multiple(),
        ]
    }
}

#[cfg(test)]
mod rejection_observation_tests {
    use super::*;
    use std::{
        collections::VecDeque,
        future,
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::ReadBuf;

    enum ReadStep {
        Eof,
        Data,
        Error(io::ErrorKind),
        Pending,
    }

    struct ScriptedRead(VecDeque<ReadStep>);

    impl ScriptedRead {
        fn new(steps: impl IntoIterator<Item = ReadStep>) -> Self {
            Self(steps.into_iter().collect())
        }
    }

    impl AsyncRead for ScriptedRead {
        fn poll_read(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            match self.0.pop_front().expect("unexpected extra read") {
                ReadStep::Eof => Poll::Ready(Ok(())),
                ReadStep::Data => {
                    buf.put_slice(b"x");
                    Poll::Ready(Ok(()))
                }
                ReadStep::Error(kind) => Poll::Ready(Err(io::Error::from(kind))),
                ReadStep::Pending => {
                    self.0.push_front(ReadStep::Pending);
                    Poll::Pending
                }
            }
        }
    }

    const WINDOW: Duration = Duration::from_secs(1);

    #[tokio::test]
    async fn established_eof_and_reset_are_accepted() {
        for close in [ReadStep::Eof, ReadStep::Error(io::ErrorKind::ConnectionReset)] {
            let mut stream = Some(ScriptedRead::new([close]));
            assert!(
                observe_rejections(|| future::ready(Ok(stream.take().unwrap())), 1, WINDOW, WINDOW)
                    .await
                    .is_ok()
            );
        }
    }

    #[tokio::test]
    async fn data_before_eof_or_reset_fails() {
        for close in [ReadStep::Eof, ReadStep::Error(io::ErrorKind::ConnectionReset)] {
            let mut stream = Some(ScriptedRead::new([ReadStep::Data, close]));
            let error = observe_rejections(|| future::ready(Ok(stream.take().unwrap())), 1, WINDOW, WINDOW)
                .await
                .unwrap_err();
            assert_eq!(
                error,
                "Only 0/1 established connections closed without application data; attempt 1: received application data"
            );
        }
    }

    #[tokio::test]
    async fn unrelated_read_errors_fail() {
        for kind in [
            io::ErrorKind::Other,
            io::ErrorKind::TimedOut,
            io::ErrorKind::UnexpectedEof,
        ] {
            let error = observe_rejections(
                || future::ready(Ok(ScriptedRead::new([ReadStep::Error(kind)]))),
                1,
                WINDOW,
                WINDOW,
            )
            .await
            .unwrap_err();
            assert_eq!(error, format!(
                "Only 0/1 established connections closed without application data; attempt 1: close observation failed ({kind:?})"
            ));
        }
    }

    #[tokio::test]
    async fn all_establishment_errors_including_reset_fail() {
        for kind in [
            io::ErrorKind::ConnectionRefused,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::Other,
        ] {
            let error = observe_rejections(
                || future::ready(Err::<ScriptedRead, _>(io::Error::from(kind))),
                1,
                WINDOW,
                WINDOW,
            )
            .await
            .unwrap_err();
            assert_eq!(error, format!(
                "Only 0/1 established connections closed without application data; attempt 1: TCP establishment failed ({kind:?})"
            ));
        }
    }

    #[tokio::test]
    async fn pending_establishment_hits_its_deadline() {
        let error = observe_rejections(future::pending::<io::Result<ScriptedRead>>, 1, Duration::ZERO, WINDOW)
            .await
            .unwrap_err();
        assert_eq!(error,
            "Only 0/1 established connections closed without application data; attempt 1: TCP establishment timed out after 0ns"
        );
    }

    #[tokio::test]
    async fn established_idle_connection_hits_close_deadline() {
        let error = observe_rejections(
            || future::ready(Ok(ScriptedRead::new([ReadStep::Pending]))),
            1,
            WINDOW,
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert_eq!(error,
            "Only 0/1 established connections closed without application data; attempt 1: established connection did not close within 0ns"
        );
    }

    #[tokio::test]
    async fn repeated_observation_runs_every_attempt_after_failure() {
        let mut observations = VecDeque::from([
            Err(io::Error::from(io::ErrorKind::ConnectionRefused)),
            Ok(ScriptedRead::new([ReadStep::Eof])),
            Ok(ScriptedRead::new([ReadStep::Data])),
            Ok(ScriptedRead::new([ReadStep::Error(io::ErrorKind::ConnectionReset)])),
            Err(io::Error::from(io::ErrorKind::Other)),
        ]);
        let mut attempts = 0;
        let error = observe_rejections(
            || {
                attempts += 1;
                future::ready(observations.pop_front().expect("too many attempts"))
            },
            5,
            WINDOW,
            WINDOW,
        )
        .await
        .unwrap_err();
        assert_eq!(attempts, 5);
        assert!(observations.is_empty());
        assert_eq!(error,
            "Only 2/5 established connections closed without application data; attempt 1: TCP establishment failed (ConnectionRefused)"
        );
    }

    #[tokio::test]
    async fn repeated_observation_requires_five_successes() {
        let mut attempts = 0;
        let result = observe_rejections(
            || {
                attempts += 1;
                future::ready(Ok(ScriptedRead::new([ReadStep::Eof])))
            },
            5,
            WINDOW,
            WINDOW,
        )
        .await;
        assert_eq!(attempts, 5);
        assert_eq!(result, Ok(()));
    }
}
