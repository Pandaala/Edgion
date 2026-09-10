---
name: async-response-body-hook-profile-before-gate
description: Read when a review flags the async-trait boxed future created for every ProxyHttp upstream response-body hook call or proposes a synchronous no-work predicate before that hook (pingora-review-23).
knowledge_type: intentional-design
---

# Async response-body hook no-work gate requires production profiling

## Conclusion

The allocation mechanism is real, but the public Pingora no-work predicate is
declined under the available evidence. A release measurement of the current `ProxyHttp` default
confirmed one 16-byte allocation per hook call with jemalloc. An isolated
Edgion-shaped benchmark showed that a conservative predicate removes the outer
allocation on deliberately idle later chunks, while installed async processors
retain their own nested boxed-future allocations.

The repository has no production-representative response chunk-count
distribution and no measurement of the fraction of chunks that are idle after
TTFT, writable processors, and route, selected-backend, Gateway, and global
body-filter chains. A synthetic 100-chunk idle benchmark proves the mechanism,
not that a public API addition is worth its lifecycle and maintenance burden.

Treat the boxed future as an accepted cost until profiling shows a material
gateway-level CPU, allocation, throughput, or tail-latency impact. Do not
re-report the existence of the allocation alone as sufficient evidence for a
code change.

## Re-evaluation rule

Re-open with production-representative offline traces, a real
production-profile gateway benchmark, or allocator sampling that demonstrates
material gateway-level cost and separates the outer hook from nested processor
allocations. Also re-evaluate if an internal Pingora dispatch change removes
most of the API burden or if a compiler/`async_trait` change removes the outer
box. A performance-based proposal must characterize both chunk counts and
no-work prevalence. Any predicate proposal must preserve the ordinary `Data`
callback contract, including first-body TTFT and every installed processor or
filter, and must always dispatch both typed terminal events:
`TerminalWithoutTrailers`, including the synthetic callback after a final
Header with EOS and the header-only path, and `TerminalBeforeTrailers`. The
pre-trailer event flushes the streaming-processor chain while leaving the
trailer lifecycle open for the actual trailer callback; it must not be skipped
or collapsed into no-trailers completion. The same conservative proof must
cover upgrades, HTTPRoute/GRPCRoute and selected-backend filters,
Gateway/global filters, retries, cache admission, sink termination, and
hot-reload snapshots. Permanent per-chunk metrics or tracing are not accepted
merely to collect this evidence.

The detailed investigation was completed on 2026-08-11 and is archived in git
history at `d484e580`. This page is the canonical maintained decision; preserve
its conclusion and re-evaluation rule without depending on the archived raw
report.

## Reference

- Source finding: Pingora review issue 23,
  `pingora/tasks/pingora-review/23-async-per-chunk-boxed-future.md`.
- Investigation completed: 2026-08-11.
