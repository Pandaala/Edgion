use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const PORT_A: u16 = 31410;
const PORT_B: u16 = 31411;
const HOST: &str = "acme-ownership.example.com";
const PATH: &str = "/.well-known/acme-challenge/ownership-token";
const KEY_AUTHORIZATION: &str = "ownership-token.authorization";

#[derive(Clone, Copy)]
pub enum AcmeGatewayRefOwnershipPhase {
    SelectedA,
    SelectedB,
    Removed,
    Expired,
}

pub struct AcmeGatewayRefOwnershipTestSuite {
    phase: AcmeGatewayRefOwnershipPhase,
}

impl AcmeGatewayRefOwnershipTestSuite {
    pub fn new(phase: AcmeGatewayRefOwnershipPhase) -> Self {
        Self { phase }
    }
}

impl TestSuite for AcmeGatewayRefOwnershipTestSuite {
    fn name(&self) -> &str {
        "ACME GatewayRef ownership"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.phase;
        vec![TestCase::new(
            "gatewayref_serving_transition",
            "HTTP-01 serving follows the selected Gateway listener",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let started = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();
                    let request = |port| {
                        client
                            .get(format!("http://{}:{port}{PATH}", ctx.target_host))
                            .header("Host", HOST)
                    };
                    let response_a = match request(PORT_A).send().await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(started.elapsed(), format!("listener A: {error}")),
                    };
                    let status_a = response_a.status();
                    let body_a = response_a.text().await.unwrap_or_default();
                    let response_b = match request(PORT_B).send().await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(started.elapsed(), format!("listener B: {error}")),
                    };
                    let status_b = response_b.status();
                    let body_b = response_b.text().await.unwrap_or_default();

                    let a_serves = status_a.is_success() && body_a == KEY_AUTHORIZATION;
                    let b_serves = status_b.is_success() && body_b == KEY_AUTHORIZATION;
                    let correct = match phase {
                        AcmeGatewayRefOwnershipPhase::SelectedA => a_serves && status_b == 404,
                        AcmeGatewayRefOwnershipPhase::SelectedB => status_a == 404 && b_serves,
                        AcmeGatewayRefOwnershipPhase::Removed | AcmeGatewayRefOwnershipPhase::Expired => {
                            status_a == 404 && status_b == 404
                        }
                    };
                    if correct {
                        TestResult::passed_with_message(
                            started.elapsed(),
                            format!("listener A={status_a}, listener B={status_b}"),
                        )
                    } else {
                        TestResult::failed(
                            started.elapsed(),
                            format!(
                                "unexpected ownership result: A={status_a} body={body_a:?}, B={status_b} body={body_b:?}"
                            ),
                        )
                    }
                })
            },
        )]
    }
}
