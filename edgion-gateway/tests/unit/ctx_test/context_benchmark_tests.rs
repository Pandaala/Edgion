use super::*;
use std::hint::black_box;

const ITERATIONS: usize = 250_000;

fn run_scenario(label: &str, mut scenario: impl FnMut()) {
    let started = std::time::Instant::now();
    for _ in 0..ITERATIONS {
        scenario();
    }
    let elapsed = started.elapsed();
    let nanos_per_op = elapsed.as_nanos() / ITERATIONS as u128;
    eprintln!("context-benchmark {label}: {nanos_per_op} ns/op ({ITERATIONS} iterations)");
}

/// Internal, dependency-free request-context microbenchmark.
///
/// Run on the target architecture with:
/// `cargo test -p edgion-gateway --release benchmark_http_context_scenarios -- --ignored --nocapture`
#[test]
#[ignore = "manual performance benchmark"]
fn benchmark_http_context_scenarios() {
    eprintln!(
        "context-benchmark layout: {} bytes",
        std::mem::size_of::<EdgionHttpContext>()
    );
    run_scenario("default", || {
        black_box(EdgionHttpContext::new());
    });
    run_scenario("hot-plugin-latch", || {
        let mut ctx = EdgionHttpContext::new();
        ctx.terminate_plugin_execution();
        black_box(ctx);
    });
    run_scenario("sparse-plugin-state", || {
        let mut ctx = EdgionHttpContext::new();
        black_box(ctx.claim_json_schema_validation());
        black_box(ctx);
    });

    let owner = Arc::new(());
    run_scenario("selected-waf", || {
        let mut ctx = EdgionHttpContext::new();
        black_box(ctx.select_waf(Some(WafSelection::new(
            Arc::clone(&owner),
            WafLogSource::Route,
            0,
            WafLogMode::On,
        ))));
        black_box(ctx);
    });
}
