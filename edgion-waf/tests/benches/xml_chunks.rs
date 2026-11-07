//! Final-only usage and timing. Owned rows have no physical usage claim.
//! Scoped usage is sampled after complete and before finish; component peaks
//! are independent historical maxima, not an aggregate simultaneous peak.
#[path = "support/xml_common.rs"]
mod common;
use bytes::Bytes;
use common::*;
use edgion_waf::seclang::*;
use std::hint::black_box;
use std::time::Instant;

fn chunked(g: &CompiledGeneration, body: &Bytes, mode: RuleEngineMode, chunk: usize) -> (Snapshot, XmlInspectionUsage) {
    let headers = [RequestHeader::new(b"content-type", b"application/xml")];
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
    let mut tx = Transaction::new_with_policy(
        g,
        head,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        mode,
        RuleCheckPolicy::default(),
    )
    .unwrap();
    tx.execute_request_headers().unwrap();
    let mut session = tx.start_xml_inspection(XmlInspectionLimits::default(), None).unwrap();
    for data in body.chunks(chunk) {
        let _ = session.feed(data);
    }
    let completion = session.complete(body.len());
    let usage = session.usage();
    let source = session.requires_raw_source().then(|| body.clone());
    let published = match completion {
        Ok(()) => session.finish(source),
        Err(error) => {
            drop(session);
            Err(error)
        }
    };
    let outcome = published.and_then(|()| tx.execute_request_body());
    (snapshot(&tx, outcome), usage)
}
fn directed_chunks(n: usize, reverse: bool) {
    let mut fixtures = cases();
    fixtures.push((
        "namespace_final_only",
        Bytes::from(format!(
            "<r xmlns:p='urn:one'>{}</r>",
            (0..256)
                .map(|i| format!("<p:v xmlns:q='urn:{i}' q:a='miss'>HIT</p:v>"))
                .collect::<String>()
        )),
    ));
    if reverse {
        fixtures.reverse();
    }
    println!("schema_version,case,source_bytes,subscribers,mode,raw,controls,captures,implementation,chunk_bytes,samples,p50_ns,p95_ns,p99_ns,logical_retained_bytes,source_admitted,parser_current,parser_peak,structure_live,structure_peak,parser_work,result_current,result_peak,result_work,semantic_sha256");
    for subscribers in [1, 32] {
        for captures in [false, true] {
            for raw in [false, true] {
                for controls in [false, true] {
                    let g = compile(subscribers, raw, controls, captures);
                    for (name, body) in &fixtures {
                        for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                            let expected = execute(&g, body, mode, EvaluationMode::Accelerated);
                            expected.assert_case(name, mode);
                            for chunk in [0, usize::MAX, 512, 16 * 1024, 64 * 1024] {
                                let feed_chunk = if chunk == usize::MAX { body.len().max(1) } else { chunk };
                                let implementation = if chunk == 0 {
                                    "owned"
                                } else if chunk == usize::MAX {
                                    "scoped_single_feed"
                                } else {
                                    "scoped_chunks"
                                };
                                let (scoped, usage) =
                                    chunked(&g, body, mode, if chunk == 0 { body.len().max(1) } else { feed_chunk });
                                let actual = if chunk == 0 {
                                    execute(&g, body, mode, EvaluationMode::Accelerated)
                                } else {
                                    scoped
                                };
                                assert_eq!(actual.semantics(), expected.semantics(), "{name}/{chunk}");
                                let mut samples = Vec::with_capacity(n);
                                for _ in 0..3 {
                                    if chunk == 0 {
                                        black_box(execute(&g, body, mode, EvaluationMode::Accelerated));
                                    } else {
                                        black_box(chunked(&g, body, mode, feed_chunk));
                                    }
                                }
                                for _ in 0..n {
                                    let started = Instant::now();
                                    let result = if chunk == 0 {
                                        execute(black_box(&g), black_box(body), mode, EvaluationMode::Accelerated)
                                    } else {
                                        chunked(black_box(&g), black_box(body), mode, feed_chunk).0
                                    };
                                    let elapsed = started.elapsed();
                                    assert_eq!(result.semantics(), expected.semantics());
                                    black_box(result);
                                    samples.push(elapsed);
                                }
                                samples.sort();
                                let physical = if chunk == 0 {
                                    ",,,,,,,,".to_string()
                                } else {
                                    format!(
                                        "{},{},{},{},{},{},{},{},{}",
                                        usage.parser.source_bytes,
                                        usage.parser.retained_bytes,
                                        usage.parser.peak_retained_bytes,
                                        usage.parser.structure_bytes,
                                        usage.parser.peak_structure_bytes,
                                        usage.parser.work,
                                        usage.result_heap_bytes,
                                        usage.result_peak_heap_bytes,
                                        usage.result_work
                                    )
                                };
                                println!("2,{name},{},{subscribers},{mode:?},{raw},{controls},{captures},{implementation},{},{n},{},{},{},{},{physical},{}",body.len(),if chunk==usize::MAX {body.len()} else {chunk},percentile(&samples,50),percentile(&samples,95),percentile(&samples,99),actual.logical,actual.digest());
                            }
                        }
                    }
                }
            }
        }
    }
}

fn main() {
    let n = std::env::var("EDGION_WAF_BENCH_SAMPLES")
        .ok()
        .map(|v| v.parse::<usize>().unwrap())
        .unwrap_or(31);
    assert!(n > 0);
    directed_chunks(n, std::env::var_os("EDGION_WAF_BENCH_REVERSE").is_some());
}
