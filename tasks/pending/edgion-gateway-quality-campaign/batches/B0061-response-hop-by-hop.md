# B0061 — Response Hop-by-Hop Headers

## Scope

- Baseline: `4b73b9e95b96ac3056ddf79172be2f2326d0a6f6` after concurrent-HEAD revalidation; intervening commits did not touch the owned code, approved knowledge, or relevant response call chain.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned code file: `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs`.
- Approved knowledge files:
  - `skills/04-review/http1/regression-guards/hop-by-hop-dynamic-stripping.md`
  - `skills/04-review/http1/intentional-design/no-connection-close-injection.md`
  - `skills/04-review/http1/SKILL.md`
  - `skills/01-architecture/02-gateway/02-pingora-lifecycle.md`
- Allowed tests: inline tests in the owned code file only.
- Existing `edgion-tests/**`, adjacent proxy/common files, and protected user paths remain forbidden.
- Exact commit allow-list: the owned code and four knowledge files plus B0060/B0061 campaign ledger, batch, main, changelog, and the provenance-verified integration freeze manifest refresh.

## Context and value gate

- Analysis verdict: `change`; implementation is plan-first because it changes the canonical response-header boundary across interim, upgrade, plugin, compression, and final-response paths.
- The main proxy response path currently handles `Connection` but does not strip the canonical static hop-by-hop set or fields nominated across repeated `Connection` lines.
- Pinned Pingora H1 rewrites `Connection` for reuse semantics, while H2 removes a fixed five-field protocol set; neither handles every repeated dynamic nomination or the complete canonical set.
- Reuse the shared `connection_listed_hop_by_hop` and `HOP_BY_HOP_HEADERS` mechanisms rather than adding another parser or header list.
- Preserve 101 upgrade headers, clean 100/102/103 before forwarding, clean final responses before and after async plugins, and retain pre-strip Transfer-Encoding presence for compression eligibility.

## Plan requirements

1. Preserve streaming/backpressure, local-reply ownership, trace headers, final status/access-log handoff, and the frozen integration driver.
2. Parse every repeated `Connection` field line and remove both static and dynamically nominated fields.
3. Leave 101 responses unchanged; interim responses must remain outside plugin and terminal bookkeeping.
4. Prevent async plugins from reintroducing hop-by-hop fields through a second canonical cleanup.
5. Let Pingora own downstream framing; never synthesize Transfer-Encoding or Connection semantics.
6. Correct only the approved knowledge pages whose current main-response ownership or shutdown description is stale.

## Verification plan

- Focused response-filter tests for static/dynamic stripping, repeated Connection lines, ordinary header preservation, 101, interim responses, and compression eligibility.
- Full campaign repository gates and frozen-path verification.

## Implementation

- Added one canonical response sanitizer that pre-inspects every repeated `Connection` field, validates nominated names, removes all dynamic nominations and the shared static hop-by-hop set, and fails atomically on non-text values.
- Preserved 101 upgrade headers, sanitized 100/102/103 before their early return, and cleaned ordinary responses before and after async plugins without displacing settled local replies or denials.
- Retained pre-strip Transfer-Encoding presence across both passes so sanitization cannot make an originally transfer-encoded response compression-eligible; Pingora remains the sole downstream framing owner.
- Corrected the four approved knowledge pages to describe the main response boundary, shutdown/connection ownership, repeated fields, and the separate plugin-subrequest limitation accurately.

## Independent reviews

- The initial plan review required both final sanitizer passes to respect `response_stage_settled(ctx)` and clarified 101, invalid-token, atomic-failure, and compression semantics.
- The revised plan was independently approved before implementation.
- Final code review found the implementation and knowledge changes technically sound; its sole campaign-record accuracy correction about the pinned H2 fixed-field behavior was applied and re-reviewed to LGTM.

## Verification

- All 22 focused response-filter tests passed.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only.
- Only the existing Kafka `err_expect`, frozen integration-test redundant-async, and cargo-deny duplicate/advisory-ignore warnings remain.
- Frozen-path verification passed for all 3,442 recorded paths after exactly 15 manifest fingerprints were refreshed to match external commit `94c56632a`; B0061 did not modify `edgion-tests/**`.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `abb88ea4e6a3c03c674ff53203d5e3aad44376d2`.

## Protected paths

- `task/`
- `tasks/pingora-0.9.0/`
- `tasks/todo/complete-k8s-dynamic-scenario-parity/`
- `waf-architecture-and-multi-pattern-matching.md`
