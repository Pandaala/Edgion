//! High-frequency observedGeneration scenario phase suites.
//!
//! Initial phase: Gateway has generation=1; assert observedGeneration matches.
//! Final phase: after 10 rapid updates (generation=2..11), assert observedGeneration
//!              caught up to metadata.generation (called via wait_phase polling).

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

const NS: &str = "edgion-test";
const GW: &str = "observed-gen-highfreq-gateway";

// ---------------------------------------------------------------------------
// Shared assertion helper
// ---------------------------------------------------------------------------

/// Fetch the Gateway and assert every status.conditions[*].observedGeneration
/// and every status.listeners[*].conditions[*].observedGeneration equals
/// metadata.generation. Returns Ok on success, Err with a diagnostic string on
/// failure (suitable for returning from a TestCase closure).
///
/// `min_generation` guards against a false-pass where the controller cache
/// still holds an early version (e.g. gen=1, og=1) — both fields would match
/// and the convergence check alone would succeed before any of the rapid
/// writes have actually been processed. Callers pass the lowest generation
/// they require the cache to have observed (e.g. 11 after 10 iter applies).
async fn assert_generation_converged(ctx: &TestContext, min_generation: i64) -> Result<String, String> {
    let gateway = helpers::fetch_resource_status(ctx, "Gateway", NS, GW).await?;

    let metadata_gen = gateway["metadata"]["generation"].as_i64();
    match metadata_gen {
        Some(g) if g >= min_generation => {}
        Some(g) => {
            return Err(format!(
                "metadata.generation={} has not reached expected minimum {} (controller cache lagging behind file)",
                g, min_generation
            ));
        }
        None => {
            return Err(format!(
                "metadata.generation missing on Gateway; expected >= {}",
                min_generation
            ));
        }
    }

    // Top-level conditions
    let conditions = match gateway["status"]["conditions"].as_array() {
        Some(c) => c.clone(),
        None => {
            return Err(format!(
                "No status.conditions found. status: {}",
                serde_json::to_string_pretty(&gateway["status"]).unwrap_or_default()
            ));
        }
    };

    if conditions.is_empty() {
        return Err("status.conditions is empty".into());
    }

    let mut failures: Vec<String> = Vec::new();

    for c in &conditions {
        let type_ = c["type"].as_str().unwrap_or("<unknown>");
        let og = c["observedGeneration"].as_i64();
        if og != metadata_gen {
            failures.push(format!(
                "condition '{}': observedGeneration={:?}, expected {:?}",
                type_, og, metadata_gen
            ));
        }
    }

    // Per-listener conditions
    if let Some(listeners) = gateway["status"]["listeners"].as_array() {
        for listener in listeners {
            let listener_name = listener["name"].as_str().unwrap_or("<unknown>");
            if let Some(lconds) = listener["conditions"].as_array() {
                for c in lconds {
                    let type_ = c["type"].as_str().unwrap_or("<unknown>");
                    let og = c["observedGeneration"].as_i64();
                    if og != metadata_gen {
                        failures.push(format!(
                            "listener '{}' condition '{}': observedGeneration={:?}, expected {:?}",
                            listener_name, type_, og, metadata_gen
                        ));
                    }
                }
            }
        }
    }

    if failures.is_empty() {
        Ok(format!("All conditions have observedGeneration={:?}", metadata_gen))
    } else {
        Err(failures.join("; "))
    }
}

// ---------------------------------------------------------------------------
// Initial phase suite
// ---------------------------------------------------------------------------

pub struct HighFreqInitialTestSuite;

impl HighFreqInitialTestSuite {
    fn test_initial_generation() -> TestCase {
        TestCase::new(
            "highfreq_initial_generation",
            "Gateway observedGeneration matches metadata.generation after initial apply",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Initial phase only requires the gateway exists with generation>=1
                    // and og caught up to it.
                    match assert_generation_converged(&ctx, 1).await {
                        Ok(msg) => TestResult::passed_with_message(start.elapsed(), msg),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for HighFreqInitialTestSuite {
    fn name(&self) -> &str {
        "HighFreqInitial"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_initial_generation()]
    }
}

// ---------------------------------------------------------------------------
// Final phase suite (polled by wait_phase)
// ---------------------------------------------------------------------------

pub struct HighFreqFinalTestSuite;

/// Final generation expected after the scenario applies iter1..=iter10.
/// initial gen=1, each iter bumps by 1 → final gen=11. Keep in sync with the
/// scenario loop in `dynamic/scenarios/observed_gen_high_freq.rs`.
const FINAL_GEN: i64 = 11;

impl HighFreqFinalTestSuite {
    fn test_final_generation_converged() -> TestCase {
        TestCase::new(
            "highfreq_final_generation_converged",
            "Gateway observedGeneration caught up to metadata.generation after high-frequency updates",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match assert_generation_converged(&ctx, FINAL_GEN).await {
                        Ok(msg) => TestResult::passed_with_message(start.elapsed(), msg),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for HighFreqFinalTestSuite {
    fn name(&self) -> &str {
        "HighFreqFinal"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_final_generation_converged()]
    }
}
