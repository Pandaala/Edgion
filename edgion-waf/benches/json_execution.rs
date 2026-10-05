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

#[derive(Debug, PartialEq, Eq)]
struct ResultSnapshot {
    outcome: Result<PhaseOutcome, RuntimeError>,
    next_phase: Option<Phase>,
    body_error: Option<BodyError>,
    // Canonical logical charge, NOT physical memory usage of either executor.
    logical_retained_bytes: usize,
}

fn execute(
    generation: &CompiledGeneration,
    body: &bytes::Bytes,
    evaluation: EvaluationMode,
    mode: RuleEngineMode,
) -> ResultSnapshot {
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
    let mut tx = Transaction::new_with_modes(generation, head, RuntimeLimits::default(), evaluation, mode).unwrap();
    tx.execute_request_headers().unwrap();
    let outcome = tx
        .publish_request_body(RequestBody::new(body.clone(), BodyProcessorSelection::Auto))
        .and_then(|()| tx.execute_request_body());
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
    let no_hits = format!(
        "{{{}}}",
        (0..10_000)
            .rev()
            .map(|i| format!("\"k{i:05}\":\"{}\"", "x".repeat(16)))
            .collect::<Vec<_>>()
            .join(",")
    )
    .into_bytes();
    let escaped_value = "\\u0078".repeat(253) + "\\u0048\\u0049\\u0054";
    let escaped = format!(
        "{{{}}}",
        (0..256)
            .map(|i| format!("\"k{i:04}\":\"{escaped_value}\""))
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
    let small = |escaped: bool| {
        let value = if escaped { "\\u0068\\u0069\\u0074" } else { "hit" };
        format!(
            "{{{}}}",
            (0..32)
                .map(|i| format!("\"key{i:03}\":\"{value}\""))
                .collect::<Vec<_>>()
                .join(",")
        )
        .into_bytes()
    };
    let siblings = format!(
        "[{}]",
        (0..128)
            .map(|_| "{\"name\":\"hit\",\"value\":0}")
            .collect::<Vec<_>>()
            .join(",")
    )
    .into_bytes();
    vec![
        ("small_plain", small(false)),
        ("small_escaped", small(true)),
        ("sibling_objects", siblings),
        ("wide_hits", wide),
        ("wide_no_hits", no_hits),
        ("escaped_hits", escaped),
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
    println!("schema_version,case,arch,os,source_bytes,subscribers,mode,evaluation,samples,p50_ns,p95_ns,p99_ns,logical_retained_bytes");
    for subscribers in [1, 8, 32] {
        let generation = generation(subscribers);
        for (name, body) in cases() {
            for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                let body = bytes::Bytes::copy_from_slice(&body);
                let expected = execute(&generation, &body, EvaluationMode::Accelerated, mode);
                if name != "malformed_tail" || mode == RuleEngineMode::DetectionOnly {
                    assert!(
                        expected.outcome.is_ok(),
                        "fixture must reach evaluation: {name}, {subscribers}, {expected:?}"
                    );
                } else {
                    assert!(matches!(expected.outcome, Err(RuntimeError::Body { .. })));
                }
                if name == "wide_no_hits" {
                    assert!(expected.outcome.as_ref().unwrap().findings().is_empty());
                } else if matches!(name, "wide_hits" | "escaped_hits") {
                    let outcome = expected.outcome.as_ref().unwrap();
                    assert_eq!(outcome.findings().len(), subscribers);
                    assert_eq!(outcome.omitted_findings(), 0);
                }
                if name == "escaped_hits" {
                    let plain = cases().into_iter().find(|(name, _)| *name == "wide_hits").unwrap().1;
                    assert_eq!(
                        expected.outcome,
                        execute(
                            &generation,
                            &bytes::Bytes::from(plain),
                            EvaluationMode::Accelerated,
                            mode
                        )
                        .outcome
                    );
                }
                assert_eq!(execute(&generation, &body, EvaluationMode::Reference, mode), expected);
                for evaluation in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
                    for _ in 0..3 {
                        black_box(execute(&generation, black_box(&body), evaluation, mode));
                    }
                    let mut samples = Vec::with_capacity(samples_count);
                    for _ in 0..samples_count {
                        let started = Instant::now();
                        black_box(execute(&generation, black_box(&body), evaluation, mode));
                        samples.push(Instant::now().saturating_duration_since(started));
                    }
                    samples.sort_unstable();
                    println!(
                        "1,{name},{},{},{},{subscribers},{mode:?},{evaluation:?},{samples_count},{},{},{},{}",
                        std::env::consts::ARCH,
                        std::env::consts::OS,
                        body.len(),
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
