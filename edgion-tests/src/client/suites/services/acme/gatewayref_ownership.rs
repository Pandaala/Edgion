use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const PORT_A: u16 = 31410;
const PORT_B: u16 = 31411;
const HOST: &str = "acme-ownership.example.com";
const PATH: &str = "/.well-known/acme-challenge/ownership-token";
const KEY_AUTHORIZATION: &str = "ownership-token.authorization";
const ACME_NS: &str = "edgion-test";
const ACME_NAME: &str = "acme-gatewayref-ownership";
const EXPIRED_AT: u64 = 1;

fn gateway_admin_url(ctx: &TestContext) -> String {
    std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host))
        .trim_end_matches('/')
        .to_owned()
}

async fn gateway_has_expired_challenge(ctx: &TestContext, client: &reqwest::Client) -> Result<(), String> {
    let url = format!(
        "{}/configclient/EdgionAcme?namespace={ACME_NS}&name={ACME_NAME}",
        gateway_admin_url(ctx)
    );
    let mut request = client.get(url);
    let gateway_token = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").ok();
    if let Some(token) = gateway_token.as_deref().or(ctx.admin_token.as_deref()) {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let body: serde_json::Value = response.json().await.map_err(|error| error.to_string())?;
    if body["success"] != true {
        return Err(format!("gateway EdgionAcme snapshot missing: {body}"));
    }
    let expire_at = body["data"]["status"]["activeChallenges"]
        .as_array()
        .and_then(|challenges| challenges.first())
        .and_then(|challenge| challenge["expireAt"].as_u64());
    if expire_at != Some(EXPIRED_AT) {
        return Err(format!("gateway snapshot is not the expired recreation: {body}"));
    }
    Ok(())
}

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
            "HTTP-01 serving follows the selected Gateway listener and stops after revocation or expiry",
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
                    let http_ok = match phase {
                        AcmeGatewayRefOwnershipPhase::SelectedA => a_serves && status_b == 404,
                        AcmeGatewayRefOwnershipPhase::SelectedB => status_a == 404 && b_serves,
                        AcmeGatewayRefOwnershipPhase::Removed | AcmeGatewayRefOwnershipPhase::Expired => {
                            status_a == 404 && status_b == 404
                        }
                    };
                    if let AcmeGatewayRefOwnershipPhase::Expired = phase {
                        if let Err(error) = gateway_has_expired_challenge(&ctx, &client).await {
                            return TestResult::failed(started.elapsed(), error);
                        }
                    }
                    if http_ok {
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
