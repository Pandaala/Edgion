# Wasm map decoding profile storage

Status: deferred representation and admission decision. No runtime change.
Source baseline before the aggregate-reuse update: 84f483895.

## Source evidence

In `edgion-gateway/src/plugins/http/wasm/host_abi.rs`, `scan_serialized_map`
checks structural ranges and computes every key/value lossy UTF-8 output length.
Its `SerializedMapProfile` retains only pair count, aggregate decoded bytes and
path-normalization bytes. `decode_profiled_map` then reads the length table again
and recomputes each decoded length before allocating and copying each String.
The copying helper itself validates UTF-8 while emitting replacement characters.
These are repeated traversals, not evidence of a correctness defect.

ReplaceAll profiles borrowed guest memory before admission, decodes owned pairs,
clones the readable snapshot and publishes the ordered operation only after the
required checks. Local-response preparation also profiles before allocating its
candidate, accounting for the previous response's simultaneously live capacity.
Malformed local-response maps with BAD_ARGUMENT become an empty header list;
other callers have different status handling. The scanner checks bounds but does
not require zero-valued terminator bytes or reject trailing bytes.

Callout preflight retains its small aggregate profile alongside the payload
estimate and reuses it for decoding. The aggregate-reuse change removed one
aggregate scan; subsequent commit 7e5509a15 removed the temporary raw-header copy
by borrowing guest bytes while decoding into owned strings. Profiling through
decoding contains no guest execution or memory mutation, and decoder errors keep
the prior BAD_ARGUMENT mapping. The conservative admission allowance remains.
Neither change allocates per-pair metadata. The per-pair storage decision here
remains deferred; aggregate reuse, borrowed decoding and the earlier upstream
scalar-length reuse do not implement it. Historical test receipts for those
changes do not verify a future metadata representation.

## Narrow emitter experiment at bc9227fb7

The emission helper push_utf8_lossy revalidates the valid prefix reported by a
failed from_utf8 call before appending it. A blanket replacement with the standard
byte-slice utf8_chunks iterator was evaluated separately from per-pair profile
storage. It appends each chunk.valid(), then one replacement character whenever
chunk.invalid() is nonempty. This adds no profile storage, but a removed explicit
prefix scan does not establish a faster emitter.

A standalone probe extracted the original helper unchanged and compiled both
emitters with rustc 1.96.1 (31fca3adb), `-O`, aarch64-apple-darwin. Each candidate
received its own output String preallocated to three times input length, cleared
before every call. Inputs and mutable outputs passed through black_box; output
length was also black-boxed. Six timing rounds alternated candidate order. For
each implementation the sorted elapsed sample at index 3 was selected; the table
is the ratio of those two upper-middle samples, not a median of paired ratios.

| Input shape | N=32 | N=1024 | N=65536 |
| --- | ---: | ---: | ---: |
| N ASCII `a` bytes | 2.503 | 10.205 | 9.547 |
| N ASCII `a` bytes plus one `0xff` | 1.500 | 5.443 | 5.866 |
| N `0xff` bytes | 0.766 | 0.744 | 0.757 |

Ratios are chunks elapsed / current elapsed; greater than one is slower. Each
round runs floor(16,777,216 / N) iterations, processing exactly 16 MiB for ASCII
and all-invalid shapes and slightly more for the N+1-byte invalid-tail shape.
The probe also compared both outputs with String::from_utf8_lossy for all 256
single-byte inputs and the nine timed fixtures; single-byte tests append to a
nonempty prefix. This is not exhaustive Unicode-equivalence coverage.

Retain the current emitter under this evidence: the candidate improved the
all-invalid fixture but substantially regressed ASCII and invalid-tail fixtures.
These are exploratory timings on one machine without CPU isolation, not a
universal rejection of chunk-based alternatives or an identified microarchitectural
cause. Profiling/sizing, allocation, guest execution and complete host-call costs
were outside the timed region. No allocation reduction, request throughput,
peak-memory or RSS conclusion follows. The per-pair profile decision below stays
deferred; this experiment neither implements nor rejects that representation.

Fresh unchanged-production host_abi tests passed 47. Formatting and workspace
all-target check/Clippy passed with existing warnings. No full suite or live
Wasm/HTTP integration ran. The probe lived outside the repository and introduced
no production or persistent test artifact.

## Decision alternatives

- Retain the allocation-free profiling pass. Measure its cost against decoding,
  allocation and guest execution before selecting a more elaborate shape.
- Retain per-pair ranges and decoded lengths for the subsequent decoder. This can
  avoid repeated sizing scans, but requires bounded metadata capacity, allocation
  failure handling and an explicit admission point. A guest-controlled pair count
  must never directly trigger a large allocation before structural validation.
- Compare a bounded inline profile with a fallible spill strategy only if evidence
  justifies its stack and heap costs. Do not add an unbounded stack array, unsafe
  guest-memory references or a new persistent VM cache.

Current aggregate profiles allocate nothing. Adding metadata before the existing
checks changes the preflight's peak and potentially its rejection order. Charging
that metadata may reject inputs previously accepted under the same configured
limit; leaving it uncharged is not justified by existing payload charges. Account
for capacity, overlap with decoded pairs and replay/snapshot owners, and release
on every failure. Preserve conservative existing admission only where it is
proven to cover the new representation; otherwise obtain an explicit decision.

This differs from [wasm-01](wasm-01-header-snapshot-ownership.md), which concerns
sharing immutable decoded payloads between the readable snapshot and queued replay.
Do not bundle those representations or assume shared payloads pay for metadata.

## Acceptance for a selected implementation

Preserve decoded bytes, lossy UTF-8 replacement grouping, pair and duplicate order,
original spelling, path handling, empty maps, ignored terminator values and trailing
bytes. Preserve checked arithmetic, caller-specific malformed-map/error precedence,
exact and one-byte-short admission, old/candidate response peak accounting, and
atomic semantic rejection. No guest-memory borrow may cross guest execution or
allocator reentry. Validate that any retained profile describes the exact immutable
bytes decoded; offsets alone are not a lifetime or content-stability proof.

Exercise huge/truncated length tables, high pair counts, invalid and incomplete
UTF-8, repeated ReplaceAll and local-response replacement, callout token/state
publication and all failure cleanup. Existing `decode_map_huge_count_returns_none_without_abort`,
`decode_map_preserves_lossy_utf8_compatibility`,
`replace_all_profiles_structure_and_keeps_first_header_spelling`, and
`local_response_preflight_is_atomic_and_keeps_compatibility_precedence` are starting
coverage, not acceptance of a new representation. Compare CPU and transient/retained
allocation for empty, short ASCII, duplicate-heavy and long malformed-UTF-8 maps.
Do not infer throughput or RSS savings from fewer scans alone.

Any selected implementation requires its own frozen scope, IMPL plan, independent
reviews and project checks. This task authorizes no runtime representation change.

Relevant original knowledge:
- skills/04-review/wasm/regression-guards/request-execution-lifecycle.md
- skills/04-review/wasm/regression-guards/buffer-range-snapshot-before-guest-allocation.md
- skills/04-review/cpu-memory/regression-guards/wasm-abi-retention-budget.md
