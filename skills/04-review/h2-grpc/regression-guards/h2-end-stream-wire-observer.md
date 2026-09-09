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
On 2026-09-09, the relevant `protocols/http/v2` implementation and tests were
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
