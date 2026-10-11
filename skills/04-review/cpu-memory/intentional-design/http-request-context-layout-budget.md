---
name: http-request-context-layout-budget
description: Review the audited ordinary HTTP context and RequestInfo inline size budgets without introducing request allocations.
knowledge_type: intentional-design
---

# HTTP request context layout budget

## Decision and scope

The native 64-bit `EdgionHttpContext` budget is 1304 bytes and `RequestInfo`
retains its audited 408-byte layout. Inline `ResolvedRequestBodyConfig` has a
48-byte budget. These are review boundaries, not a stable Rust ABI or permission
for future growth. Other pointer widths retain the previous context ceiling.
Ordinary context construction and absent-state reads remain allocation-free;
feature owners retain their existing nullable boxes.

## Accounted growth and necessity

Commit d9854dd5 introduced the 1280-byte context ceiling on 2026-10-06. Original
field declarations reconstruct a 1264-byte layout, with 16 bytes of headroom.
Native rustc layout inspection measures the current context at 1304 bytes with
alignment 8. Subsequent necessary storage adds 40 bytes; 24 bytes is the excess
over the old ceiling, rather than the size of one added field.

| Storage | Audited growth | Purpose and origin |
| --- | ---: | --- |
| RequestInfo original Content-Length and CONNECT snapshot | 16 bytes | Option<u64> preserves absent versus valid zero; CONNECT fills existing padding. Both preserve client facts before plugin mutation (2aa2686e). |
| Nullable gRPC transcoder Arc | 8 bytes | Pins one converter owner/admission without ordinary allocation (4dbcc549). |
| Inline resolved body storage policy | 8 bytes | Independent buffered-size cap grows the policy from 40 to 48; spill control fills padding (2aa2686e). |
| Five hot flags and tail alignment | 8 bytes | AiGuard-before-rewrite identity (525ee095), fatal strict-jump conflicts (aacf57b7), body enforcement and WAF partial capture (2aa2686e) grow the aligned flag tail from 16 to 24. |

The plugin publication Arc replacement has equal width. WAF/AI state and streamed
body counters are behind nullable boxes and do not enlarge the ordinary inline
owner. The request-body plugin state is 56 bytes and its nullable box remains
8 bytes; its existing size, one-allocation and monotonic-source tests pass.

## Alternatives and limits

Packing the five added flags could recover at most eight bytes and still leave 1296 above the
old 1280 ceiling, while changing crosscutting writes and atomic guard handling.
Boxing ordinary metadata/policy adds request allocation; a new view adds hot-path
indirection and ownership complexity. Content-Length sentinels lose legitimate
states. Removing independent storage limits or reading mutable headers/config
breaks the existing body contract. No measured performance case justifies these
production changes for this assertion correction.

The accepted repair updates two stale assertions and retains tight current
limits, useful measured-size diagnostics, and a separate inline-policy guard.
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

Current offsets and sizes came from native aarch64-Darwin rustc layout output.
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
