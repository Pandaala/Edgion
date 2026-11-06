---
name: proxy-shutdown-notification
description: Review Pingora proxy shutdown waiter ordering, worker shards, Edgion listener draining, and P090-005 verification boundaries.
knowledge_type: regression-guards
---

# Proxy shutdown notification and streaming drain

## Conclusion and scope

Edgion's locked Pingora revision
`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a` already contains the
P090-005 implementation. This was re-verified by source inspection at that
revision on 2026-11-06 after the fork was rebased onto upstream 0.9.1: the
`ShardedNotify`, `await_shutdown`, biased `handle_new_request` select, and
`http_cleanup` code is unchanged from `0b0d8bba` (the `edgion_0.9.0_v1` revision this page was last verified against; not the immediate predecessor lock, which was `1aaa45cc`); the
`pingora-proxy/src/lib.rs` diff between the two revisions touches only response
terminal-body dispatch and retry-test setup. Do not add another notification
layer to the Gateway. In `pingora-proxy/src/lib.rs`, `await_shutdown` polls its
shard's waiter before loading the shutdown flag with Acquire ordering;
`http_cleanup` stores the flag with Release ordering and notifies every shard.
The biased request-header branch stays first, so immediately available headers
do not poll the shutdown future or register a waiter.

`ShardedNotify` rounds the worker count to a power of two, capped at 256, with
128-byte aligned shards. Thread-local IDs choose a shard; the registered future
keeps that shard even if a work-stealing runtime moves the task. Shutdown must
therefore traverse every shard, independently for every proxy instance.

Edgion defaults to `work_stealing: true` in `edgion-gateway/src/cli/pingora.rs`.
`runtime/server/listener_builder.rs` constructs separate HTTP proxy services
for listeners, including redirect listeners. The fork owns notification and
connection draining; Edgion owns process configuration and its auxiliary drain.

## Regression coverage

The coverage below existed only in the local fork working tree used for the
2026-09-27 verification. Source inspection on 2026-11-06 found that neither the
current lock `e5f4287` nor the earlier verified revision `0b0d8bba` contains
`pingora-proxy/src/shutdown_tests.rs`; both contain only
`tests::shutdown_wakes_parked_read_requests` (still using the 50 ms sleep) and
`tests::shutdown_before_read_request_parks_returns_immediately` in
`pingora-proxy/src/lib.rs`. Treat the following as unpublished local evidence,
not as regression tests carried by the locked fork. The local
`pingora-proxy/src/shutdown_tests.rs` covered:

- Real cleanup between an HTTP header read returning Pending and the shutdown
  branch's first poll, plus shutdown before the read starts.
- Registered waiters with counted wakeups, every shard, repeated cleanup,
  independent proxy instances, and one future polled on two OS threads.
- An immediately available request completing its local response during shutdown.

In that local working tree, the original two `tests::shutdown_*` identities
remained, with manual polling replacing the old 50 ms sleep. These tests establish specific interleavings;
they do not model-check every atomic interleaving inside `await_shutdown`.
The no-registration fast path also relies on reviewing the biased select order.
Fork-owned details were intended for
`edgion-changes/review/concurrency/sharded-shutdown-notification.md` in the
Pingora repository; that page is absent at both `e5f4287` and `0b0d8bba`.

The standalone
[HTTP shutdown scenario](../../../../edgion-tests/integration/scripts/integration/test_http_process_shutdown.py)
uses two listeners and four workers per service in each scheduling mode. It
opens genuine idle keepalive connections and confirms the first complete SSE
event and gRPC message before sending SIGTERM. It checks idle closure, final
SSE framing, complete gRPC messages with success trailers and END_STREAM, and
bounded process exit. A separate unreleased-stream case verifies forced drain
expiry without accepting a prematurely successful stream as an abort.

## Reproduction

Use a task-local Python environment with `h2` installed. Build the current
Controller and Gateway, then run:

```bash
cargo build --locked -p edgion-controller --bin edgion-controller -p edgion-gateway --bin edgion-gateway
python3 edgion-tests/integration/scripts/integration/test_http_process_shutdown.py
```

The script accepts explicit binary and work-root paths, preserves process logs,
uses private ephemeral ports, and terminates only its own children. It is a
manual standalone acceptance scenario, not implicitly part of the full sweep.

The [manual A/B driver](../../../../edgion-tests/integration/scripts/integration/benchmark_http_shutdown.py)
requires `h2load` and two binaries from the same source/dependency snapshot.
The control changes only the isolated fork's shard count to one; it retains the
correct registration/flag ordering and is **not** the historical broken fork.
Never change the real consumer manifest or lockfile for this measurement.

```bash
python3 edgion-tests/integration/scripts/integration/benchmark_http_shutdown.py \
  --controller-bin /path/to/edgion-controller \
  --sharded-bin /path/to/edgion-gateway-sharded \
  --single-shard-bin /path/to/edgion-gateway-single \
  --profile dev
```

It alternates six samples, records raw h2load and per-request latency logs,
requires no reported failures and matching successful-request counts, and
reports p50/p95/p99 from those records. The timed run measures completed
requests; requests still in flight at its cutoff are not a drain assertion.
The fixture backend and load generator share the host with the Gateway. Debug
builds, Python backend throughput, heterogeneous cores and scheduler placement
limit interpretation; this is not a production capacity estimate. The upstream
`pingora-proxy/benches/shutdown_signal.rs` separately measures registration and
cancellation cost under contention. Its ns/op is a cost proxy, not a measured
mutex wait time or an Edgion speedup claim.

## Local performance evidence

Measurements on 2026-09-27 used an Apple M1 host with eight heterogeneous cores,
Rust 1.96.1, the `dev` profile, four Gateway workers with work stealing, and
h2load 1.47.0 (`--h1 -t4 -c64 -m1 -D5 --warm-up-time=1`). No compiler ran during
sampling. Both binaries use the same consumer and fork working-tree snapshots,
including pre-existing changes and test additions. The sole control difference
is `MAX_SHUTDOWN_NOTIFY_SHARDS` changing from 256 to 1 in an isolated fork copy.
The maintained manifests, lockfile and production source remain unchanged.

| Sample | Variant | Requests/s | p50 (ms) | p95 (ms) | p99 (ms) |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | Sharded | 4393.8 | 7.582 | 66.868 | 123.086 |
| 2 | Single shard | 3319.4 | 7.502 | 90.730 | 209.195 |
| 3 | Single shard | 4917.8 | 7.241 | 53.321 | 96.804 |
| 4 | Sharded | 2481.6 | 7.796 | 120.256 | 233.899 |
| 5 | Sharded | 1248.4 | 11.442 | 165.538 | 350.278 |
| 6 | Single shard | 3614.0 | 7.230 | 96.709 | 134.314 |

Every sample reported zero failed/errored/timed-out requests; all logged statuses
were 200. The wide variation and shared Python backend prevent attributing an
end-to-end speedup or regression to notification sharding. CPU affinity was not
pinned. These are local acceptance measurements, not production sizing evidence.
Raw results and per-request records are in
`tmp/p090-005/measurements/http-shutdown-benchmark.dptvdmws`.
`tmp/p090-005/variants.json`, `consumer-snapshot.json`, `fork-snapshot.json`,
`build-metadata.json`, and `single-shard-control.diff` retain binary hashes,
source provenance, dependency resolution, and the exact control change. These
ignored local artifacts are not required to build or run the regression tests.

The unchanged upstream `pingora-proxy/benches/shutdown_signal.rs` was run via a
minimal Cargo bench harness pointing directly to that source file, with futures
0.3.34 and Tokio 1.53.1. Its optimized binary used 200000 iterations per thread,
seven samples (median elapsed time), and up to eight threads (`PINGORA_BENCH_ITERATIONS=200000`,
`PINGORA_BENCH_SAMPLES=7`, `PINGORA_BENCH_MAX_THREADS=8`). Reported ns/op:

| Threads | Single Notify | ShutdownWatch | Sharded poll flag | Sharded enable |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 48.2 | 73.3 | 49.9 | 59.2 |
| 2 | 91.7 | 126.1 | 25.2 | 31.5 |
| 4 | 148.7 | 175.7 | 14.8 | 17.4 |
| 8 | 162.0 | 202.1 | 10.1 | 16.5 |

Raw output: `tmp/p090-005/microbench-final.txt`; harness:
`tmp/p090-005/microbench/Cargo.toml`. This isolates notification contention cost;
it does not measure mutex waiting time or predict the Gateway load result.

Separate stack diagnostics used `/usr/bin/sample <owned-pid> 3 1` during
`h2load --h1 -t4 -c16 -m1 -n100000` for each variant. Both completed 100000
successful requests with zero reported failures. The sharded sample captured
notification poll/drop frames; the single-shard sample did not resolve a
notification frame. This sampling resolution cannot quantify Notify lock wait
or establish absence of contention. Raw stacks and load logs are in
`tmp/p090-005/stack-profile.m01wgkgw`. Two earlier diagnostic attempts were
rejected: timed-load cancellation caused a backend BrokenPipe, and a 64-client
fixed-count run returned 58 HTTP 5xx responses. Their data is not treated as
successful profiling evidence or mixed into the unprofiled A/B measurements.

## Verification and re-evaluation

Historical local verification on 2026-09-27 (before the fork rebase onto
upstream 0.9.1; not repeated at `e5f4287`) used Edgion
`862d5add79bd7daa6830fbe7a8fd02f05780373e` plus its existing worktree changes,
with the unchanged real consumer lock selecting fork `0b0d8bb`. No manifest,
lockfile, production notification code, commit or remote branch was changed.

- Fork shutdown tests: 7 passed. Isolated negative controls removing the flag
  check and notifying only shard zero failed the intended assertions; the
  original code was recompiled and the positive suite passed again.
- Full fork proxy library: 228 passed, 2 existing manual benchmarks ignored.
  Fork all-target check and Clippy passed; seven unrelated existing Clippy
  warnings remain. These runs used the local fork worktree, including its
  pre-existing changes, not a claim that those changes are published.
- Real Edgion process tests used newly built binaries against the locked fork.
  Both scheduling modes passed normal completion and withheld-response abort.
  Each case used eight idle keepalives, four SSE streams and four gRPC streams
  across two listeners. Normal responses were released one second after SIGTERM,
  within the two-second grace window. End-to-end exit times were 7.169, 7.141,
  7.193 and 7.128 seconds respectively (including auxiliary service shutdown).
  Raw logs: `tmp/p090-005/process-tests/http-shutdown.tacueb31`.
- Edgion workspace all-target check and Clippy passed, as did formatting,
  agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  Gateway API transform contract and unit-test layout guards. The full
  workspace test suite was not run.

CMake 3.21.2 initially prevented the BoringSSL build; a task-local CMake 3.31.10
resolved it. `PROTOC` selected an existing local protoc 31.1, and the isolated
Python environment supplied `h2` and PyYAML. The initial environment failures
are not reported as passing tests. The HTTP fixture separately validates legal
GOAWAY before feeding remaining frames to hyper-h2, whose closed state otherwise
rejects legal in-flight drain frames. EOF/reset is distinct from content
assertions, so a premature successful gRPC END_STREAM cannot pass the abort case.

Re-evaluate when
the pinned fork, Tokio notification contract, biased header-read order,
service/runtime topology, worker mapping, or grace/drain policy changes.

## Resolution reference

- P090-005, sharded proxy shutdown notification adoption and verification.
