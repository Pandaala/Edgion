//! SpBackendChange scenario phase suites.
//!
//! Initial phase pins SP cookie to backend-a (weights 100/0/0); update deletes
//! backend-a's endpoints and shifts weights to 0/50/50; final phase verifies
//! that the same cookie no longer routes to backend-a (graceful degradation).

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Cross-phase cookie store: initial phase writes, final phase reads.
static SP_COOKIE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn cookie_slot() -> &'static Mutex<Option<String>> {
    SP_COOKIE.get_or_init(|| Mutex::new(None))
}

const HOST: &str = "sp-change.example.com";
const PORT: u16 = 31383;
/// Session name must match the YAML `sessionPersistence.sessionName`.
const SESSION_NAME: &str = "edgion-sp-change";

// ---------------------------------------------------------------------------
// Initial phase suite
// ---------------------------------------------------------------------------

pub struct SpChangeInitialTestSuite;

impl SpChangeInitialTestSuite {
    fn test_initial_pins_to_a() -> TestCase {
        TestCase::new(
            "sp_change_initial_pins_to_a",
            "Initial request should get Set-Cookie and be routed to backend-a (weight=100)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");

                    let trace_id = format!("sp-change-init-{}", uuid::Uuid::new_v4());
                    let url = format!("http://{}:{}/echo", ctx.target_host, PORT);

                    let resp = match client
                        .get(&url)
                        .header("host", HOST)
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Initial request failed: {}", e));
                        }
                    };

                    if resp.status() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    // Extract Set-Cookie header
                    let set_cookie = resp
                        .headers()
                        .get("set-cookie")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());

                    let cookie_header = match set_cookie {
                        Some(ref sc) if sc.starts_with(&format!("{}=", SESSION_NAME)) => sc.clone(),
                        Some(ref sc) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Set-Cookie does not start with '{}=': {}", SESSION_NAME, sc),
                            );
                        }
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                "No Set-Cookie header in initial response".to_string(),
                            );
                        }
                    };

                    // Extract name=value (drop attributes like Path, Max-Age, etc.)
                    let cookie_value = cookie_header
                        .split(';')
                        .next()
                        .unwrap_or(&cookie_header)
                        .trim()
                        .to_string();

                    // Store cookie for final phase
                    {
                        let mut slot = cookie_slot().lock().unwrap();
                        *slot = Some(cookie_value.clone());
                    }

                    // Verify via access log that backend-a served the request
                    let al_client = ctx.access_log_client();
                    match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                        Ok(entry) => {
                            let backend_name = entry
                                .data
                                .get("backend_context")
                                .and_then(|bc| bc.get("rsc"))
                                .and_then(|rsc| rsc.get("name"))
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();

                            if backend_name == "backend-a" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Initial request pinned to backend-a; cookie={}", cookie_value),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected backend-a, got '{}' (cookie={})", backend_name, cookie_value),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Access log lookup failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for SpChangeInitialTestSuite {
    fn name(&self) -> &str {
        "SpChangeInitial"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/SessionPersistenceBackendChange")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_initial_pins_to_a()]
    }
}

// ---------------------------------------------------------------------------
// Final phase suite (polled by wait_phase)
// ---------------------------------------------------------------------------

pub struct SpChangeFinalTestSuite;

impl SpChangeFinalTestSuite {
    fn test_final_routes_to_b_or_c() -> TestCase {
        TestCase::new(
            "sp_change_final_routes_to_b_or_c",
            "After deleting backend-a endpoints, SP cookie requests must not route to backend-a",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Read cookie captured by initial phase
                    let cookie_value = {
                        let slot = cookie_slot().lock().unwrap();
                        match slot.as_ref() {
                            Some(c) => c.clone(),
                            None => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No SP cookie stored from initial phase".to_string(),
                                );
                            }
                        }
                    };

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");

                    let url = format!("http://{}:{}/echo", ctx.target_host, PORT);
                    let al_client = ctx.access_log_client();

                    for i in 0..3u32 {
                        let trace_id = format!("sp-change-final-{}-{}", i, uuid::Uuid::new_v4());

                        let resp = match client
                            .get(&url)
                            .header("host", HOST)
                            .header("cookie", &cookie_value)
                            .header(
                                "traceparent",
                                crate::access_log_client::traceparent_for_marker(&trace_id),
                            )
                            .header("access_log", "test_store")
                            .send()
                            .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Request {} failed: {}", i, e));
                            }
                        };

                        if resp.status() != 200 {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Request {} got status {} (expected 200, no 503)", i, resp.status()),
                            );
                        }

                        match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                            Ok(entry) => {
                                let backend_name = entry
                                    .data
                                    .get("backend_context")
                                    .and_then(|bc| bc.get("rsc"))
                                    .and_then(|rsc| rsc.get("name"))
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                if backend_name == "backend-a" {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Request {} still routed to deleted backend-a", i),
                                    );
                                }
                                if backend_name != "backend-b" && backend_name != "backend-c" {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Request {} routed to unexpected backend '{}'", i, backend_name),
                                    );
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Access log lookup for request {} failed: {}", i, e),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "All 3 requests with SP cookie gracefully routed to backend-b or backend-c".to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for SpChangeFinalTestSuite {
    fn name(&self) -> &str {
        "SpChangeFinal"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/SessionPersistenceBackendChange")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_final_routes_to_b_or_c()]
    }
}
