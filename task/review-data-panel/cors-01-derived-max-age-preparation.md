# Evaluate Gateway-owned CORS Max-Age preparation

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `5c430d20a`. No runtime change.

## Evidence and prior decision

EdgionPlugins Cors::build_cors_header_pairs formats config.max_age with to_string
for each preflight where the Option<u32> is present. Both terminating 204
preflights and preflight_continue use this builder; actual responses omit the
field. The result is an owned Cow consumed by header insertion or queuing.

The existing intentional-design page explicitly left this derived value
allocating. It accepted the preflight-only cost and judged a shared runtime cache
unnecessary for the earlier allowMethods/allowHeaders/exposeHeaders task. Those
three values already borrow stored strings and should not gain duplicate caches.
The page permits later re-evaluation of derived values; this task records a user
decision opportunity, not approval to override that conclusion.

Native Gateway API CORS already prepares Max-Age in its own validated runtime.
Its ownership and response policy are separate and are not a reason to merge the
two implementations or change EdgionPlugins emission rules.

## Candidate and tradeoff

Evaluate one optional formatted String owned by the Gateway Cors plugin instance,
derived centrally in from_arc. Cors::new already validates/initializes its cloned
configuration and delegates there. Keep the shared CorsConfig and CRD/transport
schema unchanged. The candidate would borrow the prepared text in header pairs,
removing repeated decimal formatting and its temporary String on preflights.

The retained String and larger plugin instance cost memory per constructed plugin,
including instances that never see a preflight. A u32 needs at most ten decimal
characters; this does not bound allocator capacity, metadata or total retained
memory. Final header conversion still copies bytes. Actual requests do not benefit.
Measure construction and retained-memory cost against preflight frequency and
header-pair preparation cost before deciding whether to implement or retain the
current simpler representation. No alternative implementation or benchmark ran.

## Acceptance for a future IMPL

- Review an exact Gateway-owned allow-list and update the owning knowledge page
  only after independently verified implementation. Do not add schema cache fields.
- Preserve validation, initialization-failure and origin-selection behavior.
  Initialization failure does not imply every raw exact-origin match is denied.
  Keep from_arc's already-initialized-input contract; do not recompile matchers
  there or introduce request-time initialization.
- Preserve None omission and decimal bytes for zero, typical values and u32::MAX;
  do not clamp, normalize or change schema validation. Check both constructors.
- Preserve preflight-only emission, header order, terminating/continuing preflight
  behavior, actual-response omission, credentials, timing, private-network and
  Vary rules. Existing fixed lists and reflected values keep their owners.
- Derive the value for every new immutable plugin generation and verify old/new
  instances retain their own values across replacement. Do not mutate published
  config or share stale prepared data between generations.
- Measure only the eliminated temporary formatting allocation separately from
  final header insertion; avoid an end-to-end zero-allocation claim. Add no logs,
  metrics, response-body cache or general prepared-header abstraction.

Original authority/evidence:
`skills/04-review/cors/intentional-design/response-header-values-borrowed.md`,
`skills/04-review/cors/regression-guards/new-must-call-validate-and-init.md`, and
`skills/04-review/cors/regression-guards/native-prepared-response-values.md`.

Fresh unchanged-code verification passed 45 Gateway CORS-filtered tests,
formatting check, workspace check/Clippy and agent-doc validation. Existing
unrelated warnings remain. No full test suite, live integration or benchmark ran.
