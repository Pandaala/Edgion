# B0251 — HTTP DSL Builtins

## Scope

- Pipeline state: terminal; changed; implementation, correction re-review, targeted tests, and required gates passed.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `f326f2800c03c34fce39f9fbedf73f18cacc2dc1`.
- Target: `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`.
- Target blob: `0e8c4dd00f2e48b20d51325c7d578e3bc8c58806`.
- Read scope may expand across the complete DSL compiler/VM/runtime, request context, outbound calls, shared mechanisms and tests.
- Exact code/test allow-list: `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` and `edgion-gateway/src/plugins/http/dsl/tests/http_call_tests.rs`.
- Exact knowledge/doc support allow-list: `skills/03-coding/dsl/http-call.md`, `skills/01-architecture/02-gateway/dsl-engine-security-model.md`, `skills/04-review/dsl/SKILL.md`, `skills/04-review/dsl/intentional-design/http-call-body-truncate-to-max-string-len.md`, `skills/04-review/dsl/regression-guards/http-call-full-body-read-before-truncate.md`, `skills/04-review/dsl/regression-guards/redis-read-values-truncate-to-max-string-len.md`, and the English/Chinese DSL API references.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; complex/plan-first.
- DSL `http.call` currently stops at `httpMaxResponseBodyBytes`/`maxStringLen` and returns the partial prefix with the upstream status. This violates the root invariant that a bounded plugin-initiated outbound response must fail on overflow and never silently truncate. Upstream header names/values also enter DSL strings without `maxStringLen` enforcement.
- Reuse canonical `stream_body_capped`, compute the effective minimum body cap, preflight declared length, require EOF for exact-cap success, and return the existing non-throwing `{status: 0, body: "", headers: {}}` failure map for body/header overflow, stream failure or lossy-UTF8 expansion beyond the cap. Keep the accepted header-count cap and all outbound/SSRF/failure-shape ownership.
- Exact proposed scope: target `lang/builtins.rs`, focused `tests/http_call_tests.rs`, and the specifically listed DSL architecture/coding/review/user docs whose historical truncation conclusions become stale. Existing integrations stay frozen.
- The analyst separately identified Redis error-string bounds/diagnostics, Redis alias/request-metadata allocation work, and a frozen DSL HMAC-auth example. These require independent-review confirmation before coordinator tasks are created; config range validation remains outside this batch.
- Baseline remains `f326f280`; target blob `0e8c4dd00f2e48b20d51325c7d578e3bc8c58806`.
- Refresh at `2123fbe7`: B0250 changed rustdoc only. The target/test blobs and overflow behavior remain unchanged. The exact knowledge list above contains every direct current contradiction; other DSL indexes discuss only the existence of a cap and need no edit.
- External refresh at `2c01a075`: all ten frozen allow-list blobs remain unchanged. The intervening `ctx.rs`, integration, task and routing-knowledge changes do not affect the DSL `http.call` chain; the revised implementation plan passed independent review at this HEAD after closing bounded-lossy-UTF8 and exact header-order Must Fix items.
- External refresh through `bea47373` and `5cd384c6`: all ten baseline blobs remain unchanged. The stage-aware dye and Controller/test-runner changes do not affect this call chain; exact integration fingerprints and campaign coverage were independently reconciled.
- Required contract: effective body cap is the minimum of the outbound and VM string limits; declared and streamed overflow, lossy-UTF8 expansion, or oversized retained header names/values return the existing status-zero empty failure map. Exact-cap success requires observed EOF. Header count retains its accepted cap behavior.
- Definitive wider findings: Redis error strings, Redis alias allocation, request-metadata allocations, and frozen HMAC-auth integration guidance remain separate candidates. Configuration range speculation was dropped for lack of evidence.

## Implementation and Review

- Reused canonical `stream_body_capped` with the effective minimum of `httpMaxResponseBodyBytes` and `maxStringLen`. Declared or streamed overflow, stream errors, excessive lossy-UTF8 expansion, and oversized retained header strings now discard the result and return the established status-zero empty map; no partial body is exposed.
- Added checked lossy-UTF8 output sizing before conversion allocation. Exact-cap and zero-cap success require observed EOF. Request-body accounting, outbound SSRF/client ownership, main-upstream streaming, and Redis value truncation are unchanged.
- Preserved the raw response-header prefix contract: map fullness is checked before the next raw header; invalid UTF-8 values consume no slot; duplicate values replace only while traversal remains open; ignored tails are not inspected. Updated all scoped architecture, coding, review, and bilingual user documentation.
- Focused synthetic-response tests cover Content-Length preflight without polling, streamed overflow with an unpolled tail, exact/zero caps and EOF, string-cap interaction, lossy expansion, checked integer overflow, send/stream errors, header string bounds, invalid values, duplicate replacement and map-full duplicate/tail behavior. No network or integration fixture is used.
- Independent final review found one documentation/test boundary defect: the initial text overclaimed overall-last duplicate behavior. The correction added a `maxMapSize = 1` regression and precise raw-prefix wording. Re-review returned LGTM with no remaining Must Fix or Should Fix.

## Verification

- `http_call_tests`: 36/36; builtins helper tests: 19/19 before the correction; complete Gateway DSL suite: 350/350 after correction.
- Locked workspace all-target check and Clippy passed; Clippy retains repository warnings outside this batch. Format, agent-doc, SSA-force, metrics-inventory, Pingora-source, Gateway no-tracing, English-only, diff, and 3,231-file frozen-path guards passed.
- Existing `edgion-tests/**` files are unchanged by the campaign.
- Commit: `397d27ef`; never pushed by the campaign.

## Deferred Findings

- `tasks/todo/bound-and-redact-dsl-redis-error-results.md` owns the independently confirmed unchecked dependency/caller-controlled Redis error-string boundary.
- `tasks/todo/avoid-dsl-redis-link-key-clones.md` owns the confirmed per-call resolved Redis link-key clone.
- Request-metadata projection remains a measurement candidate, not a promoted task. The HMAC quick-start issue already exists; no duplicate was created. Config-range speculation was dropped.
