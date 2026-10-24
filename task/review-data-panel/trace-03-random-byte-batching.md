# Evaluate RequestId random-byte batching

Status: deferred for user decision. Type: performance investigation.
Inspected baseline: `26ba14f09`. Locked rand/rand_core versions: 0.9.5.

## Source evidence

`request_trace_id/mod.rs::generate` fills RandomHex's bounded stack array with
one rng.random() per output byte. The configured count remains 8 through 64.
rand's StandardUniform implementation for u8 calls next_u32 and casts to u8.
Thus n output bytes consume n buffered u32 values. The MD5 branch's random
16-byte array also uses per-element StandardUniform through array::from_fn;
its hash computation and output format are separate work.

ThreadRng delegates through ReseedingRng to BlockRng. The latter's fill_bytes
uses fill_via_chunks to consume all little-endian bytes of each u32, discarding
only the unused tail of a final partial word. That path consumes ceil(n/4)
buffered words for n bytes, including across buffer boundaries. A bulk fill of
RandomHex's existing used stack prefix could reduce this word-consumption and
loop work without introducing another random-byte allocation.

This is not an OS entropy call per output byte. ThreadRng retains a shared
thread-local generator and buffered output across calls and consumers. Its 64 KiB
reseed threshold counts complete generated buffer bytes, and the threshold is
checked when generating another buffer. Bulk filling changes buffer exhaustion
and possible reseed points, not the configured threshold. The locked dependency
panics on reseed failure; identical entropy-failure timing is not established.
The [thread owner/refill audit](CALLBACKS.md#p2c-thread-rng-owner-and-refill-boundary)
records the common dependency boundary.

## Decision

Keep the current implementation, or authorize evaluating bulk filling with the
same ThreadRng. The latter consumes different bits and a different number of
words, changing these identifiers and subsequent draws on that thread. There is
no deterministic identifier sequence promised here; sequence differences alone
are not evidence of a regression. Decide whether the distribution-level contract
and measured value justify the change rather than presenting it as an identical
random-call substitution.

No existing entropy defect, cryptographic improvement or throughput multiplier
has been established. These are correlation IDs, not a new credential-generation
contract. Inspect the current generator and failure boundaries if selecting a
candidate; do not introduce seeded production RNGs, new caches/shared locks,
custom randomness, reseed suppression or per-request random pools.

RandomHex is the primary candidate. Including MD5 seed filling is an explicit
additional scope choice. NanoId range sampling, UUID v4/v7, W3C root generation,
P2C selection and random sources elsewhere remain outside this task.

## Acceptance for later implementation

Preserve runtime bounds and RNG-handle acquisition order, stack scratch bounds,
output lengths/alphabet, prefix handling, HeaderValue validation and final value
checks. Generation stays on the same required fallback branch; first-write,
suppression, source selection, forwarding/echo and log masking remain unchanged.
Do not expand the accepted RequestId source set or select trace-01/trace-02 work.

Compare warm and first-use generation, minimum/default/maximum lengths, isolated
calls and mixed consumers. Separate word consumption, buffer refill/reseed,
formatting/allocation and complete request latency. Use a deterministic test-only
source to verify consumption and partial-word/buffer boundaries without changing
the production RNG owner. Check output contracts and statistical assumptions;
format tests alone do not establish distribution or security properties.

A selected implementation requires a frozen scope, independent IMPL reviews and
project checks. No runtime prototype or workload benchmark is included here.
Original lifecycle authority:
skills/04-review/observability/intentional-design/request-trace-id-lifecycle.md.
