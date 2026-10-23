# Evaluate preparation of Route predicate dispatch

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `fe0801491`. No runtime change.

## Evidence and decision

HTTP `HttpRouteRuleUnit::compile_match_regexes` prepares positionally aligned
`Vec<Option<Regex>>` for header and query predicates. Both normal and regex-path
builders in `routes/http/conf_handler_impl.rs` call it. Request-time `match_header`
and `match_query_param` still inspect the original optional type string and
dispatch Exact, RegularExpression or unsupported on every reached predicate.
The regex slot alone cannot distinguish Exact, unknown type and failed regex.

gRPC `GrpcMatchInfo::new` prepares its header regex vector similarly. Its header
matcher also dispatches on the original string. The current runtime policies
differ: HTTP unknown types return false; gRPC unknown types compare as Exact.
Both default a missing type to Exact and fail a RegularExpression match when
the compiled regex is absent. This audit records observed behavior, not approval
of unknown configuration types or a request to change validation.

Consider whether preparing dispatch alongside the existing compiled payload
removes enough repeated work to justify representation and construction costs.
Compare replacement of the existing regex slots, additional parallel metadata,
and retaining current dispatch. Do not assume a compact enum is smaller than
Option<Regex>, or that string comparison dominates header hashing, query lookup
or regex execution. No per-request allocation is attributed to type dispatch;
no comparative benchmark or candidate implementation has been run.

This differs from [routing-04](routing-04-header-name-preparation.md), which
concerns header-name preparation, and [routing-05](routing-05-regex-result-storage.md),
which concerns path RegexSet result storage. A future implementation may evaluate
combined storage, but this task grants no approval for either deferred design.

## Architecture and semantic gates

The [predicate-prefilter decision](../../skills/04-review/routing/intentional-design/route-predicate-prefilter-not-justified.md)
rejects a second condition enum/language, mutable predicate index and result
cache. That remains a gate. This investigation concerns only the internal
representation of existing Gateway API predicates at their current evaluation
site. Any enum proposal must demonstrate consistency with the canonical Condition
boundary and receive independent design review; it is not approved by this task.
Do not add a secondary candidate index, change evaluation order or broaden the
existing condition language. Keeping current dispatch is a valid outcome.

- Preserve protocol-specific unknown-type behavior unless separately authorized
  to correct it. Missing headers still fail; incoming repeated headers use the
  first value, and opaque header bytes retain `to_str().unwrap_or("")` behavior.
- Preserve first ASCII-equivalent configured header name, original spelling,
  type/value metadata and relative order. Keep AND composition, positional regex
  alignment, specificity and delegation. Read the original
  [HTTP](../../skills/04-review/routing/regression-guards/http-header-match-first-name-wins.md)
  and [gRPC](../../skills/04-review/routing/regression-guards/grpc-header-match-first-name-wins.md)
  contracts independently.
- Keep the frozen lazy query cache and last-decoded-duplicate semantics. Do not
  parse queries for candidates without query conditions or ahead of existing
  Listener, method and header rejection gates.
- Prepare only at existing cold constructors. Inspect both HTTP builders,
  gRPC construction and direct construction/deserialization paths. gRPC derived
  regex state is serde/schema-skipped; absence must never reinterpret a failed
  regex as an Exact success. Preserve original serialized match metadata.
- Publish derived state with its immutable owner and retain old/new generation
  isolation. Avoid duplicating names/values merely to make a prepared structure
  convenient. Add no request-time compilation, locks, logs or metrics.

## Acceptance and verification limits

Compare realistic exact/regex mixes, early/late predicate failure, candidate
counts, absent headers and query predicates. Measure request CPU, cold build
cost, per-predicate layout and retained old-generation memory. Do not extrapolate
throughput from a count of removed string comparisons.

Tests must cover missing/default/unknown types, exact and valid/invalid/missing
regexes, mixed positional predicates, duplicate normalization and delegation,
both HTTP builders, gRPC derived-state boundaries and old/new snapshots.
Independent plan/final review and project checks are required for implementation.

Fresh unchanged-code selections passed 11 gRPC match-unit, 19 HTTP match-unit,
51 HTTP conf-handler and four resource header-match tests (85 executions).
These existing tests are baseline evidence, not proof of every proposed
representation invariant. No full suite, live traffic or benchmark ran.
