# AI DNS first-address projection

Status: deferred performance decision for the user. No runtime change.
Source baseline: `cb1834053`.

## Current evidence

`ai/http/attempt.rs::resolve_ai_endpoint` awaits the canonical DNS resolver,
passes the returned `Vec<IpAddr>` through the Gateway egress adapter, and takes
the first allowed address. Empty and entirely rejected results become the same
`AiPeerPredispatchFailure::Endpoint`; the selected address receives the supplied
port. Resolver order determines selection.

The Gateway adapter supplies `observe_block` to the Common address filter.
Every rejected address increments the existing bounded-reason egress counter,
including duplicates and rejected addresses after the first allowed address.
Replacing the filter with a short-circuiting `find` would change that observation.
The scalar Gateway `is_ip_allowed` already shares the canonical classifier and
the same metric hook; there is no need for another IP safety implementation.

The current `Vec::into_iter().filter().collect()` shape can reuse the input
allocation. The inspected Rust 1.96.1 standard-library sources support in-place
collection for the same-size `IpAddr` source/destination and Filter adapter.
An intermediate Vec expression alone is therefore not evidence of an extra
allocation. No allocation saving or throughput improvement is established here.

This differs from [DNS-01](dns-01-address-result-ownership.md), which concerns
native ExternalName cache-result ownership and cloning under its cache lock.
This proposal neither changes DNS result ownership nor moves work into a lock.

## Decision

Keep the current implementation, or evaluate a full-scan projection that retains
only the first allowed address while still checking every address in order.
The latter can avoid materializing the complete allowed sequence, but adds
selection bookkeeping and still pays classification and rejection metrics for
the full input. Establish the value with measurements before selecting it.

## Acceptance for an approved implementation

- Reuse canonical egress classification and the Gateway observer. Preserve
  ordered rejection reasons and counts, including duplicates and rejected suffixes.
  Do not add tracing, metric families, address labels or another policy cache.
- Differentially compare the selected address and observer sequence against the
  existing full filter. Cover empty/all-blocked/all-allowed inputs, mixed order,
  first/last allowed positions, IPv4/IPv6, mapped IPv4, link-local, unspecified,
  private/unique-local, loopback and duplicate addresses.
- Preserve both private-policy settings and explicit loopback allowance. Cover
  the distinct unit-test and integration-test exceptions; unit tests compiled
  with the loopback exception do not prove production loopback rejection.
- Keep resolver selection before the DNS await and the current default private
  policy load after it. Preserve per-address integration-mode checks, DNS errors,
  first-result order, supplied port and predispatch failure mapping.
- Measure common single-address and larger mixed results, classification/metric
  costs, allocations and CPU. Include mostly accepted and mostly rejected inputs;
  do not infer improvement solely from removing `collect`.
- Use a new IMPL plan with an exact file allow-list. The current audit authorizes
  no shared resolver, cache, policy, retry or metric semantic changes.

## Original knowledge and verification limits

The [egress observation guard](../../skills/04-review/observability/regression-guards/gw-plugin-outbound-01-egress-resolver-tracing-removed.md)
documents per-rejected-address metrics and the request-path tracing prohibition.
Fresh baseline tests passed seven AI HTTP attempt, three Gateway egress and nine
Common egress tests. The Common tests compare scalar and batch admission plus
ordered rejection observations. These test current behavior, not a new
first-address projection. No live DNS/provider test or performance benchmark ran.
