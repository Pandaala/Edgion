use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, Instant};

use edgion_waf::aho_corasick::{AutomatonKind, Builder};

const ITERATIONS: usize = 100;
const INPUT_LENGTHS: [usize; 4] = [32, 256, 4 * 1024, 64 * 1024];

fn main() {
    println!(
        "scenario,input_bytes,prefilter,acceleration,requested_backend,selected_backend,selection_reason,patterns,states,byte_classes,compiled_bytes,acceleration_bytes,estimated_peak_build_bytes,dense_states,compile_us,p50_us,p95_us,p99_us,mib_per_second"
    );
    let mut scenarios = scenarios();
    if let Some(rules_dir) = std::env::var_os("EDGION_WAF_CRS_RULES_DIR") {
        scenarios.extend(crs_scenarios(Path::new(&rules_dir)));
    }
    for (name, patterns, fill) in scenarios {
        for input_len in INPUT_LENGTHS {
            let haystack = vec![fill; input_len];
            for automaton_kind in [
                AutomatonKind::SparseNfa,
                AutomatonKind::ContiguousNfa,
                AutomatonKind::Dfa,
                AutomatonKind::Auto,
            ] {
                for prefilter in [false, true] {
                    measure(name, &patterns, &haystack, automaton_kind, prefilter);
                }
            }
        }
    }
}

fn measure(name: &str, patterns: &[Vec<u8>], haystack: &[u8], automaton_kind: AutomatonKind, prefilter: bool) {
    let started = Instant::now();
    let mut builder = Builder::new();
    builder.automaton_kind(automaton_kind).prefilter(prefilter);
    let matcher = builder.build(patterns).expect("benchmark patterns must compile");
    let compile_time = started.elapsed();
    let mut scratch = matcher.create_pattern_set();
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        matcher
            .scan_pattern_set(black_box(haystack), &mut scratch)
            .expect("benchmark scratch shape must match");
        black_box(scratch.is_empty());
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let total_seconds = samples.iter().map(Duration::as_secs_f64).sum::<f64>();
    let total_mib = haystack.len() as f64 * ITERATIONS as f64 / (1024.0 * 1024.0);
    println!(
        "{name},{},{prefilter},{:?},{automaton_kind:?},{:?},{:?},{},{},{},{},{},{},{},{},{},{},{},{:.2}",
        haystack.len(),
        matcher.acceleration_kind(),
        matcher.automaton_kind(),
        matcher.selection_reason(),
        patterns.len(),
        matcher.state_count(),
        matcher.byte_class_count(),
        matcher.memory_usage(),
        matcher.acceleration_memory_usage(),
        matcher.estimated_peak_build_memory_usage(),
        matcher.dense_state_count(),
        compile_time.as_micros(),
        percentile_micros(&samples, 50),
        percentile_micros(&samples, 95),
        percentile_micros(&samples, 99),
        total_mib / total_seconds,
    );
}

fn percentile_micros(samples: &[Duration], percentile: usize) -> u128 {
    let index = (samples.len() - 1) * percentile / 100;
    samples[index].as_micros()
}

fn scenarios() -> Vec<(&'static str, Vec<Vec<u8>>, u8)> {
    vec![
        (
            "tiny_low_fanout_zero_hit",
            vec![b"alpha".to_vec(), b"beta".to_vec()],
            b'z',
        ),
        (
            "root_heavy_high_hit",
            (u8::MIN..=u8::MAX).map(|byte| vec![byte]).collect(),
            b'z',
        ),
        (
            "deep_prefix_zero_hit",
            (1..=64).map(|length| vec![b'a'; length]).collect(),
            b'z',
        ),
        (
            "high_output_density",
            (1..=16).map(|length| vec![b'a'; length]).collect(),
            b'a',
        ),
        (
            "bounded_packed_zero_hit",
            vec![b"a!x".to_vec(), b"b@y".to_vec(), b"c#z".to_vec(), b"d$w".to_vec()],
            b'q',
        ),
        (
            "rare_byte_zero_hit",
            vec![b"aa!".to_vec(), b"bb!".to_vec(), b"cc!".to_vec(), b"dd!".to_vec()],
            b'q',
        ),
    ]
}

fn crs_scenarios(rules_dir: &Path) -> Vec<(&'static str, Vec<Vec<u8>>, u8)> {
    [
        ("crs_scanner_user_agents", "scanners-user-agents.data"),
        ("crs_php_function_names", "php-function-names-933150.data"),
        ("crs_unix_shell", "unix-shell.data"),
        ("crs_ssrf", "ssrf.data"),
    ]
    .into_iter()
    .map(|(scenario, file_name)| {
        let path = rules_dir.join(file_name);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read CRS benchmark data {}: {error}", path.display()));
        let patterns = source
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| line.as_bytes().to_vec())
            .collect();
        (scenario, patterns, b'z')
    })
    .collect()
}
