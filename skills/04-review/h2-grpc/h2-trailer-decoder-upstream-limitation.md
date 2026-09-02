---
name: h2-trailer-decoder-upstream-limitation
description: Use when reviewing HTTP/2 terminal-trailer validation findings; covers the accepted upstream h2 pseudo-header and oversized-block limitation and the required Pingora safety boundary.
status: accepted-tradeoff
finding_id: H2-001
closed: 2026-09-01
---

# h2 Terminal-Trailer Decoder Limitation — ACCEPTED-RISK

## Conclusion

The decoder defect is real, but Edgion and its Pingora fork do not vendor or
privately fork `h2` for this low-frequency malformed-upstream case. The
END_STREAM watcher and fail-closed trailer handling remain required until a
normal upstream `h2` release provides and proves the missing decoder contract.

Fix suggestions that patch raw HPACK handling in Pingora or vendor the complete
`h2` dependency are **not accepted**.

## Core Rationale

**1. The defect belongs to the upstream decoder**

At the 2026-09-01 Pingora `edgion_v3` head (`ad15e485`) the workspace requires
`h2 >=0.4.19` and resolves `h2 0.4.19`. Its `Recv::recv_trailers` closes the
receive side and calls `frame.into_fields()` without first rejecting
`frame.is_over_size()` or trailer pseudo-fields. A mixed pseudo-header and
ordinary trailer block can therefore become a successful nonempty trailer map.

The current executable dependency baseline reports this outcome as
`Laundered`, while the ignored strict mixed-trailer contract fails on the
unwatched path. A wire observer cannot reconstruct the discarded pseudo-field
without duplicating the dependency's HPACK decoder.

**2. Local dependency ownership is disproportionate**

The trigger requires a buggy or malicious H2 origin to send a malformed
terminal trailer block. Maintaining a complete private `h2` fork for that
narrow case adds an ongoing protocol and dependency-maintenance burden that is
not justified by the expected frequency and impact.

**3. The accepted limitation does not authorize unsafe simplification**

Wire END_STREAM proves only that the peer set the transport flag. It does not
prove that the decoded trailer block was valid or that every response byte was
delivered. Pingora must retain the watcher, byte-delivery checks, permanent
trailer-error latches, and fail-closed cache-admission rules that do not depend
on the missing decoder guarantee.

## Fix Suggestions Not Accepted

- Vendor or path-patch `h2` in the Pingora fork — disproportionate long-term
  ownership for a low-frequency upstream defect.
- Parse HPACK or infer validity from raw trailer size in the watcher — creates a
  second decoder and cannot safely share the dependency's dynamic-table state.
- Treat `Some(empty)` or `Some(nonempty)` as proof of trailer validity — the
  dependency can discard forbidden pseudo-fields before exposing the map.
- Remove the END_STREAM watcher because newer h2 preserves reset-after-EOS
  state — reset preservation and trailer validation are separate contracts.

## Re-evaluation Triggers

Re-open this decision only if:

- a normal upstream `h2` release rejects oversized trailer blocks and every
  trailer pseudo-field before publishing a trailer event; or
- production evidence materially increases the frequency or impact of the
  malformed-upstream case.

Before simplifying the watcher after an upstream fix, enable and pass the
pseudo-only, mixed, oversized, fragmented-CONTINUATION, direct-trailer, poll,
same-burst-reset, cache-admission, and connection-reuse contracts at both the
minimum supported and current dependency resolutions.

## Reference Cases

- `tasks/todo/pingora-h2-end-stream-watch-simplification/04-h2-decoder-remedy.md`
- `tasks/todo/pingora-h2-end-stream-watch-simplification/06-decisions.md`
- `../pingora/edgion-changes/review/upstream-limitations.md`
- `../pingora/edgion-changes/pending-issues/h2-trailer-validation.md`
