# Decide compile-time ownership of fixed AI error bodies

Status: deferred representation decision for user review. No runtime change.
Inspected baseline: `2f3569305`.

## Current evidence

`ai/protocols/openai/error.rs::render_openai_error` and
`ai/protocols/anthropic/error.rs::render` serialize fixed borrowed envelope fields
into a new String. AiProxyError has 13 variants. Native Messages already avoids
an intermediate owned JSON Value tree; this proposal must not claim that earlier
optimization is missing. Each dialect retains its exact field order, status and
public vocabulary. String-only envelope serialization has no reachable ordinary
serialization failure today; its existing fallback still has defined wire bytes.

The HTTP consumers do not require String ownership. The OpenAI local reply and
client-API dispatcher immediately move the body into Bytes; HTTP predispatch error
writing and UnaryHead::error_reply likewise convert it to Bytes. This conversion
is a move, not a second payload copy. SSE event builders interpolate the body as
text, which could borrow a static string instead. There is no inherent need to
rebuild a String in these consumers if the producer becomes borrowed/static.

Local JSON dialect is selected from captured client_api, not the chosen provider.
The explicit Chat-to-Anthropic converter retains its OpenAI client contract. Its
separate FixedError renderer already uses static bodies. OpenAI and Anthropic
quota SSE events already reuse their canonical immutable events; do not count
those warm event paths as repeated JSON serialization on every response chunk.

## Decision and constraints

Keep the bounded typed serializer, or investigate compile-time fixed JSON bodies
with an audited single source for the variant/status/message/type/code mapping.
Borrowed static text can support SSE framing while Bytes::from_static supplies
canonical local reply bodies. A literal table must not silently become a second
independently maintained error vocabulary. Decide how new variants and escaped
message text are validated, and preserve exact bytes rather than only parsed JSON.
No representation or implementation is selected here.

Do not introduce a generalized LazyLock error-body cache. The repository's
[local error reply contract](../../skills/01-architecture/00-common/05-unification-points.md#13-local-error-replies--unified-contract)
explicitly forbids an ErrResponse-side cache and prescribes Bytes::from_static for
fixed literals. The existing quota SSE event preparation is not permission to add
another HTTP reply cache. Preserve one canonical body representation and rendering
boundary; no plugin-local wrapper, global registry or response BodyStorage.

The potential benefit is removed fixed JSON serialization/output allocation.
Compile-time bytes increase static data; retained/warm initialization and payload
copies must be distinguished. There is no measured net memory or throughput gain.
The source-controlled set is bounded, but boundedness alone does not establish
that a new owner/interface is the best tradeoff.

## Acceptance for a selected implementation

- Freeze the exact owner/caller allow-list and obtain independent IMPL design
  review. Cover both fixed renderers, the client-API dispatcher, local reply
  adapters, HTTP predispatch writer, UnaryHead::error_reply and SSE framing.
- Preserve all 26 variant/dialect status and exact JSON byte combinations,
  including field order. Keep the current fallback contract or justify its
  elimination from the now-infallible static rendering path; do not silently
  change the error returned when a different failure occurs.
- Verify client dialect independently of selected provider. Preserve content type,
  length, trace response reconciliation, final-response claim/write ordering and
  existing converter/header-hold ownership.
- Preserve named SSE prefix/suffix bytes, absence/presence of terminal markers,
  and existing quota-event reuse. Do not append Chat DONE to native Messages.
- Exclude Responses local_error: its sequence number advances per exchange and
  overflow returns None. Exclude content_filter_refusal: it echoes a bounded,
  escaped model and samples time. Neither is a fixed 13-entry JSON body table.
  This task also does not alter successful unary-conversion memory admission.
- Differentially test exact current wire output for every fixed variant, plus
  the adapter wiring. Measure fixed body preparation/delivery separately from
  ResponseHeader construction, dynamic headers and network writes. Check zero
  allocations for a selected static-body API, and account for static size and
  any initialization cost without adding a request-time cache.
- Preserve existing tests and integration fixtures, run project checks and obtain
  independent final review. Source inspection alone does not prove end-to-end
  allocation removal, protocol acceptance or performance improvement.

Original knowledge: [native Messages errors and accounting](../../skills/04-review/ai-routing/regression-guards/native-messages-preservation-and-accounting.md)
and [local response ownership](../../skills/01-architecture/00-common/05-unification-points.md#13-local-error-replies--unified-contract).
Sources above are under `edgion-gateway/src/`; additional consumers are
`ai/protocols/error.rs`, `ai/http/attempt.rs`, `ai/accounting/usage.rs` and
`ai/conversion/chat_to_anthropic/head.rs`.

Fresh unchanged-source tests passed two OpenAI error, four Anthropic error, nine
AI context and seven HTTP-attempt tests (22 total). These establish current
behavior; existing coverage does not prove every OpenAI variant's exact bytes or
a new static representation. No prototype, benchmark, full suite or live exchange ran.
Formatting, workspace all-target check/Clippy, agent-doc validation and diff
hygiene passed with existing unrelated warnings.
