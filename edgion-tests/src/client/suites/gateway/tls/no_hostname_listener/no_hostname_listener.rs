use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct GatewayTlsNoHostnameListenerTestSuite;

impl GatewayTlsNoHostnameListenerTestSuite {
    fn test_tls_listener_without_hostname_serves_as_catchall() -> TestCase {
        TestCase::new(
            "tls_listener_without_hostname_serves_as_catchall",
            "Gateway TLS listener without hostname should serve as catch-all fallback",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let host = ctx.http_host.as_deref().unwrap_or(&ctx.target_host);
                    let url = format!("https://{}:{}/health", host, ctx.https_port);

                    match ctx.http_client.get(&url).send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(err) => TestResult::failed(
                            start.elapsed(),
                            format!("TLS handshake or request failed for catch-all listener: {}", err),
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
        vec![Self::test_tls_listener_without_hostname_serves_as_catchall()]
    }
}
