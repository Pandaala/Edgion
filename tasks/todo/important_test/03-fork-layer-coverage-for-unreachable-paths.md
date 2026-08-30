# Class 3 — Behaviors Edgion cannot route to, so they must be tested in the fork

| Key | Value |
|-----|-------|
| Created | 2026-08-04 |
| Status | item A done (2026-08-04, with corrections below); item B deferred per its "Worth doing when" |
| Covers | gaps 5 and 6 of `tasks/todo/transport-coverage-observability-gaps.md` |
| Where the change goes | `pingora-proxy/tests/` — a new driver per item; **nothing in Edgion** |
| Method | build the request shape directly against the proxy, bypassing Gateway API routing |
| Effort | item A small, item B medium (needs a session type the harness has never constructed) |
| Precedent | Envoy tests both, because its config is not Gateway API and can express them |

## Why these two are one class

Neither is unobservable and neither is unimplemented — the guard exists and is
wired. The obstacle is identical in both cases: **no Edgion configuration can
produce a request that reaches the code**. So the end-to-end suite is the wrong
layer, and the fork's own suite is the right one. Recording them together keeps
the next person from re-deriving "why isn't this in the integration suite".

## Item A — CONNECT framing coercion

**The behavior:** the fork coerces a non-`Ordinary` upstream body disposition
back to `Ordinary` for CONNECT requests, so a tunnel is never re-framed as
chunked. That guard is real and worth pinning.

**Why Edgion cannot test it:** Gateway API has no CONNECT route type. No
HTTPRoute/TLSRoute/TCPRoute produces a CONNECT request through the gateway, so
the path is unreachable from any configuration we can write. Note this is a
*Gateway API* limitation, not a proxy one — Envoy tests CONNECT precisely
because its own config can express it. The **upgrade** half of the same guard
*is* reachable from Edgion and is already covered.

**Do:** assert the coercion in the fork's seam suite, driving a CONNECT request
directly at the proxy. Cover each framing shape the guard is meant to protect
(no CL, CL, chunked).

**Done (2026-08-04)** — pinned as
`seam::single::bodyless_does_not_half_close_a_connect_tunnel`
(`pingora-proxy/tests/seam/single.rs`): a plain H2 CONNECT through the h2c
listener to a scripted H2 upstream, asserting the upstream HEADERS carry no
END_STREAM and the client's tunnel bytes arrive exactly once. Discrimination:
mutating the `is_connect` condition in `safe_disposition` fails the test (500
instead of 200 — the honored `Bodyless` trips the bodyless-contract
violation). Isolated runs: 10/10. The h2c listener now sets
`allow_connect_method_proxying` (the proxy 405s CONNECT otherwise).

**Correction (2026-08-04)** — the plan above ("cover each framing shape: no
CL, CL, chunked") assumed an H1 CONNECT was constructible in the fork's own
suite. Observation disproved that, on both sides of the proxy:

- H1 downstream: an authority-form request-target needs the `patched_http1`
  feature, which does not compile in this fork — it requires a patched
  httparse (`parse_unchecked` does not exist in stock httparse, and the
  workspace pins crates.io httparse 1.10.1). Without it the request line is
  rejected with 400 in `set_raw_path`.
- H1 upstream: `http_req_header_to_wire` serializes `raw_path()`, which
  panics on an authority-form URI (no path-and-query, no raw-path fallback),
  so H2c → H1 CONNECT proxying cannot be driven either. Pre-existing
  upstream-Pingora sharp edge, not fork-added; reachable only by an
  application that both opts into CONNECT proxying and routes it to an H1
  peer.

So the three H1 framing shapes stay pinned by the pure-function truth table
(`safe_disposition_truth_table` in `proxy_common.rs`) alone; the end-to-end
test pins the coercion WIRING through the H2 pump, where the observable is
END_STREAM-on-HEADERS. `Streamed` × CONNECT on the H2 pump is wire-identical
to the coerced `Ordinary` (both leave HEADERS open and strip nothing that a
bare CONNECT carries), so no end-to-end assertion can discriminate it — not
tested, recorded here instead.

## Item B — the custom-connector pump

**The behavior:** `proxy_custom.rs` received the same seam wiring as the H1/H2
pumps — the action hook, the fail-closed disposition, the retry gate, and the
`end_of_body || data.is_none()` normalization added after a review found it
load-bearing for user-implemented sessions.

**Why nothing tests it:** no Edgion route reaches the custom connector, and the
fork's harness has never constructed a `Custom` session. Its behavior is
currently guaranteed by code review alone — and one of its defects (the missing
normalization, which can hot-spin) was found by reading, not by testing.

**Do:** build a minimal `Custom` session driver in the fork harness, then cover
at least: the action hook running, `Terminate` failing closed with a typed
error, a non-`Ordinary` disposition failing closed, and the normalization
(a `data == None` event with `is_body_done() == false` must not spin).

**Worth doing when:** the custom connector carries production traffic, or
sooner if the seam's custom-pump wiring changes again. It is the only pump whose
correctness rests entirely on review.

## Watch out for

- Do not "solve" item A by adding a CONNECT entry point to Edgion. The point is
  that the guard belongs to the proxy; testing it there is correct, not a
  workaround.
- Item B's driver is the expensive part. Build it once, generally enough that
  future custom-pump work can reuse it, rather than a single-scenario fixture.
