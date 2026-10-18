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
entry raw path/query. `compose_uri` omits `?` for absent or empty query. Current
owned assembly is checked before a final independent Arc is built. A fragment
or conditional-borrow interface may reduce intermediate work, but must preserve
exact byte-length errors, normalization/provenance and separator semantics.
Do not replace either with the raw Pingora URI, cache assembled text eagerly, or
couple this change to access-log masking. The separate log serialization proposal
is `log-01-raw-uri-fragment-serialization.md`.

## Decision and acceptance

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
