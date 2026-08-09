# Implementation record

## Status

Implementation, documentation, independent review, repository checks, and the
external DslBody integration run are complete.

## Implementation Result

- Modified files: DSL core, request-body backend, session/VM integration,
  resources validation, integration fixtures, and English/Chinese
  documentation are complete.
- Core implementation:
  - `Value::Body` and bytecode v2 with Body operations, definite JSONPath
    constants, streaming-regex constants, stage introspection, and fallible
    implicit string coercion
  - flavor-neutral immutable request-body snapshots backed by memory, spill
    files, and inline mutation segments
  - bounded streaming scans with lossy UTF-8 parity, byte accounting, disk
    concurrency limiting, and cooperative cancellation
  - transactional body mutations and retry-stable replay snapshots
  - capture-only request-body gating, fixed generic fail-close body-access
    errors, request-stage-only mutations, and read-only late access
  - scan budgets derived from the resolved physical body cap and
    `maxBodyScanPasses`
- Tests added:
  - 98 DSL-core compiler/value/validator tests pass
  - 14 request-body storage tests pass, including memory/spill parity,
    transactional rollback, replay snapshots, cancellation, and lossy UTF-8
    chunk boundaries
  - 20 resource-layer DSL validation tests pass
  - 314 gateway DSL tests pass
  - 49 request-body-related gateway tests pass
  - 31 integration-harness library tests pass
  - 2 stock-Pingora request-body state tests pass
- Independent code review:
  - fixed bodyless mutation replay-owner provisioning
  - fixed mutation arguments to require exact `Str` values
  - updated the Pingora fork family to the H2 END_STREAM-safe bodyless replay
    registration
  - fixed positional disk reads to retry valid short reads
  - moved blocking selective-JSON scanners off Tokio async workers
  - preserved JSON float spelling for scalar and selected subtree results
  - corrected body-gate versus VM BodyAccess failure documentation
  - added stock runtime tests and forced-spill integration coverage
- Issues encountered:
  - the first targeted external integration attempt stopped at preflight
    because macOS `lo0` lacked `127.0.0.2`, `127.0.0.3`, and `127.0.0.4`
  - after the host aliases were installed, the complete DslBody run passed

## Test coverage analysis

- String-compatible inspection covers byte and character length, emptiness,
  prefix, suffix, substring, streaming regex, and symmetric String/Body
  equality.
- Explicit reads cover byte ranges, full bounded materialization, SHA-256, and
  typed definite JSONPath selection.
- Mutation coverage composes literal replacement, prepend, append, removal,
  bodyless creation, spill-backed source ranges, transactional rollback, and
  stable replay.
- Boundary coverage includes split UTF-8 sequences, matches across chunks,
  physical and VM limits, cancellation, stock known-empty behavior, and
  request-versus-response stage validation.
- Failure coverage verifies typed Body-access errors always return the fixed
  generic HTTP 500 response independently of the DSL `errorPolicy`.
- Migration coverage rejects bytecode v0, v1, and unversioned payloads and
  documents the v2 source changes.

## Test execution record

Passed:

- `cargo fmt --all`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets`
- `cargo test -p edgion-dsl --locked` — 98 passed
- `cargo test -p edgion-gateway plugins::http::dsl --lib --locked` — 314 passed
- `cargo test -p edgion-gateway request_body --lib --locked` — 49 passed
- `cargo test -p edgion-resources dsl --lib --locked` — 20 passed
- `cargo test -p edgion-tests --lib --locked` — 31 passed
- stock runtime tests (historical record — the stock flavor was removed on
  2026-08-03, fork-only build; these commands no longer exist):
  `cargo test -p edgion-gateway request_body::state::stock --lib
  --no-default-features
  --features allocator-jemalloc,boringssl,upstream-pingora --locked` —
  2 passed
- stock Pingora check (same historical note):
  `cargo check -p edgion-gateway --bin edgion-gateway-stock
  --no-default-features
  --features allocator-jemalloc,boringssl,upstream-pingora --locked`
- `./edgion-tests/integration/scripts/integration/run_integration.sh
  -r EdgionPlugins -i DslBody`
  - all DslBody cases passed against real Controller and Gateway processes
  - the post-review run additionally passed disk-backed cross-chunk substring,
    streaming regex, and definite-JSONPath cases
  - resource synchronization, metrics-label, requeue-loop, and cache-residue
    checks passed
- agent-doc, SSA force, metrics inventory, Pingora source policy, gateway
  no-tracing, English-only, and diff checks
