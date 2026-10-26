# Cookie stripping survivor storage

Status: Cookie representation and CPU/allocation decision remains deferred.
Source baseline: `ff0551111` (historical Cookie evidence).

## Scope

The Query portion of auth-01 is implemented by the request-scoped lazy Query owner:
structured deletion preserves raw survivors, caches canonical decoded names, and
publishes concrete targets at reviewed transport boundaries. See
[verification](../../tasks/pending/lazy-query-management-verification.md) for
functional results and measured CPU/memory tradeoffs. This is not a claim that
all Query workloads improved. Cookie storage is unchanged and remains open here.

## Cookie evidence

`plan_cookie_strip` consumes an arbitrary iterator of borrowed Cookie lines once,
trims and classifies pairs, then joins survivors with semicolon-space separators.
It distinguishes no valid UTF-8 line from present lines with no surviving pairs.
Unlike query stripping, Cookie stripping merges/replaces survivors even when no
name matched or the names list is empty. KeyAuth and JwtAuth share both helpers;
OIDC also strips its session and state cookies through the shared Cookie helper.

The vectors retain slices, not copies of every payload. Pointer-container storage
scales with survivor count, and the joined output scales with surviving bytes.
Current unit evidence establishes exact output capacity for the selected Cookie
fixtures. The subsequent owned HeaderValue conversion preserves the output buffer
and allocates zero times in those fixtures. Pinned http 1.5.0 transfers the String
through bytes 1.12.1; its From<Vec<u8>> implementation uses a boxed-slice path when
length equals capacity, but allocates Shared metadata and retains capacity when
they differ. This is relevant to an alternative output builder, not evidence of
whole-callback zero allocation or measured throughput.

## Decision alternatives

- Keep borrowed survivor vectors and exact joining until measurements justify
  changing the representation.
- Append survivors directly to a String, measuring growth, copies and retained
  capacity. Input-length reservation must include inserted separators across
  separate header lines; summed input lengths alone are insufficient.
- Count output bytes and fill an exact-size output in a second pass. The current
  helper accepts an arbitrary Iterator without Clone/rewind, so a second pass
  requires a reviewed source interface or retained decisions.
- Evaluate bounded inline survivor storage with a heap fallback against stack
  footprint; do not impose new input limits or copy each surviving payload.

Shrinking may reallocate/copy and must be measured. Excess capacity is retained
allocation size, not evidence that output contains removed credentials. Do not
introduce unsafe views, extended mutation borrows, or global scratch pools.
Query and Cookie matching semantics must remain distinct.

## Acceptance for a selected implementation

Inspect every Cookie line, preserve UTF-8 filtering, whitespace trimming, raw-name
matching and first-equals splitting, and remove all selected names atomically.
Preserve Absent/RemoveHeader/Replace distinctions for empty lines, no survivors,
no matches and empty names lists. Merge multiple lines into one replacement.
Preserve header validation, sensitivity, protected headers, deferred cleanup and
ignored setter errors. End immutable request borrows before mutation.

Retain the existing owned-conversion allocation/pointer regression or independently
review a measured whole-path tradeoff before changing it. Compare absent, empty,
multiple-line, few/many-pair, long-value and high-removal inputs. Measure CPU,
allocation calls, requested/retained bytes and stack footprint including conversion
and mutation, with fixtures outside the measured closures. A selected design needs
its own plan, independent reviews and verification. No new observability is needed.

## Original knowledge and historical baseline

- `skills/04-review/key-auth/regression-guards/hide-credentials-query-cookie-strip.md`
- `skills/04-review/jwt-auth/regression-guards/hide-credentials-query-cookie-strip.md`

Historical verification passed 32 shared-auth, 14 KeyAuth, 47 JwtAuth and four
OIDC cookie-strip tests (97 total), formatting, workspace check and Clippy with
unrelated warnings. This is historical evidence, not measurement of a proposed
Cookie representation. No Cookie alternative has been selected or implemented.
