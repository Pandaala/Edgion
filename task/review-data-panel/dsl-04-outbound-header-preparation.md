# DSL outbound request-header preparation

Status: deferred for user decision. No runtime change.
Source baseline: e3903fcab.

## Evidence and scope

`Vm::call_builtin_http` in
`edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` consumes the ordered DSL
header Map into a reserved `Vec<(String, String)>`. Each entry first rejects
caller-managed framing for an opaque Body, then calls `Value::try_as_str` and
copies its text. The original value is released as traversal advances. Ordinary
non-string values produce empty text; Body and Xml produce a TypeError. Replacing
this coercion with `runtime_string` would change values and is not equivalent.

Only after all DSL header validation succeeds does the builtin select its client,
create the request builder and pass the staged entries to `builder.header`.
Locked reqwest 0.12.28 parses names and values there, appends successful entries,
and stores the first builder error without parsing subsequent entries. Its
`send` returns that stored error without issuing a request. The builtin still
performs opaque Body size/scan admission before send; those failures can take
precedence over the stored header syntax error. Otherwise the send error becomes
the standard status-zero failure map after upload settlement.

Locked http 1.5.0 implements `HeaderValue::try_from(String)` through shared Bytes
ownership. Thus the staged value copy is transferred to the header value, not
copied a second time there. Header-name conversion follows its separate parser;
this observation does not claim that the complete header path is allocation-free.
The existing vector-capacity optimization does not remove staging.

## Decision requested

Retain staging, or investigate a bounded design that eliminates the intermediate
vector while preserving the two validation phases. A borrowed validation pass
followed by consuming builder insertion could avoid tuple storage, but retains
the original Map and its values through the first pass and revisits entries.
Large container values currently coerce to empty text and are dropped as consumed;
keeping them alive can change peak memory. Preparing typed headers is another
design, not permission to return parser errors early or normalize away duplicate
names. A bulk HeaderMap insertion must prove equivalent append behavior and
ordering for differently cased DSL keys.

This is a request-preparation decision, separate from DSL 01's general string
consumer ownership and the existing request-vector reservation change. No
throughput benefit, allocation benchmark or implementation acceptance is claimed.

## Acceptance before implementation

Use IMPL with an exact source allow-list. Compare allocation counts, copied bytes,
traversal cost and peak retained memory for empty/small/large maps, short/long
strings, scalar/container values, shared strings and rejected late entries.
Include original-map lifetime and final HeaderValue ownership in the comparison.

Preserve insertion order, differently cased duplicate-name appends, empty values,
Nil/absent-map behavior and exact coercion errors. Test an early malformed header
name/value competing with a later Body/Xml value, forbidden Body framing, and
Body size/scan-budget failure. Verify deferred syntax failure still returns the
status-zero map, while DSL validation retains its TypeError/ApiError/BodyAccess
categories and ordering. Preserve settlement/refunds even when send never starts.
Do not change URL/method validation, SSRF, canonical client selection, redirects,
timeouts, body snapshots or bounded response handling.

At this baseline, 42 HTTP-call tests and four opaque-upload tests passed. They
include Body header-value rejection and upload cleanup/accounting, but do not
prove the full competing-error matrix, request-header wire ordering or a
performance improvement. Add focused coverage for the selected design before
changing production ownership.

Original contracts: `skills/03-coding/dsl/http-call.md`,
`skills/03-coding/dsl/builtin-error-semantics.md`, and
`skills/01-architecture/02-gateway/dsl-engine.md`. Dependency evidence comes from
reqwest's `async_impl/request.rs` and http's `header/value.rs` at the locked
versions above, not from an unversioned API assumption.
