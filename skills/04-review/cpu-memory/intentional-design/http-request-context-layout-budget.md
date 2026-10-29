---
name: http-request-context-layout-budget
description: Review the audited ordinary HTTP context and RequestInfo inline size budgets without introducing request allocations.
knowledge_type: intentional-design
---

# HTTP request context layout budget

## Decision and scope

The user-approved interim `EdgionHttpContext` ceiling is strictly below 1800
bytes on all pointer widths (2026-10-14). `RequestInfo` currently has an audited
400-byte native 64-bit layout (2026-10-29). The earlier 1328-byte context ledger below is historical. Inline `ResolvedRequestBodyConfig` has a
48-byte budget. These are review boundaries, not a stable Rust ABI or permission
for unreviewed future growth. The interim ceiling includes deliberate headroom;
it is not a claim that every current context field has been audited.
Ordinary context construction and absent-state reads remain allocation-free;
feature owners retain their existing nullable boxes.

## Historical accounted growth and necessity

Commit d9854dd5 introduced the 1280-byte context ceiling on 2026-10-06. Original
field declarations reconstruct a 1264-byte layout, with 16 bytes of headroom.
The earlier cf8dd648 ledger measured 1304 bytes with alignment 8. At that
revision, necessary storage since d9854dd5 added 40 bytes; 24 bytes exceeded
the original ceiling. This earlier measurement is historical, not the current
layout. The historical native inspection measured 1328 bytes with alignment 8; the
additional 24 bytes come entirely from the frozen entry method in RequestInfo.

| Storage | Audited growth | Purpose and origin |
| --- | ---: | --- |
| RequestInfo original Content-Length and historical CONNECT flag | 16 bytes | Option<u64> preserves absent versus valid zero; CONNECT fills existing padding. Both preserve client facts before plugin mutation (2aa2686e). |
| Nullable gRPC transcoder Arc | 8 bytes | Pins one converter owner/admission without ordinary allocation (4dbcc549). |
| Inline resolved body storage policy | 8 bytes | Independent buffered-size cap grows the policy from 40 to 48; spill control fills padding (2aa2686e). |
| Five hot flags and tail alignment | 8 bytes | AiGuard-before-rewrite identity (525ee095), fatal strict-jump conflicts (aacf57b7), body enforcement and WAF partial capture (2aa2686e) grow the aligned flag tail from 16 to 24. |
| Canonical frozen entry Method | 24 bytes | 3c0a9125 adds Option<http::Method>; subsequent consolidation derives HEAD/CONNECT identity from it instead of two flags. Outbound calls consume the frozen method (3b7edd1c), with no live-header fallback. |

That historical RequestInfo had 425 bytes of field storage and seven tail-padding bytes,
for 432 bytes with alignment 8. Its `method` occupies 24 bytes (the same size as
`http::Method`; Option uses its existing representation). Reconstructing the
previous shape removes those 24 bytes and restores the two one-byte HEAD/CONNECT
flags: 403 bytes plus five padding bytes gives 408. The containing context grows
by exactly the same 24 bytes; its other inline storage is unchanged. These are
native compiler measurements plus declaration-based historical arithmetic, not
execution of the historical crate.

The plugin publication Arc replacement has equal width. WAF/AI state and streamed
body counters are behind nullable boxes and do not enlarge the ordinary inline
owner. The request-body plugin state is 56 bytes and its nullable box remains
8 bytes; its existing size, one-allocation and monotonic-source tests pass.

## Alternatives and limits

At the earlier 1304-byte revision, packing the five added flags could recover at
most eight bytes, still leaving 1296 above the old 1280 ceiling. That historical
alternative does not account for the subsequently required complete method.
Boxing ordinary metadata/policy adds request allocation; a new view adds hot-path
indirection and ownership complexity. Content-Length sentinels lose legitimate
states. Removing independent storage limits or reading mutable headers/config
breaks the existing body contract. The method snapshot must distinguish absent
capture, preserve case-sensitive extension methods, survive live rewrites and serve shared outbound consumers. Boxing it adds
ordinary request allocation; a bespoke compact representation adds method mapping
and extension ownership without measured performance evidence. No measured
performance case justifies these production changes for this assertion correction.

The earlier user-authorized follow-up refreshed the two stale assertions to 1328/432
without extra headroom and retains tight current limits, useful measured-size
diagnostics, and a separate inline-policy guard.
It does not change runtime storage or loosen future admission, memory or transport
bounds. Re-evaluate any added field, nested owner growth or target-layout change
with actual layout and consumer evidence before increasing these budgets.

## Evidence and verification

Sources: ctx.rs, request_body/config.rs, ai/context.rs, request metadata/body
admission, strict jumps and AI guard/proxy entry. Existing unit-test identities
and allocation/lazy-owner assertions remain. Relevant filters are ai::context,
ctx::request_scenario_state_tests, ctx::request_body_plugin_state_tests and
request_body::config. Fresh execution receipts and independent approval belong
in the bounded implementation task.

The historical field sizes and ordering came from native aarch64-Darwin rustc
`-Zprint-type-sizes` output on the repository toolchain.
The original 1264-byte ledger was reconstructed from original declarations and
unchanged field layouts; an external aligned-shape probe validates its arithmetic.
This is not a full historical crate execution, a cross-platform layout proof,
an allocator/RSS measurement or a throughput claim. The separately accepted
[backend-generation layout](backend-generation-arc-slice-handle-layout.md)
provides the same audited-budget and independent-allocation-guard precedent.

## Resolution references

- ARCH-001 AI acceptance follow-up: two stale context-layout assertions found
  during fresh regression checks. Evidence and receipts are maintained in
  tasks/pending/architecture-plugin-session-ai-capability/.

- AI isolation follow-up: eight failing regression tests were explicitly approved
  for repair by the user. Independent Astra review accepted the measured tight
  1328/432 limits and the alternatives documented above. All eight exact tests
  and 324 related tests passed; workspace check, clippy and static guards passed.
  These are the repair-time receipts, not a new verification after task cleanup.
  The semantic fixture repair supplies the same frozen-method contract without
  altering production transport policy. Completed temporary task records were
  removed; the current layout decision and regression tests remain canonical.

## Wasm lifecycle observation (2026-10-14)

The request-owned Wasm lifecycle change adds one nullable typed owner pointer
(`Option<Box<WasmRequestState>>`), costing 8 inline bytes on this native 64-bit
target and no allocation for an ordinary request. The pointer pins execution
across hooks; a generic registry or eager owner would weaken ownership or add
ordinary-path allocations.

An exact run of `ai::context::tests::ordinary_context_size_stays_within_budget`
measured 1408 bytes against the existing 1328 ceiling. The current diff adds only
that pointer to the context: a 1400-byte pre-change layout is an inference from
field width/alignment, not an executed baseline measurement. The other 72 bytes
have not been audited here. The user subsequently authorized a strict 1800-byte
ceiling and deferred size reduction to a follow-up task, subsequently closed under
the revised ownership-audit scope below. That interim ceiling replaced the old
absolute context ceiling without claiming that the historical growth was audited.

The isolated allocation test was also corrected to prepare the shared LazyLock
configuration before measuring `with_gateway_config`, matching production
`pg_new_ctx`. Fresh execution passes with zero allocation calls/bytes and absent
Wasm state; allocation freedom remains an independent gate from inline size.

## Sparse test-metrics ownership

`RequestTestMetricsState` groups the diagnostic hash key and first-upstream
timestamp behind one nullable context owner. Only `Lb` mode extracts and retains
the hash key; only `Latency` mode captures the timestamp. Ordinary, disabled and
`Retry` modes leave this owner absent. Clearing an absent hash or reading absent
state does not allocate. General retry counters remain in the ordinary context.

The timestamp belongs to the request, not `BackendContext`: AI backend replacement
can discard attempt history, so deriving this timestamp from the first retained
attempt would change its meaning. Both ordinary and AI attempt entry use the same
first-write helper. This trades one small allocation in the relevant test mode
for avoiding test-only payload and clock capture on ordinary requests; it makes
no measured throughput or byte-saving claim. Regression coverage lives in
`ctx_test/test_metrics_state_tests.rs`, the ordinary/AI attempt tests, native
Service dispatch tests and the test-data JSON serialization test.

## Ownership audit closure

The user replaced the size-reduction and tighter-ceiling objective with field
necessity, coherent feature ownership and allocation on first use. The audit
retained ordinary hot state and existing typed lazy owners, and selected two
changes: the test-metrics owner above and partial-capture policy inside the
existing selected WAF owner. Both changes passed independent plan and final
reviews, 113 targeted tests, workspace check/Clippy and the project static guards.
These are implementation-time receipts, not a fresh execution during cleanup.

The follow-up task is closed under that revised scope. No compiler-layout ledger,
size reduction or throughput improvement is claimed. Existing numeric layout
assertions were left unchanged by the two selected implementations; they do not
constitute a requirement to shrink the context or box ordinary metadata.

## Baseline assertion re-audit (2026-10-29)

A fresh native `aarch64-apple-darwin` compiler `-Zprint-type-sizes` probe using
repository toolchain rustc 1.96.1 (`31fca3adb`, LLVM 22.1.2) measures `RequestInfo` at 400 bytes with alignment 8: 393 bytes of fields
and seven padding bytes. Relative to the historical 432-byte shape, commit
`7b1755089` moves the 24-byte query owner out of RequestInfo, and `25e0fefda`
replaces the owned protocol string with a borrowed protocol (eight fewer bytes).
These changes explain the exact reduction; no new storage change is made here.

`PluginExecutionState` measures 184 bytes with alignment 8: 177 bytes of fields
and seven padding bytes. Commit `6118e8e2` adds the `ip_body_denied` boolean,
which moves the prior 176-byte shape across the alignment boundary.
`PluginAuxState` remains 296 bytes (289 field bytes plus seven padding bytes),
and its nullable box remains eight bytes. The complete `EdgionHttpContext`
measures 1376 bytes, below the unchanged strict 1800-byte ceiling.

Only the two stale exact assertions change, from 432 to 400 and 176 to 184.
Ordinary-request allocation and absent-state checks, lazy-owner checks, the
auxiliary-state assertion, and the context ceiling remain independent gates.
These are native layout measurements, not a Rust ABI guarantee, cross-target
proof, heap/RSS measurement, or fresh execution of the historical verification
receipts above. Fresh regression results belong to the active repair record.
