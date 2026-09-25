//! Reproducible full-body multipart publication benchmark.
//! Run with `cargo bench -p edgion-waf --bench multipart_boundary`.
//! Set `EDGION_WAF_BENCH_SMOKE=1` for a short validation run.

use std::hint::black_box;
use std::time::Instant;

use edgion_waf::seclang::{
    compile_with_includes, BodyProcessorSelection, CompiledGeneration, FrontendLimits, IncludeFailure, IncludeRequest,
    IncludeSink, IncludeSource, Phase, PhaseDefaults, RequestBody, RequestHead, RequestHeader, RuntimeLimits,
    SemanticValidationContext, Transaction,
};

struct NoIncludes;

impl IncludeSource for NoIncludes {
    fn load(&mut self, _request: IncludeRequest<'_>, _sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

fn generation() -> CompiledGeneration {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap();
    compile_with_includes(
        "multipart-bench",
        b"SecRule REQBODY_PROCESSOR \"@streq MULTIPART\" \"id:1,phase:2,pass\"\n",
        &mut NoIncludes,
        None,
        context,
        FrontendLimits::default(),
    )
    .unwrap()
    .into_parts()
    .0
}

fn body(boundary: &[u8], kind: &str, target_bytes: usize) -> Vec<u8> {
    let mut result = Vec::with_capacity(target_bytes);
    result.extend_from_slice(b"--");
    result.extend_from_slice(boundary);
    result.extend_from_slice(b"\r\nContent-Disposition: form-data; name=\"upload\"; filename=\"upload.bin\"\r\n\r\n");
    let mut closing = b"\r\n--".to_vec();
    closing.extend_from_slice(boundary);
    closing.extend_from_slice(b"--\r\n");
    assert!(result.len() + closing.len() < target_bytes);
    let motif = match kind {
        "plain" => vec![b'x'; 4096],
        "near_match" => {
            let mut bytes = b"\r\n--".to_vec();
            bytes.extend_from_slice(&boundary[..boundary.len() - 1]);
            bytes.push(b'!');
            bytes
        }
        "invalid_suffix" => {
            let mut bytes = b"\r\n--".to_vec();
            bytes.extend_from_slice(boundary);
            bytes.push(b'!');
            bytes
        }
        _ => unreachable!("benchmark case names are fixed"),
    };
    while result.len() + closing.len() < target_bytes {
        let remaining = target_bytes - result.len() - closing.len();
        result.extend_from_slice(&motif[..remaining.min(motif.len())]);
    }
    result.extend_from_slice(&closing);
    assert_eq!(result.len(), target_bytes);
    result
}

fn publish(generation: &CompiledGeneration, body: &[u8], content_type: &[u8]) -> u128 {
    let headers = [RequestHeader::new(b"Content-Type", content_type)];
    let request = RequestHead::new(
        b"POST /upload HTTP/1.1",
        b"POST",
        b"/upload",
        b"/upload",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"multipart-bench",
        &headers,
    );
    let mut transaction = Transaction::new(generation, request, RuntimeLimits::default()).unwrap();
    transaction.execute_request_headers().unwrap();
    let started = Instant::now();
    transaction
        .publish_request_body(RequestBody::new(black_box(body), BodyProcessorSelection::Auto))
        .unwrap();
    let elapsed = started.elapsed().as_nanos();
    assert_eq!(
        black_box(transaction.execute_request_body().unwrap().findings().len()),
        1
    );
    elapsed
}

fn main() {
    let smoke = std::env::var_os("EDGION_WAF_BENCH_SMOKE").is_some();
    let iterations = if smoke { 11 } else { 101 };
    let warmup = if smoke { 3 } else { 9 };
    let generation = generation();
    println!("schema_version,case,arch,os,boundary_bytes,body_bytes,iterations,p50_ns,p95_ns");
    for boundary_len in [70, 200] {
        let boundary = vec![b'a'; boundary_len];
        let mut content_type = b"multipart/form-data; boundary=".to_vec();
        content_type.extend_from_slice(&boundary);
        for kind in ["plain", "near_match", "invalid_suffix"] {
            for target_bytes in [64 * 1024, 1024 * 1024, 8 * 1024 * 1024, 15 * 1024 * 1024] {
                if target_bytes == 15 * 1024 * 1024 && kind != "near_match" {
                    continue;
                }
                let body = body(&boundary, kind, target_bytes);
                for _ in 0..warmup {
                    black_box(publish(&generation, &body, &content_type));
                }
                let mut samples = Vec::with_capacity(iterations);
                for _ in 0..iterations {
                    samples.push(black_box(publish(&generation, &body, &content_type)));
                }
                samples.sort_unstable();
                println!(
                    "1,{kind},{},{},{boundary_len},{target_bytes},{iterations},{},{}",
                    std::env::consts::ARCH,
                    std::env::consts::OS,
                    samples[(iterations - 1) * 50 / 100],
                    samples[(iterations - 1) * 95 / 100],
                );
            }
        }
    }
}
