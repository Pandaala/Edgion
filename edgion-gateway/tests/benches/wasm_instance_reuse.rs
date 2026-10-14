//! Isolated Wasmtime lifecycle comparison, not a gateway throughput benchmark.
//! See tasks/todo/wasm-execution-research/02-instance-reuse-benchmark.md.
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use edgion_gateway::plugins::http::wasm::host_abi::{add_to_linker, HostState, WasmPhase};
use edgion_gateway::plugins::http::wasm::shared_data::SharedData;
use serde_json::json;
use wasmtime::{
    Config, Engine, Instance, InstanceAllocationStrategy, InstancePre, Linker, Module, PoolingAllocationConfig, Store,
    TypedFunc,
};

// Binary-owned System allocator. Allocation instrumentation is opt-in at compile
// time; instrumented timings must not be compared with uninstrumented timings.
const COUNT: bool = option_env!("EDGION_WASM_BENCH_ALLOCATIONS").is_some();
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
struct Allocator;
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
fn add(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}
// SAFETY: all pointer/layout contracts are forwarded unchanged to System;
// counters do not allocate or access allocated contents.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc contract.
        let p = unsafe { System.alloc(layout) };
        if COUNT && !p.is_null() {
            add(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc contract.
        let p = unsafe { System.alloc_zeroed(layout) };
        if COUNT && !p.is_null() {
            add(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: pointer and layout belong to the forwarding allocator.
        unsafe { System.dealloc(p, layout) };
        if COUNT {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: forwarded allocation and new-size contracts.
        let result = unsafe { System.realloc(p, layout, size) };
        if COUNT && !result.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            add(size);
        }
        result
    }
}

fn ns(start: Instant) -> u64 {
    Instant::now()
        .saturating_duration_since(start)
        .as_nanos()
        .min(u64::MAX as u128) as u64
}
#[derive(Default)]
struct Timings(BTreeMap<&'static str, Vec<u64>>);
impl Timings {
    fn record(&mut self, key: &'static str, start: Instant) {
        self.0.entry(key).or_default().push(ns(start));
    }
    fn report(&mut self) -> serde_json::Value {
        let mut result = serde_json::Map::new();
        for (name, values) in &mut self.0 {
            values.sort_unstable();
            let percentile = |p: usize| values[(values.len() * p).div_ceil(100).saturating_sub(1)];
            result.insert((*name).into(), json!({"count":values.len(), "p50_ns":percentile(50), "p95_ns":percentile(95), "p99_ns":percentile(99)}));
        }
        result.into()
    }
}

struct Ticker {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Ticker {
    fn new(engine: Engine) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let thread = thread::Builder::new().name("wasm-bench-epoch".into()).spawn(move || {
            while !signal.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(10));
                engine.increment_epoch();
            }
        })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// Controlled guest assertions execute inside Wasm. One live context per VM;
// headers/body/chunks must preserve it. Root request counters distinguish warm
// reuse from fresh instances without relying on host-side context IDs alone.
fn guest(expensive: bool) -> Result<Vec<u8>> {
    let pages = if expensive { 64 } else { 1 };
    let init = if expensive {
        "(memory.fill (i32.const 65536) (i32.const 7) (i32.const 4128768))"
    } else {
        ""
    };
    Ok(wat::parse_str(format!(
        r#"(module
      (memory (export "memory") {pages})
      (global $active (mut i32) (i32.const 0))
      (global $requests (mut i32) (i32.const 0))
      (global $phase (mut i32) (i32.const 0))
      (global $sum (mut i32) (i32.const 0))
      (global $deletes (mut i32) (i32.const 0))
      (func (export "proxy_abi_version_0_2_0"))
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "_initialize") {init})
      (func (export "proxy_on_context_create") (param $id i32) (param i32)
        (if (i32.ne (local.get $id) (i32.const 1)) (then
          (if (global.get $active) (then unreachable))
          (global.set $active (local.get $id)) (global.set $phase (i32.const 0)))))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func $check (param $id i32)
        (if (i32.ne (local.get $id) (global.get $active)) (then unreachable)))
      (func (export "proxy_on_request_headers") (param $id i32) (param i32 i32) (result i32)
        (call $check (local.get $id))
        (global.set $requests (i32.add (global.get $requests) (i32.const 1)))
        (global.set $phase (i32.const 1)) (i32.const 0))
      (func (export "proxy_on_request_body") (param $id i32) (param $size i32) (param i32) (result i32)
        (local $i i32)
        (call $check (local.get $id))
        (if (i32.ne (global.get $phase) (i32.const 1)) (then unreachable))
        (global.set $sum (i32.const 0))
        (block $end (loop $read
          (br_if $end (i32.ge_u (local.get $i) (local.get $size)))
          (global.set $sum (i32.add (global.get $sum) (i32.load8_u (i32.add (i32.const 1024) (local.get $i)))))
          (local.set $i (i32.add (local.get $i) (i32.const 1))) (br $read)))
        (global.set $phase (i32.const 2)) (i32.const 0))
      (func (export "proxy_on_response_headers") (param $id i32) (param i32 i32) (result i32)
        (call $check (local.get $id)) (i32.const 0))
      (func (export "proxy_on_response_body") (param $id i32) (param i32 i32) (result i32)
        (call $check (local.get $id)) (i32.const 0))
      (func (export "proxy_on_done") (param $id i32) (result i32)
        (call $check (local.get $id)) (i32.const 1))
      (func (export "proxy_on_delete") (param $id i32)
        (call $check (local.get $id)) (global.set $active (i32.const 0))
        (global.set $deletes (i32.add (global.get $deletes) (i32.const 1))))
      (func (export "requests") (result i32) (global.get $requests))
      (func (export "deletes") (result i32) (global.get $deletes))
      (func (export "sum") (result i32) (global.get $sum))
      (func (export "trap") unreachable))"#
    ))?)
}

type HttpHook = TypedFunc<(i32, i32, i32), i32>;
struct Vm {
    store: Store<HostState>,
    instance: Instance,
    create: TypedFunc<(i32, i32), ()>,
    headers: HttpHook,
    body: HttpHook,
    response: HttpHook,
    chunk: HttpHook,
    done: TypedFunc<i32, i32>,
    delete: TypedFunc<i32, ()>,
    requests: TypedFunc<(), i32>,
    deletes: TypedFunc<(), i32>,
    sum: TypedFunc<(), i32>,
    count: i32,
}
impl Vm {
    fn new(
        engine: &Engine,
        pre: &InstancePre<HostState>,
        shared: Arc<SharedData>,
        timings: &mut Timings,
    ) -> Result<Self> {
        let t = Instant::now();
        let mut store = Store::new(engine, HostState::new(Vec::new(), Vec::new(), String::new(), shared));
        store.limiter(|s| s);
        store.set_epoch_deadline(102); // 1000 ms + production epoch slack.
        timings.record("store", t);
        let t = Instant::now();
        let instance = pre.instantiate(&mut store)?;
        timings.record("instantiate_and_start", t);
        let t = Instant::now();
        for name in ["_initialize", "_start"] {
            if let Ok(f) = instance.get_typed_func::<(), ()>(&mut store, name) {
                f.call(&mut store, ())?;
                break;
            }
        }
        timings.record("guest_initializer", t);
        let t = Instant::now();
        let create = instance.get_typed_func::<(i32, i32), ()>(&mut store, "proxy_on_context_create")?;
        create.call(&mut store, (1, 0))?;
        for name in ["proxy_on_vm_start", "proxy_on_configure"] {
            let f = instance.get_typed_func::<(i32, i32), i32>(&mut store, name)?;
            ensure!(f.call(&mut store, (1, 0))? != 0, "guest rejected initialization");
        }
        timings.record("root_start_configure", t);
        let t = Instant::now();
        let vm = Self {
            headers: instance.get_typed_func(&mut store, "proxy_on_request_headers")?,
            body: instance.get_typed_func(&mut store, "proxy_on_request_body")?,
            response: instance.get_typed_func(&mut store, "proxy_on_response_headers")?,
            chunk: instance.get_typed_func(&mut store, "proxy_on_response_body")?,
            done: instance.get_typed_func(&mut store, "proxy_on_done")?,
            delete: instance.get_typed_func(&mut store, "proxy_on_delete")?,
            requests: instance.get_typed_func(&mut store, "requests")?,
            deletes: instance.get_typed_func(&mut store, "deletes")?,
            sum: instance.get_typed_func(&mut store, "sum")?,
            store,
            instance,
            create,
            count: 0,
        };
        timings.record("resolve_exports", t);
        Ok(vm)
    }
    fn arm(&mut self) {
        self.store.set_epoch_deadline(102);
    }
    fn open(&mut self, has_body: bool) -> Result<()> {
        self.arm();
        self.create.call(&mut self.store, (self.count + 2, 1))?;
        self.store
            .data_mut()
            .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
        ensure!(
            self.headers
                .call(&mut self.store, (self.count + 2, 0, i32::from(!has_body)))?
                == 0,
            "headers paused"
        );
        self.count += 1;
        ensure!(
            self.requests.call(&mut self.store, ())? == self.count,
            "root continuity failed"
        );
        self.store.data_mut().release_phase_payloads();
        Ok(())
    }
    fn finish(&mut self, body: &[u8], chunks: usize) -> Result<()> {
        if !body.is_empty() {
            self.arm();
            self.store
                .data_mut()
                .begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
            self.store.data_mut().frame.request_body = body.to_vec();
            // Controlled guest input, not BodyStorage or host ABI copy timing.
            self.instance
                .get_memory(&mut self.store, "memory")
                .context("guest memory")?
                .write(&mut self.store, 1024, body)?;
            ensure!(
                self.body
                    .call(&mut self.store, (self.count + 1, body.len() as i32, 1))?
                    == 0,
                "body paused"
            );
            ensure!(
                self.sum.call(&mut self.store, ())? == body.iter().map(|b| i32::from(*b)).sum::<i32>(),
                "body checksum failed"
            );
            self.store.data_mut().release_phase_payloads();
        }
        self.arm();
        self.store
            .data_mut()
            .begin_phase(WasmPhase::ResponseHeaders, vec![], vec![], HashMap::new());
        ensure!(
            self.response
                .call(&mut self.store, (self.count + 1, 0, i32::from(chunks == 0)))?
                == 0,
            "response paused"
        );
        self.store.data_mut().release_phase_payloads();
        for index in 0..chunks {
            self.arm();
            self.store
                .data_mut()
                .begin_phase(WasmPhase::ResponseBody, vec![], vec![], HashMap::new());
            self.store.data_mut().frame.body_chunk = bytes::Bytes::from_static(b"data: event\n\n");
            ensure!(
                self.chunk
                    .call(&mut self.store, (self.count + 1, 13, i32::from(index + 1 == chunks)))?
                    == 0,
                "chunk paused"
            );
            self.store.data_mut().release_phase_payloads();
        }
        self.arm();
        ensure!(
            self.done.call(&mut self.store, self.count + 1)? != 0,
            "deferred completion unsupported"
        );
        self.delete.call(&mut self.store, self.count + 1)?;
        ensure!(
            self.deletes.call(&mut self.store, ())? == self.count,
            "terminal count mismatch"
        );
        self.store.data_mut().release_phase_payloads();
        Ok(())
    }
}

fn arg(args: &[String], name: &str, default: usize, max: usize) -> Result<usize> {
    let Some(index) = args.iter().position(|s| s == name) else {
        return Ok(default);
    };
    let value: usize = args.get(index + 1).context("missing numeric argument")?.parse()?;
    let minimum = usize::from(name != "--hold-ms" && name != "--chunks");
    ensure!((minimum..=max).contains(&value), "{name} must be in {minimum}..={max}");
    Ok(value)
}
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cursor = 0;
    while cursor < args.len() {
        match args[cursor].as_str() {
            "--mode" | "--waves" | "--resident" | "--chunks" | "--hold-ms" => {
                ensure!(cursor + 1 < args.len(), "missing value for {}", args[cursor]);
                cursor += 2;
            }
            "--expensive" | "--body" | "--bench" => cursor += 1,
            unknown => bail!("unknown argument: {unknown}"),
        }
    }
    let mode = args
        .iter()
        .position(|s| s == "--mode")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("fresh");
    ensure!(matches!(mode, "fresh" | "warm"), "--mode must be fresh or warm");
    // Defaults intentionally small, including when Cargo runs this harness.
    let waves = arg(&args, "--waves", 3, 1000)?;
    let resident = arg(&args, "--resident", 8, 64)?;
    let chunks = arg(&args, "--chunks", 8, 1024)?;
    let hold_ms = arg(&args, "--hold-ms", 0, 1000)?;
    let expensive = args.iter().any(|s| s == "--expensive");
    let body = if args.iter().any(|s| s == "--body") {
        vec![7_u8; 32 * 1024]
    } else {
        vec![]
    };
    let mut config = Config::new();
    config.epoch_interruption(true);
    config.allocation_strategy(InstanceAllocationStrategy::Pooling(PoolingAllocationConfig::default()));
    let engine = Engine::new(&config)?;
    let _ticker = Ticker::new(engine.clone())?;
    let bytes = guest(expensive)?;
    let mut setup = Timings::default();
    let t = Instant::now();
    let module = Module::new(&engine, &bytes)?;
    setup.record("compile", t);
    let t = Instant::now();
    let mut linker = Linker::new(&engine);
    add_to_linker(&mut linker)?;
    let pre = linker.instantiate_pre(&module)?;
    setup.record("link_and_prelink", t);
    let shared = Arc::new(SharedData::new(String::new()));
    // A trap is a discard, never a pooled candidate or a cleanup callback.
    {
        let mut vm = Vm::new(&engine, &pre, shared.clone(), &mut Timings::default())?;
        vm.open(!body.is_empty())?;
        let trap = vm.instance.get_typed_func::<(), ()>(&mut vm.store, "trap")?;
        ensure!(trap.call(&mut vm.store, ()).is_err(), "trap fixture did not trap");
        drop(vm);
    }
    let mut pool = Vec::with_capacity(resident);
    let mut created = 0;
    let mut hits = 0;
    let mut timings = Timings::default();
    let mut records = Vec::with_capacity(waves);
    for wave in 0..waves {
        let start = Instant::now();
        let mut active = Vec::with_capacity(resident);
        for _ in 0..resident {
            let t = Instant::now();
            let mut vm = if let Some(vm) = pool.pop() {
                hits += 1;
                vm
            } else {
                created += 1;
                Vm::new(&engine, &pre, shared.clone(), &mut timings)?
            };
            timings.record("acquire", t);
            let t = Instant::now();
            vm.open(!body.is_empty())?;
            timings.record("open_headers", t);
            active.push(vm);
        }
        // Synthetic residency only; no network, Tokio scheduling or backpressure.
        if hold_ms > 0 {
            thread::sleep(Duration::from_millis(hold_ms as u64));
        }
        let logical_guest_bytes_active: usize = active
            .iter_mut()
            .map(|vm| {
                vm.instance
                    .get_memory(&mut vm.store, "memory")
                    .expect("fixture exports memory")
                    .data_size(&vm.store)
            })
            .sum();
        let heap_live_active = COUNT.then(|| LIVE.load(Ordering::Relaxed));
        for mut vm in active {
            let t = Instant::now();
            vm.finish(&body, chunks)?;
            timings.record("body_response_terminal", t);
            let t = Instant::now();
            if mode == "warm" {
                pool.push(vm);
            } else {
                drop(vm);
            }
            timings.record("release_or_drop", t);
        }
        records.push(json!({"wave":wave, "wall_ns":ns(start), "idle_instances":pool.len(),
            "logical_guest_bytes_active":logical_guest_bytes_active,
            "logical_guest_bytes_idle":if mode == "warm" { logical_guest_bytes_active } else { 0 },
            "host_requested_live_active":heap_live_active,
            "host_requested_live_idle":COUNT.then(||LIVE.load(Ordering::Relaxed))}));
    }
    let t = Instant::now();
    drop(pool);
    timings.record("pool_drain", t);
    if mode == "fresh" {
        ensure!(created == resident * waves && hits == 0, "fresh reused a guest");
    } else {
        ensure!(
            created == resident && hits == resident * (waves - 1),
            "warm reuse mismatch"
        );
    }
    println!(
        "{}",
        json!({"benchmark":"isolated_wasm_instance_reuse", "mode":mode,
        "allocator":"System", "allocation_instrumentation":COUNT, "os":std::env::consts::OS,
        "arch":std::env::consts::ARCH, "parallelism":thread::available_parallelism()?.get(),
        "profile":"serial_callbacks_resident_waves", "waves":waves, "resident":resident,
        "expensive_init":expensive, "body_bytes":body.len(), "chunks":chunks, "hold_ms":hold_ms,
        "instances_created":created, "pool_hits":hits, "setup":setup.report(),
        "timings":timings.report(), "wave_records":records,
        "host_requested_peak":COUNT.then(||PEAK.load(Ordering::Relaxed)),
        "rss_bytes":null, "guest_resident_bytes":null, "virtual_reservation_bytes":null})
    );
    Ok(())
}
