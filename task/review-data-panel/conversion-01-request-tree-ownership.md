# Decide Chat-to-Anthropic request-tree ownership

Status: deferred representation and performance decision for user review.
Source baseline: `663b29679`. No runtime implementation selected.

## Current evidence

`ai/runtime/dispatch.rs::prepare_ready_attempt` parses a fresh owned serde_json
Value from the retained plan body when preparing a Chat-to-Anthropic candidate.
It passes that Value into `prepare_anthropic_request`, together with the selected
provider model, backend default output limit, usage option and serialized-output
cap. The converter immediately borrows the root object and retains the input
document throughout validation, translation and serialization.

The resulting output tree still copies input-owned content: `parse_tools` clones
the parameters subtree and copies names/descriptions; `parse_stop` copies stop
strings; text-content helpers copy strings into translated text blocks. Output
IDs and names also become owned strings. These are source-to-output copies,
not the already removed reconstruction of completed output subtrees.

Two earlier optimizations are present and must not be reported as missing:

- Tool declaration names, all call IDs and pending result IDs are borrowed
  HashSet keys into the input document. Equality is by value, and iteration
  order does not construct output. IDs remain unique across completed rounds.
- Completed translated content arrays, grouped tool-result arrays and parsed
  tool-argument objects move into their parent output objects. JSON tool
  arguments still require parsing from their string representation.

The owned input makes consumption technically possible, but its borrowed
validation keys prevent an unplanned sequence of field moves. Consuming fields
may require a different validation phase or key owner. Borrowed serialization
could instead retain input ownership while avoiding some output copies, but
requires an explicit translated representation and serialization contract.
Neither direction is a mechanical replacement of clone with a reference.

## Decision and scope

Compare retaining the current bounded-exchange implementation with consuming
input fields or a borrowed output representation. Choose only after measuring
representative bodies and reviewing the exact ownership/validation design.
Keep this investigation within one synchronous request-conversion call.
Cross-attempt parsed-body caches, extra request BodyStorage, global interning,
native protocol parsing and response conversion are outside this decision.

A prepared result depends on the selected provider model and backend default
when the request omits its output limit. Reuse across candidates is not justified
by identical source bytes alone. This task does not select retry caching.

The existing BoundedWriter enforces serialized bytes while writing; it does not
bound input/output DOM heap or prove a peak-memory improvement. The explicit
unary response converter's reservation ledger is not a request-converter memory
budget. Preserve current limits; any additional admission or memory policy needs
its own reviewed scope rather than silently borrowing response guarantees.

## Acceptance for a selected implementation

- Freeze exact production/test/knowledge paths and obtain independent IMPL plan
  and final reviews. Preserve canonical request-body ownership and add no logs,
  metrics, second parser or independently retained body cache.
- Preserve validation and error precedence: unknown top-level fields, client
  model, stream/options, compatibility fields, request-first output limits,
  tool declarations, message graph, stops and tool choice. Preserve fixed
  dispatch error mapping and output-cap versus serialization failure categories.
- Preserve contiguous system-prefix rules, required text, supported roles,
  declared names, bounded/unique call IDs across rounds, pending-result removal,
  reverse-order parallel results, result grouping and final-user-turn checks.
- Compare exact serialized bytes, including object ordering, escaping,
  permitted multibyte strings, empty text parts, nested schemas/tool arguments,
  signed/unsigned numeric boundaries and negative zero. Keep exact-output-cap
  success and one-byte-short rejection. Do not equate JSON semantic equality
  alone with complete wire parity.
- Exercise malformed and large requests, many tools/calls/results, duplicate
  failures at different stages, streaming flags, explicit/default token bounds,
  and retries selecting different provider models or defaults. Keep response,
  credential, quota and body-generation behavior unchanged.
- Measure allocations, copied bytes, CPU and peak live ownership across input
  DOM, translated representation, validation sets and serialized buffer together.
  Include retained capacities and any replacement key ownership; report source
  parsing separately from conversion. Fewer clones alone do not prove lower
  retained memory or whole-request throughput.

## Sources and current verification

Paths above are relative to `edgion-gateway/src/`.

- [Gateway AI ownership](../../skills/01-architecture/02-gateway/00-overview.md)
- [Native Messages boundary](../../skills/04-review/ai-routing/regression-guards/native-messages-preservation-and-accounting.md)
- [Request-first output bound](../../skills/04-review/ai-routing/regression-guards/ai-cost-weighted-request-output-bound-parsing.md)
- [Header-baseline restoration decision](retry-03-ai-request-baseline-rebuild.md)

Fresh unchanged-source verification passed 19 request-converter and 39 dispatch
tests (58 total). Existing tests exercise exact output caps, nested wire values,
tool graph validation, multi-round IDs and candidate preparation. They do not
validate a consuming or borrowed-output prototype, establish a request-converter
heap bound or cover all future acceptance cases. No new tests, benchmark, full
suite or live-provider integration ran.
Formatting, workspace all-target check/Clippy, agent-doc validation and diff
hygiene passed with existing unrelated warnings.
