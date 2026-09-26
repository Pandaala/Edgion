# Design and Review Decisions

## Independent design review 1

- Date: 2026-08-26
- Scope: narrow-adapter proposal in `01-design.md`
- Result: rejected for production implementation; test-only baseline and mutation work may continue.

### Must Fix

1. Define terminal HEADERS as an explicit state machine: `Some(empty)` is validated success; `None` plus terminal HEADERS and no validated initial EOS is laundering.
2. Define writer classification for DATA-ended and HEADERS-ended responses, including deterministic select ordering, wake source, reservation release, and pump termination.
3. Complete the watched/unwatched and mutation matrix before deleting DATA accounting or other state.
4. Specify informational-response transitions and malformed informational EOS behavior.
5. Apply GOAWAY eligibility only after consuming the full declared payload; define malformed and multi-GOAWAY monotonic behavior.
6. Prove a second stream on the same upstream H2 connection after successful and stream-local failed exchanges.
7. Select a real proxy cache harness and prove no partial object becomes a hit after invalid trailers, truncation, flow-control drop, or local failure.
8. Approve and enforce the minimum/current dependency policy rather than relying on a one-time local run.

### Nice to Have

- Rename the weak writer fact so it cannot be mistaken for a wake source or response-integrity proof.
- Report hot-path operations in addition to line counts.
- Tie lifecycle cases to observable record ownership and non-reuse.

## Design revision conclusion

- Adopted: all Must Fix findings and all three Nice to Have findings.
- Not adopted: none.
- Reason: every finding identifies either a missing acceptance-criteria proof or a concrete ambiguity that could accept a malformed response, reject a valid response, leak a pump/record, or leave dependency behavior unenforced.
- Implementation gate: closed until the revised design records test evidence for each Must Fix item and receives another independent design review.

## Cache and upstream-reuse harness decision

- Date: 2026-08-26
- Decision: extend only the proxy test harness rather than production cache or connector APIs.
- `ExampleProxyCache` will expose connector reuse and upstream socket addresses using the same callbacks already used by `ExampleProxyHttp`.
- A test-only cache-state helper will use public lookup plus `HandleHit::can_seek()` to distinguish complete from partial state; it will not expose `MemHitHandler` or `MemCache`'s private maps.
- A multiplex-capable origin will accept stream 1 and stream 3 on one socket, while still accepting a fallback second connection so a reuse regression fails by assertion rather than timeout.
- Failed fills require three independent observations: no completed cache state, a second-request miss that reaches the origin, and the second valid body. Expected stream-local cases additionally require the same origin connection id and stream ids 1 then 3.
- The flow-control integration fixes the stream window at 1,024 and the connection window at 1 MiB. This isolates stream-level failure and makes same-connection reuse a valid expectation.

## Dependency policy decision (superseded)

- Date: 2026-08-26
- Decision: keep the upstream-owned `h2 = ">=0.4.16"` manifest constraint unchanged and do not add a tracked Pingora lockfile.
- Historical proposal: enforcement would have lived in a dedicated fork
  workflow. That workflow was removed when the vendoring plan was withdrawn.
- The minimum leg version-qualifies the currently resolved direct h2 0.4 package and runs `cargo update -p h2@<resolved> --precise 0.4.16`. The current leg starts from a fresh default resolution. Both record the inverse dependency tree so the h2 0.3 dependency elsewhere in the workspace cannot be mistaken for the direct h2 0.4 dependency.
- Both legs run the core terminal-response matrix, observer GOAWAY/lifecycle contracts, proxy writer-ordering and same-upstream-connection contracts, and cache-admission integrity cases.
- A result difference between minimum and current is a compatibility failure. It is not permission to weaken the minimum-boundary assertion.

## Boundary evidence decision

- Date: 2026-08-26
- Both h2 0.4.16 and 0.4.19 ran the expanded focused client matrix with 21 passes and eight deliberately ignored current-watcher/client/decoder defects. The lockfile was restored to 0.4.19 after the minimum run.
- The matching observer suite has 38 non-ignored passes on both boundaries. It characterizes, rather than excuses, the old scanner's short-GOAWAY, nonzero-stream-id GOAWAY, increasing-GOAWAY, `Ready(Err)` scanner, and EOF lifetime defects.
- The ignored contracts are implementation acceptance tests. They must become enabled and pass with the narrow adapter; they cannot be silently deleted or left ignored at task completion.

## Sol security review 1

- Date: 2026-08-26
- Result: production gate remains closed.
- Adopted Must Fix: direct trailer reads use the same terminal-HEADERS permanent-error latch; valid empty trailers are asserted as `Some(empty)` before later EOF polling; malformed GOAWAY receives connector/proxy non-completion and non-reuse coverage; the local-reset race has a real cancellation-before-late-EOS barrier; cache negatives cover Content-Length underflow/drop and exact-length invalid trailers.
- Rationale: all five omissions could otherwise let a bad terminal response be reported as clean, admitted to cache, or associated with a reused connection. The changes are acceptance requirements, not optional hardening.

## Sol lifecycle review 1

- Date: 2026-08-26
- Result: production gate remains closed; the narrow-adapter design returns to code-level design.
- Adopted Must Fix: persistent GOAWAY ceiling/poison under the registration/publish lock; irreversible shared local invalidation before local reset; no scanner publication from bytes paired with `AsyncRead::Ready(Err)`; EOF/error poison; terminal-HEADERS state independent of prior DATA byte count; writer EOS notification that can break capacity waits; and shutdown checks at H2 pool selection plus stream allocation.
- Rationale: the prior design could race a post-GOAWAY registration, recognize bytes h2 discards, retain evidence after local cancellation, permanently stall an upload, or allocate a stream on a shutdown connection. Each is a response-integrity, availability, or unsafe-reuse risk.

## Direct-trailer mutation decision

- Date: 2026-08-26
- Finding: with exact `Content-Length`, delivered DATA, an invalid `:status` terminal trailer block, and a same-burst `RST_STREAM(NO_ERROR)`, h2 0.4.19's direct `read_trailers()` path returned `Some(empty)` on the unwatched baseline.
- Decision: replace the prior unsafe assumption that every `Some(empty)` represents validated success. The observer records whether the terminal header block is provably exactly empty; `Some(empty)` is accepted only with that fact (or where no terminal trailer marker exists). An encoded/nonempty/unproven terminal block paired with an empty public map fails closed unless the initial response headers had already validated EOS.
- Consequence: direct trailer reads use the same permanent latch as body EOF. Add the exact-length invalid-trailer cache case and both dependency-boundary tests before implementation approval.
- Follow-up: current repeated body EOF reads can re-poll h2 and consume a stored empty map. The narrow client must short-circuit on the EOF/trailer latch; an ignored acceptance test records this requirement.

## Sol security review 2: mixed pseudo-header trailer bypass

- Date: 2026-08-26
- Result: production gate remains closed; the prior narrow-adapter terminal-map state machine is rejected.
- Confirmed on h2 0.4.16 and 0.4.19: a terminal trailer block containing `:status: 200` followed by a normal trailer field is exposed as `Some(nonempty)`. The focused watched contract fails because body EOF is reported as successful.
- Root cause: h2 decodes pseudo fields into `frame::Headers::Pseudo`, then `recv_trailers()` calls `into_fields()` and queues only regular fields. The current upstream `master` has the same trailer handoff branch, so merely resolving a newer unpinned version is not evidence of a fix.
- Decision: do not use wire size, empty-map checks, or `Some(nonempty)` as a validity proxy. A selected h2 source must reject any pseudo-header in a trailer block before queuing a trailer event. The smallest candidate repair is in h2's trailer receive path and must preserve full HPACK decoding/state before issuing a stream `PROTOCOL_ERROR`.
- Dependency policy change: the existing `>=0.4.16` source is replaced by h2 0.4.19 vendored in Pandaala/pingora. The decoder and adapter share one immutable Pingora revision; this remedy does not depend on an upstream submission, merge, or release. The vendored and consuming suites must test pseudo-only and mixed-pseudo trailers with and without same-burst reset.
- Consequence: retain the terminal-HEADERS observer only as an error-laundering detector for missing trailer events. Do not classify `Some(empty)` or `Some(nonempty)` from raw bytes. Extend async, poll, direct-trailer, cache-admission, and connection-reuse negative tests before reopening the implementation gate.

## Sol security review 3: candidate decoder patch rejected

- Date: 2026-08-26
- Result: the temporary six-pseudo-field h2 patch is not approved; production gate remains closed.
- Must Fix 1: `frame.pseudo()` is incomplete after h2 marks a header block `is_over_size`. The decoder intentionally stops retaining fields beyond the configured header-list limit, so a large `:path` or `:authority` trailer can appear pseudo-free. The h2 candidate must reject `frame.is_over_size()` before inspecting pseudo fields or queuing trailers, with oversized pseudo-only, ordinary oversized, and fragmented CONTINUATION regression coverage.
- Must Fix 2: the temporary Pingora tests called body EOF before `read_trailers()`, so their later direct-trailer error could come from Pingora's body latch. A same-burst remote reset can also erase h2's public error event after decoder rejection. Add a fresh-session direct-trailer-first watched contract and preserve a terminal-HEADERS permanent latch; the h2 patch alone is not a public-error persistence mechanism.
- Adopted design correction: h2 must reject malformed/oversized blocks before `Event::Trailers`; Pingora must treat terminal HEADERS followed by no validated trailer event as a permanent error in async, poll, and direct APIs. These are complementary defenses, not alternatives.

## H2-001 ownership decision (superseded)

- Date: 2026-08-27
- Decision: maintain the decoder remedy entirely under `Pandaala/pingora/vendor/h2`. No upstream pull request or upstream-merge dependency is part of the delivery plan.
- Source policy: Pingora and Edgion must consume one exact Pandaala Pingora revision containing the h2 decoder tests, decoder implementation, and adapter. A branch-only reference, crates.io range, local path override, or uncommitted checkout is not production evidence.
- Current state: the vendored h2 and Pingora changes pass their focused suites and received an independent read-only approval. The finding remains open until the containing Pingora revision is immutable, Edgion manifests and lockfiles resolve it, and final source-policy checks pass.

## H2-001 ownership decision superseded

- Date: 2026-08-27
- Decision: withdraw `vendor/h2` and all path-patch/CI coupling. The operational
  likelihood does not justify permanently owning the complete dependency.
- Policy: retain the normal upstream h2 dependency, keep H2-001 as a deferred
  blocker, and adopt an upstream fix after the focused contracts pass.
- Consequence: do not proceed with production END_STREAM watcher simplification
  while it assumes the missing decoder guarantee.
