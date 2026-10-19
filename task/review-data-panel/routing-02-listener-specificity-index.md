# Publication-owned listener specificity lookup

Status: deferred indexing and workload decision for the user.
Source baseline: 99c705e82. No runtime change.

## Evidence

runtime/store/listener_gateway_info.rs owns the active listener specificity
algorithm. GatewayInfoIndex::has_more_specific returns false immediately for an
exact current hostname. For a wildcard or catch-all current listener, it visits
GatewayListeners groups until a matching more-specific listener is found.
GatewayListeners::has_more_specific scans its all entries, skips absent hostnames,
uses the canonical hostname_matches_listener predicate, and compares exact versus
wildcard or wildcard string length. This is comparison work, not regex compilation
or evidence of request-time allocation.

runtime/matching/route.rs invokes the whole physical inventory check during
named and unnamed parent eligibility. Repeated candidate/parent checks can revisit
the same inventory. The HTTPS requireSniHostMatch branch in pg_request_filter.rs
instead calls GatewayListeners::has_more_specific on the selected Gateway only.
These are intentionally distinct query scopes and must not be collapsed.

More-specific listeners remain authoritative when they have no eligible Route
or reject this Route's namespace/kind. Narrowing an index to serving or policy-ready
Routes would enable forbidden fallback. The existing regression
 a_more_specific_listener_blocks_fallback_even_when_it_denies_the_route
covers one part of this contract. TLS attachment projection also requires hostname
and logical ownership eligibility on the same accepted attachment.

## Candidate designs

- Keep the existing scan when measured inventories and call frequency make its
  simple representation preferable. This defers optimization, not review coverage.
- Prepare a specificity query structure inside the immutable publication,
  preserving both full physical-key and per-Gateway scopes. Reuse canonical
  hostname mechanisms where their actual semantics match; do not introduce an
  independent matching policy or mutable global cache.
- Compute the best applicable specificity once for a query scope and reuse it
  locally across candidate checks against the same snapshot and exact hostname.
  Evaluate plumbing and retained-state cost before adding request-local storage;
  never reuse a result after hostname/snapshot/scope changes.

An additional index trades build time and retained memory for fewer scans.
Measure listeners per physical key and Gateway, exact/wildcard/catch-all mix,
lookup repetition, early exits and rebuild frequency. Include small inventories,
where indexing can cost more than scanning. No measured CPU, allocation or
throughput benefit is established by the present source audit.

## Matching and publication boundaries

The current canonical radix_hostname_matches helper is case-insensitive for
exact patterns; wildcard suffix matching uses the current raw string suffix
semantics and accepts multiple labels. A normalized radix query is not
interchangeable merely because both APIs match hostnames. Preserve actual caller
normalization and rejection behavior; do not silently tighten malformed-input
handling, fold wildcard case differently or broaden accepted patterns.

Maintain exact > wildcard > catch-all ranking, strict longer-suffix comparison,
identical/equal-length behavior, cross-Gateway shadowing in the full query and
Gateway-local shadowing in SNIHost isolation. Preserve listener port/section
identity, namespace/kind admission and authoritative unavailable owners.

Build any derived structure from the same admitted listener facts as the owning
publication. It must share snapshot lifetime and hot-reload replacement with the
existing inventory, without a second independently swapped cache. Old requests
retain their selected snapshot; a fresh lookup cannot mix new specificity with
old listener identities. This task grants no change to publication proofs,
Gateway recreation policy, route precedence or accepted attachment association.

## Acceptance and verification limits

Compare proposed results against current scanning across exact, nested wildcard,
catch-all, multi-label, case, equal-length, no-match and multiple-Gateway cases.
Exercise route-denying and empty-route more-specific owners, both parent forms,
SNIHost 421 isolation, snapshot replacement and concurrent old-reader lifetimes.
A selected implementation needs its own IMPL plan and performance evidence
appropriate to the benefit claimed. Add no request tracing or inventory metrics.

Fresh baseline tests passed 23 route-matching, 19 listener-store and 85 TLS tests.
The broader request-filter group passed 44 and failed its existing cache reload
fixture with backend policy ancestor limit; see
[the existing fixture task](verification-01-cache-policy-fixture.md).
That failure is not repaired or waived here, and the group is not passing evidence.
No live route-conformance or performance test ran.

Original knowledge: [snapshot ownership](../../skills/04-review/concurrency/regression-guards/listener-gateway-info-dual-arcswap-merged-snapshot.md),
[TLS attachments](../../skills/04-review/routing/regression-guards/tls-hostnames-per-attachment.md),
and [removed duplicate store](../../skills/04-review/routing/regression-guards/gateway-config-store-orphaned.md).
