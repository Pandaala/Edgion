# Define HTTPDNS endpoint egress and bootstrap resolution policy

## Status and evidence

Design decision required; found during Common quality batch B0083.
`edgion-common/src/dns/http_fetcher.rs` disables proxies and redirects but does not
install EgressResolver or classify literal endpoint addresses. Operator-controlled
`HttpDnsResolverConfig.url_template` can therefore reach loopback/link-local
addresses. Checking the returned backend IPs later does not protect this earlier
HTTP exchange. This is a confirmed coverage gap, not evidence of an unauthenticated
arbitrary-URL attack; the endpoint is operator configuration.

## Decision and options

Define the permitted HTTPDNS service address classes and bootstrap resolver.
Prefer canonical EgressResolver using System bootstrap plus literal pre-send
classification; recursively using the HTTPDNS resolver to resolve itself must be
impossible. Decide whether a documented local/internal DNS-service exception is
required. Blanket private-IP denial can break supported internal DNS providers;
an ad-hoc DNS library or a second classifier is not acceptable.

## Acceptance

Trace Controller background and Gateway request-resolution consumers. Preserve
configured fallback, timeouts, Secret trust, disabled proxy environment and denied
redirects. Verify literal and hostname destinations, IPv4-mapped addresses,
rebinding, allowed internal services and denied metadata/link-local targets.
Use bounded transport tests and the existing dynamic topology without restarting
shared services. No Gateway API wire/schema changes or legacy aliases are needed;
if a new operator knob is chosen, update schema, validation and documentation
through both Controller and Gateway before declaring coverage complete.
