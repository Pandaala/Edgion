use std::hint::black_box;
use std::time::{Duration, Instant};

use edgion_waf::aho_corasick::{AccelerationKind, Builder};

struct Case {
    name: &'static str,
    patterns: Vec<Vec<u8>>,
    haystack: Vec<u8>,
    expected_acceleration: AccelerationKind,
}

fn main() {
    let iterations = if std::env::var_os("EDGION_WAF_BENCH_SMOKE").is_some() {
        31
    } else {
        501
    };
    let warmup_iterations = 31;
    println!(
        "schema_version,case,arch,os,input_bytes,patterns,prefilter,expected_acceleration,selected_acceleration,iterations,operations_per_sample,compiled_bytes,acceleration_bytes,scratch_bytes,candidate_attempts,candidate_scanned_bytes,skipped_bytes,prefilter_disabled,compile_ns,p50_ns,p95_ns,p99_ns,mib_per_second,fingerprint"
    );
    for case in cases() {
        let reference = compile(&case.patterns, false);
        let accelerated = compile(&case.patterns, true);
        assert_eq!(
            fingerprint(&reference, &case.haystack),
            fingerprint(&accelerated, &case.haystack)
        );
        assert_eq!(accelerated.acceleration_kind(), case.expected_acceleration);
        for prefilter in [false, true] {
            measure(&case, prefilter, warmup_iterations, iterations);
        }
    }
}

fn compile(patterns: &[Vec<u8>], prefilter: bool) -> edgion_waf::aho_corasick::AhoCorasick {
    let mut builder = Builder::new();
    builder.prefilter(prefilter);
    builder.build(patterns).expect("internal benchmark patterns compile")
}

fn measure(case: &Case, prefilter: bool, warmup_iterations: usize, iterations: usize) {
    let compile_started = Instant::now();
    let matcher = compile(&case.patterns, prefilter);
    let compile_time = Instant::now().saturating_duration_since(compile_started);
    let expected = if prefilter {
        case.expected_acceleration
    } else {
        AccelerationKind::None
    };
    assert_eq!(matcher.acceleration_kind(), expected);
    let mut matches = matcher.create_pattern_set();
    let scratch_bytes = matches.memory_usage();
    let diagnostics = matcher
        .scan_pattern_set_with_diagnostics(&case.haystack, &mut matches)
        .expect("benchmark scratch belongs to its matcher");
    assert_eq!(matches.memory_usage(), scratch_bytes);
    let operations_per_sample = (4 * 1024 * 1024_usize).div_ceil(case.haystack.len()).max(1);
    for _ in 0..warmup_iterations {
        for _ in 0..operations_per_sample {
            matcher
                .scan_pattern_set(black_box(&case.haystack), &mut matches)
                .expect("benchmark scratch belongs to its matcher");
        }
    }
    let mut samples = Vec::with_capacity(iterations);
    let mut result_fingerprint = 0_u64;
    for _ in 0..iterations {
        let started = Instant::now();
        for _ in 0..operations_per_sample {
            matcher
                .scan_pattern_set(black_box(&case.haystack), &mut matches)
                .expect("benchmark scratch belongs to its matcher");
        }
        samples.push(Instant::now().saturating_duration_since(started));
        result_fingerprint = black_box(pattern_set_fingerprint(&matches));
    }
    samples.sort_unstable();
    let seconds = samples.iter().map(Duration::as_secs_f64).sum::<f64>();
    assert_eq!(matches.memory_usage(), scratch_bytes);
    let mib = case.haystack.len() as f64 * iterations as f64 * operations_per_sample as f64 / (1024.0 * 1024.0);
    println!(
        "1,{},{},{},{},{},{},{:?},{:?},{},{},{},{},{},{},{},{},{},{},{},{},{},{:.2},{}",
        case.name,
        std::env::consts::ARCH,
        std::env::consts::OS,
        case.haystack.len(),
        case.patterns.len(),
        prefilter,
        expected,
        matcher.acceleration_kind(),
        iterations,
        operations_per_sample,
        matcher.memory_usage(),
        matcher.acceleration_memory_usage(),
        scratch_bytes,
        diagnostics.candidate_attempts(),
        diagnostics.candidate_scanned_bytes(),
        diagnostics.skipped_bytes(),
        diagnostics.prefilter_disabled(),
        compile_time.as_nanos(),
        percentile_nanos(&samples, 50),
        percentile_nanos(&samples, 95),
        percentile_nanos(&samples, 99),
        mib / seconds,
        result_fingerprint,
    );
}

fn fingerprint(matcher: &edgion_waf::aho_corasick::AhoCorasick, haystack: &[u8]) -> u64 {
    let mut matches = matcher.create_pattern_set();
    matcher
        .scan_pattern_set(haystack, &mut matches)
        .expect("benchmark scratch belongs to its matcher");
    pattern_set_fingerprint(&matches)
}

fn pattern_set_fingerprint(matches: &edgion_waf::aho_corasick::PatternSet) -> u64 {
    matches.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, id| {
        (hash ^ id.as_usize() as u64).wrapping_mul(0x100_0000_01b3)
    })
}

fn percentile_nanos(samples: &[Duration], percentile: usize) -> u128 {
    samples[(samples.len() - 1) * percentile / 100].as_nanos()
}

fn cases() -> Vec<Case> {
    let patterns_64 = generated_patterns(64);
    let patterns_256 = generated_patterns(256);
    let mut late_hit = vec![0_u8; 64 * 1024];
    late_hit.extend_from_slice(&patterns_64[47]);
    vec![
        Case {
            name: "two_gram_64_zero_hit",
            patterns: patterns_64.clone(),
            haystack: vec![0_u8; 64 * 1024],
            expected_acceleration: AccelerationKind::TwoGram,
        },
        Case {
            name: "two_gram_64_late_hit",
            patterns: patterns_64,
            haystack: late_hit,
            expected_acceleration: AccelerationKind::TwoGram,
        },
        Case {
            name: "two_gram_256_zero_hit",
            patterns: patterns_256,
            haystack: deterministic_bytes(64 * 1024),
            expected_acceleration: AccelerationKind::TwoGram,
        },
        Case {
            name: "rare_bytes_zero_hit",
            patterns: vec![b"alpha-one".to_vec(), b"beta-two".to_vec(), b"gamma-three".to_vec()],
            haystack: vec![b'z'; 64 * 1024],
            expected_acceleration: AccelerationKind::RareBytes,
        },
    ]
}

fn deterministic_bytes(len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

fn generated_patterns(count: usize) -> Vec<Vec<u8>> {
    (0..count)
        .map(|index| {
            vec![
                0x21 + (index % 90) as u8,
                0x81 + (index % 63) as u8,
                0x40 + ((index / 3) % 50) as u8,
                0xc0 + ((index / 7) % 32) as u8,
                b'-',
                b'w',
                b'a',
                b'f',
            ]
        })
        .collect()
}
