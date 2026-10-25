# Rendered outbound header storage

Status: deferred ownership/capacity decision for user review. No runtime change.
Source baseline: `58e9416f2`.

## Evidence and decision

`CompiledRequest::collect_headers` consumes CollectedField values but constructs
HeaderValue through `from_str(&field.value)`. Literal/origin values may borrow;
rendered templates own a String. The latter is copied before the rendered owner
is dropped. The sensitive flag is applied before inserting each accepted value.

The locked http 1.5.0 `HeaderValue::try_from(String)` accepts ownership through
Bytes and uses the same byte-validation mechanism. bytes 1.12.1 transfers the
String into a Vec and retains its capacity; the spare-capacity representation
also allocates shared control storage. Thus replacing every owned case is not
an unconditional allocation or memory improvement.

The canonical renderer requests `CompiledTemplate::hint_len`, including 32 bytes
per variable. Outbound rendering uses RenderPolicy::EMPTY without a byte cap.
For example, one literal byte plus 1,000 missing variables requests an initial
capacity of 32,001 bytes while rendering one byte. Current header construction
copies the result and drops that rendering buffer during collection. Ownership
transfer would retain its spare capacity with the request headers and their
clones during network activity. This is source evidence about requested/storage
capacity, not a measured RSS claim or a newly demonstrated admission violation.

Compare these alternatives:

- Keep compact copying for all rendered header values.
- Transfer only owned strings whose capacity equals length, retaining current
  copying otherwise. This avoids retaining spare payload capacity, but its
  eligible workload, branch cost and net value are not established.
- Transfer general owned strings only after reviewing a bounded retention or
  compaction policy and measuring the lifetime tradeoff. Do not silently add
  a generic template limit or alter rejection behavior to enable the optimization.

No alternative is selected. The existing auth-common owned-header conversion
uses separately bounded claim formatting and does not establish acceptance for
unbounded outbound rendering. This task is distinct from outbound Content-Length
integer storage and URL serialization, and from preparing static header names.

## Acceptance before implementation

- Preserve canonical template evaluation, missing-value behavior, source errors,
  Secret capabilities, provenance and sensitivity before header insertion.
  Header Debug output and cloned requests must retain protection.
- Preserve byte validation, invalid-value skipping, duplicate ordering and
  last-valid overwrite behavior. Cover rendered CRLF and NUL rejection, allowed
  tab/UTF-8 values, empty values, literals/origin values, and an invalid custom
  override after an existing valid header.
- Compare allocation calls, copies and retained capacities separately. Include
  many missing variables, short and long substitutions, exact/spare capacity,
  and simultaneous headers. Account for memory after collection and throughout
  live/cloned requests, not only peak memory during rendering.
- Retain request-hop framing, caller metadata precedence, request/retry ownership,
  outbound cancellation and transport behavior. Do not change Secret logging,
  add observability, create a second renderer, or introduce new configuration.

After user selection, use IMPL with an exact implementation allow-list. Existing
validation equivalence does not itself authorize the ownership/lifetime change.

## Sources and verification

- Gateway `plugins/http/common/outbound/request.rs`: collect_headers,
  CollectedField, collect_list_fields and render_outbound.
- Gateway `link_sys/template/engine.rs`: render_template_in and Output.
- Resources `resources/common/request_template.rs`: CompiledTemplate::compile.
- Locked http `header/value.rs`: from_str, try_from_generic, TryFrom<String>.
- Locked bytes `bytes.rs`: From<String> and From<Vec<u8>>.
- `skills/04-review/plugin-runtime/regression-guards/request-template-policy-unification.md`.

Fresh existing tests passed 28 outbound request-builder and 46 outbound-call
cases. They cover current behavior, not acceptance of an ownership change.
Independent review rejected unconditional transfer for automatic implementation
and approved recording this decision. No runtime/test edits, prototype, allocation
probe, full suite, live transport or benchmark ran.
