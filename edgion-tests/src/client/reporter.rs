// Test report generator

use crate::framework::{SuiteResult, TestResults};
use anyhow::Result;
use chrono::Utc;
use console::style;
use serde::Serialize;
use std::time::Duration;

pub struct ConsoleReporter;

impl Default for ConsoleReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleReporter {
    pub fn new() -> Self {
        Self
    }

    pub fn report(&self, results: &TestResults, total_duration: Duration) {
        println!();
        self.print_suite_results(results);
        println!();
        self.print_summary(results, total_duration);
    }

    fn print_suite_results(&self, results: &TestResults) {
        for suite_result in &results.suite_results {
            self.print_suite(suite_result);
        }
    }

    fn print_suite(&self, suite: &SuiteResult) {
        println!("{} {}", style("▶").cyan().bold(), style(&suite.name).bold());

        for (test_name, test_result) in &suite.test_results {
            let status = if test_result.passed {
                style("✓").green()
            } else {
                style("✗").red()
            };

            println!(
                "  {} {} ({:.2}s)",
                status,
                test_name,
                test_result.duration.as_secs_f64()
            );

            if let Some(msg) = &test_result.message {
                println!("    {}", style(msg).dim());
            }

            if let Some(err) = &test_result.error {
                println!("    {}", style(err).red());
            }
        }

        for error in &suite.lifecycle_errors {
            println!(
                "  {} {} failed: {}",
                style("✗").red(),
                error.phase.as_str(),
                style(&error.message).red()
            );
        }
    }

    fn print_summary(&self, results: &TestResults, total_duration: Duration) {
        println!("{}", style("=".repeat(50)).dim());
        println!("{}", style("Test Summary").bold());
        println!("{}", style("=".repeat(50)).dim());

        println!("Total tests: {}", results.total_tests());
        println!("{}: {}", style("Passed").green(), results.total_passed());
        println!("{}: {}", style("Failed").red(), results.total_failed());
        println!(
            "{}: {}",
            style("Infrastructure errors").red(),
            results.run_errors.len()
                + results
                    .suite_results
                    .iter()
                    .map(|suite| suite.lifecycle_errors.len())
                    .sum::<usize>()
        );
        println!("Passed: {:.1}%", results.pass_rate());
        println!("Total time: {:.2}s", total_duration.as_secs_f64());

        if results.has_failures() {
            for error in &results.run_errors {
                println!("{}", style(error).red());
            }
            println!("\n{}", style("⚠ Some tests failed").red().bold());
        } else {
            println!("\n{}", style("✓ All tests passed").green().bold());
        }
    }
}

/// JSON test report serializer.
pub struct JsonReporter;

impl Default for JsonReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonReporter {
    pub fn new() -> Self {
        Self
    }

    pub fn save_to_file(&self, results: &TestResults, total_duration: Duration, path: &str) -> Result<()> {
        let report = self.build_report(results, total_duration);
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    fn build_report(&self, results: &TestResults, total_duration: Duration) -> JsonReport {
        let suites = results
            .suite_results
            .iter()
            .map(|suite| self.build_suite_report(suite))
            .collect();

        JsonReport {
            timestamp: Utc::now(),
            total_duration: total_duration.as_secs_f64(),
            total_tests: results.total_tests(),
            passed: results.total_passed(),
            failed: results.total_failed(),
            infrastructure_errors: results.run_errors.clone(),
            pass_rate: results.pass_rate(),
            suites,
        }
    }

    fn build_suite_report(&self, suite: &SuiteResult) -> SuiteReport {
        let tests = suite
            .test_results
            .iter()
            .map(|(name, result)| TestReport {
                name: name.clone(),
                status: if result.passed {
                    "passed".to_string()
                } else {
                    "failed".to_string()
                },
                duration: result.duration.as_secs_f64(),
                message: result.message.clone(),
                error: result.error.clone(),
            })
            .collect();

        SuiteReport {
            name: suite.name.clone(),
            passed: suite.passed_count(),
            failed: suite.failed_count(),
            lifecycle_errors: suite
                .lifecycle_errors
                .iter()
                .map(|error| LifecycleErrorReport {
                    phase: error.phase.as_str().to_string(),
                    message: error.message.clone(),
                })
                .collect(),
            duration: suite.duration.as_secs_f64(),
            tests,
        }
    }
}

#[derive(Serialize)]
struct JsonReport {
    timestamp: chrono::DateTime<Utc>,
    total_duration: f64,
    total_tests: usize,
    passed: usize,
    failed: usize,
    infrastructure_errors: Vec<String>,
    pass_rate: f64,
    suites: Vec<SuiteReport>,
}

#[derive(Serialize)]
struct SuiteReport {
    name: String,
    passed: usize,
    failed: usize,
    lifecycle_errors: Vec<LifecycleErrorReport>,
    duration: f64,
    tests: Vec<TestReport>,
}

#[derive(Serialize)]
struct LifecycleErrorReport {
    phase: String,
    message: String,
}

#[derive(Serialize)]
struct TestReport {
    name: String,
    status: String,
    duration: f64,
    message: Option<String>,
    error: Option<String>,
}
