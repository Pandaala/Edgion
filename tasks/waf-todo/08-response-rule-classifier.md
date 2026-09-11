# Classify Response-Body Rule Capabilities

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P1 |
| Depends on | `07-incremental-xml-processor.md` |

## Goal

Define which Phase 4 rule units can execute exactly on the streaming upstream
response path before any WAF code is attached to `RawWindow`.

## Work

- Add response execution classes for stream-state, finite-window, and
  unsupported/full-response semantics.
- Classify raw phrase operators, finite-width patterns, transformations,
  negation, chains, captures, and stateful actions conservatively.
- Reject unbounded regex width and complete-response transformations before
  Gateway publication.
- Define EOS, cancellation, downstream-close, and upstream-error behavior.
- Define what a post-header intervention can promise: terminate/truncate is
  allowed; replacement status is not promised after header commitment.
- Freeze cache behavior: the first Phase 4 implementation must explicitly
  disable a cache path that would bypass the upstream body hook.

## Tests

- Capability matrix tests for every supported and rejected Phase 4 shape.
- Proof-oriented tests that no unsupported rule silently enters a finite
  response window.
- Validation tests for cache incompatibility and full-response rejection.

## Exit gate

Every Phase 4 rule unit has a conservative compile-time execution class, and
Gateway publication rejects semantics that cannot be preserved by the
canonical streaming response driver.
