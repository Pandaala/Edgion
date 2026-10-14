---
name: bounded-oci-artifact-memory-cache
description: Review OCI artifact memory-cache budgets, idle eviction, ownership, and cancellation separately from compiled Wasm runtime state.
knowledge_type: intentional-design
---

# Bounded OCI artifact memory cache

## Conclusion and scope

For D0074 the user explicitly selected process-memory OCI artifact caching
and removal of the disk cache on 2026-09-08. This replaces the original
proposal to offload disk reads and writes. The accepted cost is re-downloading
artifacts after restart. Digest-pinned references remain immutable, but are
not an offline restart guarantee.

The owning implementation is `edgion-gateway/src/plugins/http/wasm/oci/cache.rs`.
`OciMemoryCacheConfig` limits retained payload allocation bytes and entry
count, with idle TTL and periodic physical cleanup. Zero bytes or entries
disables retention; it never means unlimited. The wrapper reuses the
canonical `ShardedLruCache` and does not introduce another eviction core.

This applies only to verified manifests and blobs. Compiled `ModuleEntry`,
VM leases, tick tasks, and guest `SharedData` retain their existing
reference-based lifetimes. Expiring artifacts must not evict active runtimes
or guest shared-data keys.

## Rationale and evidence

The former `load` read files and hashed them synchronously; `store` scanned,
sorted, evicted, and wrote files directly from async OCI acquisition. The
new product decision removes those operations altogether.

Envoy's [Wasm code cache](https://github.com/envoyproxy/envoy/blob/main/source/extensions/common/wasm/wasm.cc)
is a reference for digest-keyed in-memory code and separation from runtime
initialization. Its [remote fetcher](https://github.com/envoyproxy/envoy/blob/main/source/common/config/remote_data_fetcher.cc)
uses asynchronous HTTP. These sources were inspected on 2026-09-08; they do
not establish a weighted memory cap, zero-copy ownership, or an offloaded
hashing guarantee. Edgion's stricter resource budgets follow its own bounded
memory and canonical-cache rules in `skills/00-design-rules.md` and `AGENTS.md`.

Required review boundaries:

- Charge the capacity of each accepted payload allocation once; clones share
  ownership. Eviction cannot release the charge while a reader holds bytes.
  Final-drop accounting must not lock the admission mutex.
- Admission is best-effort. Byte pressure may leave a valid result uncached.
  Four full OCI acquisition pipelines independently bound transient work,
  including tag refresh, hashing, extraction, and compile consumption.
  Blocking closures retain the pipeline permit after caller cancellation.
- Retained allocation budget is not total process RSS: compressed and
  extracted bytes can coexist; Vec capacity, transport and parser allocations,
  compiled code, VM pools, and guest memory need their own accounting.
- Fixed semaphore stripes coordinate digest misses without unbounded key
  state or mutex guards across network I/O. Deduplication depends on retention;
  disabled or bypassed entries can be fetched again by subsequent callers.
- Only digest-verified successes are shared. Credential failures are not
  artifact-cache negative entries. Existing per-runtime-key negative caching,
  trust/egress checks, and last-known-good publication remain unchanged.
- Cache owners are paired with every Wasm plugin registration/unregistration.
  Last-owner removal clears entries and prevents late ownerless admission.
  Debug store-stats exposes entries, keys and allocation bytes, including
  allocations still pinned after cache removal.

### Manifest and archive CPU boundary (D0075)

The memory-cache implementation also resolves D0075's previously synchronous
manifest parsing and gzip/tar extraction. Both manifest/index parse call sites
and compatibility extraction in `oci/mod.rs::fetch_oci_inner` invoke
`blocking_stage`. The helper moves the work closure into `spawn_blocking`;
artifact handles share their immutable allocation rather than copying bodies.
`vm_manager.rs::load_inner_with_linker` consumes OCI bytes through the all-source
`admission::prepare_blocking` helper for digest validation, compatibility analysis,
and compilation. It retains the OCI pipeline permit beside the result as well as
the process preparation permit; cancellation releases neither before the work
and its result are destroyed.

The pipeline permit survives asynchronous cancellation inside the blocking
closure and beside its output. Abandoned output drops before the permit,
preventing admission of another pipeline while those buffers are still owned.
Blocking work returns results; the async runtime owner retains publication,
single-flight, negative-cache, and last-known-good decisions.

The existing 4 MiB manifest, 64 MiB blob/output/decompressed-stream limits,
4,096-descriptor/header limits, and archive metadata/path bounds still apply.
First-index selection and the bounded layer-descriptor scan remain in the
async owner; neither parses JSON nor inflates archive bytes. No separate
parser, cache, logging, or metrics mechanism is needed for this offload.

Envoy comparison rechecked on 2026-09-09: in the remote-code path linked above,
`RemoteDataFetcher::fetch` starts async HTTP; `onSuccess` hashes the response
inline and passes a string to its callback. `createWasm` separates acquisition
and code-cache state from VM creation. These inspected functions contain no
OCI manifest parsing or tar extraction, and do not demonstrate CPU-pool
offload or zero-copy transfer. They support the acquisition/publication
separation, not copying their buffer or thread model into Edgion.

## Alternatives and limits

Disk persistence and its offload/temporary-file/shutdown machinery were
rejected for this task by the user. Old files are neither reused nor deleted.
An external agent/cache service would add a deployment component and is not
needed for this process-local change. TinyUFO does not supply the required
explicit removal semantics; the canonical LRU is used instead.

A reload that temporarily removes the last plugin can lose artifact reuse;
compiled-module grace and active requests remain unaffected. Content-address
reuse is an integrity mechanism, not a fresh registry authorization check
on each cache hit; it preserves the previous digest-cache trust scope.

## Verification and re-evaluation

Fresh verification on 2026-09-08:

- The Wasm unit subset passed 214 tests. After strengthening blocking-output
  drop ordering, all five pipeline regressions passed again. The configuration
  defaults and store-stats serialization tests each passed separately.
- The revised plan and two fresh independent code/knowledge reviews received
  LGTM. Reviewers examined cancellation, allocation accounting, owner generations,
  LRU/TTL, configuration, and the limits of the recorded evidence.
- Workspace all-target `cargo check` and `cargo clippy` passed. Clippy reports
  existing warnings; this is not a warning-free workspace claim. Format,
  agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  Gateway no-tracing, English-only, and diff checks passed.
- All 1,987 files under the frozen integration and harness source roots matched
  their pre-change hashes. No full integration topology or `cargo test --all`
  was run.
- The existing `cache_residue` executable was run against an ephemeral HTTP
  stats fixture: nonzero OCI entries/allocation bytes failed with exit 1;
  zero counters and empty keys passed with exit 0. This verifies recognition
  of the new stats fields, not real Gateway/Controller wipe-all behavior.

The locked Gateway binary build also passed. Historical disk-cache tests do
not verify the replacement implementation.

Live follow-up on 2026-09-09 used real Controller/Gateway/backend processes
and a local `registry:2`: the existing Wasm suite passed 9/9; raw and tar.gz
OCI pulls both preserved guest request/response headers. After stopping the
registry and changing `vmConfig` to force a new runtime key, the suite again
passed 9/9 using pinned artifacts in memory. Real no-wipe residue detection
returned 1 and identified four OCI entries / 195746 retained bytes; normal
wipe-all deleted 31 resources without errors, returned 0 from the validator,
and a separate stats assertion confirmed zero entries and bytes. The standard
runner itself was blocked by missing macOS loopback aliases; these were targeted
tests through the existing startup script/client, not a full runner PASS.
Private-registry, Always-refresh, and offline-restart scenarios were not run.

D0075 closure verification on 2026-09-09:

- All 214 Wasm library tests passed again, including parser/extractor limits,
  pipeline cancellation and output-drop ordering, single-flight, and LKG tests.
- Format, workspace all-target check/Clippy, and the locked Gateway binary
  build passed. Clippy emitted existing warnings in unchanged code.
- Agent-doc, SSA-force, metrics-inventory, Pingora source-policy, Gateway
  no-tracing, English-only, and diff guards passed. All 3,209 files under
  `edgion-tests` matched a snapshot taken before this closure audit; this does
  not reconstruct the original B0112 baseline.
- Independent source and knowledge review returned LGTM. This closure changes
  only knowledge and task records; no new live integration run or full-workspace
  test suite was performed.

Re-evaluate if offline restart is required, bytecode becomes unusually large,
source-specific authorization is required on every cache hit, or the product
requires a global compiled-runtime memory budget. Do not represent those
different requirements as already solved by artifact eviction.

## Resolution references

- D0074 — originally deferred as disk-I/O offload; changed by explicit user
  decision to bounded memory artifacts and resolved on 2026-09-08.
- D0075 — manifest parsing and archive CPU offload, discovered in B0112.
  The D0074 implementation supplies its ownership-preserving blocking
  boundary; closure review concerns existing behavior, not a new runtime change.
- Historical `contract-03` disk-purge interval clamp is superseded; see
  [historical evidence](../../cpu-memory/superseded/wasm-oci-purge-interval-zero-clamp.md).
