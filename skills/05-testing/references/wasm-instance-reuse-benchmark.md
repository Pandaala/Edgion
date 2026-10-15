# Wasm instance reuse benchmark

The isolated lifecycle benchmark is
[wasm_instance_reuse.rs](../../../edgion-gateway/tests/benches/wasm_instance_reuse.rs).
It compares fresh instances and warm exclusive leases using production HostState,
ResourceLimiter, ABI linker and SharedData with the locked Wasmtime version.
It does not select a production execution policy or measure Gateway throughput.
The binary owns a System allocator independently of Gateway allocator features.

## Running the comparison

Run each candidate in a separate process with identical arguments:

```sh
cargo bench -j 1 -p edgion-gateway --bench wasm_instance_reuse -- --mode fresh
cargo bench -j 1 -p edgion-gateway --bench wasm_instance_reuse -- --mode warm
```

An initialization/body profile:

```sh
cargo bench -j 1 -p edgion-gateway --bench wasm_instance_reuse -- --mode fresh --waves 30 --resident 32 --expensive --body --chunks 64
cargo bench -j 1 -p edgion-gateway --bench wasm_instance_reuse -- --mode warm --waves 30 --resident 32 --expensive --body --chunks 64
```

Arguments: `--waves` 1..1000 (default 3), `--resident` 1..64 (default 8),
`--chunks` 0..1024 (default 8), `--hold-ms` 0..1000 (default 0), `--body`
(32 KiB checksum fixture), and `--expensive` (4 MiB guest with an initializer
touching all but its first page). The minimal guest uses one 64 KiB page.
A hold is one synthetic sleep per wave outside guest execution, with all bindings
alive; it does not simulate upload, callout or SSE transport.

Allocation counting is a separate compile-time mode:

```sh
EDGION_WASM_BENCH_ALLOCATIONS=1 cargo bench -j 1 -p edgion-gateway --bench wasm_instance_reuse -- --mode warm --expensive
```

Any presence of that variable enables counting; unset it for timing runs. Never
compare instrumented timings with uninstrumented timings. For peak RSS wrap the
built executable, rather than Cargo, in `/usr/bin/time -l` on macOS or
`/usr/bin/time -v` on Linux. Record revision, hardware, OS, build settings and
arguments. Repeat independent processes in alternating or randomized order.

## Interpreting output

Each process emits one JSON object. `setup` separates compilation and prelinking;
Engine creation, WAT parsing and the trap preflight are outside those measurements.
`timings` separates Store creation, instantiation/start, initializer, root setup,
export resolution, acquisition, headers, combined body/response/terminal work,
release and final pool drain. Acquisition includes initialization on misses;
do not add nested stage totals.

Percentiles are nearest-rank sample statistics without confidence intervals or
warmup exclusion. Acquisition includes the first application-pool-empty wave;
warm medians mostly measure the harness Vec pop, omitting production locking,
admission and metrics. `resident` means simultaneously live bindings driven
serially on one thread, rather than parallel execution or offered-load throughput.
Fresh creates `waves * resident` instances; warm creates `resident` and reuses
healthy closed guests. The trap preflight has already warmed a Wasmtime slot,
so neither mode is a cold-process measurement.

Logical guest bytes come from exported memory size, not resident pages. Optional
host counters include process-wide requested Rust allocations from engine, setup
and harness, excluding allocator overhead and Wasmtime mmap reservations. RSS,
resident guest pages and virtual reservation are emitted as `null`. External RSS
is process-level; Store destruction need not immediately lower it.

Guest assertions cover one active context, headers/body continuity, checksum,
root request counts and one deletion per completed request. An untimed trap
preflight discards an open instance without cleanup. Calls use epoch interruption,
the limiter is installed before instantiation, frames release after each phase,
and deadlines re-arm after synthetic waits. Assertion/probe costs are included
equally in measured driving.

The harness omits production admission/pool locking, timer roots, publication,
generation overlap, network callouts, BodyStorage spill, actual ABI buffer-get
allocation, real SDK workload, open-loop load, work stealing, shared-VM scheduling
and HTTP transport. Its body callback safety deadline is not the production
reduced budget. A production winner requires equal full-lifecycle semantics,
workload SLOs, throughput/rejections and CPU/latency/host heap/RSS measurements
across slow uploads/callouts, SSE, sparse traffic and generation overlap. See
[execution-model constraints](../../04-review/wasm/regression-guards/request-execution-lifecycle.md#execution-model-selection-and-measurement).

## Historical smoke evidence

On 2026-10-14, fresh/warm runs passed for minimal, expensive body/chunks/wait,
single-resident and 64-resident profiles; five invalid CLI cases were rejected.
Independent plan and final reviews passed, including reviewer execution of both
modes. Workspace check and Clippy exited zero with eight unrelated existing
warning sites; formatting and applicable repository guards passed. These are
implementation-time records, not fresh results from task cleanup.

On macOS/aarch64 with 10 available CPUs and other active builds, an instrumented
expensive/body run (three waves, eight residents, eight chunks) reported:

| Mode | Created | Pool hits | Final idle logical guest bytes | Host requested peak bytes |
|---|---:|---:|---:|---:|
| fresh | 24 | 0 | 0 | 1373572 |
| warm | 8 | 16 | 33554432 | 961447 |

These single-process samples have no confidence intervals and establish no
memory or performance winner. The measured source SHA-256 was
`505b266b53628a945b7e3b6073b19065494b07791fb580129d1662491cba40ca`;
later source edits require revalidation before reusing measurements.
