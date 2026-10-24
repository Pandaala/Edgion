# RequestId source evaluation timing

Status: deferred optimization/API decision for user review. No runtime change.
Revalidated baseline: `ced96f648`.

## Admitted sources and evidence

RequestIdConfig::validation_error permits only no source, TraceId, Header and Ctx.
RequestId::create validates the configuration, and CompiledRequestIdRule::new
revalidates each rule. Header/Ctx names also pass their sensitive-source checks.
The generic KeyGet enum and adapter implement other variants, but that does not
make them reachable through canonical RequestId construction.

Early attachment further limits sources to no source, Header and TraceId, and
key conditions to Header/Method/TraceId. Other conditions and body/dye wrappers
are rejected. plugin_store computes the invalid-early flag from this validator;
run_early_request_ids checks it before dispatch. Ctx remains a normal-phase source.

`plugins/runtime/session_adapter.rs::apply_request_ids` extracts an owned target
header and configured source before calling canonical RequestTraceId::apply_rule.
The core returns Preserved for an established ID or LimitExceeded for a new ID
beyond the slot cap, discarding both inputs. An accepted incoming target also
wins without using the source. Invalid-target and suppression-marker cases still
consider the source under the existing rules.

Header sources use single_header, preserving duplicate-value and sensitivity
rejection. TraceId reads canonical request identity; Ctx reads current context
storage. These two sources copy into String and move it into HeaderValue. No
measured workload establishes how often discarded extraction is significant.

The adapter applies the default patch and reconciles headers, then reconciles
again after every rule, including preserved/limited outcomes. Later rules can
read headers produced or removed by earlier rules. Temporarily moving out the
canonical identity to satisfy borrowing would alter the TraceId source.

## Corrected historical premise

The earlier audit at ad647a342 inferred RequestId reachability from generic
KeyGet dispatch and treated early Query cache initialization as a blocker. Current
constructor and early-attachment validation reject that source, so this is not
a demonstrated side effect of admitted RequestId rules. The general frozen-query
cache caveat remains valid for other or synthetic pre-capture callers; it must
not be used as evidence against optimizing the current admitted source set.
Re-evaluate if source validation expands or a new construction path bypasses it.

## Decision alternatives and acceptance

Keep eager extraction if bounded rule counts make the work cheaper than a more
complex transition API. Otherwise design canonical decision-before-extraction
and completion operations, or evaluate borrowed projection independently. Keep
selection policy with RequestTraceId rather than duplicating state predicates in
the adapter. Header cardinality/sensitivity cannot become a generic first-header
lookup. No interface or implementation is selected here.

Preserve first-write values and forwarding/echo policy, suppression markers,
validation/fallback order, slot and byte bounds, absent markers, ApplyOutcome,
capacity reporting and copied-value masking. Generation/random calls must remain
on the same required fallback path. Preserve default-patch and per-rule header
reconciliation, live Ctx/Header reads and the canonical TraceId exception.

Measure allocations, CPU and retained storage for established/absent/suppressed
IDs, cap exhaustion, valid/invalid targets and repeated sources. Compare real
adapter chains, duplicate/sensitive headers, source names matching earlier
destinations, normal-phase Ctx changes and global/Gateway/route composition.
Pure apply_rule tests do not establish adapter timing equivalence. Broader KeyGet
or query-cache policy changes are outside this task.

Any selected implementation requires a frozen scope, independent IMPL reviews
and project checks. No lazy prototype, benchmark or new observability is included.

Original knowledge:
- [Request identity lifecycle](../../skills/04-review/observability/intentional-design/request-trace-id-lifecycle.md)
- [Frozen query source boundary](../../skills/04-review/uri-normalization/mandatory-design/normalization-contract.md#frozen-query-cache-source)
