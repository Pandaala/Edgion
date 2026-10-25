# Evaluate URL decoder consolidation without changing consumer semantics

Status: deferred for user decision. No runtime change.
Source baseline: 59f102d0c.

## Evidence

Gateway DSL `BuiltinId::UrlDecode` and Resources `uri_utils::url_decode` both
handle percent encodings and ordinary plus-to-space conversion. They currently
have distinct algorithms and ownership contracts. The
[corrected knowledge page](../../skills/04-review/dsl/intentional-design/url-decode-form-semantics.md)
records exact counterexamples to the former full-parity claim.

DSL requires two ASCII hex bytes, reprocesses bytes after an invalid percent,
and applies lossy UTF-8 conversion to its assembled output buffer. Resources
consumes up to two Unicode scalars per token, accepts a radix leading plus,
and uses Latin-1 fallback for an entire invalid contiguous percent-byte run.
DSL checks exact output UTF-8 bytes against the VM limit and returns independent
Arc text; Resources returns an owned String to query and encoded-name consumers.
Both already bypass decoding work for plain text; DSL still performs its
existing RuntimeString conversion. No duplicate decoded cache is proposed.

Simply replacing DSL's loop with the resource helper changes malformed-input
results and can introduce another intermediate String. A shared implementation
could reduce maintenance duplication, but no common API or runtime performance
benefit has been demonstrated. Current differences alone do not establish a bug
or authorize normalizing them.

## Decision requested

Retain separate implementations, or investigate a shared implementation with
explicit consumer policies. Any deliberate convergence of malformed-input
semantics requires a separate product/security decision. Do not infer that
shared plus handling, a historical parity claim or pre-release status supplies
that decision. No implementation is selected by this task.

## Acceptance before implementation

- Freeze a minimal source allow-list under IMPL. Preserve argument coercion,
  exact DSL output-limit errors, malformed-token consumption and UTF-8 outcomes
  unless a specific semantic change is approved first.
- Differentially cover empty/plain inputs, marker positions, ASCII/UTF-8,
  adjacent percent runs, invalid bytes, incomplete tokens, embedded plus,
  `%+A`, `%%41`, and valid-prefix/invalid-tail runs. Include exact and over-limit
  DSL outputs and paths where the existing input String is already oversized.
- Keep query provenance: `req.query` reads the frozen decoded cache; whole-query
  reads are raw. Keep route matching, query duplicate-key behavior, LB raw-name
  fast paths/raw selected values, and auth query-removal policy unchanged.
- Compare construction, copies, allocation/retained capacity and scan work for
  plain, sparse-marker, dense-marker and malformed inputs. Policy dispatch and
  output adaptation may outweigh reuse; measure before claiming improvement.
- Preserve the workspace dependency direction, canonical query owner and DSL
  output ownership. No cache, shared mutable scratch, tracing, metric or new
  builtin is authorized.

DSL-01 separately tracks argument string ownership. URI-01 separately tracks
comparison-only decoding for query names under the resource decoder's current
semantics; neither task authorizes cross-decoder convergence.

This task records source evidence and an unresolved design choice, not acceptance
of a new decoder or exhaustive end-to-end verification.

## Knowledge correction

The owning form-semantics page, both public URL-decode API sections and
`skills/03-coding/dsl/builtin-error-semantics.md` now distinguish malformed-token
scanning from literal preservation and avoid presenting already-decoded
`req.query` values as raw input. That documentation follow-up is resolved;
the consolidation decision above remains deferred and changes neither runtime.
