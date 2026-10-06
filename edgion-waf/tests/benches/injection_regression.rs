use std::hint::black_box;
use std::time::Instant;

use edgion_waf::seclang::{
    compile_with_includes, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink, IncludeSource, Phase,
    PhaseDefaults, RequestHead, RequestHeader, RuntimeLimits, SemanticValidationContext, Transaction,
};

struct NoIncludes;

impl IncludeSource for NoIncludes {
    fn load(&mut self, _request: IncludeRequest<'_>, _sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

fn main() {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap();
    let iterations = if std::env::var_os("EDGION_WAF_BENCH_SMOKE").is_some() {
        101
    } else {
        1001
    };
    println!("schema_version,case,input_bytes,iterations,p50_ns,p95_ns,findings");
    for (name, operator, input) in [
        ("sqli_hit", "detectSQLi", b"1 OR 1=1".to_vec()),
        ("sqli_miss", "detectSQLi", b"ordinary text".to_vec()),
        ("xss_hit", "detectXSS", b"<script>alert(1)</script>".to_vec()),
        ("xss_miss", "detectXSS", b"<p>Hello</p>".to_vec()),
        ("sqli_long_miss", "detectSQLi", vec![b'a'; 64 * 1024]),
        ("sqli_dotted_32k", "detectSQLi", b"a.a`".repeat(8 * 1024)),
        ("sqli_dotted_64k", "detectSQLi", b"a.a`".repeat(16 * 1024)),
        ("xss_long_miss", "detectXSS", vec![b'a'; 64 * 1024]),
    ] {
        let source = format!("SecRule REQUEST_HEADERS:X-Input \"@{operator}\" \"id:100,phase:1,pass\"\n");
        let generation = compile_with_includes(
            "injection-bench",
            source.as_bytes(),
            &mut NoIncludes,
            None,
            context,
            FrontendLimits::default(),
        )
        .unwrap()
        .into_parts()
        .0;
        let headers = [RequestHeader::new(b"X-Input", &input)];
        let request = RequestHead::new(
            b"GET / HTTP/1.1",
            b"GET",
            b"/",
            b"/",
            b"HTTP/1.1",
            b"192.0.2.1",
            b"bench-1",
            &headers,
        );
        let run = || {
            let mut transaction = Transaction::new(&generation, request, RuntimeLimits::default()).unwrap();
            transaction.execute_request_headers().unwrap().findings().len()
        };
        let expected = run();
        for _ in 0..31 {
            black_box(run());
        }
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let started = Instant::now();
            let findings = black_box(run());
            assert_eq!(findings, expected);
            samples.push(started.elapsed().as_nanos());
        }
        samples.sort_unstable();
        println!(
            "1,{name},{},{iterations},{},{},{expected}",
            input.len(),
            samples[(iterations - 1) * 50 / 100],
            samples[(iterations - 1) * 95 / 100],
        );
    }
}
