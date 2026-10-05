//! Engine-only comparison: input is resident memory, with no Gateway capture or disk IO.
//! Compilation is excluded; each timed operation includes transaction setup and drop.
use std::hint::black_box;
use std::time::{Duration, Instant};

use edgion_waf::seclang::*;

struct NoIncludes;
impl IncludeSource for NoIncludes {
    fn load(&mut self, _: IncludeRequest<'_>, _: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

fn generation(subscribers: usize) -> CompiledGeneration {
    let source = (1..=subscribers)
        .map(|id| format!("SecRule ARGS \"@pm hit\" \"id:{id},phase:2,pass,t:lowercase\"\n"))
        .collect::<String>();
    let defaults = [PhaseDefaults::pass(Phase::RequestBody, true, true)];
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap();
    compile_with_includes(
        "json-benchmark",
        source.as_bytes(),
        &mut NoIncludes,
        None,
        context,
        FrontendLimits::default(),
    )
    .unwrap()
    .into_parts()
    .0
}

// Benchmark envelopes only: these values are not production recommendations.
fn limits() -> JsonExecutionLimits {
    JsonExecutionLimits {
        lexical: JsonStreamLimits::default(),
        document: JsonDocumentLimits {
            max_retained_bytes: 16 * 1024 * 1024,
            max_key_bytes: 1024 * 1024,
            max_members: 65_536,
            max_events: 131_072,
            max_path_bytes: 16 * 1024,
            max_depth: 64,
            max_work: 128 * 1024 * 1024,
        },
        plan: JsonPlanLimits {
            max_subscribers: 128,
            max_retained_bytes: 1024 * 1024,
        },
        facts: JsonFactLimits {
            max_facts: 131_072,
            max_retained_bytes: 32 * 1024 * 1024,
            // Includes speculative work for all shared-group pattern mappings.
            max_work: 1024 * 1024 * 1024,
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ResultSnapshot {
    outcome: Result<PhaseOutcome, RuntimeError>,
    next_phase: Option<Phase>,
    body_error: Option<BodyError>,
    // Canonical logical charge, NOT physical memory usage of either executor.
    logical_retained_bytes: usize,
}

fn canonical_error(error: JsonExecutionError) -> RuntimeError {
    match error {
        JsonExecutionError::Runtime(error) => error,
        JsonExecutionError::Body(JsonBodyError::Body(kind)) => RuntimeError::Body { kind },
        error => panic!("independent structured failure is not a comparable runtime result: {error:?}"),
    }
}

fn execute(generation: &CompiledGeneration, body: &[u8], chunk: usize, mode: RuleEngineMode) -> ResultSnapshot {
    let headers = [RequestHeader::new(b"content-type", b"application/json")];
    let head = RequestHead::new(
        b"POST /?q=miss HTTP/1.1",
        b"POST",
        b"/?q=miss",
        b"/?q=miss",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"benchmark",
        &headers,
    );
    let mut tx = Transaction::new_with_modes(
        generation,
        head,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        mode,
    )
    .unwrap();
    tx.execute_request_headers().unwrap();
    let outcome = if chunk == 0 {
        tx.publish_request_body(RequestBody::new(body, BodyProcessorSelection::Auto))
            .and_then(|()| tx.execute_request_body())
    } else {
        let mut collect = || -> Result<PhaseOutcome, JsonExecutionError> {
            tx.start_json_request_body(BodyProcessorSelection::Auto, limits())?;
            for bytes in body.chunks(chunk) {
                tx.feed_json_request_body(JsonStreamInput::Data(bytes))?;
            }
            tx.feed_json_request_body(JsonStreamInput::Complete)?;
            tx.finish_json_request_body()
        };
        collect().map_err(canonical_error)
    };
    ResultSnapshot {
        outcome,
        next_phase: tx.next_phase(),
        body_error: tx.request_body_error(),
        logical_retained_bytes: tx.retained_bytes(),
    }
}

fn cases() -> Vec<(&'static str, Vec<u8>)> {
    let value = "x".repeat(253) + "HIT";
    let wide = format!(
        "{{{}}}",
        (0..256)
            .map(|i| format!("\"k{i:04}\":\"{value}\""))
            .collect::<Vec<_>>()
            .join(",")
    )
    .into_bytes();
    let duplicate = format!(
        "{{{},\"same\":\"miss\"}}",
        (0..512)
            .map(|_| format!("\"same\":\"{value}\""))
            .collect::<Vec<_>>()
            .join(",")
    )
    .into_bytes();
    let mut malformed = wide.clone();
    malformed.pop();
    vec![
        ("wide_hits", wide),
        ("duplicates_last_miss", duplicate),
        ("malformed_tail", malformed),
        (
            "large_scalar",
            format!("{{\"value\":\"{}HIT\"}}", "x".repeat(256 * 1024)).into_bytes(),
        ),
    ]
}

fn percentile(samples: &[Duration], percent: usize) -> u128 {
    // Empirical nearest-rank quantile; small samples do not establish stable tails.
    samples[(samples.len() * percent).div_ceil(100) - 1].as_nanos()
}

fn main() {
    let samples_count = if std::env::var_os("EDGION_WAF_BENCH_SMOKE").is_some() {
        3
    } else {
        31
    };
    println!("schema_version,case,arch,os,source_bytes,subscribers,mode,lane,chunk_bytes,samples,p50_ns,p95_ns,p99_ns,logical_retained_bytes");
    for subscribers in [1, 8, 32] {
        let generation = generation(subscribers);
        for (name, body) in cases() {
            for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                let expected = execute(&generation, &body, 0, mode);
                if name != "malformed_tail" || mode == RuleEngineMode::DetectionOnly {
                    assert!(
                        expected.outcome.is_ok(),
                        "fixture must reach evaluation: {name}, {subscribers}, {expected:?}"
                    );
                } else {
                    assert!(matches!(expected.outcome, Err(RuntimeError::Body { .. })));
                }
                // Validate all compared lanes before publishing any timing for this fixture.
                for chunk in [1024, 64 * 1024] {
                    assert_eq!(
                        execute(&generation, &body, chunk, mode),
                        expected,
                        "{name}, {subscribers}, {mode:?}"
                    );
                }
                for chunk in [0, 1024, 64 * 1024] {
                    for _ in 0..3 {
                        black_box(execute(&generation, black_box(&body), chunk, mode));
                    }
                    let mut samples = Vec::with_capacity(samples_count);
                    for _ in 0..samples_count {
                        let started = Instant::now();
                        black_box(execute(&generation, black_box(&body), chunk, mode));
                        samples.push(Instant::now().saturating_duration_since(started));
                    }
                    samples.sort_unstable();
                    println!(
                        "1,{name},{},{},{},{subscribers},{mode:?},{},{chunk},{samples_count},{},{},{},{}",
                        std::env::consts::ARCH,
                        std::env::consts::OS,
                        body.len(),
                        if chunk == 0 { "complete" } else { "structured" },
                        percentile(&samples, 50),
                        percentile(&samples, 95),
                        percentile(&samples, 99),
                        expected.logical_retained_bytes
                    );
                }
            }
        }
    }
}
