//! Transaction-level benchmark over the supported CRS 4.25.1 bundles.
//!
//! One iteration = construct `Transaction` + phase 1 + body publication +
//! phase 2, matching the gateway request path. The counting global allocator
//! reports allocations and requested bytes per request; the `fingerprint`
//! column hashes the sorted finding rule ids plus the intervention so later
//! optimizations can prove identical results.
//!
//! Environment:
//! - `EDGION_WAF_BENCH_SMOKE`: 31 measured iterations instead of 501.
//! - `EDGION_WAF_BENCH_PROFILE=1`: run 20 000 untimed iterations over a mix
//!   of cases for an external sampling profiler; prints no CSV.
//! - `EDGION_WAF_BENCH_PROFILE_CASE=<case>`: with profile mode, run only that
//!   case so its own hotspots can be attributed.
use std::alloc::{GlobalAlloc, Layout, System};
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use edgion_waf::seclang::{
    compile_rule_bundles, BodyProcessorSelection, CompiledGeneration, EvaluationMode, FrontendLimits,
    JsonInspectionLimits, Phase, PhaseDefaults, PhaseOutcome, RequestBody, RequestHead, RequestHeader,
    RuleBundleMember, RuleBundleSource, RuleCheckPolicy, RuleEngineMode, RuntimeLimits, SemanticValidationContext,
    Transaction, XmlInspectionLimits,
};
use serde::Deserialize;

type Fallible<T> = Result<T, Box<dyn Error>>;

static CALLS: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);

struct CountingSystem;

#[global_allocator]
static ALLOCATOR: CountingSystem = CountingSystem;

fn allocated(size: usize) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    REQUESTED.fetch_add(size, Ordering::Relaxed);
}

// SAFETY: all pointer/layout contracts are forwarded to System unchanged;
// accounting uses only allocation-free atomics and successful operations.
unsafe impl GlobalAlloc for CountingSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the GlobalAlloc layout contract.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the GlobalAlloc layout contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer and layout originate from this forwarding allocator.
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller supplies the original allocation and new size.
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if !replacement.is_null() {
            allocated(size);
        }
        replacement
    }
}

#[derive(Deserialize)]
struct Member {
    name: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bundle {
    digest: String,
    roots: Vec<String>,
    rules: Vec<Member>,
    phrase_assets: Vec<Member>,
}

fn load(profile: &str) -> Fallible<Bundle> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("config/waf/crs-4.25.1-supported-v1/generated")
        .join(format!("EdgionConfigData_crs-supported-{profile}.yaml"));
    let value: yaml_serde::Value = yaml_serde::from_slice(&fs::read(path)?)?;
    Ok(yaml_serde::from_value(value["spec"]["data"]["config"].clone())?)
}

fn context() -> Fallible<SemanticValidationContext> {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    Ok(SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults)?)
}

fn members(list: &[Member]) -> Vec<RuleBundleMember<'_>> {
    list.iter()
        .map(|member| RuleBundleMember {
            path: &member.name,
            bytes: member.content.as_bytes(),
        })
        .collect()
}

fn compile(tuning: &Bundle, core: &Bundle) -> Fallible<CompiledGeneration> {
    let bundles = [tuning, core];
    let roots = bundles.map(|bundle| bundle.roots.iter().map(String::as_str).collect::<Vec<_>>());
    let rules = bundles.map(|bundle| members(&bundle.rules));
    let assets = bundles.map(|bundle| members(&bundle.phrase_assets));
    let sources = [0, 1].map(|i| RuleBundleSource {
        identity: &bundles[i].digest,
        entrypoints: &roots[i],
        rules: &rules[i],
        phrase_assets: &assets[i],
    });
    Ok(
        compile_rule_bundles(&sources, "", context()?, FrontendLimits::default())?
            .into_parts()
            .0,
    )
}

/// One request: owned head parts plus an optional body.
struct Case {
    name: &'static str,
    method: &'static str,
    uri: String,
    headers: Vec<(String, String)>,
    body: Bytes,
}

const BROWSER_HEADERS: [(&str, &str); 8] = [
    ("host", "shop.example.test"),
    (
        "user-agent",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
    ),
    (
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
    ),
    ("accept-language", "en-US,en;q=0.9"),
    ("accept-encoding", "gzip, deflate, br"),
    ("referer", "https://shop.example.test/catalog/index.html"),
    ("cookie", "session=4f2a9c1e7b3d5a8f; theme=light"),
    ("connection", "keep-alive"),
];

fn browser_headers() -> Vec<(String, String)> {
    BROWSER_HEADERS
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

fn with_body(mut headers: Vec<(String, String)>, content_type: &str, body: &[u8]) -> Vec<(String, String)> {
    headers.push(("content-type".to_owned(), content_type.to_owned()));
    headers.push(("content-length".to_owned(), body.len().to_string()));
    headers
}

fn form_2k() -> String {
    let mut fields = Vec::new();
    for i in 0..30 {
        fields.push(format!(
            "field_{i:02}=customer+entered+value+number+{i}+with+some+plain+text+abc"
        ));
    }
    fields.join("&")
}

fn json_4k() -> String {
    // Nested object with 40 leaf keys spread over four sections.
    let mut sections = Vec::new();
    for section in 0..4 {
        let mut leaves = Vec::new();
        for key in 0..10 {
            leaves.push(format!(
                "\"key_{section}_{key}\":{{\"label\":\"ordinary product description text number {key} in section {section} for the catalog entry\",\"count\":{key}}}"
            ));
        }
        sections.push(format!("\"section_{section}\":{{{}}}", leaves.join(",")));
    }
    format!("{{{}}}", sections.join(","))
}

fn xml_4k() -> String {
    // Nested elements with attributes: four sections of ten items each.
    let mut text = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><order id=\"A-1001\" region=\"eu\">");
    for section in 0..4 {
        text.push_str(&format!("<section index=\"{section}\" kind=\"catalog\">"));
        for item in 0..10 {
            text.push_str(&format!(
                "<item sku=\"SKU-{section}-{item}\" qty=\"{item}\"><name>Ordinary product {item}</name><note>Description of item {item}</note></item>"
            ));
        }
        text.push_str("</section>");
    }
    text.push_str("</order>");
    text
}

fn multipart() -> String {
    let boundary = "----edgionbenchboundary";
    format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nQuarterly report\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"comment\"\r\n\r\nPlease review the attached notes.\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"upload\"; filename=\"notes.txt\"\r\nContent-Type: text/plain\r\n\r\n\
         line one of the notes\nline two of the notes\nline three of the notes\n\r\n\
         --{boundary}--\r\n"
    )
}

fn corpus() -> Vec<Case> {
    let xml = xml_4k();
    let form = form_2k();
    let json = json_4k();
    let xss = "name=alice&comment=%3Cscript%3Ealert(1)%3C%2Fscript%3E&rating=5";
    let parts = multipart();
    let mut big_cookie = browser_headers();
    big_cookie.retain(|(name, _)| name != "cookie");
    big_cookie.push(("cookie".to_owned(), format!("blob={}", "a".repeat(8 * 1024 - 5))));
    vec![
        Case {
            name: "clean_get",
            method: "GET",
            uri: "/catalog/item.html".to_owned(),
            headers: browser_headers(),
            body: Bytes::new(),
        },
        Case {
            name: "post_xml_4k",
            method: "POST",
            uri: "/api/orders".to_owned(),
            headers: with_body(browser_headers(), "application/xml", xml.as_bytes()),
            body: Bytes::from(xml),
        },
        Case {
            name: "post_form_2k",
            method: "POST",
            uri: "/account/profile".to_owned(),
            headers: with_body(browser_headers(), "application/x-www-form-urlencoded", form.as_bytes()),
            body: Bytes::from(form),
        },
        Case {
            name: "post_json_4k",
            method: "POST",
            uri: "/api/items".to_owned(),
            headers: with_body(browser_headers(), "application/json", json.as_bytes()),
            body: Bytes::from(json),
        },
        Case {
            name: "sqli_query",
            method: "GET",
            uri: "/item?id=1'%20UNION%20SELECT%20password%20FROM%20users--".to_owned(),
            headers: browser_headers(),
            body: Bytes::new(),
        },
        Case {
            name: "xss_form",
            method: "POST",
            uri: "/guestbook".to_owned(),
            headers: with_body(browser_headers(), "application/x-www-form-urlencoded", xss.as_bytes()),
            body: Bytes::from_static(xss.as_bytes()),
        },
        Case {
            name: "lfi_path",
            method: "GET",
            uri: "/download?file=../../etc/passwd".to_owned(),
            headers: browser_headers(),
            body: Bytes::new(),
        },
        Case {
            name: "big_cookie_8k",
            method: "GET",
            uri: "/catalog/item.html".to_owned(),
            headers: big_cookie,
            body: Bytes::new(),
        },
        Case {
            name: "multipart_3_parts",
            method: "POST",
            uri: "/upload".to_owned(),
            headers: with_body(
                browser_headers(),
                "multipart/form-data; boundary=----edgionbenchboundary",
                parts.as_bytes(),
            ),
            body: Bytes::from(parts),
        },
    ]
}

/// Result digest of one transaction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Digest {
    findings: usize,
    intervention: Option<(u64, u16)>,
    fingerprint: u64,
}

fn fnv1a(state: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *state ^= u64::from(*byte);
        *state = state.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Run one request through phase 1, body publication and phase 2.
/// The returned outcomes are folded into a digest after the measured window.
fn run(generation: &Arc<CompiledGeneration>, case: &Case) -> [Option<PhaseOutcome>; 2] {
    let headers = case
        .headers
        .iter()
        .map(|(name, value)| RequestHeader::new(name.as_bytes(), value.as_bytes()))
        .collect::<Vec<_>>();
    let request_line = format!("{} {} HTTP/1.1", case.method, case.uri);
    let head = RequestHead::new(
        request_line.as_bytes(),
        case.method.as_bytes(),
        case.uri.as_bytes(),
        case.uri.as_bytes(),
        b"HTTP/1.1",
        b"192.0.2.1",
        b"bench-request",
        &headers,
    );
    let mut transaction = Transaction::new_shared_with_policy(
        Arc::clone(generation),
        head,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::On,
        RuleCheckPolicy::default(),
    )
    .expect("transaction");
    let phase_one = transaction.execute_request_headers().expect("phase 1");
    if transaction.next_phase() != Some(Phase::RequestBody) {
        return [Some(phase_one), None];
    }
    let processor = transaction
        .preflight_request_body(BodyProcessorSelection::Auto)
        .expect("preflight");
    if processor == BodyProcessorSelection::Xml {
        // Gateway structured path for XML bodies.
        let mut session = transaction
            .start_xml_inspection(XmlInspectionLimits::default(), Some(case.body.len()))
            .expect("xml session");
        session.feed(&case.body).expect("xml feed");
        session.complete(case.body.len()).expect("xml complete");
        let source = session.requires_raw_source().then(|| case.body.clone());
        session.finish(source).expect("xml finish");
    } else if processor == BodyProcessorSelection::Json {
        // Gateway structured path: stream the captured body into the session.
        let mut session = transaction
            .start_json_inspection(JsonInspectionLimits::default(), Some(case.body.len()))
            .expect("json session");
        session.feed(&case.body).expect("json feed");
        session.complete(case.body.len()).expect("json complete");
        let source = session.requires_raw_source().then(|| case.body.clone());
        session.finish(source).expect("json finish");
    } else {
        transaction
            .publish_request_body(RequestBody::new(case.body.clone(), processor))
            .expect("publish body");
    }
    let phase_two = transaction.execute_request_body().expect("phase 2");
    [Some(phase_one), Some(phase_two)]
}

fn digest(outcomes: &[Option<PhaseOutcome>; 2]) -> Digest {
    let mut ids = Vec::new();
    let mut intervention = None;
    for outcome in outcomes.iter().flatten() {
        ids.extend(outcome.findings().iter().map(|finding| finding.rule_id()));
        if let Some(found) = outcome.intervention() {
            intervention.get_or_insert((found.rule_id(), found.status()));
        }
    }
    ids.sort_unstable();
    let mut state = 0xcbf2_9ce4_8422_2325_u64;
    for id in &ids {
        fnv1a(&mut state, &id.to_le_bytes());
    }
    if let Some((rule, status)) = intervention {
        fnv1a(&mut state, b"intervention");
        fnv1a(&mut state, &rule.to_le_bytes());
        fnv1a(&mut state, &status.to_le_bytes());
    }
    Digest {
        findings: ids.len(),
        intervention,
        fingerprint: state,
    }
}

fn percentile(sorted: &[u128], pct: usize) -> u128 {
    sorted[(sorted.len() - 1) * pct / 100]
}

fn main() -> Fallible<()> {
    let standard = load("standard")?;
    let core = load("core")?;
    let generation = Arc::new(compile(&standard, &core)?);
    drop((standard, core));
    let cases = corpus();

    if std::env::var("EDGION_WAF_BENCH_PROFILE").is_ok_and(|value| value == "1") {
        let only = std::env::var("EDGION_WAF_BENCH_PROFILE_CASE").ok();
        let mix = ["clean_get", "post_form_2k", "sqli_query", "post_json_4k"].map(|name| {
            let name = only.as_deref().unwrap_or(name);
            cases.iter().find(|case| case.name == name).expect("profile case")
        });
        for i in 0..20_000 {
            black_box(run(&generation, mix[i % mix.len()]));
        }
        return Ok(());
    }

    let iterations = if std::env::var_os("EDGION_WAF_BENCH_SMOKE").is_some() {
        31
    } else {
        501
    };
    println!(
        "schema_version,case,arch,os,body_bytes,iterations,p50_ns,p95_ns,p99_ns,allocations_per_request,allocated_bytes_per_request,findings,intervention,fingerprint"
    );
    for case in &cases {
        let expected = digest(&run(&generation, case));
        for _ in 0..31 {
            black_box(run(&generation, case));
        }
        let mut samples = Vec::with_capacity(iterations);
        let calls_before = CALLS.load(Ordering::Relaxed);
        let bytes_before = REQUESTED.load(Ordering::Relaxed);
        let mut last = [None, None];
        for _ in 0..iterations {
            let started = Instant::now();
            let outcomes = black_box(run(&generation, case));
            samples.push(started.elapsed().as_nanos());
            // Dropping outcomes is part of the request; keep the last for checking.
            last = outcomes;
        }
        let calls = CALLS.load(Ordering::Relaxed) - calls_before;
        let bytes = REQUESTED.load(Ordering::Relaxed) - bytes_before;
        assert_eq!(
            digest(&last),
            expected,
            "{}: result changed between iterations",
            case.name
        );
        samples.sort_unstable();
        // `samples` was preallocated; per-iteration allocations are the transaction's only.
        let intervention = expected
            .intervention
            .map_or_else(|| "none".to_owned(), |(rule, status)| format!("{rule}/{status}"));
        println!(
            "1,{},{},{},{},{iterations},{},{},{},{:.1},{:.0},{},{intervention},{:016x}",
            case.name,
            std::env::consts::ARCH,
            std::env::consts::OS,
            case.body.len(),
            percentile(&samples, 50),
            percentile(&samples, 95),
            percentile(&samples, 99),
            calls as f64 / iterations as f64,
            bytes as f64 / iterations as f64,
            expected.findings,
            expected.fingerprint,
        );
    }
    println!(
        "# sizes: RequestHead={} RuntimeLimits={} PhaseOutcome={} Transaction={} CompiledGeneration={}",
        std::mem::size_of::<RequestHead<'_>>(),
        std::mem::size_of::<RuntimeLimits>(),
        std::mem::size_of::<PhaseOutcome>(),
        std::mem::size_of::<Transaction<'_>>(),
        std::mem::size_of::<CompiledGeneration>(),
    );
    Ok(())
}
