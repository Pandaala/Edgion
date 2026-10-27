# DSL string consumer ownership

Status: deferred for user decision. No runtime change.
Source baseline: 282623ca0.

## Evidence and distinct decisions

`Value::try_into_string` copies Str's Arc payload into String. `RuntimeString`
adds the frozen implicit-coercion error shape, and many builtin argument reads
call it before passing a string slice to the session. This is evidence of a
temporary copy, not a measured request-level bottleneck. A local borrowed-input
path could preserve Value storage without migrating the entire string type.
The choice of a canonical coercion owner and the first bounded caller group
needs review; do not spread per-builtin copies of the coercion match.

`try_as_str` is not a substitute: it returns empty text for ordinary non-string
values, whereas `try_into_string` formats integers, booleans, nil, lists and
map counts. Both reject Body/Xml. Any borrowing design must retain the consumed
Value or its Arc for the complete borrow and release it at the appropriate point.
Preserve argument pop order, once-only evaluation and error timing.

BodyAppend, BodyPrepend and BodyReplace are a separate ownership boundary.
`expect_string` strictly accepts Str, creates String, and transfers it into
Bytes before awaiting DslBodyOps. Eliminating the payload copy by retaining an
Arc-backed owner could change retained memory and owner overhead. Compare the
entire Bytes/storage lifetime and budgets, not just the conversion expression.
Preserve strict TypeError details, empty replacement-pattern rejection, body
generation/deadline/cancellation behavior and the canonical BodyStorage owner.
No new request cache or response aggregation is authorized.

`req.uri` and `req.request_uri` also need separate treatment: the former composes
the current normalized path with the live raw query; the latter composes frozen
entry raw path/query. `compose_uri` omits `?` only for an absent query; a
present-empty query retains `?`. Current owned assembly is checked before a final independent Arc is built. A fragment
or conditional-borrow interface may reduce intermediate work, but must preserve
exact byte-length errors, normalization/provenance and separator semantics.
Do not replace either with the raw Pingora URI, cache assembled text eagerly, or
couple this change to access-log masking. The separate log serialization proposal
is `log-01-raw-uri-fragment-serialization.md`.

## Decision and acceptance

### Body literal pattern preparation follow-up

Source reinspection at `e2cb52a8e` follows the Body consumers beyond the VM
conversion boundary. `OpCode::Contains` copies a Body operand's Str pattern
into BodyTextOperation; TextOperationScanner consumes that String into bytes
without another payload copy, then builds a KMP failure Vec for each scanner.
The ordinary Str branch already borrows its pattern and uses string search.
Do not change that branch as a side effect of Body optimization.

`BodyStorageView::replace_literal` rejects an empty search, captures the logical
snapshot, returns early for equal operands, then copies `from` into the
LiteralMatchScanner's Vec.
The scanner also builds a KMP failure table. It needs owned Send + 'static
state because scans may move into the bounded blocking pool. Borrowing from
the waiting caller cannot satisfy that lifetime. Sharing the input Bytes could
remove this second copy, but a small Bytes slice can retain a large backing
allocation after cancellation while the worker exits. Current VM conversion
owns a String, whereas the generic session/storage API accepts arbitrary Bytes;
the safety argument must cover that boundary, not only today's VM caller.

Static pattern preparation is a separate design choice from borrowing VM
strings. BodyReplace currently evaluates object, search and replacement
expressions in order and emits a generic opcode; even its compile-time literal
empty-pattern check does not install a prepared matcher. A constant-pattern
representation would need reviewed compiler/validator/runtime ownership and
bounded prepared storage tied to the compiled generation. Dynamic operands must
retain their current evaluation and failure behavior. Do not add a per-request
or unbounded process-global matcher cache. Respect the runtime-independent DSL
leaf boundary; Gateway scan owners cannot become leaf dependencies.

For a chosen design, preserve lossy UTF-8 semantics for Body contains and raw
byte, non-overlapping replacement semantics; empty/equal fast paths; match and
segment limits; atomic generation-checked publication; cancellation cleanup;
and scan reservation/refund ordering. Preparing a matcher must not imply that
scanning is free or bypass maxBodyScanPasses. Compare repeated constant and
dynamic patterns, large backings with small views, memory/disk scans and
cancellation. Measure construction work and retained memory separately; no
prepared-pattern or throughput improvement has been measured here.

Additional original sources: `request_body/operation.rs`,
`request_body/storage.rs` under `edgion-gateway/src/`,
`edgion-dsl/src/compiler/expr.rs`, and the body-scan budget/ownership contract in
`skills/01-architecture/02-gateway/dsl-engine-security-model.md`.

### Redis write-command consumer follow-up

Source inspection at `a7b8a76e4` follows SET/SET_PX/HSET from
`plugins/http/dsl/lang/builtins.rs` into
`link_sys/providers/redis/ops.rs` in the Gateway. RuntimeString first produces
owned Strings; provider slice interfaces then retain those caller inputs while
Fred 10.1.0 builds owned command data. Key::from(&str) copies bytes, and borrowed
Value strings use bytes_utils::Str construction. HSET additionally builds Fred's
single-entry Map before command arguments. The prior alias-release optimization
and DEL/HDEL reference-vector removal do not eliminate these payload copies.

The two boundaries are distinct: borrowing the original DSL Value instead of
coercing it into String, and consuming an already-owned String into Fred. Fred's
From<String> conversions can transfer storage into Key or Value::String, but
selecting a provider input interface requires review of all callers, retained
owners and failure context. Existing provider error adapters use key/field after
await. A values-only consuming seam may be evaluated independently; it need not
wait for key/field migration, and existing diagnostics do not prove every copy
necessary. Do not create another coercion match or bypass the LinkSys provider.

Keep runtime pop/coercion order and live-client lookup. SET resolves the client
before rejecting an invalid TTL type; absent/nil/nonpositive integer TTL uses
no expiration, positive integer TTL selects PX. EXPIRE/INCRBY validate numeric
arguments before lookup. Preserve exact command flags, key/field/value bytes,
empty and permitted UTF-8 strings, and fixed script-visible dependency failures
with bounded sensitive diagnostics. HSET success remains true even when its
added-field count is zero. Admin callers share the provider interface; changing
it must not silently remove their required result/error context.

Before implementation, compare short/long, empty and spare-capacity inputs,
scalar coercions, missing/not-ready clients, invalid TTL precedence, provider
failure and cancellation. Measure temporary copies and final retained command
storage separately, including shared/sole DSL owners. Existing byte ownership
and type conversion establish candidate mechanisms, not measured net savings.
Keep the bounded GET validation and HGETALL conversion choices in redis-06 and
redis-07; they are separate response-side decisions. No runtime change or new
benchmark accompanies this follow-up.

### Existing decision scope

Choose a bounded consumer group for measurement/design, or retain its current
ownership. Whole-Value SSO migration is an independent, larger proposal.
The historical SSO knowledge page contains unmeasured workload estimates;
these cannot reject localized borrowing or establish a net migration benefit.

Before implementation, review an exact source allow-list and compare fixture-free
allocation counts, copied bytes and retained memory for short/long strings,
shared and sole-owner values, repeated calls, and scalar/error cases. Measure
final output/storage ownership as well as temporary conversions. Keep original
limits and test exact TypeError/StringTooLong values, nil/empty differences,
URI rewrite visibility and frozen entry metadata as applicable. No throughput
claim follows from allocation counts alone. Use IMPL for any selected change.

## Sources and verification limits

- `edgion-dsl/src/value.rs`: `try_into_string`, `try_as_str`.
- `edgion-gateway/src/plugins/http/dsl/lang/vm.rs`: `RuntimeString`, `expect_string`, Body opcodes.
- `edgion-gateway/src/plugins/http/dsl/lang/body_ops.rs`: owned Bytes operations.
- `edgion-gateway/src/ctx.rs`: `uri`, `request_uri`, `compose_uri`.
- `skills/03-coding/dsl/builtin-error-semantics.md` and `skills/04-review/dsl/intentional-design/sso-migration-not-worth-it.md`.

This source audit adds no benchmark, runtime test result or implementation
acceptance. Library-version conclusions in the old SSO investigation were not
rechecked and must not be treated as current dependency advice.

The body-pattern follow-up passed formatting, workspace check/Clippy, agent-doc
validation and diff whitespace checks. It changed documentation only and ran no
new runtime tests or benchmarks; Clippy warnings remain outside this diff.
