# Evaluate replay header-name parsing through Pingora

Status: deferred for user decision. Priority: P3. Source baseline: 37041f543.
Documentation only; no runtime, dependency or API change.

## Evidence and decision

`edgion-gateway/src/response_cache/storage.rs::Entry::decode` validates each
stored name with `http::HeaderName::from_bytes` before passing the typed name to
`ResponseHeader::append_header`. The locked Edgion Pingora revision
`1aaa45cc70430bc782990bc20ddee3fef36d6b99` then converts that HeaderName through
`IntoCaseHeaderName` and parses the resulting bytes into a HeaderName again in
`pingora-http/src/lib.rs::append_header_value`.

In `pingora-http/src/case_header_name.rs`, mapped standard names use static
titled spelling (for example Cache-Control and Date). Other names use
`Bytes::copy_from_slice` of the normalized name. The title table covers only a
subset of standard headers; unmapped standard names also use that copy path.
The locked http 1.5.0 parser represents recognized standard names directly,
while custom names create owned normalized bytes. This is evidence of repeated
name processing, not a claim that every name allocates or that removing a parse
improves whole-request throughput.

Passing raw metadata through Bytes is not automatically equivalent: stored names
are normally normalized by capture, whereas the current typed conversion applies
Pingora's title table. Raw bytes would change mapped names' case in the enabled
case map and HTTP/1 serialization. A syntactically valid mixed-case custom name
in stored metadata is also normalized by the current first parse. Removing the
case map or directly mutating the underlying HeaderMap does not preserve that
owner's representation contract.

Keep the current implementation as a valid option. If measurements justify it,
evaluate a separately reviewed canonical Pingora API for already parsed names
that preserves the same case representation without reparsing normalized bytes.
That would cross the Gateway/fork API boundary and requires its own scope and
review; no API, dependency revision or implementation is selected here. Do not
copy the title table into Gateway or add another header representation/cache.

## Required boundaries and acceptance after selection

Preserve complete metadata framing/name/value validation, malformed-tail rejection,
field/byte limits, duplicate value order, standard/custom name normalization,
case-map consistency and current fallible map-size behavior. Preserve existing
failure outcomes and evaluation order; do not make malformed metadata acceptable
because it was valid at capture. Do not introduce a persistent validity token
that can outlive header mutation.

Keep origin-policy and Vary validation, current-identity eviction, freshness,
epoch fencing, header-work charges, reader/body ownership and admission budgets.
A typed insertion path must not bypass these checks or weaken body bounds. Any
additional owner/field needs a memory ledger. No tracing, new metrics, schema
change or independent Gateway header subsystem is part of this investigation.

Measure current versus proposed parsing, copies, allocations and retained bytes
for title-mapped standard names, unmapped standard names and short/long custom
names, including mixed case and repeated values. Verify exact case-aware HTTP/1
serialization and HTTP/2 header semantics, malformed names/values/tails, map-size
errors and downstream mutations. Compare complete replay, not just the append
helper; field-count capacity and Age/Content-Length construction are independent
costs. Obtain independent plan and final review before any implementation.

This is separate from [replay header capacity](cache-05-replay-header-capacity.md)
and [boundary-local Vary validation](cache-03-boundary-local-vary-validation.md).
The [ForwardAuth name ownership task](forward-auth-01-prepared-denial-header-names.md)
and [session name ownership task](lb-04-prepared-session-header-name.md) document
related fork conversions but select no general typed insertion API.
Original ownership contract: [bounded response cache](../../skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md).

Fresh unchanged-code verification: `cargo test -p edgion-gateway --lib response_cache::`
passed 43 tests; workspace all-target check and Clippy passed with existing
warnings. These tests establish the current cache baseline, not a proposed fork
API or a new exact wire-case matrix. No prototype, benchmark, full suite or live
origin test ran.
