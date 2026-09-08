# B0184 — HTTP Request Body Filter

## Scope

- Candidate analysis HEAD: `d2da7f4dd1332f56d1564d22f1e8cd398cb3d96c`; call-chain freshness-refreshed at `238d430d851bd1db244c6271c630357d5a9fbc69`
- Target blob: `fb058345fa12aaf9d3231377fb815777fd225ef0`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned code: `edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter.rs`
- Frozen support allow-list:
  - `edgion-gateway/src/ctx.rs`
  - `skills/04-review/request-mirror/regression-guards/mirror-resource-budgets.md`

## Handoff

- Analysis: `b0169_final_review` read the complete per-chunk filter/action path, context body-source ownership, mirror writer/cancellation/budget lifecycle, handler/WAF/observer coexistence, logging cleanup, tests, frozen integrations, and review precedent.
- Finding: after mirror completion, abandonment, queue/buffer exhaustion, or writer closure, production drops the live mirror resources but persists `MirrorState::Abandoned` and its source bit. Every remaining body chunk then misses the outer null-state fast path and performs a zero-work async take/restore cycle, precisely on the degraded mirror path that must not burden the primary request.
- Intended fix: retire the mirror source after every terminal outcome while preserving cancellation/disarm/drop order. A context helper clears only the mirror source and removes the optional body-plugin state only when no handler/WAF/observer source remains; handler/WAF non-replayable latches and the frozen relay plan remain untouched.
- Integration: existing files remain frozen and already cover mirror wire behavior; deterministic state/fast-path unit coverage is sufficient, so no new issue is warranted.
- Implementation: terminal completion, downstream abandonment, channel-full, byte-limit, closed-writer, and defensive pre-abandoned paths now detach the mirror source. Cancellation/disarm, sender close, and writer-handle drop ordering remains explicit; the outer request-body state is released only for a mirror-only request.
- Review: the approved complex plan preserved the handler/WAF monotonic replay latch and observer residency. Fresh final review requested only correction of the knowledge page's verification date; after correction it returned LGTM with no remaining Must Fix or Should Fix item.
- Verification: context state tests passed 6/6, filter/action tests 14/14, mirror tests 38/38, and the complete proxy-http suite passed 244/244. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed.
- Commit: `ec774be0`; never pushed by the campaign.
