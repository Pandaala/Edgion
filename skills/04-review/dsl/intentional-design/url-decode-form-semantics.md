---
name: url-decode-form-semantics
description: DSL URL decoding uses form-style plus handling; preserve its byte scanner and limits without assuming equivalence to the resource query decoder.
knowledge_type: intentional-design
---

# DSL URL decoding: form semantics and separate decoder boundaries

## Scope and decision

`BuiltinId::UrlDecode` in
`edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` decodes two ASCII hex
bytes after `%` and maps an ordinary literal `+` to space. This form-style
behavior is deliberate; it is not RFC 3986 path decoding. Keep the documented
path-ACL warning and do not introduce an unrequested rename or sibling builtin.
Edgion is pre-release: historical deployed-script compatibility arguments do
not override the current no-compatibility-burden rule. A requested product/API
change still requires an explicit design decision.

The resource query decoder shares ordinary plus handling and well-formed
percent encodings of valid UTF-8, but the implementations are not interchangeable. The prior
claim that they always produce identical decoded bytes was incorrect. The table
below records current source behavior, not approval to broaden either contract.

## Current differences

| Input/boundary | DSL `UrlDecode` | Resources `uri_utils::url_decode` |
|---|---|---|
| `%+A` | `% A`: invalid hex leaves `%` literal, then processes `+` as space | LF: two-byte radix parsing accepts a leading plus |
| `%%41` | `%A`: invalid first escape, then decodes `%41` | `%%41`: failed token consumes the next two Unicode scalars |
| `%FF` | Replacement character U+FFFD | Latin-1 character U+00FF |
| Invalid UTF-8 in a percent run | Lossy UTF-8 over the assembled output byte buffer | Entire contiguous percent-byte run falls back to bytewise Latin-1, including any valid prefix |
| Output admission | Exact decoded UTF-8 byte limit before returning independent Arc text | Owned String with no DSL VM output-limit contract |

DSL invalid escapes leave the percent byte literal; subsequent bytes still go
through the scanner. Resources consumes up to two Unicode scalars after `%`,
preserves the consumed token on failure (including a consumed plus), and flushes
successful percent runs separately. Neither description licenses substituting
an off-the-shelf decoder. The resource contract is maintained in the
[canonical query token boundary](../../uri-normalization/mandatory-design/normalization-contract.md#canonical-query-percent-token-contract).

DSL's plain-input shortcut avoids decoding scratch only when neither `%` nor
`+` occurs; it retains implicit argument coercion, output byte checking and
independent Arc ownership. Marker-present decoding remains unchanged.

## Query provenance and round trips

`ReqQuery` delegates to `PluginSession::get_query_param_ref`; the real adapter
reads `EdgionHttpContext::query_param`, populated by the resource query parser
from the logical current query. Consequently `req.query("callback")` is already
decoded. Applying `url_decode` again performs a second decoding operation; it
is appropriate only when an explicit application contract requires another
encoding layer. The API example uses explicitly encoded text instead.

For a literal plus, `url_encode("+")` yields `%2B`, and `url_decode("%2B")`
yields `+`. The former claim that preserving this round trip requires dropping
form semantics was incorrect. This statement assumes both calls pass their
runtime limits; it does not assert that arbitrary malformed encodings round-trip.

The consistent-hash Arg selector uses the resource decoder for encoded names
after its raw-name equality fast path; selected values stay raw. These consumer
policies are distinct from DSL decoding and must not be unified accidentally.

## Optimization and re-evaluation

A shared decoder or prepared/reusable decoding interface must first resolve
malformed-token policy, invalid UTF-8 policy, output limits, failure ordering and
ownership. The consolidation proposal was
the historical task `dsl-11-url-decoder-consolidation` (declined in the 2026-11-05 data-plane triage; the record survives in git history at `a59c3da81`). No runtime defect, selected replacement, new cache or
measured performance benefit follows from this documentation correction.

Re-evaluate this page when either scanner, query-cache provenance, output-limit
contract or explicitly requested API scope changes. A change to the resource
decoder does not automatically authorize changing DSL semantics. Request-level
tracing remains forbidden; decoding paths do not gain warnings or new metrics.

## Evidence and limits

- Original F-021 finding: `tasks/plugins/plugins-http-dsl-F021-urldecode-plus-decodes-as-space.md`
  (historical path; form-style plus handling was retained).
- Gateway source: `plugins/http/dsl/lang/builtins.rs`,
  `plugins/runtime/session_adapter.rs`, `ctx.rs`, `backends/selector.rs`.
- Existing tests: Gateway DSL `utility_functions.rs` URL cases and Resources
  `tests/unit/uri_utils_test.rs`, including malformed scalar consumption, radix
  leading-plus acceptance and whole-run invalid UTF-8 fallback.
- [Builtin error contract](../../../03-coding/dsl/builtin-error-semantics.md)
  and [output limits](../regression-guards/builtin-output-string-limit.md).

The table is source-traced evidence. Existing tests provide scoped regression
coverage; this correction does not add an exhaustive cross-decoder differential
suite or establish security equivalence across consumers.
