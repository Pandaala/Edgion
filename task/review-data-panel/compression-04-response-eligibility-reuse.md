# Evaluate callback-local compression eligibility reuse

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Edgion baseline: `12e76a965`.
Inspected Pingora fork: `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence and scope

Edgion's final response hook selects a compression level before the downstream
ResponseCompression module forwards the header to ResponseCompressionCtx.
In the pinned fork's `pingora-core/src/protocols/http/compression/mod.rs`,
response_header_filter calls depends_on_accept_encoding and then decide_action.
Both can call compressible on the same response: compression is enabled, the
response has no Content-Encoding, and the parsed Accept-Encoding list is nonempty.
The intervening add_vary_header only changes Vary, which compressible does not read.

compressible looks up and parses Content-Length, rejects a parsed length below
20, then reads Content-Type, scans for the case-sensitive substring `zip` and
uses the existing lazy regex. Small responses stop before the MIME work. Missing
or unparsable lengths fall through to MIME checks. These are source-level repeated
operations, not measured CPU cost or evidence of repeated regex compilation.

Evaluate sharing a lazily computed eligibility result within that single header
callback. Keep queries outside that callback, including will_decompress, independent:
headers can change between calls. This is not a persistent header-derived cache,
new request context field, or a replacement for Edgion's final eligibility gate.

## Boundaries and decision

This needs separately authorized work in an exact external Pingora repository
and revision, followed by an explicit adoption plan. Do not modify Cargo's
dependency checkout or change this workspace's dependency revision as cleanup.
The alternative is to retain the current code if measured savings do not justify
the helper/API complexity. This task grants no external mutation authority.

Preserve short circuits rather than eagerly computing eligibility everywhere:
disabled contexts, informational and bodyless responses return early; empty
preferences and Content-Encoding can bypass the second check. A present coding
can still reach the first check when decompression is disabled. Keep Vary insertion
even where negotiation or a disabled/unsupported selected codec creates no encoder.
The first preference, Any/Other behavior, coding comparisons and decompression
selection are existing behavior to preserve, not protocol corrections authorized
by this optimization.

Retain exact MIME matching, case sensitivity, the zip heuristic, the 20-byte
threshold, usize length parsing and invalid/missing header handling. Do not merge
these checks with Edgion's different single-value, trimmed u64 length admission.
Keep the request-captured switch, no-Transfer-Encoding and non-SSE requirements,
origin/cache-hit behavior, ETag adjustment, header/body phase transitions, encoder
ownership, and footer-before-trailer/exactly-once finalization.
The known-length restriction remains governed by
`skills/04-review/http1/compatibility-constraints/compression-requires-known-response-length.md`.

Preference storage (compression-01), empty output buffers (compression-02) and
exclusive Zstd encoder access (compression-03) remain separate proposals. No
logging, per-request metric, codec change or whole-response aggregation is needed.

## Acceptance for later implementation

Compare exact actions and response headers before/after for enabled/disabled
compression and decompression, empty/nonempty preferences, Any and unsupported
preferences, disabled selected codecs, absent/present/opaque Content-Encoding,
existing Vary values, and informational/bodyless/final responses. Include length
boundaries 19/20, missing/malformed/overflow values, missing/opaque/mixed-case MIME,
zip-containing values and accepted MIME families. Keep independent query behavior
when headers mutate between will_decompress and the final header callback.

Use scoped call-count evidence and CPU measurements to separate duplicate
classification from structured-field parsing, encoder creation and actual encoding.
Verify disabled/early-return paths gain no unnecessary work. Run the fork's
compression tests, finalization/trailer regressions and Edgion's final-response
eligibility tests. Preserve output and decoded-body equivalence across chunk/EOS
boundaries. No benchmark, allocator measurement or runtime change was made for
this deferred proposal.
