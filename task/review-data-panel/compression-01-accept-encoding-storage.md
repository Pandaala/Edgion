# Evaluate inline compression algorithm preference storage

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Edgion baseline: `7cffcc9cf`.
Inspected Pingora fork: `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence and scope

Edgion registers ResponseCompression for every HTTP module set. Its early request
hook applies the captured compression switch before the module request filter;
disabled compression skips Accept-Encoding parsing. Enabled requests enter the
fork's protocols/http/compression::ResponseCompressionCtx::request_filter.

The fork's HeaderPhase starts with Vec::new() for accept_encoding. Exact bytes
`gzip` take a dedicated parser fast path and push Algorithm::Gzip into that empty
Vec. A nonzero-sized enum in an initially empty Vec requires dynamic backing
storage on this path. This is source evidence, not a fresh allocator measurement.
Other inputs go through sfv::Parser::parse_list before recognized tokens are
pushed. Replacing the retained Vec would not remove that parser's own storage.

Small inline preference storage might avoid the common short-list allocation.
It also changes context/enum layout, copying and cache pressure. Measure actual
ResponseCompressionCtx and request module size rather than assuming inline storage
is free. This is distinct from context-01's outer module Boxes/Vec and shared
TypeId lookup ownership; no outer-storage benefit is claimed here.

## Semantic and ownership boundaries

Algorithm has seven variants, but the parser retains repeated recognized tokens
in input order. Variant count is not a bound on list length. Preserve ordered
preferences and overflow; a fixed seven-slot array or deduplication is not an
equivalent substitution. decide_action uses first-element and contains queries.

Preserve the inspected parser behavior when evaluating storage. The current
Algorithm::from recognizes five coding names case-insensitively, maps an empty
string to Any and maps the literal wildcard to Other; the parser drops Other.
Quality parameters are not used in selection. This records observed behavior,
not protocol correctness or acceptance of a new requirement. Any negotiation
correction needs separate review and must not be hidden in the storage change.
Do not add raw-header diagnostics or per-request metrics to measure this proposal.

Keep request-captured configuration, hot updates, encoder ownership, header/body
phase transitions and EOS finishing unchanged. Final Edgion response eligibility
still requires one valid Content-Length, no Transfer-Encoding and non-SSE stream
classification under the captured switch. Do not enable unknown-length streaming
compression without the independent flush/delivery evidence required by
skills/04-review/http1/compatibility-constraints/compression-requires-known-response-length.md.

## Decision and acceptance

Retain the current Vec or authorize an external Pingora-fork storage investigation
with an exact repository/revision scope and independent implementation reviews.
Do not edit Cargo's dependency checkout or change this workspace's dependency
revision as an incidental cleanup. This task grants no external mutation scope.

Measure allocation calls/bytes, actual context layout and CPU for disabled,
missing, exact-gzip, several-coding and repeated-long inputs. Distinguish list
storage from structured-field parsing and encoder construction. Test output
selection, unsupported inputs, mixed casing, existing parameter/wildcard behavior,
repeated invocation and request isolation. Preserve origin/cache response rules,
known-length eligibility, streaming latency and encoder/trailer completion.
No end-to-end throughput, heap or transport improvement is established here.
