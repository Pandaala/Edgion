use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct GatewayTlsNoHostnameListenerTestSuite;

impl GatewayTlsNoHostnameListenerTestSuite {
    fn test_tls_listener_without_hostname_is_not_used() -> TestCase {
        TestCase::new(
            "tls_listener_without_hostname_is_not_used",
            "Gateway TLS listener without hostname should not be used for cert selection",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let host = ctx.http_host.as_deref().unwrap_or(&ctx.target_host);
                    let url = format!("https://{}:{}/health", host, ctx.https_port);

                    match ctx.http_client.get(&url).send().await {
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected TLS handshake to fail for listener without hostname, got status {}",
                                response.status()
                            ),
                        ),
                        Err(err) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Listener without hostname was rejected during TLS setup: {}", err),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GatewayTlsNoHostnameListenerTestSuite {
    fn name(&self) -> &str {
        "GatewayTlsNoHostnameListener"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_tls_listener_without_hostname_is_not_used()]
    }
}
