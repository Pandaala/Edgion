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

fn execute_chunked(
    generation: &CompiledGeneration,
    body: &bytes::Bytes,
    mode: RuleEngineMode,
    chunk_size: usize,
) -> (ResultSnapshot, JsonInspectionUsage) {
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
    let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
    for chunk in body.chunks(chunk_size) {
        // Capture continues after a latched failure; the lexer no longer admits bytes.
        let _ = inspection.feed(chunk);
    }
    let completion = inspection.complete(body.len());
    let usage = inspection.usage();
    let source = inspection.requires_raw_source().then(|| body.clone());
    let publication = match completion {
        Ok(()) => inspection.finish(source),
        Err(error) => {
            drop(inspection);
            Err(error)
        }
    };
    let outcome = publication.and_then(|()| tx.execute_request_body());
    (
        ResultSnapshot {
            outcome,
            next_phase: tx.next_phase(),
            body_error: tx.request_body_error(),
            logical_retained_bytes: tx.retained_bytes(),
        },
        usage,
    )
}

fn directed_chunks() {
    let samples_count = std::env::var("EDGION_WAF_BENCH_SAMPLES")
        .ok()
        .map(|value| value.parse::<usize>().expect("positive sample count"))
        .unwrap_or(31);
    assert!(samples_count > 0);
    let selected = [
        "small_plain",
        "wide_no_hits",
        "escaped_hits",
        "duplicates_last_miss",
        "malformed_tail",
        "large_scalar",
    ];
    println!("schema_version,case,arch,os,source_bytes,subscribers,mode,implementation,chunk_bytes,samples,p50_ns,p95_ns,p99_ns,logical_retained_bytes,inspected_source_bytes,lexical_current,structure_current,result_current,lexical_peak,structure_peak,result_peak,lexical_work,structure_work,result_work");
    let reverse = std::env::var_os("EDGION_WAF_BENCH_REVERSE").is_some();
    let mut rule_counts = [1, 32];
    let mut chunk_sizes = [0, 512, 16 * 1024, 64 * 1024];
    let mut fixtures = cases();
    if reverse {
        rule_counts.reverse();
        chunk_sizes.reverse();
        fixtures.reverse();
    }
    for subscribers in rule_counts {
        let generation = generation(subscribers);
        for (name, body) in fixtures.iter().filter(|(name, _)| selected.contains(name)) {
            let body = bytes::Bytes::copy_from_slice(body);
            for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                let expected = execute(&generation, &body, EvaluationMode::Accelerated, mode);
                for chunk_size in chunk_sizes {
                    let (actual, usage) = if chunk_size == 0 {
                        (execute(&generation, &body, EvaluationMode::Accelerated, mode), None)
                    } else {
                        let (result, usage) = execute_chunked(&generation, &body, mode, chunk_size);
                        (result, Some(usage))
                    };
                    assert_eq!(
                        actual.outcome, expected.outcome,
                        "outcome: {name}/{mode:?}/{chunk_size}"
                    );
                    assert_eq!(actual.next_phase, expected.next_phase);
                    assert_eq!(actual.body_error, expected.body_error);
                    // Capacity hints affect retained charges; semantic parity does not require their equality.
                    let run = || {
                        if chunk_size == 0 {
                            black_box(execute(
                                &generation,
                                black_box(&body),
                                EvaluationMode::Accelerated,
                                mode,
                            ));
                        } else {
                            black_box(execute_chunked(&generation, black_box(&body), mode, chunk_size));
                        }
                    };
                    for _ in 0..3 {
                        run();
                    }
                    let mut samples = Vec::with_capacity(samples_count);
                    for _ in 0..samples_count {
                        let started = Instant::now();
                        run();
                        samples.push(started.elapsed());
                    }
                    samples.sort_unstable();
                    let usage_columns = usage
                        .map(|u| {
                            format!(
                                "{},{},{},{},{},{},{},{},{},{}",
                                u.inspected_source_bytes,
                                u.lexical_heap_bytes,
                                u.structure_heap_bytes,
                                u.result_heap_bytes,
                                u.lexical_peak_heap_bytes,
                                u.structure_peak_heap_bytes,
                                u.result_peak_heap_bytes,
                                u.lexical_work,
                                u.structure_work,
                                u.result_work
                            )
                        })
                        .unwrap_or_else(|| ",,,,,,,,,".to_owned());
                    println!(
                        "2,{name},{},{},{},{subscribers},{mode:?},{},{chunk_size},{samples_count},{},{},{},{},{}",
                        std::env::consts::ARCH,
                        std::env::consts::OS,
                        body.len(),
                        if chunk_size == 0 { "owned" } else { "chunked" },
                        percentile(&samples, 50),
                        percentile(&samples, 95),
                        percentile(&samples, 99),
                        actual.logical_retained_bytes,
                        usage_columns
                    );
                }
            }
        }
    }
}

fn main() {
    if std::env::var_os("EDGION_WAF_BENCH_CHUNKS").is_some() {
        directed_chunks();
        return;
    }
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
                let reference = execute(&generation, &body, EvaluationMode::Reference, mode);
                // Executors can retain different reusable capacities while preserving semantics.
                assert_eq!(reference.outcome, expected.outcome);
                assert_eq!(reference.next_phase, expected.next_phase);
                assert_eq!(reference.body_error, expected.body_error);
                for evaluation in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
                    let retained = match evaluation {
                        EvaluationMode::Accelerated => expected.logical_retained_bytes,
                        EvaluationMode::Reference => reference.logical_retained_bytes,
                    };
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
                        retained
                    );
                }
            }
        }
    }
}
