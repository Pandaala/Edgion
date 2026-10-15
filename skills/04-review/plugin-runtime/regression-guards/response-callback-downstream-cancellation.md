---
name: response-callback-downstream-cancellation
description: Keep awaited response callbacks cancellable without borrowing downstream transport.
knowledge_type: regression-guards
---

# Downstream cancellation of awaited response callbacks

## Trigger and root cause

The Task 06 transport proof on 2026-10-15 found that an awaited response
processor retained its chunk, Wasm execution and permits for more than two seconds
after H1 socket closure or H2 RST_STREAM. The independent test gate remained
closed with a 15-second deadline. An H1-upstream Pingora pump awaits the response
callback inside its selected branch, so its outer downstream read cannot
interrupt that wait. A callback holding `&mut PluginSession` also borrows the
same transport that close detection needs. Merely having a driver cancellation
lease does not create transport cancellation.

Source issue: `wasm-stream-transport-proof/05-issues.md` (pending processor
cancellation); the real transport regression persists in
`edgion-gateway/tests/wasm_stream_transport.rs`.

## Correct ownership and outcome

`ResponseStreamSession` contains only diagnostics, terminal arbitration,
canonical AI transfer and trusted template variables. `ResponseTrailerSession`
contains diagnostics and committed trailer framing facts. Production adapters
borrow request context without borrowing downstream I/O. The AI context remains
the sole lazy state owner; full-session and response adapters delegate to its
same canonical helpers.

The body/trailer driver creates an execution lease before constructing the
callback future and moves that lease into it. `response_callback::run` polls
canonical downstream closure before the callback. Dropping even an unpolled
callback then releases its lease; do not create the only lease after the first
poll. On closure, the callback and lease are destroyed before a non-retryable
downstream error reaches the pump. Later observers, cache capture and trailer
dispatch do not run. Abort releases shared Wasm bindings without entering guest
body, trailer, EOS, done or delete callbacks. Universal access logging remains
the sole terminal-log owner and preserves an already committed HTTP status.

No background task, socket alias, raw descriptor probe, new timer, response
BodyStorage, full-response cache, per-chunk log or metric is involved. The
no-processor path skips the watcher. Trailer policy captures only existing
committed Trailer declaration values once per dispatch, never per body chunk.

## Protocol limits

- H2 uses `watch_h2_stream_close`, which does not consume request DATA. Reset
  is stream-scoped; a retained connection must continue serving other requests.
- H1 uses `read_body_or_idle(true)` only after `is_body_done()` and outside
  upgrades. That check excludes replay/prefix delivery. The canonical reader
  owns abort-on-close, half-close tolerance and pipelined-byte preservation.
- With tolerated FIN, the response callback remains alive. With incomplete H1
  upload, or after a pipeline byte has been stashed, prompt close detection is
  not guaranteed. Deadlines remain mandatory. The unresolved extension is
  tracked in [the H1 transport task](../../../../tasks/todo/response-callback-h1-transport-cancellation.md).
- Custom/subrequest transports retain their existing semantics. Real transport
  acceptance uses an H1 upstream; it does not claim upstream-H2 wire coverage.

## Trusted outbound capability

Guardrail uses the canonical `TemplateContext::Trusted` entry. Its admitted
request needs no incoming headers, cookies, query, path or method fallback.
Unavailable request capabilities fail explicitly even under missing-as-empty.
The existing segment walker, builder and outbound call loop remain canonical.
Each window independently selects its current Service endpoint, reads current
authority policy, renders current trusted values and applies the same gates,
deadline and bounded response consumption. Freezing a complete URL or authority
at final-header time would violate these update semantics. Resolved Secrets
retain their separate capability and sensitivity propagation.

## Verification and re-evaluation

Required acceptance includes closed-gate body/trailer cancellation, immediate
closure before callback entry, no late guest entry, one inflight release,
admission recovery, one committed-status log, H2 connection isolation, normal
SSE/backpressure/capacity, H1 half-close/upload/pipelining, and dynamic trusted
outbound authority/endpoint behavior. Fresh execution receipts are recorded in
the transport proof task after implementation checks; historical failure is not
current verification.

Re-evaluate when adding a response capability that needs transport I/O, changing
lease construction or terminal logging order, broadening H1 monitoring, or
changing protocol pumps, full-duplex uploads, custom/cache/retry paths or trusted
template admission. Do not infer complete cancellation from an async signature
or a passing timeout-driven cleanup test.

Fresh 2026-10-15 acceptance passes all three real transport matrices, with no
ignored tests. Closed-gate body H1/H2 cancellation was observed at 10/13 ms and
actual-trailer cancellation at 13/10 ms with roughly 10 ms sampling; this is not
a production SLO. Admission, terminal log and recovery complete before gate
release, the H2 connection remains usable, guest records remain unchanged after
abort, and fixture retirement reaches zero. Focused selections pass 391 distinct
unit tests; workspace all-targets check/Clippy pass, retaining existing warning
sites. Dedicated required-feature target Clippy passes without warnings and
independent final review returns LGTM. Guard receipts live in the transport proof task. TLS and
upstream-H2 wire coverage are not inferred from these results.
