# Prepared Wasm callout authorization

Status: deferred representation and measurement decision. No runtime change.
Source baseline: 7e5509a15.

## Source evidence

`edgion-gateway/src/plugins/http/wasm/plugin.rs::WasmPlugin::build` normalizes
configured origins through the Resources canonical normalizer, sorts and
deduplicates them, then shares an `Arc<[String]>` with request dispatch and the
runtime's `CalloutGrant`. `host_abi.rs::host_http_call` normalizes each guest target
and performs a linear exact-string membership check before token publication.

The production builder prepares a sorted list, but the representation does not
enforce that invariant. Public `HostState::arm_dispatch`, the public callback
frame field and `vm_manager.rs::CalloutGrant` accept ordinary arrays. A direct
replacement of `iter().any(...)` with binary search can reject an otherwise
authorized origin in an unsorted array. Constructor documentation describes
normalized authorization, not a mandatory sorted-array contract.

`WasmConfig::validate` validates each entry without an allowlist cardinality cap.
The per-phase eight-dispatch limit does not bound comparisons per dispatch. This
is not evidence of a production bottleneck: small lists, hit position and URL
normalization cost may dominate the choice. No latency or allocation benchmark
has compared the alternatives.

## Decision alternatives

- Keep linear membership if realistic lists and workloads do not justify change.
- Introduce one encapsulated, cold-prepared authorization set with a sorted
  representation and binary membership, consumed by both request and timer paths.
- Compare a hashed representation only if measurements justify its construction,
  hashing and retained-capacity costs. Do not add a parallel authorization cache.

Prove the invariant at every construction/publication boundary before relying on
it. Do not sort or allocate per dispatch, silently impose a new cardinality cap,
skip guest-target normalization, or bypass the canonical normalizer. Account for
the prepared owner's memory across runtime generations, requests and timer roots.
Changing public construction and equality semantics needs a reviewed scope;
the absence of a release compatibility burden does not prove behavior neutrality.

Compare empty, singleton, small and large lists; first/last/missing hits; repeated
prefixes and normalized duplicates. Measure construction, retained bytes and
complete hostcall latency, not just comparison counts. Include the existing URL
normalization and unchanged ABI admission work in end-to-end comparisons.

## Acceptance for a selected implementation

Preserve exact canonical-origin equality, including scheme, normalized host and
explicit effective port; reject credentials, non-root paths, queries and fragments
according to the existing normalizer. Never replace normalization with raw-string
comparison or treat a hostname match as authorization for a different port.

Preserve request and timer dispatch enablement, empty-list behavior, denied metrics,
error precedence, timeout grants, guest token/state atomicity and runtime-generation
isolation. The timer path keeps callouts disabled for an empty grant; an armed
request with an empty list cannot authorize an origin. Request/timer leases retain
their existing lifecycle and egress enforcement after authorization.

Cover unsorted inputs at every supported construction boundary, duplicates,
equivalent normalized origins, disallowed origins and empty lists, plus request
and timer consumers. Existing host ABI, runtime and timer tests are starting
coverage, not proof of a future representation's correctness or performance.
Run the applicable project checks and independent IMPL plan/final reviews.

This task does not change allocator preparation (wasm-05), header snapshot or map
profile storage (wasm-01/02), shared data (wasm-03), or pool discard (wasm-04).
Relevant original knowledge:
- skills/04-review/wasm/regression-guards/request-execution-lifecycle.md
- skills/04-review/cpu-memory/regression-guards/wasm-abi-retention-budget.md
