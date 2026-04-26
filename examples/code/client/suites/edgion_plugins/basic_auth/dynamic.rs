// BasicAuth dynamic hot-reload test suites.
//
// Initial: alice 200, bob 200
// Update:  alice 200, bob 401

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::time::Instant;

const HOST: &str = "basic-auth-dynamic.example.com";

fn basic(u: &str, p: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{}:{}", u, p).as_bytes()))
}

async fn do_request(ctx: &TestContext, user: &str, pass: &str) -> reqwest::Result<u16> {
    // Hard-code the EdgionPlugins listener port (matches dsl/dynamic.rs).
    // ctx.http_url() defaults to the test_server backend (30001) under
    // run_dynamic_scenario, which would bypass the gateway entirely.
    let url = format!("http://{}:31180/health", ctx.target_host);
    ctx.http_client
        .get(&url)
        .header("host", HOST)
        .header("Authorization", basic(user, pass))
        .send()
        .await
        .map(|r| r.status().as_u16())
}

// ============================================================
// Initial phase: alice 200, bob 200
// ============================================================

pub struct BasicAuthDynamicInitialTestSuite;

impl BasicAuthDynamicInitialTestSuite {
    fn alice_200() -> TestCase {
        TestCase::new(
            "basic_auth_dyn_initial_alice_200",
            "[INITIAL] alice credentials → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match do_request(&ctx, "alice", "alice-password").await {
                        Ok(200) => TestResult::passed(start.elapsed()),
                        Ok(s) => TestResult::failed(start.elapsed(), format!("expected 200 got {}", s)),
                        Err(e) => TestResult::failed(start.elapsed(), format!("req err: {}", e)),
                    }
                })
            },
        )
    }

    fn bob_200() -> TestCase {
        TestCase::new(
            "basic_auth_dyn_initial_bob_200",
            "[INITIAL] bob credentials → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match do_request(&ctx, "bob", "bob-secret").await {
                        Ok(200) => TestResult::passed(start.elapsed()),
                        Ok(s) => TestResult::failed(start.elapsed(), format!("expected 200 got {}", s)),
                        Err(e) => TestResult::failed(start.elapsed(), format!("req err: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait::async_trait]
impl TestSuite for BasicAuthDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "BasicAuth Dynamic - Initial Phase"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::alice_200(), Self::bob_200()]
    }
}

// ============================================================
// Update phase: alice 200, bob 401
// ============================================================

pub struct BasicAuthDynamicUpdateTestSuite;

impl BasicAuthDynamicUpdateTestSuite {
    fn alice_200() -> TestCase {
        TestCase::new(
            "basic_auth_dyn_update_alice_200",
            "[UPDATE] alice still → 200 after bob removed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match do_request(&ctx, "alice", "alice-password").await {
                        Ok(200) => TestResult::passed(start.elapsed()),
                        Ok(s) => TestResult::failed(start.elapsed(), format!("expected 200 got {}", s)),
                        Err(e) => TestResult::failed(start.elapsed(), format!("req err: {}", e)),
                    }
                })
            },
        )
    }

    fn bob_401() -> TestCase {
        TestCase::new(
            "basic_auth_dyn_update_bob_401",
            "[UPDATE] bob → 401 after secretRef removed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match do_request(&ctx, "bob", "bob-secret").await {
                        Ok(401) => TestResult::passed(start.elapsed()),
                        Ok(s) => TestResult::failed(start.elapsed(), format!("expected 401 (bob removed) got {}", s)),
                        Err(e) => TestResult::failed(start.elapsed(), format!("req err: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait::async_trait]
impl TestSuite for BasicAuthDynamicUpdateTestSuite {
    fn name(&self) -> &str {
        "BasicAuth Dynamic - Update Phase"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::alice_200(), Self::bob_401()]
    }
}
