---
name: h2-end-stream-wire-observer
description: Historical review evidence for Pingora fork / H2 END_STREAM wire observer; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 end stream wire observer

## Recorded conclusion

A GOAWAY's `last_stream_id` is retained as a persistent connection ceiling that later registrations are checked against, not a one-time `retain`. Malformed (declared payload < 8 octets), nonzero-stream, EOF-truncated, and `last_stream_id`-raising GOAWAY frames poison the observer permanently instead of contributing a guessed threshold. Clamping to 0, trusting a complete 4-octet id behind a truncated `error_code`, and keeping the ceiling in `FrameScanner` are not accepted.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-goaway-persistent-ceiling-fail-closed.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-goaway-persistent-ceiling-fail-closed.md)

## Resolution references

- `H2-005` — 2026-08-27 (fixed in pingora fork `edgion_v3`).
- `H2-002` — read-terminal evidence poisoning, fixed in fork commit `a0ada7a`;
  revalidated as already fixed on 2026-09-09.

## Gateway GOAWAY regression boundary

`edgion-h2-upstream-lifecycle-e2e-validation` adds controlled GOAWAY cases in
`edgion-tests/src/client/suites/http_route/transport/h2_lifecycle.rs`:
maximum-to-current descending ceilings, maximum-to-preceding ceilings that
exclude a later accepted stream, an attempted raise that re-includes that
stream, a short payload, a nonzero frame stream id, and EOF inside the payload.
Raw backend proofs check the exact frames and ensure a complete response
attempt follows the complete GOAWAY sequences, including excluded streams.

Through the Gateway, an excluded stream may qualify for a protocol-directed
retry: GOAWAY promises that streams above its ceiling were not processed.
The origin's first attempted response is 200; its second attempt returns 201.
The test requires exactly two attempts on different connections and the full
201 response, so accepting the excluded stream's attempted 200 cannot pass.
The raising sequence retains the earlier exclusion and follows this same
retry outcome. Legal descending sequences deliver the full accepted response.
Malformed, nonzero-stream, and truncated sequences must fail without accepting
a successful response, harness deadline, or body-cap error. Each faulted
connection becomes ineligible for the follow-up; after an eligible retry the
follow-up must reuse that healthy replacement without another accept.

The Gateway's upstream stream limit defaults to one. Accepted concurrent
sibling behavior is tested separately in `h2_lifecycle_connector.rs`, using
the exact locked dependency's public Connector with two slots. A legal
descending GOAWAY preserves the earlier held response, while a new connection
carries the release request before that response completes. This supplemental
contract does not claim product-level upstream multiplexing.

These external outcomes do not isolate the private watcher poison state from
the h2 decoder's GOAWAY handling. Inspection of the Cargo.lock fork
`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`, `h2 0.4.20`,
confirms persistent ceiling (`ActiveWatch::goaway_ceiling`, applied by
`note_connection_torn_down` and checked in `Registration::register`;
publication resolves only through the pending map that ceiling pruning and
poisoning empty) and permanent poison checks (`WatchState::Poisoned`, also
entered when a later GOAWAY raises the ceiling) in
`pingora-core/src/protocols/http/v2/end_stream_watch.rs`. This was re-verified
by source inspection at that revision on 2026-11-06 after the fork was rebased
onto upstream 0.9.1. The rebase consolidated `FrameScanner`'s former separate
`goaway`/`padded_data`/`terminal_data` fields into one `PayloadState` enum
(`Skip`/`PaddedData`/`TerminalData`/`Goaway`); the ceiling still lives in the
watch state, not the scanner, and the poison, padding-wait, and GOAWAY-finish
semantics are unchanged by inspection. h2 0.4.20 additionally rejects a
nonzero-stream GOAWAY itself (`frame::GoAway::load` returns
`InvalidStreamId`), so that case's product outcome is now jointly produced by
h2 and the watcher poison. Product tests supplement that evidence rather than
replacing the fork's internal registration/race tests or the retained watcher.

h2 0.4.20 private-fact audit (2026-11-06, source diff of the registry crates
`h2-0.4.19` against `h2-0.4.20`, read against fork `e5f4287`). The watcher's
module comment still says "audited against h2 0.4.19"; that fork-side wording
is a tracked fork follow-up, not an Edgion defect. Each of its five handoff
facts holds at 0.4.20:

1. `State::recv_reset` / `Cause::ErrorAfterEndStream`: `proto/streams/state.rs`
   is byte-identical between the two releases.
2. `Recv::poll_data` popping `pending_recv` before consulting state: unchanged.
   The 0.4.20 `recv.rs` changes are duplicate-conflicting `content-length`
   rejection (a stream reset at HEADERS, before any DATA reaches the reader)
   and push/refusal accounting. `streams.rs` adds a lock-scope change for both
   `recv_headers` and `recv_data` (`reset_on_recv_stream_err_deferred` takes the
   send-buffer lock only for a `Reset` error, same outcome). `client.rs` now
   resets a response without `:status` and reorders its shutdown snapshot, and
   `framed_read.rs` moves decoder state into a `FrameDecoder` struct; none of
   these alters the facts below. A new decode-then-drop path, if any, is still
   caught by the wire/delivered byte-count equality.
3. Connection errors and `recv_go_away` keep `pending_recv`: those paths are
   unchanged. The new `GoAway::load` stream-id check produces a connection
   error that the watcher already poisoned on (`stream_id != 0` in the scanner),
   so h2 and the watcher now agree instead of the watcher being stricter.
4. One codec frame applied before polling the next: still true. The new
   `ReceivedFrame::Refused` branch defers a REFUSED_STREAM until the codec is
   writable before reading another frame; it is reachable only for a
   peer-opened stream, and the fork client builds with `enable_push(false)`
   (`pingora-core/src/connectors/http/v2.rs`), so it does not occur upstream.
5. Small-DATA-frame budget: `Budget` now stores `spent`/`max` and, in `Auto`
   mode, follows `set_target_window_size` at runtime. The fork only sets
   `initial_connection_window_size(conn_window)` at build time, which 0.4.20
   applies immediately after construction, and never calls
   `set_target_window_size` later, so the effective budget is
   `max(conn_window / 2, DEFAULT_DATA_FRAME_BUDGET)` exactly as on 0.4.19. The
   continuing-upload reset contract therefore keeps the same budget headroom.

Re-audit on any further `h2` bump or if the fork starts adjusting the
connection window at runtime. No fork or Edgion test was re-run for this audit.

Historical run (against the earlier `edgion_0.9.0_v1@0b0d8bba` lock, not
repeated for `e5f4287`): on 2026-10-06, `cargo test -p edgion-tests --bin test_server transport::h2
-- --nocapture` passed 14 tests, including all five lifecycle wire proofs.
The final fresh-Gateway Transport run passed all 165 registered entries,
including the twelve H1-to-H2/H2-to-H2 GOAWAY scenarios and the supplemental
public Connector descending-GOAWAY sibling scenario. See
[Gateway reuse verification scope](h2-proxy-cache-reuse-harness-consumers.md#gateway-reuse-verification-scope)
for exact production/fork revisions, launch commands, executed/skip counts,
and the outer-wrapper environment limitation. Private watcher/race tests were
not freshly run; interrupted runs are not passing results.

## Read errors and incomplete EOF

`EndStreamWatchStream::poll_read` does not scan bytes delivered together with
`Ready(Err)`, because h2 does not consume those bytes. It poisons the observer
on read error or EOF with an incomplete frame header/payload. `WatchState`
transitions from `Active(pending)` to `Poisoned` under the same lock used for
registration/publication, drops pending records, and prevents later publication.
Previously published evidence from a complete successful read stays immutable;
this is not a claim that poisoning revokes all historical evidence. Wrapped
read results and bytes remain transparent to the caller.

DATA END_STREAM publication waits for the complete declared payload, including
padding. This prevents a truncated padding-only terminal frame from becoming
completion evidence. Read-terminal poisoning and the GOAWAY ceiling above are
distinct contracts sharing the same poison primitive.

The canonical fork explanation is
`edgion-changes/review/h2-grpc/h2-end-stream-observer-read-terminal-poison.md`.
`EndStreamWatchStream::poll_read` still poisons on `Ready(Err)` and on an
empty read with `has_partial_frame()` at `e5f4287`, and the four
read-terminal `final_contract_*` H2-002 regressions (of eight `final_contract_*` tests) remain present and not ignored in
`end_stream_watch_tests.rs` (source inspection only, 2026-11-06).
Historically, on 2026-09-09, the relevant `protocols/http/v2` implementation and tests were
identical between Edgion's locked fork `518f7274` and sibling `dba39b6`.
Enabled regressions cover bytes plus read error, every partial frame-header
split, partial terminal DATA including padding, and every partial GOAWAY
payload split. Each also checks that later registration cannot publish.

This closes only H2-002. Terminal-header classification, trailer API latching,
test determinism, documentation alignment, and per-failure connection policy
remain tracked in the watcher simplification task directory. Re-evaluate this
rule if h2 read-buffer consumption, scanner completion, poison state, or record
publication changes. Historical verification above is not a fresh test run.

Fresh closure checks on 2026-09-09: `cargo test -p pingora-core --lib
final_contract_ -- --test-threads=1` passed 8 tests with none ignored, including
all four H2-002 regressions, in sibling `dba39b6`. Edgion format, workspace
check, Clippy, agent-doc, SSA-force, metrics-inventory, and Pingora-source
checks passed; Clippy retained existing warnings. This does not claim that
the ignored contracts belonging to other H2 issues have been resolved.
Independent read-only closure review returned LGTM; the stale H2-002 task was
removed after these checks while the remaining simplification work stays open.
