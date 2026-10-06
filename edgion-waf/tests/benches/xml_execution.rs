//! Common owned XML harness, compatible with the frozen baseline.
#[path = "support/xml_common.rs"]
mod common;
use common::*;
use edgion_waf::seclang::*;
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let n = std::env::var("EDGION_WAF_BENCH_SAMPLES")
        .ok()
        .map(|value| value.parse::<usize>().unwrap())
        .unwrap_or(31);
    assert!(n > 0);
    let reverse = std::env::var_os("EDGION_WAF_BENCH_REVERSE").is_some();
    let mut fixtures = cases();
    let mut counts = [1, 32];
    if reverse {
        fixtures.reverse();
        counts.reverse();
    }
    println!("schema_version,case,arch,os,source_bytes,subscribers,mode,raw,controls,captures,samples,p50_ns,p95_ns,p99_ns,logical_retained_bytes,semantic_sha256");
    for subscribers in counts {
        for captures in [false, true] {
            for raw in [false, true] {
                for controls in [false, true] {
                    let g = compile(subscribers, raw, controls, captures);
                    for (name, body) in &fixtures {
                        for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                            let reference = execute(&g, body, mode, EvaluationMode::Reference);
                            let expected = execute(&g, body, mode, EvaluationMode::Accelerated);
                            expected.assert_case(name, mode);
                            assert_eq!(expected.semantics(), reference.semantics(), "{name}");
                            let mut samples = Vec::with_capacity(n);
                            for _ in 0..3 {
                                black_box(execute(&g, body, mode, EvaluationMode::Accelerated));
                            }
                            for _ in 0..n {
                                let start = Instant::now();
                                let actual = execute(black_box(&g), black_box(body), mode, EvaluationMode::Accelerated);
                                let elapsed = start.elapsed();
                                assert_eq!(actual.semantics(), expected.semantics());
                                black_box(actual);
                                samples.push(elapsed);
                            }
                            samples.sort();
                            println!("1,{name},{},{},{},{subscribers},{mode:?},{raw},{controls},{captures},{n},{},{},{},{},{}",std::env::consts::ARCH,std::env::consts::OS,body.len(),percentile(&samples,50),percentile(&samples,95),percentile(&samples,99),expected.logical,expected.digest());
                        }
                    }
                }
            }
        }
    }
}
