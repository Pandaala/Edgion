//! Same product CRS profiles, unchanged engine/storage defaults and separate instrumentation modes.
//! EDGION_WAF_PROFILE_MODE=timing|allocations cargo bench -p edgion-gateway --bench waf_supported_profile
//! EDGION_WAF_PROFILE_REVERSE=1 reverses case order in a fresh process.
//! Measurements are public components (resident request transaction or sealed
//! BodyStorage + incremental inspection + snapshot replay), not private Gateway
//! capture callbacks. Fixture backing, Tokio and generation are outside request
//! spans; whole-request simultaneous peak is never reset between component stages.
//! Successful requested allocation bytes/peaks exclude allocator overhead and RSS.
//! System is benchmark-owned; timing mode compiles out counters. Spill is warm-cache.
#[allow(dead_code)]
#[path = "../../../edgion-resources/tests/common/supported_crs.rs"]
mod profile;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use bytes::Bytes;
use edgion_gateway::request_body::{BodyStorage, BodyStorageView, ResolvedRequestBodyConfig};
use edgion_resources::resources::edgion_gateway_config::RequestBodyConfig;
use edgion_waf::seclang::*;
use pingora_core::protocols::http::RequestBodyBuffer;
use serde::Serialize;
use std::sync::Arc;

const MODE: &str = match option_env!("EDGION_WAF_PROFILE_MODE") {
    Some(mode) => mode,
    None => "timing",
};
const ALLOCATIONS: bool = matches!(MODE.as_bytes(), [b'a', ..]);
const CHUNK_BYTES: usize = 64 * 1024;
static CALLS: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct CountingSystem;
#[global_allocator]
static ALLOCATOR: CountingSystem = CountingSystem;

fn allocated(bytes: usize) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    REQUESTED.fetch_add(bytes, Ordering::Relaxed);
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: pointer/layout contracts are forwarded unchanged to System. The
// counters allocate no memory and never inspect the allocation's contents.
unsafe impl GlobalAlloc for CountingSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies GlobalAlloc's allocation contract.
        let pointer = unsafe { System.alloc(layout) };
        if ALLOCATIONS && !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies GlobalAlloc's allocation contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if ALLOCATIONS && !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer/layout belong to this same forwarding allocator.
        unsafe { System.dealloc(pointer, layout) };
        if ALLOCATIONS {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: original pointer/layout and new size obey GlobalAlloc's contract.
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if ALLOCATIONS && !replacement.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            allocated(size);
        }
        replacement
    }
}

#[derive(Serialize)]
struct Measurement {
    ns: u128,
    calls: Option<usize>,
    requested: Option<usize>,
    live_delta: Option<i128>,
    peak_extra: Option<usize>,
}
struct Stage {
    at: Instant,
    calls: usize,
    requested: usize,
    live: usize,
}
impl Stage {
    fn start(reset_peak: bool) -> Self {
        let live = LIVE.load(Ordering::Relaxed);
        if ALLOCATIONS && reset_peak {
            PEAK.store(live, Ordering::Relaxed);
        }
        Self {
            at: Instant::now(),
            calls: CALLS.load(Ordering::Relaxed),
            requested: REQUESTED.load(Ordering::Relaxed),
            live,
        }
    }
    fn finish(&self, whole: bool) -> Measurement {
        Measurement {
            ns: self.at.elapsed().as_nanos(),
            calls: ALLOCATIONS.then(|| CALLS.load(Ordering::Relaxed) - self.calls),
            requested: ALLOCATIONS.then(|| REQUESTED.load(Ordering::Relaxed) - self.requested),
            live_delta: ALLOCATIONS.then(|| LIVE.load(Ordering::Relaxed) as i128 - self.live as i128),
            peak_extra: (ALLOCATIONS && whole).then(|| PEAK.load(Ordering::Relaxed).saturating_sub(self.live)),
        }
    }
}
struct Fixture {
    name: &'static str,
    xml: bool,
    body: Bytes,
    length: String,
}
impl Fixture {
    fn new(name: &'static str, xml: bool, body: Vec<u8>) -> Self {
        Self {
            name,
            xml,
            length: body.len().to_string(),
            body: Bytes::from(body),
        }
    }
}
fn fixtures() -> Vec<Fixture> {
    let mut cases = vec![
        Fixture::new("json-small-clean", false, br#"{"probe":"safe"}"#.to_vec()),
        Fixture::new("json-small-attack", false, br#"{"probe":"phar://payload"}"#.to_vec()),
        Fixture::new("xml-small-clean", true, b"<r a='safe'>safe</r>".to_vec()),
        Fixture::new("xml-small-attack", true, b"<r a='phar://payload'/>".to_vec()),
    ];
    cases.push(Fixture::new(
        "json-wide-1024",
        false,
        format!(
            "{{{}}}",
            (0..1024)
                .map(|i| format!("\"k{i:04}\":\"{}\"", "x".repeat(240)))
                .collect::<Vec<_>>()
                .join(",")
        )
        .into_bytes(),
    ));
    cases.push(Fixture::new(
        "xml-wide-1024",
        true,
        format!(
            "<r>{}</r>",
            (0..1024)
                .map(|_| format!("<v>{}</v>", "x".repeat(240)))
                .collect::<String>()
        )
        .into_bytes(),
    ));
    for (name, xml, count) in [
        ("json-large-scalar", false, 256 * 1024),
        ("xml-large-text", true, 256 * 1024),
        ("json-token-boundary", false, 1024 * 1024),
        ("xml-resource-boundary", true, 1024 * 1024),
    ] {
        cases.push(Fixture::new(
            name,
            xml,
            if xml {
                format!("<r>{}</r>", "x".repeat(count))
            } else {
                format!("{{\"probe\":\"{}\"}}", "x".repeat(count))
            }
            .into_bytes(),
        ));
    }
    cases.push(Fixture::new("json-malformed", false, br#"{"probe":"safe""#.to_vec()));
    cases.push(Fixture::new("xml-malformed", true, b"<r>safe</wrong>".to_vec()));
    cases
}
fn transaction(g: &Arc<CompiledGeneration>, f: &Fixture, mode: RuleEngineMode) -> Transaction<'static> {
    let headers = [
        RequestHeader::new(b"host", b"profile.example.com"),
        RequestHeader::new(b"user-agent", b"edgion-profile-benchmark"),
        RequestHeader::new(b"accept", b"*/*"),
        RequestHeader::new(
            b"content-type",
            if f.xml { b"application/xml" } else { b"application/json" },
        ),
        RequestHeader::new(b"content-length", f.length.as_bytes()),
    ];
    let head = RequestHead::new(
        b"POST /echo HTTP/1.1",
        b"POST",
        b"/echo",
        b"/echo",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"profile.example.com",
        &headers,
    );
    Transaction::new_shared_with_policy(
        g.clone(),
        head,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        mode,
        RuleCheckPolicy::default(),
    )
    .unwrap()
}
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    outcome: Result<PhaseOutcome, RuntimeError>,
    body_error: Option<BodyError>,
}
#[derive(Serialize)]
struct Sample {
    whole: Measurement,
    capture: Option<Measurement>,
    inspection: Measurement,
    replay: Option<Measurement>,
    drop: Measurement,
    post_transaction_storage_live_delta: Option<i128>,
    runtime_retained_bytes: usize,
}
async fn inspected(
    tx: &mut Transaction<'static>,
    view: &BodyStorageView,
    f: &Fixture,
) -> Result<PhaseOutcome, RuntimeError> {
    let mut reader = view.snapshot_reader().unwrap();
    if f.xml {
        let mut session = tx.start_xml_inspection(XmlInspectionLimits::default(), Some(f.body.len()))?;
        let mut failure = None;
        while let Some(chunk) = reader.next_chunk().await.unwrap() {
            if failure.is_none() {
                if let Err(error) = session.feed(&chunk) {
                    failure = Some(error);
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        session.complete(f.body.len())?;
        let source = if session.requires_raw_source() {
            Some(view.read_all().await.unwrap())
        } else {
            None
        };
        session.finish(source)?;
    } else {
        let mut session = tx.start_json_inspection(JsonInspectionLimits::default(), Some(f.body.len()))?;
        let mut failure = None;
        while let Some(chunk) = reader.next_chunk().await.unwrap() {
            if failure.is_none() {
                if let Err(error) = session.feed(&chunk) {
                    failure = Some(error);
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        session.complete(f.body.len())?;
        let source = if session.requires_raw_source() {
            Some(view.read_all().await.unwrap())
        } else {
            None
        };
        session.finish(source)?;
    }
    tx.execute_request_body()
}
async fn sample(
    g: &Arc<CompiledGeneration>,
    f: &Fixture,
    mode: RuleEngineMode,
    storage: bool,
    policy: ResolvedRequestBodyConfig,
) -> (Snapshot, Sample) {
    let whole = Stage::start(true);
    let mut capture = None;
    let mut view = None;
    if storage {
        let stage = Stage::start(false);
        let (mut owner, v) = BodyStorage::with_storage_timeout(
            policy.memory_buffer_bytes,
            policy.max_body_size_bytes,
            policy.storage_operation_timeout,
        );
        for start in (0..f.body.len()).step_by(CHUNK_BYTES) {
            owner
                .write(&f.body.slice(start..(start + CHUNK_BYTES).min(f.body.len())))
                .await
                .unwrap();
        }
        owner.finish().await.unwrap();
        drop(owner);
        capture = Some(stage.finish(false));
        view = Some(v);
    }
    let inspect = Stage::start(false);
    let mut tx = transaction(g, f, mode);
    let outcome = match tx.execute_request_headers() {
        Err(e) => Err(e),
        Ok(outcome) if outcome.intervention().is_some() => Ok(outcome),
        Ok(_) => {
            if let Some(view) = &view {
                inspected(&mut tx, view, f).await
            } else {
                tx.publish_request_body(RequestBody::new(f.body.clone(), BodyProcessorSelection::Auto))
                    .and_then(|()| tx.execute_request_body())
            }
        }
    };
    let inspection = inspect.finish(false);
    let body_error = tx.request_body_error();
    let runtime_retained_bytes = tx.retained_bytes();
    let replay = if let Some(v) = &view {
        let stage = Stage::start(false);
        let mut reader = v.snapshot_reader().unwrap();
        let mut len = 0;
        while let Some(chunk) = reader.next_chunk().await.unwrap() {
            len += chunk.len();
            std::hint::black_box(&chunk);
        }
        assert_eq!(len, f.body.len());
        Some(stage.finish(false))
    } else {
        None
    };
    // Snapshot outcome ownership is a measured runtime result; transaction/storage drop follows.
    let releasing = Stage::start(false);
    drop(tx);
    drop(view);
    let drop = releasing.finish(false);
    let measurement = whole.finish(true);
    let post_transaction_storage_live_delta =
        ALLOCATIONS.then(|| LIVE.load(Ordering::Relaxed) as i128 - whole.live as i128);
    (
        Snapshot { outcome, body_error },
        Sample {
            whole: measurement,
            capture,
            inspection,
            replay,
            drop,
            post_transaction_storage_live_delta,
            runtime_retained_bytes,
        },
    )
}
fn classification(s: &Snapshot) -> String {
    match &s.outcome {
        Err(e) => format!("runtime-error:{e:?}"),
        Ok(o) => {
            if s.body_error.is_some() {
                format!("malformed:{:?}", s.body_error)
            } else if o.intervention().is_some() {
                "policy-denied".into()
            } else if o.findings().iter().any(|f| f.rule_id() == 949110) {
                "detected".into()
            } else {
                "allowed".into()
            }
        }
    }
}
fn witness(snapshot: &Snapshot) -> serde_json::Value {
    match &snapshot.outcome {
        Err(error) => {
            serde_json::json!({"runtime_error":format!("{error:?}"),"body_error":format!("{:?}",snapshot.body_error)})
        }
        Ok(outcome) => {
            serde_json::json!({"rule_ids":outcome.findings().iter().map(|f|f.rule_id()).collect::<Vec<_>>(),"intervention":outcome.intervention().map(|i|format!("{i:?}")),"omitted":outcome.omitted_findings(),"body_error":format!("{:?}",snapshot.body_error)})
        }
    }
}
fn main() -> anyhow::Result<()> {
    anyhow::ensure!(MODE == "timing" || MODE == "allocations", "unknown benchmark mode");
    let setup = Stage::start(true);
    let package = profile::load()?;
    let mut generations = Vec::new();
    for p in ["standard", "strict"] {
        generations.push((
            p,
            Arc::new(
                profile::config(&package, p)?
                    .compile_generation()
                    .map_err(anyhow::Error::msg)?,
            ),
        ));
    }
    let setup = setup.finish(true);
    let cases_stage = Stage::start(true);
    let mut cases = fixtures();
    if std::env::var_os("EDGION_WAF_PROFILE_REVERSE").is_some() {
        cases.reverse();
    }
    let fixtures_cost = cases_stage.finish(true);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let policy = ResolvedRequestBodyConfig::resolve(&RequestBodyConfig::default());
    println!(
        "{}",
        serde_json::json!({"kind":"metadata","instrumentation":MODE,"core_digest":package.core.compute_digest(),"provenance_sha256":profile::PROVENANCE_SHA256,"setup":setup,"fixtures":fixtures_cost,"storage_memory_bytes":policy.memory_buffer_bytes,"storage_max_bytes":policy.max_body_size_bytes,"limits":"unchanged RuntimeLimits/JsonInspectionLimits/XmlInspectionLimits defaults","samples":31,"warmups":3,"runtime_error_observations":1,"runtime_error_warmups":0,"sampling":"non-malformed runtime-error lanes have one observation after preflight; successful, policy and malformed lanes retain 31 samples/3 warmups regardless of input size","measurement_end":"transaction/storage dropped; PhaseOutcome still held until caller validates/drops it","session_failure":"latch first feed error, drain snapshot reader to true EOF, then return it","fixture_backing":"excluded from request counters; retained at baseline","allocator":"System","component":"sealed storage; snapshot replay is measured even for denied/failed outcomes, as a separate storage capability"})
    );
    for (p, g) in generations {
        for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
            for f in &cases {
                let (resident, _) = runtime.block_on(sample(&g, f, mode, false, policy));
                let (incremental, _) = runtime.block_on(sample(&g, f, mode, true, policy));
                let parity = resident == incremental;
                for snapshot in [&resident, &incremental] {
                    if let Ok(outcome) = &snapshot.outcome {
                        assert_eq!(outcome.omitted_findings(), 0);
                    }
                }
                if resident.outcome.is_ok() && incremental.outcome.is_ok() {
                    assert!(parity, "non-limit component semantics differ: {}", f.name);
                }
                if f.name.contains("small") {
                    for result in [&resident, &incremental] {
                        assert!(result.body_error.is_none());
                        let outcome = result
                            .outcome
                            .as_ref()
                            .expect("small canonical request runtime success");
                        assert_eq!(outcome.omitted_findings(), 0);
                        let attack = f.name.ends_with("attack");
                        assert_eq!(
                            outcome.findings().iter().any(|finding| finding.rule_id() == 933200),
                            attack
                        );
                        assert_eq!(
                            outcome.findings().iter().any(|finding| finding.rule_id() == 949110),
                            attack
                        );
                        assert_eq!(outcome.intervention().is_some(), attack && mode == RuleEngineMode::On);
                    }
                }
                println!(
                    "{}",
                    serde_json::json!({"kind":"preflight","profile":p,"mode":format!("{mode:?}"),"case":f.name,"semantic_parity":parity,"resident_witness":witness(&resident),"incremental_witness":witness(&incremental),"resident_class":classification(&resident),"incremental_class":classification(&incremental)})
                );
                for storage in [false, true] {
                    let expected = if storage { &incremental } else { &resident };
                    let observation = expected.outcome.is_err() && !f.name.contains("malformed");
                    let samples = if observation { 1 } else { 31 };
                    let warmups = if observation { 0 } else { 3 };
                    let class = classification(expected);
                    for _ in 0..warmups {
                        let (actual, _) = runtime.block_on(sample(&g, f, mode, storage, policy));
                        assert_eq!(&actual, expected);
                    }
                    for index in 0..samples {
                        let (actual, result) = runtime.block_on(sample(&g, f, mode, storage, policy));
                        assert_eq!(&actual, expected, "preflight outcome changed");
                        println!(
                            "{}",
                            serde_json::json!({"kind":"sample","instrumentation":MODE,"profile":p,"mode":format!("{mode:?}"),"case":f.name,"body_bytes":f.body.len(),"lane":if storage{"storage-incremental-replay"}else{"resident"},"storage":if storage && f.body.len()>policy.memory_buffer_bytes{"spill-warm-cache"}else{"memory"},"class":class,"index":index,"sample_count":samples,"warmups":warmups,"sampling":if observation{"runtime-error-observation"}else{"distribution"},"measurement":result})
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
