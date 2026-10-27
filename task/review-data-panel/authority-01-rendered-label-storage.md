# Rendered authority label storage

Status: deferred ownership/capacity decision for user review. No runtime change.
Source baseline: `b61c20ad4`.

## Evidence and decision

Gateway `render_authority_in` obtains an owned label String from the canonical
renderer with a 63-byte output budget. `validate_label` borrows lowercase labels
and owns normalized uppercase labels. `CompiledUpstreamAuthority::compose` then
requests a new String sized to the label plus fixed suffix and copies both into
it. The rendered and any normalized label owners are discarded on return.

Appending the fixed suffix to an existing owned label could remove the separate
composition allocation and label copy when capacity permits. It may instead
require growth, and can retain spare renderer capacity in the returned authority.
The renderer requests a hint of 32 bytes per variable, capped at 63 for this
consumer, and uses bounded geometric growth. For one variable rendering one byte
with `.sandbox.internal`, the initial hint requests 32 bytes while current
composition requests 18. These are source-level capacity requests, not measured
allocation sizes or RSS. Unlike unbounded outbound header rendering, label
rendering here is bounded; suffix append occurs after that label budget and must
not change its meaning into a total-host limit.

Consumers have different lifetimes:

- Main upstream selection moves the returned authority into
  `ctx.upstream_authority`; TLS peer SNI and access-log SNI receive independent
  clones. Host publication later reads the retained context value. Do not infer
  that every clone preserves the original String's spare capacity.
- Response-cache nomination uses the authority for its stamp and key construction
  while building the candidate; it does not retain this String as request authority.
- Managed outbound calls use the authority to replace the URL host, then discard
  the temporary. Their selected live policy remains distinct from the main
  proxy's captured policy.

Compare these alternatives before implementation:

- Keep the current compact composition copy.
- Transfer a rendered or normalized owner and append the suffix, after measuring
  growth and retained capacity separately for each consumer lifetime.
- Use a narrowly justified capacity predicate or compaction policy. Its branches,
  reallocations and eligible workload need evidence; do not assume ownership
  transfer means zero allocations or that shrinking is free.

No alternative is selected. This is separate from
[outbound header ownership](../../skills/04-review/plugin-runtime/regression-guards/request-template-policy-unification.md#rendered-outbound-header-storage-outbound-06): no HeaderValue
or Bytes representation change is proposed here. Existing behavior is valid.

## Acceptance before implementation

- Retain the canonical renderer and validator, trusted/request capabilities,
  Secret denial, missing-as-empty behavior and the 63-byte aggregate label bound
  enforced while rendering. No second parser or new configuration limit.
- Preserve RenderEmpty versus InvalidLabel, mixed-case normalization, literal and
  variable ordering, multi-segment boundary checks, invalid separators and UTF-8
  rejection. Never render the fixed suffix or weaken the composed-IP guard,
  including tests using manually constructed compiled values.
- Preserve captured main-proxy policy, live outbound lookup, cache eligibility and
  authority stamps, pinned dialing, Host/SNI/pool identity and access-log ownership.
  Do not broaden the task into removing independently required SNI/header owners.
- Compare copies, allocation/reallocation calls and capacity retained after return.
  Include lowercase/uppercase, 1/32/63-byte labels, multiple or missing variables,
  varying legal suffix lengths, exact/spare capacity, and concurrent request
  lifetimes. Separate temporary peaks from retained memory; do not infer throughput
  from a source-level allocation removal.

After user selection, use IMPL with an exact implementation allow-list. No runtime,
unit-test or integration-test mutation, prototype or benchmark is part of this task.

## Sources and verification

- Gateway `backends/policy/backend_traffic/authority.rs`: render_authority_in.
- Gateway `link_sys/template/engine.rs`: render_template_in and Output::append.
- Resources `resources/common/request_template.rs`: hint calculation.
- Resources `resources/upstream_authority.rs`: validate_label and compose.
- Gateway `routes/http/proxy_http/pg_upstream_peer.rs`: authority decision ownership.
- Gateway `response_cache/storage.rs`: candidate authority rendering and key use.
- Gateway `plugins/http/common/outbound/call.rs`: selected-policy URL rewrite.
- `skills/01-architecture/02-gateway/request-template-runtime.md` and
  `skills/04-review/plugin-runtime/regression-guards/request-template-policy-unification.md`.

Fresh existing authority tests passed six cases, covering current rendering,
normalization, bounds, empty values, separator rejection and the composed-IP guard.
They establish current behavior, not approval of a storage change. Formatting,
workspace all-target check/Clippy and agent-doc validation passed with existing
warnings. No full suite, live traffic, allocation probe or benchmark ran.
