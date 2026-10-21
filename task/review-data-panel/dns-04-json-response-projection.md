# HTTPDNS JSON response projection

Status: deferred representation decision for user review. No runtime change.
Source baseline: 03717df76.

## Evidence

Gateway ExternalName and AI DNS resolution consume Common's prepared HTTPDNS
generation. In `edgion-common/src/dns/httpdns.rs`, `PreparedResponse::parse`
deserializes the entire JSON document into `serde_json::Value`, then follows the
prepared pointer and reads only the selected array or string. Object keys, strings
and containers outside that path are materialized and subsequently discarded.
The response-mode, pointer and delimiter metadata are already prepared; this
proposal concerns per-response representation, not rebuilding clients or metadata.

The production fetcher enforces a configurable byte limit, at most 65,536 bytes,
while reading chunks. It converts the accumulated Vec into a String by ownership
transfer. This is already a bounded separate outbound exchange, not unbounded
main-response aggregation. The 256 accepted-address limit applies after JSON
parsing and does not avoid constructing the rest of the document. A byte bound
does not imply an identical heap bound; no measured heap amplification or security
defect is claimed here. Direct parser calls do not themselves impose the fetcher
byte cap.

The locked serde_json 1.0.151 Value visitor consumes all elements and replaces
duplicate object keys through Map::insert. Therefore a later duplicate parent
can replace the entire selected subtree. Returning after the first matching
path or after 256 addresses would not preserve current behavior. Complete JSON
validation precedes missing-path, wrong-node-type and delimiter checks.

## Decision alternatives

- Keep the bounded full Value representation if realistic responses are small
  and the additional parser machinery costs more than it saves.
- Evaluate borrowed or raw-value projection that avoids retaining unrelated
  values while validating the complete input. Escaped strings may still require
  decoded owned storage; do not assume all strings can borrow the input.
- Evaluate a narrowly scoped serde seed/visitor for the prepared path. Treat
  complete syntax and number validation, depth handling and duplicate ancestor
  replacement as design requirements, not incidental library details.

No representation, new parser dependency or implementation is selected. Compare
allocation count, CPU and peak live heap for small ordinary responses, metadata-
heavy documents near the byte cap, large selected arrays, escaped/unescaped
strings and invalid inputs. Include the retained input String and any auxiliary
parser state; a reduced allocation count alone does not establish a net benefit.

## Independent lookup-only candidate

Preparing encoded pointer text does not prepare all traversal work. The locked
serde_json 1.0.151 `Value::pointer` splits the pointer and invokes
`replace("~1", "/").replace("~0", "~")` for each visited token, producing
temporary Strings even for ordinary object keys. Empty-token behavior must be
measured separately; this is not a claim of two heap allocations for every token.
A lookup-only change could leave complete Value parsing and its error precedence
intact, independently of the full-document representation decision above.

The repository already uses jsonptr 0.7.1 for Resources' compiled JSON Patch
paths. Its PointerBuf stores encoded text; resolution borrows unescaped object
tokens, but decoding escaped tokens allocates storage. Invalid array-index
characters can also allocate error detail. Reuse therefore needs a comparison
covering both success and failure paths, not an allocation-free claim. Common
has no direct jsonptr dependency today; adoption would add a direct edge to an
already locked version, not require a new version.

Compare the current traversal, existing jsonptr traversal, and a cold prepared
decoded-token/index plan. A decoded plan can avoid repeated work but adds
per-generation storage, allocations and reload overlap. Prefer an existing
canonical mechanism before introducing another generic pointer implementation.
Preserve empty keys, literal slash/tilde keys, nested arrays, object keys that
look numeric, leading zeros, plus signs, dash, index overflow and out-of-bounds
results. A failed traversal must still become the same authored-path diagnostic
and follow the same configured fallback. Measure plain and escaped paths, empty
segments, early/late failures, short/long paths and persistent configuration heap.
No lookup implementation or dependency change is selected by this task.

## Acceptance for a later implementation

Use the current full Value path as a differential oracle. Preserve:

- Full-document validation, including malformed unused subtrees, trailing data,
  Unicode/escape handling, numeric and recursion boundaries. Preserve Parse
  error text/order, or present any deliberate change for a separate decision.
- Last duplicate key wins, including duplicate ancestors and later replacement
  with a missing path or wrong type. Do not stop validation at the IP limit.
- Dotted-path to JSON Pointer semantics, literal empty keys, slash/tilde escaping,
  and array index acceptance. Keep absent-path versus wrong-type errors distinct.
- Mixed JSON arrays ignore non-string members. Array strings retain their current
  whitespace behavior; delimited strings trim candidates. Keep IP order,
  deduplication, bracket handling, the 256-address cap and NoAddresses behavior.
- Empty delimiters reject only branches that split strings. Preset response
  format precedence, unused ttlPath and immutable generation ownership remain
  unchanged. No TTL consumption or new query-time configuration parsing.
- Configured fallback and primary-error precedence, endpoint SSRF policy, client
  reuse, transport byte limits and URL redaction remain unchanged.

The change would remain inside the canonical Common resolver. It grants no new
response BodyStorage, general full-response plugin API, network cache or metrics.
Freeze an exact implementation scope and obtain independent IMPL plan and final
reviews before changing runtime code.

## Original knowledge and verification

- [Prepared HTTPDNS generations](../../skills/04-review/link-sys/regression-guards/httpdns-prepared-runtime.md)
- [HTTPDNS endpoint policy](../../skills/04-review/ssrf/regression-guards/httpdns-endpoint-egress.md)

Existing tests cover nested/escaped paths, mixed candidates, format-specific
whitespace, branch-local delimiter checks, error order, preset precedence and
fallback. They are baseline evidence, not proof that a new projection preserves
all duplicate-key or malformed-unused-subtree behavior. No projection prototype
or allocation benchmark was implemented in this audit.

Independent plan review passed. Fresh unchanged-code verification passed 71
Common DNS and 16 HTTPDNS transport tests, formatting, workspace all-target
check/Clippy and agent-doc validation. Existing unrelated Clippy warnings remain.
No full test suite, deployed integration or performance benchmark ran.
