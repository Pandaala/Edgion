// BasicAuth dynamic hot-reload test suites.
//
// Initial: alice 200, bob 200
// Update:  alice 200, bob 401
//
// Grouped variant (initial_grouped / updates_grouped):
// Initial: alice (partner-a) 200, bob (partner-b) 200
// Update:  alice (partner-a) 200, bob (partner-b) 401 (partner-b group removed)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::time::Instant;

const HOST: &str = "basic-auth-dynamic.example.com";
const GROUPED_DYNAMIC_HOST: &str = "basic-auth-dynamic-grouped.example.com";

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

// ============================================================
// Grouped initial phase: alice (partner-a) 200, bob (partner-b) 200
// ============================================================

pub struct BasicAuthDynamicGroupedInitialTestSuite;

impl BasicAuthDynamicGroupedInitialTestSuite {
    fn initial_grouped_partner_a() -> TestCase {
        TestCase::new(
            "initial_grouped_partner_a",
            "[INITIAL GROUPED] alice (partner-a) credentials → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let auth = format!("Basic {}", STANDARD.encode(b"alice:dynamic-partner-a-pass" as &[u8]));
                    let url = format!("http://{}:31180/health", ctx.target_host);
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", GROUPED_DYNAMIC_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(r) if r.status().as_u16() == 200 => TestResult::passed(start.elapsed()),
                        Ok(r) => TestResult::failed(start.elapsed(), format!("expected 200 got {}", r.status())),
                        Err(e) => TestResult::failed(start.elapsed(), format!("req err: {}", e)),
                    }
                })
            },
        )
    }

    fn initial_grouped_partner_b() -> TestCase {
        TestCase::new(
            "initial_grouped_partner_b",
            "[INITIAL GROUPED] bob (partner-b) credentials → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let auth = format!("Basic {}", STANDARD.encode(b"bob:dynamic-partner-b-pass" as &[u8]));
                    let url = format!("http://{}:31180/health", ctx.target_host);
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", GROUPED_DYNAMIC_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(r) if r.status().as_u16() == 200 => TestResult::passed(start.elapsed()),
                        Ok(r) => TestResult::failed(start.elapsed(), format!("expected 200 got {}", r.status())),
                        Err(e) => TestResult::failed(start.elapsed(), format!("req err: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait::async_trait]
impl TestSuite for BasicAuthDynamicGroupedInitialTestSuite {
    fn name(&self) -> &str {
        "BasicAuth Dynamic Grouped - Initial Phase"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::initial_grouped_partner_a(), Self::initial_grouped_partner_b()]
    }
}

// ============================================================
// Grouped update phase: alice (partner-a) 200, bob (partner-b) 401
// ============================================================

pub struct BasicAuthDynamicGroupedUpdateTestSuite;

impl BasicAuthDynamicGroupedUpdateTestSuite {
    fn update_grouped_partner_a_still_works() -> TestCase {
        TestCase::new(
            "update_grouped_partner_a_still_works",
            "[UPDATE GROUPED] alice (partner-a) still → 200 after partner-b removed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let auth = format!("Basic {}", STANDARD.encode(b"alice:dynamic-partner-a-pass" as &[u8]));
                    let url = format!("http://{}:31180/health", ctx.target_host);
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", GROUPED_DYNAMIC_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(r) if r.status().as_u16() == 200 => TestResult::passed(start.elapsed()),
                        Ok(r) => TestResult::failed(start.elapsed(), format!("expected 200 got {}", r.status())),
                        Err(e) => TestResult::failed(start.elapsed(), format!("req err: {}", e)),
                    }
                })
            },
        )
    }

    fn update_grouped_partner_b_gone() -> TestCase {
        TestCase::new(
            "update_grouped_partner_b_gone",
            "[UPDATE GROUPED] bob (partner-b) → 401 after partner-b group removed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let auth = format!("Basic {}", STANDARD.encode(b"bob:dynamic-partner-b-pass" as &[u8]));
                    let url = format!("http://{}:31180/health", ctx.target_host);
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", GROUPED_DYNAMIC_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(r) if r.status().as_u16() == 401 => TestResult::passed(start.elapsed()),
                        Ok(r) => {
                            TestResult::failed(start.elapsed(), format!("expected 401 (bob gone) got {}", r.status()))
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("req err: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait::async_trait]
impl TestSuite for BasicAuthDynamicGroupedUpdateTestSuite {
    fn name(&self) -> &str {
        "BasicAuth Dynamic Grouped - Update Phase"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::update_grouped_partner_a_still_works(),
            Self::update_grouped_partner_b_gone(),
        ]
    }
}
