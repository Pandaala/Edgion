# gRPC query preflight decoding

Status: deferred user decision. Source baseline: `dcfc5ffb2`.

## Evidence and scope

Gateway GrpcJsonTranscoder reaches the canonical Resources binding path in
`edgion-resources/src/grpc_transcoding/mapping.rs`. After path captures are decoded
and converted to their scalar fields, CompiledBinding::bind calls
`decode_path(query, false)` for the entire query and discards the result. It then
uses `url::form_urlencoded::parse` to split and decode individual names and values.

The preflight borrows a query containing no percent sign. When any percent escape
exists, it reserves a byte buffer using encoded query length, validates every
escape, decodes the bytes and applies strict UTF-8 validation. That decoded String
is released before pair processing. Plus signs remain literal during preflight;
the subsequent form decoder converts them to spaces. The public outer bind bounds
combined encoded path/query length at 65,536 bytes before selecting a binding.
This is bounded repeated decoding, not evidence of unbounded gateway allocation.

The dependency form decoder splits on raw ampersands and the first raw equals sign,
then applies plus replacement, percent decoding and lossy UTF-8 conversion.
Its permissive malformed-escape/UTF-8 behavior is not a replacement for strict
preflight. Current tests require malformed escapes and invalid UTF-8 to reject
even in an unknown parameter when ignoreUnknownQueryParameters is enabled.

## Preserved ordering and ownership

Global query validation follows path capture/scalar processing but precedes query
pair iteration, field-count checks, field resolution and ignored-unknown decisions.
The relevant failures currently share Invalid; order still controls work and early
exit behavior rather than distinct error strings. Do not describe preflight as
preceding every binding failure.

The existing 64-entry check concerns accumulated field selectors before each
query pair, not a blanket 64-raw-pair limit. Repeated list parameters append values
to one selector, while ignored unknown fields do not add a selector. Changing this
counting model would be a policy change. Preserve overlap checks against path/body
bindings, duplicate scalar rejection, list order and proto/JSON field aliases.

Reusing the decoded whole query by splitting it is incorrect: escaped ampersands
and equals signs are parameter data, not raw separators. A second plus-replacement
pass on decoded text would also corrupt percent-encoded plus data. Returned field
values and BoundRequest must retain their existing independent ownership after the
source query is dropped.

## Alternatives for user decision

- Keep the bounded two-stage path and measure realistic plain, ASCII-escaped,
  UTF-8-escaped and malformed traffic before adding a new validation path.
- Investigate validation-only percent/UTF-8 scanning that avoids materializing the
  whole decoded query. Reuse canonical decoding rules; a separate permissive scan
  or incomplete incremental UTF-8 check would weaken admission.
- Consider an ASCII-escape fast path with strict fallback for decoded non-ASCII
  bytes. Measure additional scans on fallback traffic and preserve rejection of
  every malformed escape, including ignored parameters and trailing bytes.
- Consider one strict component parser replacing the current preflight/form pair.
  It must preserve raw separator, empty-segment, first-equals, plus and UTF-8 rules.
  Maintaining whole-query validation before field processing may require two
  passes or retained intermediate values; quantify those costs. Any deliberate
  validation-order change requires explicit review rather than an assumed match.

No implementation is selected. Do not introduce a second generic query decoder,
request cache, configured selector cache or new resource policy as a side effect.
Path wildcard slash preservation remains a separate existing decoder mode.

## Acceptance before implementation

Compare accepted pairs, field values and errors against the original path for
plain and escaped UTF-8, percent-equivalent names, encoded separators, literal and
encoded plus, empty segments/keys/values, repeated scalar/list parameters and
unknown fields with ignoring both enabled and disabled. Include malformed escapes,
invalid/truncated UTF-8, later invalid parameters after earlier field failures,
64 accumulated selectors, long repeated-list input and the aggregate byte cap.
Exercise path/body overlap and source-query-drop lifetime cases through encoding.

Measure request CPU, allocation count/bytes and peak live buffers for small and
near-cap inputs, including fallback and rejected requests. Preserve the shared
conversion admission, request BodyStorage, fixed client failures, redaction and
access-log ownership; add no tracing, metric or response aggregation behavior.

Relevant original knowledge is the gRPC transcoder admission/outcome ledger under
`skills/04-review/h2-grpc/regression-guards/`. Source evidence also includes the
locked form_urlencoded and percent-encoding implementations. Fresh existing test
results are recorded in INDEX and are baseline evidence only. No replacement
parser or benchmark accompanies this task.
