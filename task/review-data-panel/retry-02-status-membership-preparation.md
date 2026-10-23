# Prepared retry-status membership

Status: deferred optimization for user decision. Baseline: `0a966a136`.
No runtime change or selected representation.

## Evidence and decision

`edgion-resources/src/resources/http_route_preparse.rs::parse_retry` already
filters status codes to 400..=599 and deduplicates them in first-seen order using
a temporary 200-element presence array. ParsedRouteRetry retains Option<Vec<u16>>
with at most 200 codes. HTTPRouteRule.parsed_retry is skipped by serde and schemars;
this is prepared runtime state, not a new operator-facing schema field.

Three Gateway consumers still use list membership: the ordinary status retry in
`routes/http/proxy_http/pg_upstream_response_filter.rs`, useful-work classification
in `lb/sample.rs`, and AI status handling in `ai/http/attempt.rs::on_raw_response`.
They read the request-captured route rule. EWMA sampling runs before ordinary
status retry, but its classification checks membership only below 500. Sampling
also exits for absent upstream/backend or non-EWMA policy. Consequently two list
lookups occur only on ordinary EWMA-selected attempts below 500 with configured
codes. A 5xx is always peak-only for EWMA without checking codes; it triggers
ordinary retry only when configured. AI attempts currently have no LB-selected
backend and therefore do not pay the EWMA lookup.

Options are to retain the bounded vector, prepare compact membership at route
publication, or reuse one response-local membership result for the duplicate
lookup subset. A compact 200-status bitmap can avoid linear lookup, but storage,
alignment, access cost and generation construction must be measured against
typical short lists. Retaining both vector and bitmap adds state; replacing the
vector changes its ordered representation and consumers. Local result reuse has
narrower reach and must not add eager work on previously skipped paths. No option
has measured net value yet, and none is approved here.

## Acceptance for a selected implementation

- Preserve absent versus explicitly empty codes, accepted range, invalid-code
  handling and warnings, and bounded deduplication. The existing vector test
  requires first-seen order; if replacing that representation, explicitly review
  affected consumers/tests rather than silently changing it or retaining an
  obsolete compatibility copy. No new configuration field or serialized cache.
- Publish any prepared state with the same immutable rule generation and rebuild
  it at the canonical preparse boundary. Preserve old requests during reload and
  keep all three consumers on one canonical membership contract. Freeze an exact
  Gateway/Resources implementation allowlist before editing shared types.
- Preserve EWMA's all-5xx peak-only behavior and configured-status classification,
  error-sample ceiling, header-time sampling and per-attempt ordering before retry
  returns. Do not merge useful-work classification with outlier FailureClass:
  429 intentionally has different meanings there.
- Preserve interim-header early return, terminal 101 behavior, AI cooldown/failure
  classification, attempts/deadline/body-replay/final-write/budget gates and their
  error ordering. Membership is a retry trigger, not proof a retry is permitted.
  Do not add GRPCRoute retry-code support or change its existing unsupported scope.
- Compare all u16 statuses against current membership, including 399/400/499/500/
  599/600, None, empty, duplicates and invalid raw inputs. Exercise ordinary and
  AI paths, EWMA versus other policies, old/new generations, 429 and configured
  versus unconfigured 5xx. Keep existing first-seen and per-attempt regressions
  unless a reviewed replacement covers their full contracts.
- Measure empty/short/full lists, match position and misses, complete response
  callback costs, per-rule retained state, construction/reload and paths that
  currently skip lookup. A source-level operation count is not throughput proof.
  Add no hot-path tracing, metrics, shared cache or lazy request-time preparation.
  Require independent IMPL plan/final review and repository checks.

## Verification limits

Fresh current-code tests passed eight LB sample, five response-hook LB sample and
seven AI attempt tests. No resource representation, prototype, benchmark, live
reload or integration test was added. Relevant original contracts are the
[interim/status-range guard](../../skills/04-review/routing/regression-guards/1xx-response-filter-state-pollution.md),
[per-attempt outlier guard](../../skills/04-review/routing/regression-guards/status-retry-outlier-detection-per-attempt.md)
and [Peak-EWMA design](../../skills/12-algorithms/lb-peak-ewma.md).
