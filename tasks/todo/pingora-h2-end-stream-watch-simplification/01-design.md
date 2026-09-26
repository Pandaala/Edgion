# Pingora HTTP/2 END_STREAM Watch Simplification Design

## Status

Deferred pending an upstream h2 trailer-validation fix. No production watcher
or Cargo manifest change is authorized by this document yet. Earlier vendoring
sections below are retained as rejected design history and are superseded by
the 2026-08-27 decision in `06-decisions.md`.

## Requirement summary

- Problem to solve: the current wire watcher carries approximately 779 production lines and 1,190 test/benchmark lines based largely on h2 0.4.15 behavior, while Pingora now requires h2 0.4.16 or newer.
- Expected behavior: preserve every response-integrity, request-writer, connection-reuse, and cache-admission contract while removing production parser/state that h2 now makes redundant. A watcher simplification is conditional on an h2 decoder guarantee that oversized terminal blocks and terminal trailers containing any pseudo-header are rejected before an event reaches Pingora.
- Explicitly out of scope: dependency downgrades, Cargo manifest changes without separate approval, unrelated H2 connector redesign, and weakening malformed-response rejection.

## Baseline and ownership

- Pingora worktree: `edgion_v3` at `26024af` when analysis started.
- Existing staged prerequisite: `pingora-core/src/protocols/http/v2/client.rs` latches poll-API trailer errors. This task preserves it and adds only unstaged work above that index state.
- Existing unrelated changes: staged `pingora-core/src/protocols/l4/proxy_protocol.rs` and untracked `.superpowers/`; neither is in scope.
- Historical manifest baseline: upstream main declared `h2 = ">=0.4.16"`, and stock 0.4.16/0.4.19 were tested during analysis.
- Dependency policy: retain upstream `h2 >=0.4.16`; no vendored decoder is maintained by this fork.

## Current responsibility map

| Current state or transition | Production consumer | Safety or lifecycle responsibility | Existing evidence | Initial classification |
|---|---|---|---|---|
| `StreamRecord.end_stream` | `PeerEndStream::observed` in `pingora-proxy/src/proxy_h2.rs` | Lets a request-body write failure stop the upload without discarding a response whose wire EOS already arrived | `h2_upstream_no_error_reset_does_not_cost_the_downstream_connection` kills removal | `adapted`: writer-only weak signal |
| `StreamRecord.end_stream` plus `data_bytes` | `EndOfBodyProof::holds` in H2 client reads | Recovers clean EOF after DATA EOS plus remote NO_ERROR reset and rejects h2-dropped bytes | 0.4.16/0.4.19 unwatched complete/truncated/underflow/flow-control runs | `upstream-covered` for DATA completion; remove from read completion |
| `terminal_headers_after_data` | Async and poll body EOF validation | Prevents invalid terminal HEADERS plus same-burst reset from becoming clean EOF | Invalid-trailer mutation fails without it; zero-DATA and empty-valid mutations expose current heuristic defects | `adapted`: all terminal HEADERS, with explicit public-result state machine |
| `pending` map and registration guard | H2 request creation and scanner | Prevents a fast response from beating stream-id registration; scopes records per connection/stream | `a_scan_waits_for_an_in_progress_registration` | `ported` while wire facts remain session-associated |
| `forget` and `forget_generation` | Session drop/local reset and scanner cache | Bounds lifetime and prevents late publication after local abandonment | Existing tests are insufficient for publication-before-local-reset race | `adapted`: shared irreversible invalidation before wire reset; `obsolete`: generation |
| `data_records[2]` cache | DATA scanner only | Reduces per-DATA-frame map locking and freezes byte counts after publication | Cache equivalence tests and benchmarks | `obsolete` with DATA byte accounting |
| `PaddedData` | DATA byte accounting | Excludes padding from delivered-byte comparison and rejects malformed padded DATA as evidence | Padded DATA scanner tests | `obsolete`; weak EOS never decides response success |
| `TerminalFrameHandled` | DATA publication/cache eviction | Couples publication with cache invalidation | Cache-freeze tests | `obsolete` with DATA cache |
| `RST_STREAM` handling | Pending record and cache | Makes reset-before-EOS win and prevents later frames publishing | Reset-order scanner tests | `ported`: map removal only |
| `GOAWAY.last_stream_id` | Registration and publication state | Prevents a racing post-GOAWAY registration or malformed connection from publishing stream evidence | Existing split/malformed/scope tests; persistent-state additions pending | `adapted`: persistent ceiling/poison under registration lock |
| reserved stream-id masking | All scanner lookups | Prevents cross-stream attribution | `reserved_bit_in_stream_id_is_masked` | `ported` |
| incremental frame header and payload skip | All wire observations | Handles arbitrary AsyncRead boundaries without changing bytes | Split-read and pass-through tests | `adapted`: reduced scanner |
| `EndStreamWatchStream` AsyncRead/AsyncWrite delegation | Connector IO | Keeps wrapping transparent while treating EOF or `Ready(Err)` as connection terminal | IO delegation/compile coverage | `adapted`: narrow observer name/docs and shared terminal invalidation |
| `response_body_error` | Async body, poll body, direct trailer API | Permanently latches trailer validation failure | Invalid-trailer async and staged poll test; direct-trailer Content-Length regression pending | `ported`: independent of observer simplification |
| `note_local_reset` | Core shutdown and proxy cancellation/error paths | Invalidates every clone before local RST can race late wire facts | Core/proxy local-reset paths | `adapted`: publish-invalidating state before send-reset; final barrier test pending |
| `Drop::forget` | Session lifecycle | Releases pending record on cancellation/drop | Watcher lifecycle tests | `ported` |

## Public h2 behavior report

### h2 0.4.16

- Changelog contract: resets received after END_STREAM allow already-ended receive data to remain available.
- Public-behavior mutation run without `EndStreamWatchStream`: complete response reset passed; truncated response, Content-Length underflow, rejected trailers, and flow-control-dropped DATA remained errors.
- Invalid pseudo-header trailers with END_STREAM followed by same-burst `RST_STREAM(NO_ERROR)` became a clean `None`, so h2 does not cover this integrity contract.
- A mixed terminal block containing `:status: 200` followed by an ordinary trailer field surfaced as `Some(nonempty)`: h2 discarded the pseudo-header while retaining the ordinary field.
- Restored watched run: all six focused watched client contracts passed.

### h2 0.4.19

- Same public-behavior results as 0.4.16: five baseline cases correct, invalid terminal trailers laundered without the watcher.
- The mixed pseudo-header-plus-regular-trailer rejection contract also failed. Upstream `master` has the same trailer handoff shape, so an unverified later resolver result is not a remedy.
- Private source supports but does not replace the executable evidence: `State::recv_reset` stores `ErrorAfterEndStream` when receive EOS was accepted.
- Restored watched run: complete, truncated, invalid-trailer, underflow, rejected-trailer, and flow-control contracts passed.

## Watched versus unwatched baseline

| Scenario | Watched 0.4.16/0.4.19 | Unwatched 0.4.16/0.4.19 | Classification |
|---|---|---|---|
| Complete DATA EOS then NO_ERROR reset before first read | Clean EOF | Clean EOF | `upstream-covered` for read completion |
| DATA without EOS then NO_ERROR reset | Error | Error | `upstream-covered` |
| Content-Length underflow with dropped EOS DATA | Error | Error | `upstream-covered` plus Pingora Content-Length proof |
| Rejected trailers while Content-Length remains unsatisfied | Error | Error | `upstream-covered` plus Pingora Content-Length proof |
| Stream flow-control drop carrying EOS | Error | Error | `upstream-covered` |
| Invalid pseudo-header trailers EOS plus same-burst reset | Permanent error | Incorrect clean EOF | `adapter-required` |
| Invalid pseudo-header plus regular trailer EOS | Incorrect successful nonempty trailer result | Incorrect successful nonempty trailer result | `decoder-required`; `Some(nonempty)` is not validity proof |
| Invalid pseudo-header trailers with zero DATA plus same-burst reset | Incorrect clean EOF | Incorrect clean EOF | Existing marker heuristic is incomplete; final adapter must mark terminal HEADERS without requiring prior DATA |
| Valid empty trailers after DATA plus reset | Incorrect error | N/A | Current marker heuristic is over-broad; after strict decoder validation, the public trailer event is authoritative |

The writer mutation is killed by the deterministic downstream-connection reuse case. GOAWAY, upstream connection reuse, cache admission, and lifecycle rows still require final-design tests before implementation closure.

### Expanded current/minimum terminal and scanner matrix

- On both h2 0.4.16 and restored-current 0.4.19, `cargo test -p pingora-core --lib 'protocols::http::v2::client::tests_h2::h2_'` now runs 21 passing tests and eight explicit expected-final contracts ignored against the old watcher/client: validated empty trailers after nonempty DATA are falsely rejected today, invalid zero-DATA trailers are falsely accepted today, direct trailer reads do not consult terminal-HEADERS state, repeated body EOF reads can consume validated empty trailers, pseudo-only and mixed pseudo-header trailers bypass public h2 validation, direct-trailer-first requires the terminal latch, and the poll path needs the final terminal-HEADERS latch after decoder rejection.
- The passing rows exercise one and multiple `100`/`103` prefixes before DATA EOS, final header EOS, empty trailers, and nonempty trailers; malformed informational EOS fails when reading the response header on both observed and unobserved IO; header-only final responses remain clean through a pre-poll reset; nonempty trailers are preserved exactly once.
- On both boundaries, `cargo test -p pingora-core --lib protocols::http::v2::end_stream_watch::tests:: -- --skip final_contract` ran 38 passing tests and eight ignored unsafe characterizations/manual benchmarks. Characterization runs confirm short GOAWAY payload lengths 4 through 7 are currently misclassified by their partial `last_stream_id`, GOAWAY on a nonzero stream id is currently accepted by the scanner, increasing GOAWAY does not poison the surviving record, `Ready(Err)` bytes are incorrectly scanned, and EOF mid-header/mid-GOAWAY currently leaks registrations.
- The four parser-poison contracts remain ignored until the narrow adapter replaces the scanner. Their ignored state is intentional and is not production approval: every one must be enabled and pass before the task closes.

## Mutation results

| Mutation | Result | Conclusion |
|---|---|---|
| Remove the entire IO watcher for focused client cases | Five cases remain correct; invalid trailers fail closed only with the watcher | Broad read-completion evidence is redundant; terminal HEADERS observation is load-bearing |
| Remove terminal HEADERS marker by using the unwatched path | Invalid trailer body read returns `None` on both 0.4.16 and 0.4.19 | Marker must remain until h2 exposes an equivalent public error |
| Remove DATA byte-count evidence together with wire completion evidence | Underflow and flow-control-drop remain errors | Byte counting is unnecessary in the selected design because wire DATA EOS will no longer participate in read success |
| Keep wire DATA EOS but ignore delivered-byte equality | Existing underflow/flow-control tests would permit false completion | Forbidden hybrid; either keep the full equality proof or remove wire DATA EOS from read completion entirely |
| Remove poll trailer-error latch | Staged regression test observes an error once and a later trailer call can retry consumed state | Latch remains independent and load-bearing |
| Remove response DATA from the invalid-trailer fixture | Current watched path returns clean `None` | `data_bytes != 0` is not a safe trailer discriminator |
| Replace invalid trailer block with a valid empty trailer block | Current watched path reports the same terminal error | Preserve `Some(empty)` as validated trailers; only `None` plus an unvalidated terminal marker is suspicious |
| Force proxy writer classification to ignore wire EOS | Basic complete-response delivery still passed because the response branch won; deterministic delayed request EOS closed the downstream before reuse | Weak writer EOS is load-bearing for the concurrent write-failure ordering and must remain unless the pump is redesigned |

Mutations for GOAWAY invalidation, local reset, and cache admission are mandatory before final implementation acceptance.

## Solution assessment

| Dimension | No watcher | Narrow adapter | Retain current watcher plus gate |
|---|---|---|---|
| Change difficulty | High because invalid trailers and writer coordination need replacement elsewhere | Medium | Low |
| Production hot path | Best if public-only coordination is correct | Small frame-header scanner remains | Current parser, atomics, map cache, and byte accounting remain |
| Safety | Rejected by current invalid-trailer baseline and writer race | Best current evidence-to-complexity ratio | Safe but retains obsolete assumptions and maintenance risk |
| Compatibility | Requires proxy pump coordination changes beyond current evidence | Compatible with h2 0.4.16+ and existing connector shape | Compatible but private-state rationale is stale |
| Test difficulty | Highest | High but bounded | Medium |
| Rollback ability | Medium | High: restore deleted record fields/scanner paths | High |

### Option A: no watcher

- Core idea: rely only on public `RecvStream` behavior and coordinate request-writer failures with the response reader.
- Rejected for this iteration: h2 0.4.16 and 0.4.19 both launder invalid terminal trailers after a same-burst reset, and a public-only writer coordination design has not yet killed the race where the write half fails before the read half publishes completion. EOS without reset/window update can also leave a flow-control-blocked upload permanently pending, so the adapter requires an explicit notification rather than a post-error classification bit alone.

### Option B: narrow adapter (blocked pending decoder repair)

- Keep per-connection registration, stream isolation, lifecycle removal, incremental frame headers, RST ordering, and scoped GOAWAY invalidation.
- Keep a weak wire EOS bit solely for `upstream_write_error_outcome`; it must never participate in read completion or cache admission.
- Keep a terminal HEADERS marker solely to ensure async/poll/direct-trailer validation cannot turn a consumed/rejected trailer block into clean EOF. It must not try to validate HPACK or infer trailer validity from map emptiness.
- Prerequisite: the selected h2 source rejects an oversized trailer block and any pseudo-header in a trailer block before it queues a trailer event. Both supported crates.io boundaries and current upstream `master` lack this guarantee. A small decoder-level patch is the only currently identified safe remedy; a parallel HPACK parser is explicitly rejected because it recreates the complexity this task removes.
- Remove `data_bytes`, `vouches_for`, `EndOfBodyProof.peer_end_stream`, the DATA cache, cache generations, padded DATA accounting, combined-terminal machinery, related benchmarks, and stale h2 0.4.15 documentation.
- Latch validated initial-header EOS from public `RecvStream::is_end_stream()` so a terminal HEADERS marker can cover every terminal HEADERS frame without rejecting a legitimate header-only final response.
- Treat `Some(empty)` and `Some(nonempty)` as validated success only after the h2 decoder prerequisite is proven by pseudo-only and mixed-pseudo trailer tests. Until then, no production simplification is authorized. The wire observer remains responsible only for making a missing trailer event after terminal HEADERS fail closed.

#### Final terminal-HEADERS state machine

| Public h2 result | Terminal HEADERS observed | Validated initial-header EOS | Strict h2 trailer-pseudo guarantee | Result |
|---|---:|---:|---:|---|
| `Some(nonempty)` | either | either | yes | Validated trailers; preserve exactly once |
| `Some(empty)` | either | either | yes | Validated empty trailers; clean EOF |
| any `Some(_)` | either | either | no | Unsupported source; fail the implementation gate rather than guessing from wire shape |
| `None` | false | false | yes | No trailers; clean EOF from DATA EOS/ordinary h2 completion |
| `None` | true | true | yes | Validated header-only final response; clean EOF |
| `None` | true | false | yes | Terminal HEADERS was consumed or rejected without a validated event; latch permanent error |

`read_response_header` and `poll_read_response_header` latch the final response reader's public `is_end_stream()` before storing it. Informational responses do not become `response_header`; a normal `103 -> final header-only` sequence therefore lands in the `wire=true, validated-initial=true` row. A malformed informational HEADERS carrying EOS may set the weak wire bits, but cannot produce success because the h2 response future/read half remains the only response verdict. No raw-frame subclassification is used as a validity oracle: padded, priority-bearing, fragmented, and dynamic-table-update-only blocks show why the observer cannot safely decide whether a public trailer map is valid.

Raw informational controls use literal `:status` payloads (`100`: `[0x08, 0x03, b'1', b'0', b'0']`; `103`: `[0x08, 0x03, b'1', b'0', b'3']`). Run one and multiple informational prefixes before final DATA EOS, final header-only EOS, and valid empty/nonempty trailers. A malformed informational EOS must fail at response-header read, with or without a same-burst reset; observer bits cannot override a failed response future.

#### Writer ordering and wakeup

- `h2`'s received reset, not the observer, wakes a blocked `poll_capacity`/write and releases the stream reservation.
- The observer bit only classifies the resulting stream-gone error. Rename it to `peer_response_wire_eos_for_write_failure` or an equivalent explicit name.
- DATA or HEADERS carrying wire EOS may set this weak bit before h2 validation. This is safe only because every `UpstreamDoneReceiving` path continues the duplex loop and the outer `DownstreamRequestOutcome::Complete` path awaits the response future. The weak bit cannot emit `HttpTask::Done`, finish cache admission, or select connection reuse. Invalid/malformed HEADERS still produce `HttpTask::Failed` from the read half.
- A deterministic test must force the write-failure branch to win before the response verdict for DATA EOS, header-only EOS, valid trailers, and invalid trailers. Success is allowed only after the read half validates the response; invalid cases must still fail and abort cache admission.
- The existing delayed request-EOS/downstream-reuse test kills removal of the weak bit for DATA EOS. New reservation/pump counters or bounded completion channels must prove no writer task remains after reset.

### Selected policy: retain the current watcher and wait for upstream h2

- Keep production simplification blocked because the stock minimum/current
  matrix does not provide the required decoder contract.
- Do not vendor the decoder. Re-evaluate this design after an upstream release
  passes the focused decoder and Pingora integration contracts.

## Code change plan

### `pingora-core/src/protocols/http/v2/end_stream_watch.rs`

- Reduce `StreamRecord` to terminal HEADERS presence, a writer EOS notification/fact, and a shared irreversible invalidation state visible to every clone. HEADERS EOS sets the writer notification and terminal marker; DATA EOS sets the writer notification.
- Keep scanner state under the registration/publish lock: incremental header/payload skipping, persistent effective GOAWAY ceiling, and permanent connection poison. GOAWAY is applied only after its full declared payload. New registrations and every publication consult this state.
- Delete delivered-byte accounting, two-slot DATA cache, generation tracking, padding arithmetic, terminal-publication token, randomized cache equivalence model, and cache microbenchmarks.
- Retain deterministic split-read, registration race, stream isolation, RST order, GOAWAY scope, forget/local reset, and IO transparency tests, rewritten around the two retained facts.
- Add a connection-level parser poison bit. Any malformed GOAWAY, illegal increasing `last_stream_id`, nonzero GOAWAY stream id, partial header at EOF, incomplete GOAWAY payload at EOF, or `AsyncRead::Ready(Err)` clears pending registrations and permanently prevents later publication. On `Ready(Err)`, do not scan bytes in `ReadBuf`: h2 will not consume them either. EOF/error terminalization remains transparent to the byte stream.

### `pingora-core/src/protocols/http/v2/client.rs`

- Preserve the pre-existing staged poll trailer-error latch.
- Remove wire evidence from `EndOfBodyProof`; read completion uses public/latching and Content-Length evidence only.
- Rename `PeerEndStream` if needed so its API documents writer-only scope.
- Consume the terminal HEADERS marker in both async and poll paths.
- Keep the permanent unwatched builder/tuning helper for dependency-boundary tests.
- Apply the terminal-HEADERS validation-loss discriminator to direct `read_trailers()` as well as async and poll body EOF. A caller that reads exactly the declared Content-Length and calls `read_trailers()` first must receive the same permanent error for invalid terminal HEADERS plus same-burst `RST_STREAM(NO_ERROR)`; repeated trailer reads must return that error.
- Once body EOF has validated and stored trailers, later async or poll body EOF calls must return the latched EOF without polling h2 again. Otherwise h2 can consume a previously observed `Some(empty)` and erase the distinction the terminal-HEADERS state machine needs.

### `pingora-core/src/connectors/http/v2.rs`

- Keep the IO wrapper integration and optional test-only no-adapter connection path.
- Update names/comments from broad END_STREAM completion proof to narrow wire observations.

### `pingora-proxy/src/proxy_h2.rs`

- Keep writer EOS notification/classification: its removal deterministically fails the delayed request-EOS/downstream-reuse integration test, and notification must race `poll_capacity` so a completed response cannot permanently block an upload. Cancellation must release/drop any capacity reservation.
- Rewrite comments that currently claim the read half compares watcher byte counts.
- Do not let the weak fact decide response success, retries, cache admission, or connection health.
- Ensure `mark_shutdown()` is observed at both pool selection and stream allocation. A shutdown connection must not issue a new H2 stream, including during a concurrent release/pool-removal window.

### Proxy cache tests

- Use the existing `ExampleProxyCache` plus process-global `MemCache` in `pingora-proxy/tests/utils/server_utils.rs`, served on port 6148. Use unique host/path keys per case to avoid cross-test state.
- Add test-only cache-proxy observability matching `ExampleProxyHttp`: `CacheCTX` records connector reuse and upstream socket addresses, `connected_to_upstream` delegates to `connected_to_upstream_common`, and `response_filter` publishes `x-conn-reuse` plus the two upstream address headers. Production cache behavior is unchanged.
- Add test-only request headers that set `peer.options.h2_stream_window_size` and `h2_connection_window_size`. The flow-control case uses a 1,024-byte stream window and a 1 MiB connection window so the 2,000-byte terminal DATA violates only the stream window rather than relying on Pingora's 8 MiB default or accidentally testing a connection error.
- Expose a test helper over the existing `CACHE_BACKEND`: build the exact `CacheKey` used by `cache_key_callback` and call `lookup` with an inactive trace. Report `None` when lookup misses; otherwise use the public `HandleHit::can_seek()` contract to distinguish complete (seekable) from streaming partial (not seekable) entries. Do not expose `MemHitHandler` or alter `MemCache`'s private maps.
- Add a test-only, header-controlled `upstream_response_body_filter` failure on the first nonempty chunk for the explicit local-cancellation case.
- Add a multiplex-capable two-exchange H2 origin. It records accepted TCP connection id and request stream id, supports stream 1 then stream 3 on the same socket as well as stream 1 on a fallback second socket, and responds to the second origin request with `second-valid-body`. Raw cases parse complete nine-byte frame headers; they must not locate HEADERS with arbitrary byte-window searches.
- For invalid trailers, truncation, stream flow-control drop, and local filter failure, make the origin serve a failed first response and a valid second response for the same cache key. After the first failure, wait boundedly for cache state `None` and fail immediately if it becomes `Complete`. The second request must reach the origin, report `x-cache-status: miss`, return exactly `second-valid-body`, report `x-conn-reuse: 1`, and be origin stream 3 on the same accepted socket; a hit, partial first body, second TCP connection, or timeout fails the test.
- Include two byte-accounting-removal regressions explicitly: (1) a declared Content-Length underflow where an END_STREAM-bearing DATA frame is dropped by h2 and followed by same-burst `RST_STREAM(NO_ERROR)`, and (2) exact declared length DATA followed by illegal terminal trailers and the same reset. Neither response may complete or create a cache entry.
- Issue a third identical request as the positive cache control: it must be a hit for `second-valid-body`, cache state must be `Complete`, and the origin request count must remain two.
- Add a successful valid-empty-trailer control whose second request is a genuine hit, returns the validated entity, and does not create a second origin stream.

### Rejected vendoring design: fork documentation and CI

- A complete path-only h2 source and dedicated fork workflow were evaluated.
- This option was withdrawn because its long-term ownership cost outweighed the
  low operational frequency of the defect.
- The focused contracts remain useful as the adoption gate for a future
  upstream release.

## Connection disposition

| Outcome | Expected upstream H2 connection behavior |
|---|---|
| Complete DATA EOS plus remote NO_ERROR reset | Stream ends successfully; connection remains eligible and accepts a second stream |
| Valid initial HEADERS EOS or valid trailers plus stream reset | Stream ends successfully; connection remains eligible and accepts a second stream |
| Truncated DATA plus stream reset | Response fails; reset is stream-local, so a second stream may succeed if h2 keeps the connection healthy |
| Invalid trailers/content-length/stream flow-control failure | Response and cache fill fail; if h2 classifies the fault stream-locally, a second stream must succeed without stale evidence |
| Explicit local stream reset/cancellation | Invalidate stream evidence before sending RST; a late peer EOS/reset cannot reverse the local failure; healthy connection may serve a second stream only if h2 remains healthy |
| Malformed connection-level frame, truncated GOAWAY, or connection flow-control failure | Connection is not reusable |

Tests must observe the same accepted socket/connection id at the origin for the reusable rows, not merely downstream keepalive.

## Data flow

1. The connector wraps upstream IO in the narrow observer.
2. Request creation registers the allocated stream id under the same lock used by scanning.
3. The observer records wire EOS for writer termination and terminal HEADERS for validation-loss detection, scoped to the connection and stream; its notification races blocked capacity waits, but never decides response integrity.
4. h2 remains the sole decoder and sole read-completion authority for accepted DATA/HEADERS.
5. The response reader validates trailers and latches errors; only a clean terminal task can finish cache admission.
6. Session drop, local reset, stream reset, connection EOF/error, or ineligible GOAWAY irreversibly invalidates pending observation state before later wire events can publish.

## Test design

### Core contract tests

- Parameterize watched/unwatched construction for complete, truncated, pre-poll, Content-Length, rejected trailer, flow-control, and invalid trailer cases at h2 0.4.16 and current.
- Add zero-DATA invalid trailers and valid header-only/valid empty/nonempty trailer controls.
- Cover async body, poll body, and trailer APIs after the terminal error.
- Add raw GOAWAY below/equal/above, split, malformed, and graceful two-stage cases against the final narrow scanner, plus connector/proxy tests proving malformed short payloads, nonzero stream ids, increasing ids, and EOF-truncated GOAWAY cannot cleanly complete/cache a response or accept a later stream on that connection.
- GOAWAY handling validates the declared payload length at the frame header, collects the first four bytes across splits, waits until the full declared payload is skipped, then monotonically retains only stream ids at or below `last_stream_id`. A short declared payload or EOF mid-payload cannot publish later evidence; an increasing later GOAWAY never restores a removed record.
- Explicit GOAWAY cases: below/equal/above ids; reserved-bit masking; split at every header/payload byte; declared lengths 0 through 7; nonzero frame stream id; decreasing `2^31-1 -> 3`; illegal increasing `3 -> 2^31-1`; error-code variants; EOF in a partial header; EOF before the declared payload completes; and a read error that carries final bytes. The latter must poison without scanning those bytes because h2 does not consume bytes returned together with an error.
- Add a local-reset-before-remote-reset race with a deterministic barrier: deliver initial DATA, make the local filter/cancellation win, then deliver terminal wire evidence and `RST_STREAM(NO_ERROR)`. The original local failure remains authoritative, no cache entry exists, and no stale observer record is reused. Add a writer EOS-without-RST/window-update bounded-completion test and a shutdown-pool race test.

### Proxy integration

- Kill removal of weak writer EOS with the complete-response reset test and same downstream-connection reuse test.
- Confirm truncated, invalid-trailer, stream-flow-control, and explicit local-filter failures terminate without a completed cache entity.
- Prove every stream-local case accepts stream 3 on the same origin socket. The raw invalid-trailer burst is final trailer HEADERS with illegal `:status`, immediately followed by `RST_STREAM(NO_ERROR)` in the same write; the raw flow-control burst uses the isolated stream window described above.
- Verify no request upload pump task or flow-control reservation remains after the reset.

### Dependency policy

- Keep the normal upstream h2 dependency and rerun the focused consuming
  contracts when upgrading it.
- Do not resume watcher simplification until upstream h2 rejects malformed and
  oversized trailers before delivering a trailer event.

## Quantified target

- Creation-time baseline: 779 production lines before `#[cfg(test)]`, 1,190 test/benchmark lines, 1,969 total.
- Design target: remove at least 400 production lines, all per-DATA byte accounting/cache atomics, all padding-specific state, and all cache benchmark code.
- Remaining per-read cost: incremental frame-header parsing and payload skipping; map access only when a frame carries a retained observation or teardown event, not for ordinary nonterminal DATA frames.
- Final counts must be measured from the reviewed diff rather than treated as a deletion quota.

## Risks and compatibility

- The terminal HEADERS marker distinguishes validated initial EOS from invalid-reset laundering; it is not a trailer validator. Focused direct-trailer mutation established that h2 may expose a pseudo-only invalid block as `Some(empty)`, while the mixed mutation exposes pseudo plus ordinary trailers as `Some(nonempty)`. Public map state alone is insufficient until h2 rejects pseudo trailers before event delivery. Permanent positive and negative tests remain mandatory.
- A validated trailer result is also single-consumption state: repeated body EOF calls must not re-enter `RecvStream` and replace a stored empty map with `None`.
- Removing broad wire evidence from read completion remains blocked until an
  upstream decoder release passes the consuming contract suite.
- Weak writer EOS remains a wire fact and must stay isolated from response success and cache admission.
- GOAWAY eligibility and registration races remain protocol-sensitive and require deterministic tests after simplification.
- Parser poison is connection-scoped: it clears pending registrations and blocks every later publication, but never changes or withholds the byte stream passed to h2.
- Rollback point: restore the current watcher implementation and retain the dependency matrix if any narrow-adapter mutation or cross-layer test fails.

## Open assumptions for design review

1. Resolved: informational-prefix/header-only terminal behavior is identical in the focused 0.4.16 and 0.4.19 matrix. The final response reader alone must latch `RecvStream::is_end_stream()`; malformed informational EOS remains a response-header failure.
2. Does the weak writer EOS mutation produce the same deterministic downstream-reuse failure at h2 0.4.16 as it does at 0.4.19?
3. Resolved: the public `HandleHit::can_seek()` contract permits a test-only complete-versus-partial state probe without widening `MemCache` internals. Black-box miss/hit and origin request counts remain authoritative; the probe is an additional invariant.
4. Resolved dependency decision: keep upstream `h2 >=0.4.16`, do not maintain a
   vendored decoder, and defer production watcher simplification until an
   upstream release passes the trailer-validation contracts.
