# XML lookahead batching

Status: conditional performance and accounting decision for the user.
Source baseline: `230016d10`. No runtime change or prototype.

## Evidence

Active WAF Full XML inspection reaches `XmlStream::data` in
`edgion-waf/src/seclang/xml_stream.rs`. A contiguous ordinary text/attribute or
CDATA value can be borrowed during its synchronous callback. Before taking that
path, the parser scans bytes through `lookahead`; success then admits the same
source bytes individually through `admit_byte`. This avoids token copying, but
does not eliminate per-byte budget and character validation.

Ordinary lookahead checks the stopping byte too. On successful text/attribute
borrowing, decoded accounting and callback occur before actual delimiter
admission; attribute_done also precedes that admission. CDATA instead admits the
whole three-byte closing delimiter before decoded accounting and callback. A
chunk-local probe_until prevents repeatedly probing the same failed region,
including whitespace that leaves token scratch empty. This is already linear
bounded processing, not an unbounded-rescan defect.

## Conditional decision

Retain the scalar path unless representative profiling identifies material probe
cost. If justified, evaluate bounded search windows with exact scalar handling
at admission and stopping boundaries. Existing search primitives might help find
markers, but searching an arbitrary remaining slice before budget checks would
violate the current lookahead contract. No implementation is selected here.

Measure text, attributes and CDATA separately, short/long values, dense/sparse
markers, CR normalization, UTF-8, successes and failed probes, and varied chunk
partitions. Include complete parser/collector cost and any extra scans or branch
costs, not just delimiter search. There is no measured CPU, allocation or latency
gain from this inspection. This does not authorize changing borrowed eligibility,
scratch ownership or structural parsing.

## Accounting and acceptance gates

lookahead checks checked source-plus-distance and its limit, then raw-token-plus-
distance and its limit, then charges one work unit before inspecting the byte.
It does not advance source/raw counters. Work-limit failure retains the increased
work value; arithmetic overflow does not assign it. A bulk debit at entry or exit
can change competing-error identity and usage even with identical parsed values.

admit_byte separately checks source before assigning it, assigns raw-token count
before checking its limit, charges work and then validates UTF-8/XML characters.
Preserve these partial-state/error boundaries. Keep source/raw/work checks before
unadmitted inspection, and do not silently lower logical charges because scanning
is faster. Any approved redesign must specify equivalent scalar boundary handling
or explicitly approve changed accounting before implementation.

Preserve marker priority, quote-dependent stop sets, CR/CRLF state, incomplete
UTF-8 handling, raw/decoded limits, pending whitespace, tag accounting, callback
borrows, sink/attribute errors and delimiter admission order. Keep chunk-local
probe suppression, sticky first errors and completion behavior. Compare exact and
one-below competing budgets, malformed bytes adjacent to stops, splits at every
delimiter/UTF-8 boundary, whitespace-only chunks, empty CDATA, and rejected sinks.
Check callback sequence, summary and usage as well as final success/error.

The existing `whitespace_chunks_are_linear_and_probes_stop_at_admission_limits`
test, all-splits, borrowed-pointer and strict sticky-error tests are regression
evidence, not acceptance of a batched replacement. Use the original
[native XML contract](../../skills/04-review/waf/regression-guards/native-xml-inspection.md).
This is separate from [WAF-18 namespace lookup](waf-18-xml-prefix-lookup-reuse.md).
Freeze the exact supporting-file allowlist and obtain independent IMPL plan/final
review before implementation. Add no body cache, response aggregation, request
tracing, payload logs or profiling metrics.

Fresh unchanged-production WAF tests passed 873 cases with six existing ignored;
formatting and workspace all-target check/Clippy passed with existing warnings.
No full-workspace suite, external XML comparison, live Gateway integration,
prototype or benchmark ran. Current scalar processing remains the baseline.
